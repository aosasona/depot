//! Docker container lifecycle and localhost readiness boundaries for service releases.

use std::{path::Path, process::Stdio, time::Duration};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    process::Command,
};

/// Inputs for starting one isolated release container on an ephemeral localhost port.
#[derive(Debug)]
pub struct StartContainerRequest<'a> {
    pub docker_binary: &'a str,
    pub deployment_id: &'a str,
    pub image_id: &'a str,
    pub container_port: u16,
    pub timeout: Duration,
    pub environment: &'a [(String, String)],
    pub project_id: &'a str,
    pub persistent_paths: &'a [String],
}

/// Inputs for moving one stopped release directory into its first managed volume.
#[derive(Debug)]
pub struct MigrateVolumeRequest<'a> {
    pub docker_binary: &'a str,
    pub project_id: &'a str,
    pub container_id: &'a str,
    pub image_id: &'a str,
    pub container_path: &'a str,
    pub workspace_root: &'a Path,
    pub timeout: Duration,
}

/// Runtime identity needed for persistence, readiness, routing, and later cleanup.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RunningContainer {
    pub container_id: String,
    pub host_port: u16,
}

/// Expected immutable identity for one active service route.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteIdentity<'a> {
    pub deployment_id: &'a str,
    pub container_id: &'a str,
    pub host_port: u16,
}

/// Current running state for an otherwise exact Docker route identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RouteState {
    pub running: bool,
}

/// Fixed lifecycle operations accepted from Depot's durable worker queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainerAction {
    Start,
    Stop,
    Restart,
}

/// One published Docker port returned by the structured inspect template.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PublishedPort {
    host_ip: String,
    host_port: String,
}

/// Safe failures from Docker execution, runtime identity parsing, or readiness.
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Docker runtime command could not be started: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("Docker runtime command exceeded its configured timeout")]
    Timeout,
    #[error("Docker could not start the release container")]
    StartFailed,
    #[error("Docker could not apply the requested container lifecycle action")]
    ControlFailed,
    #[error("Docker returned an invalid container identity or port mapping")]
    InvalidRuntimeIdentity,
    #[error("release container did not become HTTP-ready before its deadline")]
    NotReady,
    #[error("runtime environment contains an invalid name or value")]
    InvalidEnvironment,
    #[error("persistent volume configuration contains an unsafe container path")]
    InvalidVolume,
    #[error("Docker could not inspect image volume metadata")]
    ImageInspectFailed,
    #[error("Docker could not migrate existing container data into persistent storage")]
    VolumeMigrationFailed,
    #[error("Docker could not inspect persistent volume state")]
    VolumeInspectFailed,
    #[error("Docker could not read release container logs")]
    LogsFailed,
    #[error("the recorded active container no longer exists in Docker")]
    ContainerMissing,
    #[error("active runtime identity does not match the requested route")]
    RouteMismatch,
}

/// Verifies a route against the live Docker container identity and loopback mapping.
pub async fn verify_route(
    docker_binary: &str,
    identity: RouteIdentity<'_>,
    timeout: Duration,
) -> Result<(), RuntimeError> {
    let state = inspect_route(docker_binary, identity, timeout).await?;
    state
        .running
        .then_some(())
        .ok_or(RuntimeError::RouteMismatch)
}

