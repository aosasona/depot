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
    pub railpack_binary: &'a str,
    pub checkout_path: &'a Path,
    pub context_path: &'a str,
    pub dockerfile_path: &'a str,
    pub builder: &'a str,
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
    #[error("static output requires a package.json, Dockerfile, or Railpack project")]
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
    let builder = resolve_static_builder(
        request.builder,
        &checkout,
        &context,
        request.dockerfile_path,
        request.output_directory,
    )?;
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

    let result = match builder {
        StaticBuilder::Node(package_command) => {
            build_and_extract_static_image(
                request.docker_binary,
                &context,
                request.output_directory,
                package_command,
                &staging,
                request.timeout,
            )
            .await
        }
        StaticBuilder::Dockerfile => {
            build_and_extract_dockerfile_static(&request, &context, &staging).await
        }
        StaticBuilder::Railpack => build_and_extract_railpack(&request, &context, &staging).await,
        StaticBuilder::Passthrough(source) => {
            copy_static_directory(&source, &staging)?;
            Ok(format!(
                "No build step required; published committed static files from {}.",
                source.strip_prefix(&context).unwrap_or(&source).display()
            ))
        }
    };
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

/// Supported static build engines after resolving the user's explicit or automatic choice.
enum StaticBuilder {
    Node(&'static str),
    Dockerfile,
    Railpack,
    Passthrough(PathBuf),
}

/// Resolves auto to a Dockerfile first and then the deterministic Node builder.
fn resolve_static_builder(
    configured: &str,
    checkout: &Path,
    context: &Path,
    dockerfile_path: &str,
    output_directory: &str,
) -> Result<StaticBuilder, BuildError> {
    match configured {
        "dockerfile" => Ok(StaticBuilder::Dockerfile),
        "railpack" => Ok(StaticBuilder::Railpack),
        "auto" if checkout.join(dockerfile_path).is_file() => Ok(StaticBuilder::Dockerfile),
        "auto" => match static_package_command(context) {
            Ok(command) => Ok(StaticBuilder::Node(command)),
            Err(_) => resolve_passthrough_root(context, output_directory)
                .map(StaticBuilder::Passthrough)
                .ok_or(BuildError::UnsupportedStaticProject),
        },
        _ => Err(BuildError::UnsupportedStaticProject),
    }
}

/// Chooses committed static output, preferring the configured directory then the context root.
fn resolve_passthrough_root(context: &Path, output_directory: &str) -> Option<PathBuf> {
    let configured = context.join(output_directory);
    let configured_is_safe = fs::symlink_metadata(&configured)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        && configured
            .canonicalize()
            .is_ok_and(|path| path.starts_with(context));
    if configured_is_safe && configured.join("index.html").is_file() {
        return Some(configured);
    }
    context
        .join("index.html")
        .is_file()
        .then(|| context.to_owned())
}

/// Copies a committed static tree without following links or publishing Git metadata.
fn copy_static_directory(source: &Path, destination: &Path) -> Result<(), BuildError> {
    for entry in
        fs::read_dir(source).map_err(|error| BuildError::StaticArtifact(error.to_string()))?
    {
        let entry = entry.map_err(|error| BuildError::StaticArtifact(error.to_string()))?;
        if entry.file_name() == ".git" {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| BuildError::StaticArtifact(error.to_string()))?;
        if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
            return Err(BuildError::StaticArtifact(
                "plain static output contains a link or special file".to_owned(),
            ));
        }
        let target = destination.join(entry.file_name());
        if metadata.is_dir() {
            fs::create_dir(&target)
                .map_err(|error| BuildError::StaticArtifact(error.to_string()))?;
            copy_static_directory(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)
                .map_err(|error| BuildError::StaticArtifact(error.to_string()))?;
        }
    }
    Ok(())
}

