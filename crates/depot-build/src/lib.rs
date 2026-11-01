//! Contained build-engine adapters which never invoke repository commands through a shell.

use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};

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

/// Inputs for one isolated Node static build and immutable artifact extraction.
#[derive(Debug)]
pub struct StaticBuildRequest<'a> {
    pub docker_binary: &'a str,
    pub checkout_path: &'a Path,
    pub context_path: &'a str,
    pub output_directory: &'a str,
    pub static_root: &'a Path,
    pub project_id: &'a str,
    pub release_id: &'a str,
    pub timeout: Duration,
}

/// Promoted static artifact identity and bounded builder diagnostics.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StaticBuildResult {
    pub release_path: PathBuf,
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
    #[error("static output requires a supported npm, pnpm, or Yarn project")]
    UnsupportedStaticProject,
    #[error("static artifact extraction failed: {0}")]
    StaticArtifact(String),
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

/// Builds a static Node project and atomically promotes a validated artifact tree.
pub async fn build_static(
    request: StaticBuildRequest<'_>,
) -> Result<StaticBuildResult, BuildError> {
    let checkout = request
        .checkout_path
        .canonicalize()
        .map_err(|_| BuildError::UnsafePath)?;
    let context = request
        .checkout_path
        .join(request.context_path)
        .canonicalize()
        .map_err(|_| BuildError::UnsafePath)?;
    if !context.starts_with(&checkout) || !safe_relative_path(request.output_directory) {
        return Err(BuildError::UnsafePath);
    }
    let package_command = static_package_command(&context)?;
    let project_root = request.static_root.join(request.project_id);
    let releases_root = project_root.join("releases");
    fs::create_dir_all(&releases_root)
        .map_err(|error| BuildError::StaticArtifact(error.to_string()))?;
    let staging = project_root.join(format!(".staging-{}", request.release_id));
    let release = releases_root.join(request.release_id);
    if staging.exists() || release.exists() {
        return Err(BuildError::StaticArtifact(
            "release staging path already exists".to_owned(),
        ));
    }
    fs::create_dir_all(&staging).map_err(|error| BuildError::StaticArtifact(error.to_string()))?;

    let result = build_and_extract_static_image(
        request.docker_binary,
        &context,
        request.output_directory,
        package_command,
        &staging,
        request.timeout,
    )
    .await;
    let logs = match result {
        Ok(logs) => logs,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    validate_artifact_tree(&staging)?;
    fs::rename(&staging, &release)
        .map_err(|error| BuildError::StaticArtifact(error.to_string()))?;
    Ok(StaticBuildResult {
        release_path: release,
        logs,
    })
}

/// Selects a deterministic install/build command from a committed Node lockfile.
fn static_package_command(context: &Path) -> Result<&'static str, BuildError> {
    if context.join("pnpm-lock.yaml").is_file() {
        Ok("corepack enable && pnpm install --frozen-lockfile && pnpm run build")
    } else if context.join("yarn.lock").is_file() {
        Ok("corepack enable && yarn install --immutable && yarn build")
    } else if context.join("package-lock.json").is_file() {
        Ok("npm ci && npm run build")
    } else {
        Err(BuildError::UnsupportedStaticProject)
    }
}

