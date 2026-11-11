//! Safe Git checkout and inspectable build-plan detection for worker jobs.

use std::{
    fs::Permissions,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use depot_core::{ProjectValidationError, validate_repository_url};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::process::Command;

/// Supported build pathways inferred without executing repository code.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectedBuilder {
    Dockerfile,
    Railpack,
    Static,
    Unknown,
}

/// Immutable repository facts and the reason for the detected build plan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryInspection {
    pub commit_sha: String,
    pub builder: DetectedBuilder,
    pub reason: String,
    pub has_mise_config: bool,
    pub exposed_ports: Vec<u16>,
}

/// Decrypted SSH material borrowed only for the duration of one Git operation.
#[derive(Clone, Copy)]
pub struct GitSshCredentials<'a> {
    pub private_key: &'a str,
    pub known_hosts: &'a str,
}

/// Short-lived HTTPS token borrowed only for one GitHub App checkout.
#[derive(Clone, Copy)]
pub struct GitHttpCredentials<'a> {
    pub token: &'a str,
}

/// A verified immutable checkout whose workspace is removed when dropped.
#[derive(Debug)]
pub struct RepositoryCheckout {
    path: PathBuf,
    commit_sha: String,
}

impl RepositoryCheckout {
    /// Returns the repository root containing the verified immutable commit.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the exact commit resolved and verified after checkout.
    pub fn commit_sha(&self) -> &str {
        &self.commit_sha
    }
}

/// Failures from validation, bounded Git execution, or repository inspection.
#[derive(Debug, Error)]
pub enum GitError {
    #[error(transparent)]
    Validation(#[from] ProjectValidationError),
    #[error("could not prepare isolated Git workspace: {0}")]
    Workspace(#[source] std::io::Error),
    #[error("Git could not be started: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("Git operation exceeded its configured timeout")]
    Timeout,
    #[error("Git checkout failed without exposing remote credentials")]
    CheckoutFailed,
    #[error("SSH repository has no project deploy credential")]
    MissingSshCredentials,
    #[error("checked out repository did not resolve to a valid commit SHA")]
    InvalidCommit,
}

/// Clones one branch without a shell, resolves its immutable SHA, and detects a build plan.
pub async fn inspect_repository(
    repository_url: &str,
    branch: &str,
    context_path: &str,
    dockerfile_path: &str,
    workspace_root: &Path,
    timeout: Duration,
    ssh_credentials: Option<GitSshCredentials<'_>>,
    http_credentials: Option<GitHttpCredentials<'_>>,
) -> Result<RepositoryInspection, GitError> {
    validate_repository_url(repository_url)?;
    require_ssh_credentials(repository_url, ssh_credentials)?;
    std::fs::create_dir_all(workspace_root).map_err(GitError::Workspace)?;
    let checkout_path = workspace_root.join(format!("inspect-{}", uuid::Uuid::now_v7()));
    let workspace = WorkspaceGuard::new(checkout_path);
    let mut clone = git_command();
    let ssh = prepare_ssh_command(workspace_root, ssh_credentials)?;
    if let Some(ssh) = &ssh {
        ssh.configure(&mut clone);
    }
    let http = prepare_http_command(workspace_root, http_credentials)?;
    if let Some(http) = &http {
        http.configure(&mut clone);
    }
    clone.args([
        "clone",
        "--depth",
        "1",
        "--single-branch",
        "--no-tags",
        "--branch",
        branch,
        "--",
        repository_url,
    ]);
    clone.arg(workspace.path());
    run_status(&mut clone, timeout).await?;
    drop(ssh);
    drop(http);

    let mut revision = git_command();
    revision
        .arg("-C")
        .arg(workspace.path())
        .args(["rev-parse", "HEAD"]);
    let output = tokio::time::timeout(timeout, revision.output())
        .await
        .map_err(|_| GitError::Timeout)?
        .map_err(GitError::Spawn)?;
    if !output.status.success() {
        return Err(GitError::InvalidCommit);
    }
    let commit_sha = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if commit_sha.len() != 40 || !commit_sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GitError::InvalidCommit);
    }
    let context = workspace.path().join(context_path);
    let dockerfile = workspace.path().join(dockerfile_path);
    let (builder, reason) = detect_builder(&context, &dockerfile);
    let exposed_ports = dockerfile_exposed_ports(&dockerfile);
    Ok(RepositoryInspection {
        commit_sha,
        builder,
        reason,
        has_mise_config: context.join("mise.toml").is_file()
            || context.join(".mise.toml").is_file(),
        exposed_ports,
    })
}

