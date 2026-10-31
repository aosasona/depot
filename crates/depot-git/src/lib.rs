//! Safe Git checkout and inspectable build-plan detection for worker jobs.

use std::{
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
    #[error("checked out repository did not resolve to a valid commit SHA")]
    InvalidCommit,
}

/// Clones one branch without a shell, resolves its immutable SHA, and detects a build plan.
pub async fn inspect_repository(
    repository_url: &str,
    branch: &str,
    workspace_root: &Path,
    timeout: Duration,
) -> Result<RepositoryInspection, GitError> {
    validate_repository_url(repository_url)?;
    std::fs::create_dir_all(workspace_root).map_err(GitError::Workspace)?;
    let checkout_path = workspace_root.join(format!("inspect-{}", uuid::Uuid::now_v7()));
    let workspace = WorkspaceGuard::new(checkout_path);
    let mut clone = git_command();
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
    let (builder, reason) = detect_builder(workspace.path());
    Ok(RepositoryInspection {
        commit_sha,
        builder,
        reason,
        has_mise_config: workspace.path().join("mise.toml").is_file()
            || workspace.path().join(".mise.toml").is_file(),
    })
}

/// Checks out a branch and verifies it resolves to the deployment's recorded commit SHA.
pub async fn checkout_repository(
    repository_url: &str,
    branch: &str,
    expected_commit_sha: &str,
    workspace_root: &Path,
    timeout: Duration,
) -> Result<RepositoryCheckout, GitError> {
    if expected_commit_sha.len() != 40
        || !expected_commit_sha
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(GitError::InvalidCommit);
    }
    validate_repository_url(repository_url)?;
    std::fs::create_dir_all(workspace_root).map_err(GitError::Workspace)?;
    let checkout_path = workspace_root.join(format!("deploy-{}", uuid::Uuid::now_v7()));
    let mut clone = git_command();
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
fn detect_builder(repository: &Path) -> (DetectedBuilder, String) {
    if repository.join("Dockerfile").is_file() {
        return (
            DetectedBuilder::Dockerfile,
            "Dockerfile exists at the repository root.".to_owned(),
        );
    }
    if repository.join("package.json").is_file() || repository.join("Cargo.toml").is_file() {
        return (
            DetectedBuilder::Railpack,
            "A supported application manifest is available for Railpack detection.".to_owned(),
        );
    }
    if repository.join("index.html").is_file() {
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

/// Removes the isolated checkout on every success and error path.
struct WorkspaceGuard {
    path: PathBuf,
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
    use std::fs;

    use super::{DetectedBuilder, detect_builder};

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
        assert_eq!(detect_builder(&path).0, DetectedBuilder::Dockerfile);
        fs::remove_dir_all(path).expect("test directory is removed");
    }

    /// Ensures plain HTML repositories are classified without a persistent runtime.
    #[test]
    fn root_html_detects_static_site() {
        let path = test_directory();
        fs::write(path.join("index.html"), "<!doctype html>").expect("HTML writes");
        assert_eq!(detect_builder(&path).0, DetectedBuilder::Static);
        fs::remove_dir_all(path).expect("test directory is removed");
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
            &workspace,
            std::time::Duration::from_secs(120),
        )
        .await
        .expect("public repository inspection succeeds");
        assert_eq!(result.commit_sha.len(), 40);
        fs::remove_dir_all(workspace).expect("test workspace root is removed");
    }
}