/// Builds an artifact-only image, copies `/output`, and removes temporary Docker state.
async fn build_and_extract_static_image(
    docker_binary: &str,
    context: &Path,
    output_directory: &str,
    package_command: &str,
    staging: &Path,
    timeout: Duration,
) -> Result<String, BuildError> {
    let nonce = uuid::Uuid::now_v7();
    let dockerfile_path = std::env::temp_dir().join(format!("depot-static-{nonce}.Dockerfile"));
    let iid_path = std::env::temp_dir().join(format!("depot-static-{nonce}.iid"));
    let dockerfile = format!(
        "FROM node:22-alpine AS build\nWORKDIR /workspace\nCOPY . .\nRUN {package_command}\nFROM alpine:3.21\nCOPY --from=build /workspace/{output_directory} /output\n"
    );
    fs::write(&dockerfile_path, dockerfile).map_err(BuildError::ImageId)?;
    let mut build = Command::new(docker_binary);
    build
        .args(["build", "--pull", "--file"])
        .arg(&dockerfile_path)
        .args(["--iidfile"])
        .arg(&iid_path)
        .arg(context)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(timeout, build.output())
        .await
        .map_err(|_| BuildError::Timeout)?
        .map_err(BuildError::Spawn)?;
    let logs = bounded_logs(&output.stdout, &output.stderr);
    let _ = fs::remove_file(&dockerfile_path);
    if !output.status.success() {
        let _ = fs::remove_file(&iid_path);
        return Err(BuildError::Failed(logs));
    }
    let image_id = fs::read_to_string(&iid_path).map_err(BuildError::ImageId)?;
    let _ = fs::remove_file(&iid_path);
    let image_id = image_id.trim().to_owned();
    if !valid_image_id(&image_id) {
        return Err(BuildError::InvalidImageId);
    }
    let mut create = Command::new(docker_binary);
    create
        .args(["create", &image_id, "/bin/true"])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let created = tokio::time::timeout(timeout, create.output())
        .await
        .map_err(|_| BuildError::Timeout)?
        .map_err(BuildError::Spawn)?;
    if !created.status.success() {
        return Err(BuildError::StaticArtifact(
            "could not create artifact extraction container".to_owned(),
        ));
    }
    let container_id = String::from_utf8_lossy(&created.stdout).trim().to_owned();
    let mut copy = Command::new(docker_binary);
    copy.args(["cp", &format!("{container_id}:/output/.")])
        .arg(staging)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let copied = tokio::time::timeout(timeout, copy.output())
        .await
        .map_err(|_| BuildError::Timeout)?
        .map_err(BuildError::Spawn)?;
    let mut remove = Command::new(docker_binary);
    let _ = remove.args(["rm", "--force", &container_id]).output().await;
    let mut remove_image = Command::new(docker_binary);
    let _ = remove_image.args(["image", "rm", &image_id]).output().await;
    if !copied.status.success() {
        return Err(BuildError::StaticArtifact(
            "could not copy the configured output directory".to_owned(),
        ));
    }
    Ok(logs)
}

/// Rejects absolute paths, parent traversal, and empty static output paths.
fn safe_relative_path(value: &str) -> bool {
    let path = Path::new(value);
    !value.trim().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Walks extracted artifacts without following symlinks or accepting special files.
fn validate_artifact_tree(root: &Path) -> Result<(), BuildError> {
    let mut pending = vec![root.to_owned()];
    let mut files = 0_u64;
    let mut bytes = 0_u64;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| BuildError::StaticArtifact(error.to_string()))?
        {
            let entry = entry.map_err(|error| BuildError::StaticArtifact(error.to_string()))?;
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| BuildError::StaticArtifact(error.to_string()))?;
            if metadata.file_type().is_symlink() {
                return Err(BuildError::StaticArtifact(
                    "static output contains a symbolic link".to_owned(),
                ));
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                files += 1;
                bytes = bytes.saturating_add(metadata.len());
            } else {
                return Err(BuildError::StaticArtifact(
                    "static output contains a special file".to_owned(),
                ));
            }
            if files > 100_000 || bytes > 2 * 1024 * 1024 * 1024 {
                return Err(BuildError::StaticArtifact(
                    "static output exceeds artifact limits".to_owned(),
                ));
            }
        }
    }
    if files == 0 {
        return Err(BuildError::StaticArtifact(
            "static output is empty".to_owned(),
        ));
    }
    Ok(())
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

    use super::{
        BuildError, DockerBuildRequest, build_dockerfile, safe_relative_path,
        validate_artifact_tree,
    };

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

    /// Prevents configured artifact paths from escaping the verified build context.
    #[test]
    fn static_output_paths_must_be_normal_relative_paths() {
        assert!(safe_relative_path("dist"));
        assert!(safe_relative_path("apps/site/dist"));
        assert!(!safe_relative_path(""));
        assert!(!safe_relative_path("/tmp/dist"));
        assert!(!safe_relative_path("../dist"));
        assert!(!safe_relative_path("dist/../public"));
    }

    /// Rejects symlinks so a promoted artifact cannot escape its immutable root.
    #[test]
    fn static_artifacts_reject_symbolic_links() {
        let root = std::env::temp_dir().join(format!("depot-static-test-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&root).expect("test root is created");
        fs::write(root.join("index.html"), "ok").expect("index writes");
        std::os::unix::fs::symlink("/etc/passwd", root.join("leak")).expect("symlink writes");
        assert!(matches!(
            validate_artifact_tree(&root),
            Err(BuildError::StaticArtifact(_))
        ));
        fs::remove_dir_all(root).expect("test root is removed");
    }
}