/// Inspects an exact container, label, and port mapping even while it is stopped.
pub async fn inspect_route(
    docker_binary: &str,
    identity: RouteIdentity<'_>,
    timeout: Duration,
) -> Result<RouteState, RuntimeError> {
    let template = r#"{{.Id}}
{{.State.Running}}
{{index .Config.Labels "depot.deployment"}}
{{json .NetworkSettings.Ports}}
{{json .HostConfig.PortBindings}}"#;
    let mut inspect = Command::new(docker_binary);
    inspect
        .args(["inspect", "--format", template, identity.container_id])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = command_output(&mut inspect, timeout).await?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        if error.contains("no such object") || error.contains("no such container") {
            return Err(RuntimeError::ContainerMissing);
        }
        return Err(RuntimeError::RouteMismatch);
    }
    let output = String::from_utf8(output.stdout).map_err(|_| RuntimeError::RouteMismatch)?;
    let mut lines = output.lines();
    let container_id = lines.next().ok_or(RuntimeError::RouteMismatch)?;
    let running = lines.next().ok_or(RuntimeError::RouteMismatch)?;
    let deployment_id = lines.next().ok_or(RuntimeError::RouteMismatch)?;
    let network_ports = lines.next().ok_or(RuntimeError::RouteMismatch)?;
    let configured_ports = lines.next().ok_or(RuntimeError::RouteMismatch)?;
    if lines.next().is_some() {
        return Err(RuntimeError::RouteMismatch);
    }
    // Containers created before deployment labels were introduced still have an exact,
    // immutable Docker ID and loopback port recorded by Depot. Accept only a missing
    // legacy label; a present label must continue to match the requested deployment.
    let deployment_matches = deployment_id == identity.deployment_id
        || deployment_id.is_empty()
        || deployment_id == "<no value>";
    if container_id != identity.container_id || !deployment_matches {
        return Err(RuntimeError::RouteMismatch);
    }
    let selected_ports = if running == "true" {
        network_ports
    } else {
        configured_ports
    };
    let ports: std::collections::HashMap<String, Option<Vec<PublishedPort>>> =
        serde_json::from_str(selected_ports).map_err(|_| RuntimeError::RouteMismatch)?;
    let expected = identity.host_port.to_string();
    let matches = ports
        .values()
        .filter_map(Option::as_ref)
        .flatten()
        .filter(|binding| {
            binding.host_port == expected && matches!(binding.host_ip.as_str(), "127.0.0.1" | "::1")
        })
        .count();
    (matches == 1)
        .then_some(RouteState {
            running: running == "true",
        })
        .ok_or(RuntimeError::RouteMismatch)
}

/// Applies one idempotent fixed lifecycle action to a verified Depot container.
pub async fn control_container(
    docker_binary: &str,
    identity: RouteIdentity<'_>,
    action: ContainerAction,
    timeout: Duration,
) -> Result<RouteState, RuntimeError> {
    let state = inspect_route(docker_binary, identity.clone(), timeout).await?;
    let command_name = match action {
        ContainerAction::Start if state.running => None,
        ContainerAction::Stop if !state.running => None,
        ContainerAction::Start => Some("start"),
        ContainerAction::Stop => Some("stop"),
        ContainerAction::Restart => Some("restart"),
    };
    if let Some(command_name) = command_name {
        let mut command = Command::new(docker_binary);
        command
            .args([command_name, identity.container_id])
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let output = command_output(&mut command, timeout).await?;
        if !output.status.success() {
            return Err(RuntimeError::ControlFailed);
        }
    }
    inspect_route(docker_binary, identity, timeout).await
}

/// Starts an image with an ephemeral loopback-only port and resolves its runtime identity.
pub async fn start_container(
    request: StartContainerRequest<'_>,
) -> Result<RunningContainer, RuntimeError> {
    let name = format!("depot-{}", request.deployment_id);
    let publish = format!("127.0.0.1::{}", request.container_port);
    let label = format!("depot.deployment={}", request.deployment_id);
    let mut run = Command::new(request.docker_binary);
    run.args([
        "run",
        "--detach",
        "--restart",
        "unless-stopped",
        "--label",
        &label,
        "--name",
        &name,
        "--publish",
        &publish,
    ]);
    if !request.environment.is_empty() {
        run.args(["--env-file", "/dev/stdin"]);
    }
    for path in request.persistent_paths {
        if !valid_container_path(path) {
            return Err(RuntimeError::InvalidVolume);
        }
        let mount = format!(
            "type=volume,src={},dst={path}",
            persistent_volume_name(request.project_id, path)
        );
        run.args(["--mount", &mount]);
    }
    run.arg(request.image_id)
        .stdin(if request.environment.is_empty() {
            Stdio::null()
        } else {
            Stdio::piped()
        })
        .kill_on_drop(true);
    let environment = encode_environment(request.environment)?;
    let output = command_output_with_input(&mut run, request.timeout, &environment).await?;
    if !output.status.success() {
        return Err(RuntimeError::StartFailed);
    }
    let container_id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if container_id.len() < 12 || !container_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(RuntimeError::InvalidRuntimeIdentity);
    }
    let port_key = format!("{}/tcp", request.container_port);
    let template =
        format!("{{{{(index (index .NetworkSettings.Ports \"{port_key}\") 0).HostPort}}}}");
    let mut inspect = Command::new(request.docker_binary);
    inspect
        .args(["inspect", "--format", &template, &container_id])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = match command_output(&mut inspect, request.timeout).await {
        Ok(output) => output,
        Err(error) => {
            let _ = remove_container(request.docker_binary, &container_id, request.timeout).await;
            return Err(error);
        }
    };
    let host_port = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u16>()
        .ok();
    if !output.status.success() || host_port.is_none_or(|port| port == 0) {
        let _ = remove_container(request.docker_binary, &container_id, request.timeout).await;
        return Err(RuntimeError::InvalidRuntimeIdentity);
    }
    Ok(RunningContainer {
        container_id,
        host_port: host_port.expect("validated host port is present"),
    })
}