/// Checks out a branch and verifies it resolves to the deployment's recorded commit SHA.
pub async fn checkout_repository(
    repository_url: &str,
    branch: &str,
    expected_commit_sha: &str,
    workspace_root: &Path,
    timeout: Duration,
    ssh_credentials: Option<GitSshCredentials<'_>>,
    http_credentials: Option<GitHttpCredentials<'_>>,
) -> Result<RepositoryCheckout, GitError> {
    if expected_commit_sha.len() != 40
        || !expected_commit_sha
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(GitError::InvalidCommit);
    }
    validate_repository_url(repository_url)?;
    require_ssh_credentials(repository_url, ssh_credentials)?;
    std::fs::create_dir_all(workspace_root).map_err(GitError::Workspace)?;
    let checkout_path = workspace_root.join(format!("deploy-{}", uuid::Uuid::now_v7()));
    let mut clone = git_command();
    let ssh = prepare_ssh_command(workspace_root, ssh_credentials)?;
    if let Some(ssh) = &ssh {
        ssh.configure(&mut clone);
    }
    let http = prepare_http_command(workspace_root, http_credentials)?;
    if let Some(http) = &http {
        http.configure(&mut clone);
    }
    clone.args([
        "clone",
        "--depth",
        "1",
        "--single-branch",
        "--no-tags",
        "--branch",
        branch,
        "--",
        repository_url,
    ]);
    clone.arg(&checkout_path);
    if let Err(error) = run_status(&mut clone, timeout).await {
        let _ = std::fs::remove_dir_all(&checkout_path);
        return Err(error);
    }
    drop(ssh);
    drop(http);
    let mut revision = git_command();
    revision
        .arg("-C")
        .arg(&checkout_path)
        .args(["rev-parse", "HEAD"]);
    let output = tokio::time::timeout(timeout, revision.output())
        .await
        .map_err(|_| GitError::Timeout)?
        .map_err(GitError::Spawn)?;
    let resolved = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || !resolved.eq_ignore_ascii_case(expected_commit_sha) {
        let _ = std::fs::remove_dir_all(&checkout_path);
        return Err(GitError::InvalidCommit);
    }
    Ok(RepositoryCheckout {
        path: checkout_path,
        commit_sha: resolved,
    })
}

/// Creates a non-interactive Git command with output and system configuration constrained.
fn git_command() -> Command {
    let mut command = Command::new("git");
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Prevents SSH repositories from falling back to ambient agent or user keys.
fn require_ssh_credentials(
    repository_url: &str,
    credentials: Option<GitSshCredentials<'_>>,
) -> Result<(), GitError> {
    if repository_url
        .get(..6)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("ssh://"))
        && credentials.is_none()
    {
        return Err(GitError::MissingSshCredentials);
    }
    Ok(())
}