/// Selects a deterministic install/build command from a committed Node lockfile.
fn static_package_command(context: &Path) -> Result<&'static str, BuildError> {
    if context.join("pnpm-lock.yaml").is_file() {
        Ok("corepack enable && pnpm install --frozen-lockfile && pnpm run build")
    } else if context.join("yarn.lock").is_file() {
        Ok("corepack enable && yarn install --immutable && yarn build")
    } else if context.join("package-lock.json").is_file() {
        Ok("npm ci && npm run build")
    } else if context.join("package.json").is_file() {
        Ok("npm install --no-audit --no-fund && npm run build")
    } else {
        Err(BuildError::UnsupportedStaticProject)
    }
}

/// Builds a configured Dockerfile and extracts the static tree from its final image.
async fn build_and_extract_dockerfile_static(
    request: &StaticBuildRequest<'_>,
    _context: &Path,
    staging: &Path,
) -> Result<String, BuildError> {
    let image_tag = format!("depot/{}:static-{}", request.project_id, request.release_id);
    let built = build_dockerfile(DockerBuildRequest {
        docker_binary: request.docker_binary,
        checkout_path: request.checkout_path,
        context_path: request.context_path,
        dockerfile_path: request.dockerfile_path,
        image_tag: &image_tag,
        timeout: request.timeout,
    })
    .await?;
    let artifact_path = dockerfile_artifact_path(
        request.checkout_path,
        request.dockerfile_path,
        request.output_directory,
    );
    let extraction = extract_image_directory(
        request.docker_binary,
        &built.image_id,
        &artifact_path,
        staging,
        request.timeout,
    )
    .await;
    let mut remove_image = Command::new(request.docker_binary);
    let _ = remove_image
        .args(["image", "rm", &built.image_id])
        .output()
        .await;
    extraction.map(|()| built.logs)
}

/// Follows a simple multi-stage COPY when the final image relocates the configured output.
fn dockerfile_artifact_path(checkout: &Path, dockerfile_path: &str, output: &str) -> String {
    let Ok(contents) = fs::read_to_string(checkout.join(dockerfile_path)) else {
        return output.to_owned();
    };
    for line in contents.lines().rev() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 4
            || !fields[0].eq_ignore_ascii_case("COPY")
            || !fields[1].starts_with("--from=")
        {
            continue;
        }
        let source = fields[fields.len() - 2].trim_end_matches('/');
        let destination = fields[fields.len() - 1].trim_end_matches('/');
        let source_matches = source == output.trim_matches('/')
            || source.ends_with(&format!("/{}", output.trim_matches('/')));
        let destination_is_safe = destination.starts_with('/')
            && destination.len() <= 255
            && !destination.split('/').any(|segment| segment == "..");
        if source_matches && destination_is_safe {
            return destination.trim_start_matches('/').to_owned();
        }
    }
    output.to_owned()
}

/// Loads a Railpack image into Docker and copies only the configured static artifact.
async fn build_and_extract_railpack(
    request: &StaticBuildRequest<'_>,
    context: &Path,
    staging: &Path,
) -> Result<String, BuildError> {
    let image_tag = format!(
        "depot/{}:railpack-static-{}",
        request.project_id, request.release_id
    );
    let mut command = Command::new(request.railpack_binary);
    command
        .args(["build", "--progress", "plain", "--name", &image_tag])
        .arg(context)
        .env("BUILDKIT_HOST", "docker-container://depot-buildkit")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(request.timeout, command.output())
        .await
        .map_err(|_| BuildError::Timeout)?
        .map_err(BuildError::Spawn)?;
    let logs = bounded_logs(&output.stdout, &output.stderr);
    if !output.status.success() {
        return Err(BuildError::Failed(logs));
    }
    let artifact_result = extract_railpack_image_directory(
        request.docker_binary,
        &image_tag,
        request.output_directory,
        staging,
        request.timeout,
    )
    .await;
    let mut remove_image = Command::new(request.docker_binary);
    let _ = remove_image
        .args(["image", "rm", "--force", &image_tag])
        .output()
        .await;
    artifact_result?;
    Ok(logs)
}