/// Reads absolute volume targets declared by the built OCI image.
pub async fn image_volume_paths(
    docker_binary: &str,
    image_id: &str,
    timeout: Duration,
) -> Result<Vec<String>, RuntimeError> {
    let mut inspect = Command::new(docker_binary);
    inspect
        .args([
            "image",
            "inspect",
            "--format",
            "{{json .Config.Volumes}}",
            image_id,
        ])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = command_output(&mut inspect, timeout).await?;
    if !output.status.success() {
        return Err(RuntimeError::ImageInspectFailed);
    }
    let value = String::from_utf8(output.stdout).map_err(|_| RuntimeError::ImageInspectFailed)?;
    if value.trim() == "null" || value.trim().is_empty() {
        return Ok(Vec::new());
    }
    let declared: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_str(value.trim()).map_err(|_| RuntimeError::ImageInspectFailed)?;
    Ok(declared
        .into_keys()
        .filter(|path| valid_container_path(path))
        .collect())
}

/// Confirms whether an existing project-scoped volume is still present in Docker.
pub async fn persistent_volume_exists(
    docker_binary: &str,
    project_id: &str,
    container_path: &str,
    timeout: Duration,
) -> Result<bool, RuntimeError> {
    if !valid_container_path(container_path) {
        return Err(RuntimeError::InvalidVolume);
    }
    let volume_name = persistent_volume_name(project_id, container_path);
    let mut inspect = Command::new(docker_binary);
    inspect
        .args(["volume", "inspect", &volume_name])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = command_output(&mut inspect, timeout).await?;
    if output.status.success() {
        return Ok(true);
    }
    let error = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    if error.contains("no such volume") {
        return Ok(false);
    }
    Err(RuntimeError::VolumeInspectFailed)
}