/// Writes short-lived SSH files and a fixed wrapper enforcing trusted-host checks.
fn prepare_ssh_command(
    workspace_root: &Path,
    credentials: Option<GitSshCredentials<'_>>,
) -> Result<Option<SshCommandGuard>, GitError> {
    let Some(credentials) = credentials else {
        return Ok(None);
    };
    let path = workspace_root.join(format!("ssh-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&path).map_err(GitError::Workspace)?;
    let guard = SshCommandGuard { path };
    std::fs::set_permissions(&guard.path, Permissions::from_mode(0o700))
        .map_err(GitError::Workspace)?;
    let private_key = guard.path.join("identity");
    let known_hosts = guard.path.join("known_hosts");
    let wrapper = guard.path.join("ssh");
    std::fs::write(&private_key, credentials.private_key).map_err(GitError::Workspace)?;
    std::fs::write(&known_hosts, credentials.known_hosts).map_err(GitError::Workspace)?;
    std::fs::write(
        &wrapper,
        "#!/bin/sh\nexec ssh -i \"$DEPOT_SSH_KEY_FILE\" -o IdentitiesOnly=yes -o UserKnownHostsFile=\"$DEPOT_SSH_KNOWN_HOSTS_FILE\" -o StrictHostKeyChecking=yes -o BatchMode=yes \"$@\"\n",
    )
    .map_err(GitError::Workspace)?;
    std::fs::set_permissions(&private_key, Permissions::from_mode(0o600))
        .map_err(GitError::Workspace)?;
    std::fs::set_permissions(&known_hosts, Permissions::from_mode(0o600))
        .map_err(GitError::Workspace)?;
    std::fs::set_permissions(&wrapper, Permissions::from_mode(0o700))
        .map_err(GitError::Workspace)?;
    Ok(Some(guard))
}

/// Writes an ephemeral askpass helper and token file outside command arguments.
fn prepare_http_command(
    workspace_root: &Path,
    credentials: Option<GitHttpCredentials<'_>>,
) -> Result<Option<HttpCommandGuard>, GitError> {
    let Some(credentials) = credentials else {
        return Ok(None);
    };
    let path = workspace_root.join(format!("https-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&path).map_err(GitError::Workspace)?;
    let guard = HttpCommandGuard { path };
    std::fs::set_permissions(&guard.path, Permissions::from_mode(0o700))
        .map_err(GitError::Workspace)?;
    let token = guard.path.join("token");
    let askpass = guard.path.join("askpass");
    std::fs::write(&token, credentials.token).map_err(GitError::Workspace)?;
    std::fs::write(
        &askpass,
        "#!/bin/sh\ncase \"$1\" in\n  *Username*) printf '%s\\n' x-access-token ;;\n  *) cat \"$DEPOT_GIT_TOKEN_FILE\" ;;\nesac\n",
    )
    .map_err(GitError::Workspace)?;
    std::fs::set_permissions(&token, Permissions::from_mode(0o600)).map_err(GitError::Workspace)?;
    std::fs::set_permissions(&askpass, Permissions::from_mode(0o700))
        .map_err(GitError::Workspace)?;
    Ok(Some(guard))
}

/// Runs a Git command under the configured wall-clock timeout.
async fn run_status(command: &mut Command, timeout: Duration) -> Result<(), GitError> {
    let status = tokio::time::timeout(timeout, command.status())
        .await
        .map_err(|_| GitError::Timeout)?
        .map_err(GitError::Spawn)?;
    if status.success() {
        Ok(())
    } else {
        Err(GitError::CheckoutFailed)
    }
}

/// Detects the most explicit supported builder and records why it was selected.
fn detect_builder(context: &Path, dockerfile: &Path) -> (DetectedBuilder, String) {
    if dockerfile.is_file() {
        return (
            DetectedBuilder::Dockerfile,
            "The configured Dockerfile exists.".to_owned(),
        );
    }
    if context.join("package.json").is_file() || context.join("Cargo.toml").is_file() {
        return (
            DetectedBuilder::Railpack,
            "A supported application manifest is available for Railpack detection.".to_owned(),
        );
    }
    if context.join("index.html").is_file() {
        return (
            DetectedBuilder::Static,
            "A root index.html is available as a static artifact.".to_owned(),
        );
    }
    (
        DetectedBuilder::Unknown,
        "No Dockerfile or supported application/static manifest was detected.".to_owned(),
    )
}

/// Returns the distinct TCP ports declared by EXPOSE in the Dockerfile's final stage.
fn dockerfile_exposed_ports(path: &Path) -> Vec<u16> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut logical_lines = Vec::new();
    let mut current = String::new();
    for raw in contents.lines() {
        let line = raw.trim();
        if line.is_empty() || (current.is_empty() && line.starts_with('#')) {
            continue;
        }
        let continued = line.ends_with('\\');
        let fragment = line.strip_suffix('\\').unwrap_or(line).trim();
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(fragment);
        if !continued {
            logical_lines.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        logical_lines.push(current);
    }

    let mut ports = Vec::new();
    for line in logical_lines {
        let mut fields = line.split_ascii_whitespace();
        let Some(instruction) = fields.next() else {
            continue;
        };
        if instruction.eq_ignore_ascii_case("FROM") {
            ports.clear();
            continue;
        }
        if !instruction.eq_ignore_ascii_case("EXPOSE") {
            continue;
        }
        for value in fields {
            let value = value.trim_matches([',', '"', '[', ']']);
            let (port, protocol) = value.split_once('/').unwrap_or((value, "tcp"));
            if !protocol.eq_ignore_ascii_case("tcp") {
                continue;
            }
            if let Ok(port) = port.parse::<u16>()
                && port > 0
                && !ports.contains(&port)
            {
                ports.push(port);
            }
        }
    }
    ports.sort_unstable();
    ports
}

/// Removes the isolated checkout on every success and error path.
struct WorkspaceGuard {
    path: PathBuf,
}

/// Owns materialized SSH credentials and removes them after the clone exits.
struct SshCommandGuard {
    path: PathBuf,
}

/// Owns an ephemeral HTTPS askpass helper and installation token.
struct HttpCommandGuard {
    path: PathBuf,
}

impl HttpCommandGuard {
    /// Configures non-interactive Git authentication without putting a token in argv.
    fn configure(&self, command: &mut Command) {
        command
            .env("GIT_ASKPASS", self.path.join("askpass"))
            .env("GIT_ASKPASS_REQUIRE", "force")
            .env("DEPOT_GIT_TOKEN_FILE", self.path.join("token"));
    }
}

impl Drop for HttpCommandGuard {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %self.path.display(), %error, "could not remove temporary HTTPS credentials");
        }
    }
}

impl SshCommandGuard {
    /// Applies fixed wrapper and credential paths without exposing key bytes in argv.
    fn configure(&self, command: &mut Command) {
        command
            .env("GIT_SSH", self.path.join("ssh"))
            .env("GIT_SSH_VARIANT", "ssh")
            .env("DEPOT_SSH_KEY_FILE", self.path.join("identity"))
            .env("DEPOT_SSH_KNOWN_HOSTS_FILE", self.path.join("known_hosts"));
    }
}

impl Drop for SshCommandGuard {
    /// Best-effort cleanup removes every plaintext credential file.
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %self.path.display(), %error, "could not remove temporary SSH credentials");
        }
    }
}