/// Tries Railpack's conventional application roots without exporting its full filesystem.
async fn extract_railpack_image_directory(
    docker_binary: &str,
    image: &str,
    output_directory: &str,
    staging: &Path,
    timeout: Duration,
) -> Result<(), BuildError> {
    for prefix in ["", "app", "workspace"] {
        let candidate = if prefix.is_empty() {
            output_directory.to_owned()
        } else {
            format!("{prefix}/{output_directory}")
        };
        if extract_image_directory(docker_binary, image, &candidate, staging, timeout)
            .await
            .is_ok()
        {
            return Ok(());
        }
    }
    Err(BuildError::StaticArtifact(format!(
        "configured output directory '{output_directory}' was not present in the Railpack image"
    )))
}

/// Copies one directory from an immutable image into the release staging tree.
async fn extract_image_directory(
    docker_binary: &str,
    image_id: &str,
    output_directory: &str,
    staging: &Path,
    timeout: Duration,
) -> Result<(), BuildError> {
    let mut create = Command::new(docker_binary);
    create
        .args(["create", image_id, "/bin/true"])
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
    let container_path = format!("{container_id}:/{}/.", output_directory.trim_matches('/'));
    let mut copy = Command::new(docker_binary);
    copy.args(["cp", &container_path])
        .arg(staging)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let copied = tokio::time::timeout(timeout, copy.output())
        .await
        .map_err(|_| BuildError::Timeout)?
        .map_err(BuildError::Spawn)?;
    let mut remove = Command::new(docker_binary);
    let _ = remove.args(["rm", "--force", &container_id]).output().await;
    if copied.status.success() {
        Ok(())
    } else {
        Err(BuildError::StaticArtifact(format!(
            "configured output directory '{output_directory}' was not present in the final Docker image"
        )))
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
    if value.trim() == "." {
        return true;
    }
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
    use std::{
        fs::{self, File},
        io::Write,
        os::unix::fs::PermissionsExt,
        path::Path,
        time::Duration,
    };

    use super::{
        BuildError, DockerBuildRequest, StaticBuildRequest, build_and_extract_railpack,
        build_dockerfile, build_static, dockerfile_artifact_path, safe_relative_path,
        static_package_command, validate_artifact_tree,
    };

    fn write_executable(path: &Path, contents: &str) {
        let mut file = File::create(path).expect("fake executable is created");
        file.write_all(contents.as_bytes())
            .expect("fake executable writes");
        file.sync_all().expect("fake executable is flushed");
        drop(file);
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .expect("fake executable is executable");
    }

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

    /// Proves static Railpack builds load an image instead of exporting privileged metadata.
    #[tokio::test]
    async fn railpack_static_build_extracts_only_the_artifact() {
        let root =
            std::env::temp_dir().join(format!("depot-railpack-test-{}", uuid::Uuid::now_v7()));
        let staging = root.join("staging");
        fs::create_dir_all(&staging).expect("staging directory is created");
        let arguments = root.join("railpack-arguments");
        let fake_railpack = root.join("fake-railpack");
        write_executable(
            &fake_railpack,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf 'railpack complete'\n",
                arguments.display()
            ),
        );
        let fake_docker = root.join("fake-docker");
        write_executable(
            &fake_docker,
            "#!/bin/sh\ncase \"$1\" in\ncreate) printf 'container\\n' ;;\ncp) mkdir -p \"$3\"; printf 'ok' > \"$3/index.html\" ;;\nrm|image) ;;\n*) exit 1 ;;\nesac\n",
        );
        let request = StaticBuildRequest {
            docker_binary: fake_docker.to_str().expect("Docker path is UTF-8"),
            railpack_binary: fake_railpack.to_str().expect("Railpack path is UTF-8"),
            checkout_path: &root,
            context_path: ".",
            dockerfile_path: "Dockerfile",
            builder: "railpack",
            output_directory: "dist",
            static_root: &root,
            project_id: "project",
            release_id: "release",
            timeout: Duration::from_secs(5),
        };
        let logs = build_and_extract_railpack(&request, &root, &staging)
            .await
            .expect("Railpack artifact extracts");
        let invoked = fs::read_to_string(arguments).expect("Railpack arguments read");
        assert!(invoked.contains("--name"));
        assert!(!invoked.contains("--output"));
        assert_eq!(logs, "railpack complete");
        assert_eq!(
            fs::read_to_string(staging.join("index.html")).expect("artifact reads"),
            "ok"
        );
        fs::remove_dir_all(root).expect("test root is removed");
    }

    /// Prevents configured artifact paths from escaping the verified build context.
    #[test]
    fn static_output_paths_must_be_normal_relative_paths() {
        assert!(safe_relative_path("."));
        assert!(safe_relative_path("dist"));
        assert!(safe_relative_path("apps/site/dist"));
        assert!(!safe_relative_path(""));
        assert!(!safe_relative_path("/tmp/dist"));
        assert!(!safe_relative_path("../dist"));
        assert!(!safe_relative_path("dist/../public"));
    }

    /// Publishes committed HTML directly when no build manifest or Dockerfile exists.
    #[tokio::test]
    async fn plain_html_static_site_requires_no_builder() {
        let root =
            std::env::temp_dir().join(format!("depot-plain-static-{}", uuid::Uuid::now_v7()));
        let checkout = root.join("checkout");
        let static_root = root.join("static");
        fs::create_dir_all(checkout.join("assets")).expect("checkout is created");
        fs::create_dir(checkout.join(".git")).expect("Git metadata is created");
        fs::write(checkout.join("index.html"), "<h1>plain</h1>").expect("index writes");
        fs::write(checkout.join("assets/site.css"), "body{}").expect("asset writes");

        let result = build_static(StaticBuildRequest {
            docker_binary: "docker-is-not-needed",
            railpack_binary: "railpack-is-not-needed",
            checkout_path: &checkout,
            context_path: ".",
            dockerfile_path: "Dockerfile",
            builder: "auto",
            output_directory: "dist",
            static_root: &static_root,
            project_id: "plain",
            release_id: "release",
            timeout: Duration::from_secs(5),
        })
        .await
        .expect("plain site publishes");

        assert_eq!(
            fs::read_to_string(result.release_path.join("index.html"))
                .expect("published index reads"),
            "<h1>plain</h1>"
        );
        assert!(!result.release_path.join(".git").exists());
        assert!(result.logs.contains("No build step required"));
        fs::remove_dir_all(root).expect("test root removes");
    }

    /// Proves a conventional package.json can build without a committed lockfile.
    #[test]
    fn static_node_builder_accepts_package_json_without_lockfile() {
        let root =
            std::env::temp_dir().join(format!("depot-static-package-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&root).expect("test root is created");
        fs::write(root.join("package.json"), "{}").expect("package manifest writes");
        assert!(
            static_package_command(&root)
                .expect("command resolves")
                .contains("npm install")
        );
        fs::remove_dir_all(root).expect("test root is removed");
    }

    /// Follows the final image location used by Chimney-style multi-stage builds.
    #[test]
    fn dockerfile_static_output_follows_multistage_copy() {
        let root =
            std::env::temp_dir().join(format!("depot-static-dockerfile-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&root).expect("test root is created");
        fs::write(
            root.join("Dockerfile"),
            "FROM node AS build\nFROM ghcr.io/aosasona/chimney:latest\nCOPY --from=build /app/dist /var/www/html/example\n",
        )
        .expect("Dockerfile writes");
        assert_eq!(
            dockerfile_artifact_path(&root, "Dockerfile", "dist"),
            "var/www/html/example"
        );
        fs::remove_dir_all(root).expect("test root is removed");
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