/// Seeds a new named volume from a stopped predecessor container exactly once.
pub async fn migrate_volume_from_container(
    request: MigrateVolumeRequest<'_>,
) -> Result<bool, RuntimeError> {
    if !valid_container_path(request.container_path) {
        return Err(RuntimeError::InvalidVolume);
    }
    let staging = request
        .workspace_root
        .join(format!("volume-seed-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&staging).map_err(|_| RuntimeError::VolumeMigrationFailed)?;
    let volume_name = persistent_volume_name(request.project_id, request.container_path);
    let mut inspect = Command::new(request.docker_binary);
    inspect
        .args(["volume", "inspect", &volume_name])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let inspected = match command_output(&mut inspect, request.timeout).await {
        Ok(output) => output,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    if inspected.status.success() {
        let _ = std::fs::remove_dir_all(&staging);
        return Ok(false);
    }

    let mut create = Command::new(request.docker_binary);
    create
        .args(["volume", "create", &volume_name])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let created = match command_output(&mut create, request.timeout).await {
        Ok(output) => output,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    if !created.status.success() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(RuntimeError::VolumeMigrationFailed);
    }

    let seed_name = format!("depot-volume-seed-{}", uuid::Uuid::now_v7());
    let migration = seed_volume_contents(&request, &volume_name, &seed_name, &staging).await;
    let _ = std::fs::remove_dir_all(&staging);
    let mut remove_seed = Command::new(request.docker_binary);
    remove_seed
        .args(["rm", "--force", &seed_name])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let _ = command_output(&mut remove_seed, request.timeout).await;
    if migration.is_err() {
        let mut remove_volume = Command::new(request.docker_binary);
        remove_volume
            .args(["volume", "rm", &volume_name])
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let _ = command_output(&mut remove_volume, request.timeout).await;
    }
    migration.map(|()| true)
}

/// Copies one stopped container directory through bounded worker storage into a mounted seed container.
async fn seed_volume_contents(
    request: &MigrateVolumeRequest<'_>,
    volume_name: &str,
    seed_name: &str,
    staging: &Path,
) -> Result<(), RuntimeError> {
    let source = format!("{}:{}/.", request.container_id, request.container_path);
    let mut copy_out = Command::new(request.docker_binary);
    copy_out
        .arg("cp")
        .arg(source)
        .arg(staging)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    if !command_output(&mut copy_out, request.timeout)
        .await?
        .status
        .success()
    {
        return Err(RuntimeError::VolumeMigrationFailed);
    }
    let mount = format!(
        "type=volume,src={volume_name},dst={}",
        request.container_path
    );
    let mut create_seed = Command::new(request.docker_binary);
    create_seed
        .args([
            "create",
            "--name",
            seed_name,
            "--mount",
            &mount,
            request.image_id,
        ])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    if !command_output(&mut create_seed, request.timeout)
        .await?
        .status
        .success()
    {
        return Err(RuntimeError::VolumeMigrationFailed);
    }
    let staged_contents = format!("{}/.", staging.display());
    let destination = format!("{seed_name}:{}", request.container_path);
    let mut copy_in = Command::new(request.docker_binary);
    copy_in
        .arg("cp")
        .arg(staged_contents)
        .arg(destination)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    if !command_output(&mut copy_in, request.timeout)
        .await?
        .status
        .success()
    {
        return Err(RuntimeError::VolumeMigrationFailed);
    }
    Ok(())
}

/// Derives a stable project-scoped Docker volume name without exposing the path.
fn persistent_volume_name(project_id: &str, path: &str) -> String {
    let digest = Sha256::digest(path.as_bytes());
    let suffix = digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("depot-{project_id}-{suffix}")
}

/// Defensively validates a Docker volume destination received from persisted config or image data.
fn valid_container_path(value: &str) -> bool {
    !matches!(value, "" | "/")
        && value.len() <= 255
        && value.starts_with('/')
        && !value.contains([',', '\0'])
        && !value.chars().any(char::is_control)
        && !value.split('/').any(|part| matches!(part, "." | ".."))
}

/// Encodes already-validated names and newline-free values for Docker env-file stdin.
fn encode_environment(environment: &[(String, String)]) -> Result<Vec<u8>, RuntimeError> {
    let mut encoded = String::new();
    for (name, value) in environment {
        if name.is_empty()
            || name.contains(['=', '\n', '\r', '\0'])
            || value.contains(['\n', '\r', '\0'])
        {
            return Err(RuntimeError::InvalidEnvironment);
        }
        encoded.push_str(name);
        encoded.push('=');
        encoded.push_str(value);
        encoded.push('\n');
    }
    Ok(encoded.into_bytes())
}

/// Polls a loopback HTTP endpoint until it returns a non-5xx response or times out.
pub async fn wait_for_http_ready(
    host_port: u16,
    path: &str,
    timeout: Duration,
) -> Result<(), RuntimeError> {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if http_probe(host_port, path).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(RuntimeError::NotReady)
}

/// Force-removes a failed candidate container without affecting earlier releases.
pub async fn remove_container(
    docker_binary: &str,
    container_id: &str,
    timeout: Duration,
) -> Result<(), RuntimeError> {
    let mut command = Command::new(docker_binary);
    command
        .args(["rm", "--force", container_id])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = command_output(&mut command, timeout).await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(RuntimeError::StartFailed)
    }
}

/// Removes one unreferenced retained image after release retention selection.
pub async fn remove_image(
    docker_binary: &str,
    image_id: &str,
    timeout: Duration,
) -> Result<(), RuntimeError> {
    let mut command = Command::new(docker_binary);
    command
        .args(["image", "rm", image_id])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = command_output(&mut command, timeout).await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(RuntimeError::StartFailed)
    }
}

/// Reads a bounded tail from one server-selected release container without a shell.
pub async fn container_logs(
    docker_binary: &str,
    container_id: &str,
    tail: u16,
    timeout: Duration,
) -> Result<String, RuntimeError> {
    if container_id.len() < 12
        || !container_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || tail == 0
        || tail > 5_000
    {
        return Err(RuntimeError::InvalidRuntimeIdentity);
    }
    let mut command = Command::new(docker_binary);
    command
        .args([
            "logs",
            "--timestamps",
            "--tail",
            &tail.to_string(),
            container_id,
        ])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = command_output(&mut command, timeout).await?;
    if !output.status.success() {
        return Err(RuntimeError::LogsFailed);
    }
    let mut combined = output.stdout;
    combined.extend_from_slice(&output.stderr);
    const MAX_LOG_BYTES: usize = 512 * 1024;
    if combined.len() > MAX_LOG_BYTES {
        combined.drain(..combined.len() - MAX_LOG_BYTES);
    }
    Ok(String::from_utf8_lossy(&combined).into_owned())
}

/// Executes one bounded Docker command without a shell.
async fn command_output(
    command: &mut Command,
    timeout: Duration,
) -> Result<std::process::Output, RuntimeError> {
    tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| RuntimeError::Timeout)?
        .map_err(RuntimeError::Spawn)
}