impl WorkspaceGuard {
    /// Owns a unique path which is deleted when inspection leaves scope.
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Returns the unique checkout destination owned by this guard.
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WorkspaceGuard {
    /// Best-effort cleanup never hides the primary inspection result.
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %self.path.display(), %error, "could not remove Git workspace");
        }
    }
}

impl Drop for RepositoryCheckout {
    /// Best-effort cleanup removes deployment source after its build attempt completes.
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %self.path.display(), %error, "could not remove deployment checkout");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use super::{
        DetectedBuilder, GitError, GitSshCredentials, detect_builder, dockerfile_exposed_ports,
        prepare_ssh_command, require_ssh_credentials,
    };

    /// Creates an isolated test directory without adding a runtime temp dependency.
    fn test_directory() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("depot-git-test-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&path).expect("test directory is created");
        path
    }

    /// Ensures an explicit Dockerfile takes precedence over inferred manifests.
    #[test]
    fn dockerfile_has_detection_precedence() {
        let path = test_directory();
        fs::write(path.join("Dockerfile"), "FROM scratch").expect("Dockerfile writes");
        fs::write(path.join("package.json"), "{}").expect("manifest writes");
        assert_eq!(
            detect_builder(&path, &path.join("Dockerfile")).0,
            DetectedBuilder::Dockerfile
        );
        fs::remove_dir_all(path).expect("test directory is removed");
    }

    /// Ensures plain HTML repositories are classified without a persistent runtime.
    #[test]
    fn root_html_detects_static_site() {
        let path = test_directory();
        fs::write(path.join("index.html"), "<!doctype html>").expect("HTML writes");
        assert_eq!(
            detect_builder(&path, &path.join("Dockerfile")).0,
            DetectedBuilder::Static
        );
        fs::remove_dir_all(path).expect("test directory is removed");
    }

    /// Uses only valid TCP declarations from the final Docker build stage.
    #[test]
    fn dockerfile_ports_are_deduplicated_and_stage_aware() {
        let path = test_directory();
        let dockerfile = path.join("Containerfile");
        fs::write(
            &dockerfile,
            "FROM node AS build\nEXPOSE 9999\nFROM alpine\nEXPOSE 3000 3000/tcp 5353/udp \\\n+             8080\nEXPOSE $PORT\n",
        )
        .expect("Dockerfile writes");
        assert_eq!(dockerfile_exposed_ports(&dockerfile), vec![3000, 8080]);
        fs::remove_dir_all(path).expect("test directory is removed");
    }

    /// Ensures plaintext SSH files are private, strict, and deleted with their guard.
    #[test]
    fn ssh_credentials_are_private_strict_and_ephemeral() {
        let root = test_directory();
        let guard = prepare_ssh_command(
            &root,
            Some(GitSshCredentials {
                private_key: "private-key",
                known_hosts: "example.com ssh-ed25519 AAAA",
            }),
        )
        .expect("SSH command prepares")
        .expect("guard exists");
        let credential_path = guard.path.clone();
        assert_eq!(
            fs::metadata(credential_path.join("identity"))
                .expect("identity metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let wrapper = fs::read_to_string(credential_path.join("ssh")).expect("wrapper reads");
        assert!(wrapper.contains("StrictHostKeyChecking=yes"));
        assert!(wrapper.contains("BatchMode=yes"));
        assert!(!wrapper.contains("StrictHostKeyChecking=no"));
        drop(guard);
        assert!(!credential_path.exists());
        fs::remove_dir_all(root).expect("test workspace root is removed");
    }

    /// Ensures an SSH URL can never silently use the worker account's ambient keys.
    #[test]
    fn ssh_repositories_require_project_credentials() {
        assert!(matches!(
            require_ssh_credentials("ssh://git@example.com/repository.git", None),
            Err(GitError::MissingSshCredentials)
        ));
        require_ssh_credentials("https://example.com/repository.git", None)
            .expect("HTTPS does not need SSH material");
    }

    /// Exercises a real public remote only when an operator explicitly supplies its URL.
    #[tokio::test]
    #[ignore = "requires DEPOT_LIVE_GIT_URL and external network access"]
    async fn live_public_repository_resolves_commit() {
        let repository_url = std::env::var("DEPOT_LIVE_GIT_URL")
            .expect("DEPOT_LIVE_GIT_URL must name a public repository");
        let workspace = test_directory();
        let result = super::inspect_repository(
            &repository_url,
            "main",
            ".",
            "Dockerfile",
            &workspace,
            std::time::Duration::from_secs(120),
            None,
            None,
        )
        .await
        .expect("public repository inspection succeeds");
        assert_eq!(result.commit_sha.len(), 40);
        fs::remove_dir_all(workspace).expect("test workspace root is removed");
    }
}
