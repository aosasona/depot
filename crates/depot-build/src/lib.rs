//! Contained build-engine adapters which never invoke repository commands through a shell.

use std::{path::Path, process::Stdio, time::Duration};

use serde::Serialize;
use thiserror::Error;
use tokio::process::Command;

const MAX_LOG_BYTES: usize = 256 * 1024;

/// Inputs for one Dockerfile image build from an already verified checkout.
#[derive(Debug)]
pub struct DockerBuildRequest<'a> {
    pub docker_binary: &'a str,
    pub checkout_path: &'a Path,
    pub context_path: &'a str,
    pub dockerfile_path: &'a str,
    pub image_tag: &'a str,
    pub timeout: Duration,
}

/// Immutable image identity and bounded diagnostic output from a successful build.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DockerBuildResult {
    pub image_id: String,
    pub image_tag: String,
    pub logs: String,
}

/// Safe failures from Docker startup, timeout, build status, or image-ID validation.
#[derive(Debug, Error)]
pub enum BuildError {
    #[error("Docker could not be started: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("Docker build exceeded its configured timeout")]
    Timeout,
    #[error("Docker build failed: {0}")]
    Failed(String),
    #[error("Docker did not return a valid immutable image ID")]
    InvalidImageId,
    #[error("could not read Docker's image ID output: {0}")]
    ImageId(#[source] std::io::Error),
    #[error("Docker build paths must remain inside the verified repository checkout")]
    UnsafePath,
}

/// Builds a Dockerfile using argument-safe process invocation and a verified image-ID file.
pub async fn build_dockerfile(
    request: DockerBuildRequest<'_>,
) -> Result<DockerBuildResult, BuildError> {
    let iid_path = std::env::temp_dir().join(format!("depot-iid-{}", uuid::Uuid::now_v7()));
    let checkout = request
        .checkout_path
        .canonicalize()
        .map_err(|_| BuildError::UnsafePath)?;
    let context = request
        .checkout_path
        .join(request.context_path)
        .canonicalize()
        .map_err(|_| BuildError::UnsafePath)?;
    let dockerfile = request
        .checkout_path
        .join(request.dockerfile_path)
        .canonicalize()
        .map_err(|_| BuildError::UnsafePath)?;
    if !context.starts_with(&checkout) || !dockerfile.starts_with(&checkout) {
        return Err(BuildError::UnsafePath);
    }
    let mut command = Command::new(request.docker_binary);
    command
        .args(["build", "--pull", "--file"])
        .arg(dockerfile)
        .args(["--tag", request.image_tag, "--iidfile"])
        .arg(&iid_path)
        .arg(context)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(request.timeout, command.output())
        .await
        .map_err(|_| BuildError::Timeout)?
        .map_err(BuildError::Spawn)?;
    let logs = bounded_logs(&output.stdout, &output.stderr);
    if !output.status.success() {
        let _ = std::fs::remove_file(&iid_path);
        return Err(BuildError::Failed(logs));
    }
    let image_id = std::fs::read_to_string(&iid_path).map_err(BuildError::ImageId)?;
    let _ = std::fs::remove_file(&iid_path);
    let image_id = image_id.trim().to_owned();
    if !valid_image_id(&image_id) {
        return Err(BuildError::InvalidImageId);
    }
    Ok(DockerBuildResult {
        image_id,
        image_tag: request.image_tag.to_owned(),
        logs,
    })
}

/// Combines process streams into a UTF-8-lossy diagnostic bounded for durable storage.
fn bounded_logs(stdout: &[u8], stderr: &[u8]) -> String {
    let mut bytes = Vec::with_capacity((stdout.len() + stderr.len()).min(MAX_LOG_BYTES));
    for source in [stdout, stderr] {
        let remaining = MAX_LOG_BYTES.saturating_sub(bytes.len());
        bytes.extend_from_slice(&source[..source.len().min(remaining)]);
        if bytes.len() == MAX_LOG_BYTES {
            break;
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Accepts only Docker's canonical sha256 image identifier representation.
fn valid_image_id(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

    use super::{DockerBuildRequest, build_dockerfile};

    /// Proves the adapter passes paths as arguments and accepts canonical image IDs.
    #[tokio::test]
    async fn docker_adapter_returns_verified_image_id() {
        let root = std::env::temp_dir().join(format!("depot-build-test-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&root).expect("test root is created");
        fs::write(root.join("Dockerfile"), "FROM scratch").expect("Dockerfile writes");
        let fake = root.join("fake-docker");
        fs::write(
            &fake,
            "#!/bin/sh\nwhile [ \"$1\" != \"--iidfile\" ]; do shift; done\nshift\nprintf 'sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' > \"$1\"\nprintf 'build complete'\n",
        )
        .expect("fake Docker writes");
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).expect("fake is executable");
        let result = build_dockerfile(DockerBuildRequest {
            docker_binary: fake.to_str().expect("path is UTF-8"),
            checkout_path: &root,
            context_path: ".",
            dockerfile_path: "Dockerfile",
            image_tag: "depot/example:test",
            timeout: Duration::from_secs(5),
        })
        .await
        .expect("build succeeds");
        assert_eq!(result.image_id.len(), 71);
        assert_eq!(result.logs, "build complete");
        fs::remove_dir_all(root).expect("test root is removed");
    }
}