/// Executes a bounded command while streaming a secret payload through stdin.
async fn command_output_with_input(
    command: &mut Command,
    timeout: Duration,
    input: &[u8],
) -> Result<std::process::Output, RuntimeError> {
    if input.is_empty() {
        return command_output(command, timeout).await;
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(RuntimeError::Spawn)?;
    let mut stdin = child.stdin.take().ok_or(RuntimeError::StartFailed)?;
    tokio::time::timeout(timeout, async move {
        stdin.write_all(input).await?;
        drop(stdin);
        child.wait_with_output().await
    })
    .await
    .map_err(|_| RuntimeError::Timeout)?
    .map_err(RuntimeError::Spawn)
}

/// Performs one bounded HTTP/1.1 readiness probe against loopback only.
async fn http_probe(host_port: u16, path: &str) -> bool {
    let attempt = async {
        let mut stream = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, host_port)).await?;
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await?;
        let mut response = [0_u8; 64];
        let count = stream.read(&mut response).await?;
        Ok::<_, std::io::Error>(healthy_http_response(&response[..count]))
    };
    tokio::time::timeout(Duration::from_secs(2), attempt)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(false)
}

/// Accepts HTTP/1.x responses below 500 as readiness without parsing bodies.
fn healthy_http_response(response: &[u8]) -> bool {
    response.starts_with(b"HTTP/1.1 2")
        || response.starts_with(b"HTTP/1.1 3")
        || response.starts_with(b"HTTP/1.1 4")
        || response.starts_with(b"HTTP/1.0 2")
        || response.starts_with(b"HTTP/1.0 3")
        || response.starts_with(b"HTTP/1.0 4")
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

    use super::{
        MigrateVolumeRequest, RouteIdentity, RuntimeError, StartContainerRequest, container_logs,
        healthy_http_response, image_volume_paths, inspect_route, migrate_volume_from_container,
        persistent_volume_exists, start_container, verify_route,
    };

    /// Avoids executing a just-closed script while the test filesystem still holds it writable.
    fn make_executable(path: &std::path::Path) {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("fake is executable");
        std::thread::sleep(Duration::from_millis(10));
    }

    /// Proves runtime identity and ephemeral port mappings are strictly parsed.
    #[tokio::test]
    async fn docker_runtime_resolves_container_and_host_port() {
        let root = std::env::temp_dir().join(format!("depot-runtime-test-{}", std::process::id()));
        fs::create_dir_all(&root).expect("test root is created");
        let fake = root.join("fake-docker");
        fs::write(
            &fake,
            "#!/bin/sh\nif [ \"$1\" = \"run\" ]; then printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'; else printf '43123'; fi\n",
        )
        .expect("fake Docker writes");
        make_executable(&fake);
        let running = start_container(StartContainerRequest {
            docker_binary: fake.to_str().expect("path is UTF-8"),
            deployment_id: "deployment",
            image_id: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            container_port: 3000,
            timeout: Duration::from_secs(5),
            environment: &[],
            project_id: "project",
            persistent_paths: &[],
        })
        .await
        .expect("container starts");
        assert_eq!(running.host_port, 43123);
        fs::remove_dir_all(root).expect("test root is removed");
    }

    /// Mounts configured paths with a deterministic project-scoped named volume.
    #[tokio::test]
    async fn docker_runtime_mounts_persistent_project_volume() {
        let root = std::env::temp_dir().join(format!(
            "depot-runtime-volume-test-{}",
            uuid::Uuid::now_v7()
        ));
        fs::create_dir_all(&root).expect("test root is created");
        let fake = root.join("fake-docker");
        let captured = root.join("arguments");
        fs::write(
            &fake,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"run\" ]; then printf '%s\\n' \"$@\" > '{}'; printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'; else printf '43123'; fi\n",
                captured.display()
            ),
        )
        .expect("fake Docker writes");
        make_executable(&fake);
        start_container(StartContainerRequest {
            docker_binary: fake.to_str().expect("path is UTF-8"),
            deployment_id: "deployment",
            image_id: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            container_port: 3000,
            timeout: Duration::from_secs(5),
            environment: &[],
            project_id: "project",
            persistent_paths: &["/data".to_owned()],
        })
        .await
        .expect("container starts");
        let arguments = fs::read_to_string(captured).expect("arguments read");
        assert!(arguments.contains("--mount\n"));
        assert!(arguments.contains("type=volume,src=depot-project-"));
        assert!(arguments.contains(",dst=/data"));
        fs::remove_dir_all(root).expect("test root is removed");
    }

    /// Reads valid persistent targets emitted in OCI image configuration.
    #[tokio::test]
    async fn image_volume_metadata_is_detected() {
        let root = std::env::temp_dir().join(format!(
            "depot-runtime-image-volume-test-{}",
            uuid::Uuid::now_v7()
        ));
        fs::create_dir_all(&root).expect("test root is created");
        let fake = root.join("fake-docker");
        fs::write(
            &fake,
            "#!/bin/sh\nprintf '%s' '{\"/data\":{},\"/var/lib/app\":{},\"relative\":{}}'\n",
        )
        .expect("fake Docker writes");
        make_executable(&fake);
        let paths = image_volume_paths(
            fake.to_str().expect("path is UTF-8"),
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Duration::from_secs(5),
        )
        .await
        .expect("image metadata reads");
        assert_eq!(paths, vec!["/data", "/var/lib/app"]);
        fs::remove_dir_all(root).expect("test root is removed");
    }

    /// Copies a stopped release directory into a newly created managed volume.
    #[tokio::test]
    async fn new_persistent_volume_is_seeded_from_predecessor() {
        let root = std::env::temp_dir().join(format!(
            "depot-runtime-volume-migration-test-{}",
            uuid::Uuid::now_v7()
        ));
        fs::create_dir_all(&root).expect("test root is created");
        let fake = root.join("fake-docker");
        let captured = root.join("commands");
        fs::write(
            &fake,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1 $2\" = \"volume inspect\" ]; then exit 1; fi\nexit 0\n",
                captured.display()
            ),
        )
        .expect("fake Docker writes");
        make_executable(&fake);
        let migrated = migrate_volume_from_container(MigrateVolumeRequest {
            docker_binary: fake.to_str().expect("path is UTF-8"),
            project_id: "project",
            container_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            image_id: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            container_path: "/var/lib/app",
            workspace_root: &root,
            timeout: Duration::from_secs(5),
        })
        .await
        .expect("volume migration succeeds");
        assert!(migrated);
        let commands = fs::read_to_string(captured).expect("commands read");
        assert!(commands.contains("volume create depot-project-"));
        assert!(commands.contains(
            "cp aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:/var/lib/app/."
        ));
        assert!(commands.contains("type=volume,src=depot-project-"));
        assert!(commands.contains("dst=/var/lib/app"));
        fs::remove_dir_all(root).expect("test root is removed");
    }

    /// Leaves an existing managed volume untouched on later deployments.
    #[tokio::test]
    async fn existing_persistent_volume_is_not_seeded_again() {
        let root = std::env::temp_dir().join(format!(
            "depot-runtime-existing-volume-test-{}",
            uuid::Uuid::now_v7()
        ));
        fs::create_dir_all(&root).expect("test root is created");
        let fake = root.join("fake-docker");
        fs::write(&fake, "#!/bin/sh\nexit 0\n").expect("fake Docker writes");
        make_executable(&fake);
        let migrated = migrate_volume_from_container(MigrateVolumeRequest {
            docker_binary: fake.to_str().expect("path is UTF-8"),
            project_id: "project",
            container_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            image_id: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            container_path: "/data",
            workspace_root: &root,
            timeout: Duration::from_secs(5),
        })
        .await
        .expect("existing volume is accepted");
        assert!(!migrated);
        fs::remove_dir_all(root).expect("test root is removed");
    }

    /// Rejects port reuse unless Docker confirms every persisted identity field.
    #[tokio::test]
    async fn route_verification_fails_closed_on_identity_mismatch() {
        let root =
            std::env::temp_dir().join(format!("depot-runtime-route-test-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&root).expect("test root is created");
        let fake = root.join("fake-docker");
        fs::write(
            &fake,
            "#!/bin/sh\nprintf '%s\\n' 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb' 'true' 'other-deployment' '{\"80/tcp\":[{\"HostIp\":\"127.0.0.1\",\"HostPort\":\"43123\"}]}' '{\"80/tcp\":[{\"HostIp\":\"127.0.0.1\",\"HostPort\":\"43123\"}]}'\n",
        )
        .expect("fake Docker writes");
        make_executable(&fake);
        let result = verify_route(
            fake.to_str().expect("path is UTF-8"),
            RouteIdentity {
                deployment_id: "deployment",
                container_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                host_port: 43123,
            },
            Duration::from_secs(5),
        )
        .await;
        assert!(result.is_err());
        fs::remove_dir_all(root).expect("test root is removed");
    }

    /// Distinguishes a definitely deleted container from an unverifiable identity.
    #[tokio::test]
    async fn route_inspection_reports_a_missing_container() {
        let root = std::env::temp_dir().join(format!(
            "depot-runtime-route-missing-test-{}",
            uuid::Uuid::now_v7()
        ));
        fs::create_dir_all(&root).expect("test root is created");
        let fake = root.join("fake-docker");
        fs::write(
            &fake,
            "#!/bin/sh\nprintf '%s\\n' 'Error: No such object: missing' >&2\nexit 1\n",
        )
        .expect("fake Docker writes");
        make_executable(&fake);
        let result = inspect_route(
            fake.to_str().expect("path is UTF-8"),
            RouteIdentity {
                deployment_id: "deployment",
                container_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                host_port: 43123,
            },
            Duration::from_secs(5),
        )
        .await;
        assert!(matches!(result, Err(RuntimeError::ContainerMissing)));
        fs::remove_dir_all(root).expect("test root is removed");
    }

    /// Keeps Docker daemon and permission failures out of the automatic recovery path.
    #[tokio::test]
    async fn route_inspection_fails_closed_on_docker_error() {
        let root = std::env::temp_dir().join(format!(
            "depot-runtime-route-error-test-{}",
            uuid::Uuid::now_v7()
        ));
        fs::create_dir_all(&root).expect("test root is created");
        let fake = root.join("fake-docker");
        fs::write(
            &fake,
            "#!/bin/sh\nprintf '%s\\n' 'permission denied' >&2\nexit 1\n",
        )
        .expect("fake Docker writes");
        make_executable(&fake);
        let result = inspect_route(
            fake.to_str().expect("path is UTF-8"),
            RouteIdentity {
                deployment_id: "deployment",
                container_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                host_port: 43123,
            },
            Duration::from_secs(5),
        )
        .await;
        assert!(matches!(result, Err(RuntimeError::RouteMismatch)));
        fs::remove_dir_all(root).expect("test root is removed");
    }

    /// Checks existing named volumes without creating replacements.
    #[tokio::test]
    async fn persistent_volume_check_distinguishes_missing_from_errors() {
        let root = std::env::temp_dir().join(format!(
            "depot-runtime-volume-check-test-{}",
            uuid::Uuid::now_v7()
        ));
        fs::create_dir_all(&root).expect("test root is created");
        let fake = root.join("fake-docker");
        fs::write(
            &fake,
            "#!/bin/sh\nprintf '%s\\n' 'Error: No such volume: missing' >&2\nexit 1\n",
        )
        .expect("fake Docker writes");
        make_executable(&fake);
        assert!(
            !persistent_volume_exists(
                fake.to_str().expect("path is UTF-8"),
                "project",
                "/data",
                Duration::from_secs(5),
            )
            .await
            .expect("missing volume is classified")
        );
        fs::remove_dir_all(root).expect("test root is removed");
    }

    /// Accepts exactly one matching loopback mapping for the expected container.
    #[tokio::test]
    async fn route_verification_accepts_exact_runtime_identity() {
        let root = std::env::temp_dir().join(format!(
            "depot-runtime-route-ok-test-{}",
            uuid::Uuid::now_v7()
        ));
        fs::create_dir_all(&root).expect("test root is created");
        let fake = root.join("fake-docker");
        fs::write(
            &fake,
            "#!/bin/sh\nprintf '%s\\n' 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' 'true' 'deployment' '{\"80/tcp\":[{\"HostIp\":\"127.0.0.1\",\"HostPort\":\"43123\"}]}' '{\"80/tcp\":[{\"HostIp\":\"127.0.0.1\",\"HostPort\":\"43123\"}]}'\n",
        )
        .expect("fake Docker writes");
        make_executable(&fake);
        verify_route(
            fake.to_str().expect("path is UTF-8"),
            RouteIdentity {
                deployment_id: "deployment",
                container_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                host_port: 43123,
            },
            Duration::from_secs(5),
        )
        .await
        .expect("identity matches");
        fs::remove_dir_all(root).expect("test root is removed");
    }

    /// Keeps routes for pre-label containers working after a Depot upgrade.
    #[tokio::test]
    async fn route_verification_accepts_unlabeled_legacy_container() {
        let root = std::env::temp_dir().join(format!(
            "depot-runtime-route-legacy-test-{}",
            uuid::Uuid::now_v7()
        ));
        fs::create_dir_all(&root).expect("test root is created");
        let fake = root.join("fake-docker");
        fs::write(
            &fake,
            "#!/bin/sh\nprintf '%s\\n' 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' 'true' '<no value>' '{\"80/tcp\":[{\"HostIp\":\"127.0.0.1\",\"HostPort\":\"43123\"}]}' '{\"80/tcp\":[{\"HostIp\":\"127.0.0.1\",\"HostPort\":\"43123\"}]}'\n",
        )
        .expect("fake Docker writes");
        make_executable(&fake);
        verify_route(
            fake.to_str().expect("path is UTF-8"),
            RouteIdentity {
                deployment_id: "deployment",
                container_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                host_port: 43123,
            },
            Duration::from_secs(5),
        )
        .await
        .expect("legacy identity matches without a deployment label");
        fs::remove_dir_all(root).expect("test root is removed");
    }

    /// Uses persistent host port bindings to identify an intentionally stopped service.
    #[tokio::test]
    async fn stopped_route_identity_uses_configured_port_binding() {
        let root = std::env::temp_dir().join(format!(
            "depot-runtime-route-stopped-test-{}",
            uuid::Uuid::now_v7()
        ));
        fs::create_dir_all(&root).expect("test root is created");
        let fake = root.join("fake-docker");
        fs::write(
            &fake,
            "#!/bin/sh\nprintf '%s\\n' 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' 'false' 'deployment' '{}' '{\"80/tcp\":[{\"HostIp\":\"127.0.0.1\",\"HostPort\":\"43123\"}]}'\n",
        )
        .expect("fake Docker writes");
        make_executable(&fake);
        let state = inspect_route(
            fake.to_str().expect("path is UTF-8"),
            RouteIdentity {
                deployment_id: "deployment",
                container_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                host_port: 43123,
            },
            Duration::from_secs(5),
        )
        .await
        .expect("stopped identity matches");
        assert!(!state.running);
        fs::remove_dir_all(root).expect("test root is removed");
    }

    /// Proves runtime values travel over stdin rather than Docker command arguments.
    #[tokio::test]
    async fn docker_runtime_streams_environment_over_stdin() {
        let root =
            std::env::temp_dir().join(format!("depot-runtime-env-test-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&root).expect("test root is created");
        let fake = root.join("fake-docker");
        let captured = root.join("environment");
        fs::write(
            &fake,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"run\" ]; then cat > '{}'; printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'; else printf '43123'; fi\n",
                captured.display()
            ),
        )
        .expect("fake Docker writes");
        make_executable(&fake);
        let environment = vec![("DATABASE_URL".to_owned(), "secret-value".to_owned())];
        start_container(StartContainerRequest {
            docker_binary: fake.to_str().expect("path is UTF-8"),
            deployment_id: "deployment",
            image_id: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            container_port: 3000,
            timeout: Duration::from_secs(5),
            environment: &environment,
            project_id: "project",
            persistent_paths: &[],
        })
        .await
        .expect("container starts");
        assert_eq!(
            fs::read_to_string(captured).expect("stdin was captured"),
            "DATABASE_URL=secret-value\n"
        );
        fs::remove_dir_all(root).expect("test root is removed");
    }

    /// Proves readiness accepts healthy statuses and rejects server failures.
    #[test]
    fn readiness_classifies_http_status_lines() {
        assert!(healthy_http_response(b"HTTP/1.1 204 No Content\r\n"));
        assert!(healthy_http_response(b"HTTP/1.0 404 Not Found\r\n"));
        assert!(!healthy_http_response(
            b"HTTP/1.1 503 Service Unavailable\r\n"
        ));
    }

    /// Proves runtime logs use the fixed Docker argument shape and return output.
    #[tokio::test]
    async fn runtime_logs_are_read_from_a_valid_container() {
        let root =
            std::env::temp_dir().join(format!("depot-runtime-logs-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&root).expect("test root is created");
        let fake = root.join("fake-docker");
        fs::write(&fake, "#!/bin/sh\nprintf '2026-01-01T00:00:00Z ready\\n'\n")
            .expect("fake Docker writes");
        make_executable(&fake);
        let logs = container_logs(
            fake.to_str().expect("path is UTF-8"),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            1_000,
            Duration::from_secs(5),
        )
        .await
        .expect("logs load");
        assert!(logs.contains("ready"));
        fs::remove_dir_all(root).expect("test root is removed");
    }
}
