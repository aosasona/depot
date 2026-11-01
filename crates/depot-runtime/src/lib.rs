//! Docker container lifecycle and localhost readiness boundaries for service releases.

use std::{process::Stdio, time::Duration};

use serde::Serialize;
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
}

/// Runtime identity needed for persistence, readiness, routing, and later cleanup.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RunningContainer {
    pub container_id: String,
    pub host_port: u16,
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
    #[error("Docker returned an invalid container identity or port mapping")]
    InvalidRuntimeIdentity,
    #[error("release container did not become HTTP-ready before its deadline")]
    NotReady,
    #[error("runtime environment contains an invalid name or value")]
    InvalidEnvironment,
    #[error("Docker could not read release container logs")]
    LogsFailed,
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

    use super::{StartContainerRequest, container_logs, healthy_http_response, start_container};

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
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).expect("fake is executable");
        std::thread::sleep(Duration::from_millis(10));
        let running = start_container(StartContainerRequest {
            docker_binary: fake.to_str().expect("path is UTF-8"),
            deployment_id: "deployment",
            image_id: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            container_port: 3000,
            timeout: Duration::from_secs(5),
            environment: &[],
        })
        .await
        .expect("container starts");
        assert_eq!(running.host_port, 43123);
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
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).expect("fake is executable");
        std::thread::sleep(Duration::from_millis(10));
        let environment = vec![("DATABASE_URL".to_owned(), "secret-value".to_owned())];
        start_container(StartContainerRequest {
            docker_binary: fake.to_str().expect("path is UTF-8"),
            deployment_id: "deployment",
            image_id: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            container_port: 3000,
            timeout: Duration::from_secs(5),
            environment: &environment,
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
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).expect("fake is executable");
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
