//! Depot's single executable and process-mode entry point.

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    Json, Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use clap::{Parser, Subcommand};
use depot_auth::{issue_bootstrap_token, issue_deploy_token};
use depot_core::{Config, ShellHandshake, WorkerOperation};
use depot_store::Store;
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::Duration;
use tokio::net::{TcpListener, UnixListener, UnixStream};
use tracing::info;
use tracing_subscriber::EnvFilter;
use zeroize::Zeroize;

/// Depot's top-level command-line interface.
#[derive(Debug, Parser)]
#[command(name = "depot", version, about = "Self-hosted application deployments")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Selects the independently runnable mode of the Depot executable.
#[derive(Debug, Subcommand)]
enum Command {
    /// Creates or rotates the short-lived token used for first-admin setup.
    Setup {
        /// TOML configuration path; local development defaults apply when omitted.
        #[arg(long, env = "DEPOT_CONFIG")]
        config: Option<PathBuf>,
        /// Number of minutes before the displayed setup token expires.
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(i64).range(1..=120))]
        expires_in_minutes: i64,
    },
    /// Creates a project-scoped bearer token for curl and CI deployments.
    DeployToken {
        /// TOML configuration path; local development defaults apply when omitted.
        #[arg(long, env = "DEPOT_CONFIG")]
        config: Option<PathBuf>,
        /// Opaque project identifier shown by the dashboard and API.
        #[arg(long)]
        project_id: String,
        /// Active user recorded as the credential creator and deployment actor.
        #[arg(long)]
        created_by_user_id: String,
        /// Human-readable purpose retained after the secret is hidden.
        #[arg(long, default_value = "CI deployment")]
        label: String,
    },
    /// Generates a new master key file without overwriting an existing key.
    GenerateKey {
        /// Destination referenced by security.master_key_file.
        #[arg(long)]
        output: PathBuf,
    },
    /// Serves the API, private gateway, and embedded web application.
    Serve {
        /// TOML configuration path; local development defaults apply when omitted.
        #[arg(long, env = "DEPOT_CONFIG")]
        config: Option<PathBuf>,
    },
    /// Applies all pending database migrations and exits.
    Migrate {
        /// TOML configuration path; local development defaults apply when omitted.
        #[arg(long, env = "DEPOT_CONFIG")]
        config: Option<PathBuf>,
    },
    /// Checks configuration, database access, migrations, and listener availability.
    Doctor {
        /// TOML configuration path; local development defaults apply when omitted.
        #[arg(long, env = "DEPOT_CONFIG")]
        config: Option<PathBuf>,
    },
    /// Interactively resets one local user's password from the trusted server console.
    ResetPassword {
        /// TOML configuration path; local development defaults apply when omitted.
        #[arg(long, env = "DEPOT_CONFIG")]
        config: Option<PathBuf>,
    },
    /// Validates a configuration file without opening the database or binding a port.
    ValidateConfig {
        #[arg(long, env = "DEPOT_CONFIG")]
        config: PathBuf,
    },
    /// Claims and executes durable background jobs from the shared SQLite queue.
    Worker {
        /// TOML configuration path; local development defaults apply when omitted.
        #[arg(long, env = "DEPOT_CONFIG")]
        config: Option<PathBuf>,
        /// Claims at most one available job and exits, primarily for supervision checks.
        #[arg(long)]
        once: bool,
    },
    /// Reconciles an authenticated desired-domain snapshot on the Caddy host.
    CaddyAgent {
        /// Host-owned Caddy agent TOML configuration path.
        #[arg(long, env = "DEPOT_CADDY_AGENT_CONFIG")]
        config: PathBuf,
        /// JSON file containing the complete desired hostname array.
        #[arg(long)]
        domains_file: Option<PathBuf>,
        /// Validates agent configuration and the full Caddyfile, then exits.
        #[arg(long, conflicts_with = "domains_file")]
        check: bool,
    },
}

/// Describes a successful liveness response without overstating dependency health.
#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

/// Trusted queue payload linking a deployment build job to its durable record.
#[derive(Debug, Deserialize)]
struct DeploymentJobPayload {
    deployment_id: String,
}

/// Durable intent optionally chaining repository inspection into a first deployment.
#[derive(Debug, Default, Deserialize)]
struct InspectionJobPayload {
    #[serde(default)]
    deploy_after_inspection: bool,
    #[serde(default)]
    requested_by_user_id: Option<String>,
}

/// Embeds the production Vite output into the release executable.
#[derive(Embed)]
#[folder = "../../web/dist"]
struct WebAssets;

/// Represents startup and listener failures at the executable boundary.
#[derive(Debug, Error)]
enum AppError {
    #[error("could not bind HTTP listener: {0}")]
    Bind(#[source] std::io::Error),
    #[error("HTTP server stopped unexpectedly: {0}")]
    Serve(#[source] std::io::Error),
    #[error("database initialization failed: {0}")]
    Store(#[from] depot_store::StoreError),
    #[error("setup token creation failed: {0}")]
    Auth(#[from] depot_auth::AuthError),
    #[error("configuration failed: {0}")]
    Config(#[from] depot_core::ConfigError),
    #[error("Caddy reconciliation failed: {0}")]
    Caddy(#[from] depot_caddy::CaddyError),
    #[error("could not read desired domains file: {0}")]
    DomainsRead(#[source] std::io::Error),
    #[error("desired domains file must be a JSON string array: {0}")]
    DomainsParse(#[source] serde_json::Error),
    #[error("deploy token label must contain 1-80 characters")]
    InvalidDeployTokenLabel,
    #[error("secret encryption failed: {0}")]
    Secrets(#[from] depot_secrets::SecretError),
    #[error("could not create master key file: {0}")]
    KeyWrite(#[source] std::io::Error),
    #[error("GitHub App configuration could not be loaded: {0}")]
    GithubConfig(String),
    #[error("required dependency check failed: {0}")]
    Dependency(String),
    #[error("worker shell broker failed: {0}")]
    ShellBroker(#[source] std::io::Error),
    #[error("interactive password recovery failed: {0}")]
    Interactive(#[source] std::io::Error),
    #[error("there are no local users to recover")]
    NoUsers,
    #[error("select a user by entering one of the displayed numbers")]
    InvalidUserSelection,
    #[error("the new passwords do not match")]
    PasswordMismatch,
}

/// Parses the command line and runs the selected Depot process mode.
#[tokio::main]
async fn main() -> Result<(), AppError> {
    init_tracing();

    match Cli::parse().command {
        Command::Setup {
            config,
            expires_in_minutes,
        } => setup(load_config(config.as_deref())?, expires_in_minutes).await,
        Command::DeployToken {
            config,
            project_id,
            created_by_user_id,
            label,
        } => {
            create_deploy_token(
                load_config(config.as_deref())?,
                &project_id,
                &created_by_user_id,
                &label,
            )
            .await
        }
        Command::GenerateKey { output } => generate_master_key(&output),
        Command::Serve { config } => serve(load_config(config.as_deref())?).await,
        Command::Migrate { config } => migrate(load_config(config.as_deref())?).await,
        Command::Doctor { config } => doctor(load_config(config.as_deref())?).await,
        Command::ResetPassword { config } => reset_password(load_config(config.as_deref())?).await,
        Command::ValidateConfig { config } => {
            Config::load(&config)?;
            println!("Configuration is valid.");
            Ok(())
        }
        Command::Worker { config, once } => worker(load_config(config.as_deref())?, once).await,
        Command::CaddyAgent {
            config,
            domains_file,
            check,
        } => caddy_agent(&config, domains_file.as_deref(), check).await,
    }
}

/// Selects one local account and rotates its password without echoing either entry.
async fn reset_password(config: Config) -> Result<(), AppError> {
    use std::io::Write;

    let store = Store::connect(&config.database.url).await?;
    let users = store.list_users().await?;
    if users.is_empty() {
        return Err(AppError::NoUsers);
    }
    println!("Select the account to recover:");
    for (index, user) in users.iter().enumerate() {
        let status = if user.active { "active" } else { "disabled" };
        println!(
            "  {}) {} ({}, {status})",
            index + 1,
            user.email,
            user.role.as_str()
        );
    }
    print!("User [1-{}]: ", users.len());
    std::io::stdout().flush().map_err(AppError::Interactive)?;
    let mut selection = String::new();
    std::io::stdin()
        .read_line(&mut selection)
        .map_err(AppError::Interactive)?;
    let selected =
        parse_user_selection(&selection, users.len()).ok_or(AppError::InvalidUserSelection)?;
    println!("Passwords must contain at least 12 characters.");
    let password = rpassword::prompt_password("New password: ").map_err(AppError::Interactive)?;
    let confirmation =
        rpassword::prompt_password("Confirm new password: ").map_err(AppError::Interactive)?;
    if password != confirmation {
        return Err(AppError::PasswordMismatch);
    }
    depot_auth::recover_password(&store, &users[selected].id, password).await?;
    println!(
        "Password reset for {}. All existing sessions were revoked.",
        users[selected].email
    );
    Ok(())
}

/// Parses a one-based menu selection without accepting signs or trailing input.
fn parse_user_selection(input: &str, user_count: usize) -> Option<usize> {
    let selected = input.trim().parse::<usize>().ok()?;
    selected
        .checked_sub(1)
        .filter(|zero_based| *zero_based < user_count)
}

/// Creates a mode-0600 master key file and refuses to replace existing material.
fn generate_master_key(output: &Path) -> Result<(), AppError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(output).map_err(AppError::KeyWrite)?;
    use std::io::Write;
    writeln!(file, "{}", depot_secrets::generate_key_hex()).map_err(AppError::KeyWrite)?;
    println!("Created Depot master key at {}.", output.display());
    Ok(())
}

/// Loads the external secret key required by API and worker process modes.
fn load_secret_cipher(config: &Config) -> Result<depot_secrets::SecretCipher, AppError> {
    depot_secrets::SecretCipher::load(Path::new(&config.security.master_key_file))
        .map_err(AppError::from)
}

/// Creates and prints one project deploy token exactly once for operator delivery.
async fn create_deploy_token(
    config: Config,
    project_id: &str,
    created_by_user_id: &str,
    label: &str,
) -> Result<(), AppError> {
    let label = label.trim();
    if label.is_empty() || label.len() > 80 {
        return Err(AppError::InvalidDeployTokenLabel);
    }
    let store = Store::connect(&config.database.url).await?;
    let issued = issue_deploy_token(&store, project_id, label, created_by_user_id).await?;
    println!("Depot project deploy token (shown once):");
    println!("{}", issued.token);
    println!("Token ID: {}", issued.metadata.id);
    Ok(())
}

/// Applies one complete desired-domain snapshot through the restricted host reconciler.
async fn caddy_agent(
    config_path: &Path,
    domains_file: Option<&Path>,
    check: bool,
) -> Result<(), AppError> {
    let config = depot_caddy::CaddyAgentConfig::load(config_path)?;
    if check {
        depot_caddy::check(&config).await?;
        println!("Depot Caddy agent configuration is valid.");
        return Ok(());
    }
    if let Some(domains_file) = domains_file {
        let source = std::fs::read_to_string(domains_file).map_err(AppError::DomainsRead)?;
        let domains: Vec<String> = serde_json::from_str(&source).map_err(AppError::DomainsParse)?;
        depot_caddy::reconcile(&config, &domains).await?;
        println!("Depot-managed Caddy routes are reconciled.");
        Ok(())
    } else {
        depot_caddy::serve(config).await.map_err(AppError::from)
    }
}

/// Initializes storage and prints a short-lived bootstrap token exactly once.
async fn setup(config: Config, expires_in_minutes: i64) -> Result<(), AppError> {
    let store = Store::connect(&config.database.url).await?;
    let issued = issue_bootstrap_token(&store, Duration::minutes(expires_in_minutes)).await?;
    println!("Depot first-admin setup token (shown once):");
    println!("{}", issued.token);
    println!(
        "Expires at Unix timestamp: {}",
        issued.expires_at.unix_timestamp()
    );
    Ok(())
}

/// Loads strict operator configuration or the documented local development defaults.
fn load_config(path: Option<&Path>) -> Result<Config, depot_core::ConfigError> {
    path.map_or_else(|| Ok(Config::local_default()), Config::load)
}

/// Opens storage, applies embedded migrations, and exits after durable completion.
async fn migrate(config: Config) -> Result<(), AppError> {
    Store::connect(&config.database.url).await?;
    println!("Depot database migrations are current.");
    Ok(())
}

/// Performs non-destructive startup checks and reports each verified dependency.
async fn doctor(config: Config) -> Result<(), AppError> {
    println!("[ok] configuration is valid");
    Store::connect(&config.database.url).await?;
    println!("[ok] SQLite is reachable and migrations are current");
    let listener = TcpListener::bind(config.server.bind)
        .await
        .map_err(AppError::Bind)?;
    drop(listener);
    println!("[ok] listener {} is available", config.server.bind);
    verify_command(&config.worker.docker_binary, &["--version"]).await?;
    println!("[ok] Docker CLI is available");
    verify_command(&config.worker.railpack_binary, &["--version"]).await?;
    println!("[ok] Railpack CLI is available");
    println!("Depot {} is ready to start.", env!("CARGO_PKG_VERSION"));
    Ok(())
}

/// Verifies one configured dependency using fixed non-shell arguments.
async fn verify_command(binary: &str, arguments: &[&str]) -> Result<(), AppError> {
    let status = tokio::process::Command::new(binary)
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map_err(|error| AppError::Dependency(format!("{binary}: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::Dependency(format!(
            "{binary} exited unsuccessfully"
        )))
    }
}

/// Runs the durable worker claim loop with bounded polling and graceful shutdown.
async fn worker(config: Config, once: bool) -> Result<(), AppError> {
    let store = Store::connect(&config.database.url).await?;
    let secrets = load_secret_cipher(&config)?;
    let mut master_key = depot_secrets::load_key(Path::new(&config.security.master_key_file))
        .map_err(AppError::from)?;
    let shell_key = Arc::new(depot_core::derive_shell_signing_key(&master_key));
    master_key.zeroize();
    let github = load_github_integration(&config)?.map(|integration| integration.client);
    let shell_listener = bind_shell_listener(&config.worker.shell_socket).await?;
    let shell_store = store.clone();
    let shell_worker = config.worker.clone();
    tokio::spawn(async move {
        run_shell_broker(shell_listener, shell_store, shell_worker, shell_key).await;
    });
    cleanup_retired_containers(&store, &config.worker).await;
    let worker_id = format!("worker-{}", uuid::Uuid::now_v7());
    info!(%worker_id, "Depot worker started");
    loop {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let claimed = store.claim_job(&worker_id, now, 60).await?;
        if let Some(job) = claimed {
            let dashboard_github = match load_dashboard_github_client(&store, &secrets).await {
                Ok(client) => client,
                Err(error) => {
                    tracing::error!(%error, "dashboard-managed GitHub App is unavailable");
                    None
                }
            };
            process_job(
                &store,
                &config.worker,
                &secrets,
                dashboard_github.as_ref().or(github.as_ref()),
                &worker_id,
                job,
            )
            .await?;
        }
        if once {
            return Ok(());
        }
        tokio::select! {
            () = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    tracing::warn!(%error, "could not install worker shutdown signal");
                }
                info!(%worker_id, "Depot worker stopping");
                return Ok(());
            }
        }
    }
}

/// Replaces only a stale Unix socket before binding the worker shell broker.
async fn bind_shell_listener(path: &str) -> Result<UnixListener, AppError> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};

    let socket_path = Path::new(path);
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(AppError::ShellBroker)?;
    }
    if let Ok(metadata) = std::fs::symlink_metadata(socket_path) {
        if !metadata.file_type().is_socket() {
            return Err(AppError::ShellBroker(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "configured shell socket path exists and is not a socket",
            )));
        }
        tokio::fs::remove_file(socket_path)
            .await
            .map_err(AppError::ShellBroker)?;
    }
    let listener = UnixListener::bind(socket_path).map_err(AppError::ShellBroker)?;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o660))
        .map_err(AppError::ShellBroker)?;
    Ok(listener)
}

/// Accepts a small bounded number of signed local terminal sessions.
async fn run_shell_broker(
    listener: UnixListener,
    store: Store,
    worker_config: depot_core::WorkerConfig,
    signing_key: Arc<[u8; 32]>,
) {
    let permits = Arc::new(tokio::sync::Semaphore::new(4));
    info!(socket = %worker_config.shell_socket, "worker shell broker started");
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(connection) => connection,
            Err(error) => {
                tracing::warn!(%error, "shell broker accept failed");
                continue;
            }
        };
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            tokio::spawn(reject_busy_shell(stream));
            continue;
        };
        let connection_store = store.clone();
        let docker_binary = worker_config.docker_binary.clone();
        let connection_key = signing_key.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(error) =
                serve_shell_connection(stream, connection_store, docker_binary, connection_key)
                    .await
            {
                tracing::warn!(%error, "worker shell session ended with an error");
            }
        });
    }
}

/// Returns a safe operator message when the global shell limit is exhausted.
async fn reject_busy_shell(mut stream: UnixStream) {
    use tokio::io::AsyncWriteExt;
    let _ = stream
        .write_all(b"Depot shell capacity is full. Try again shortly.\r\n")
        .await;
}

/// Verifies one capability and streams a bounded-lifetime shell for its active container.
async fn serve_shell_connection(
    mut stream: UnixStream,
    store: Store,
    docker_binary: String,
    signing_key: Arc<[u8; 32]>,
) -> Result<(), std::io::Error> {
    use tokio::io::AsyncReadExt;
    let mut encoded = Vec::with_capacity(1024);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let byte = stream.read_u8().await?;
            if byte == b'\n' {
                return Ok::<(), std::io::Error>(());
            }
            if encoded.len() >= 4 * 1024 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "shell handshake is too large",
                ));
            }
            encoded.push(byte);
        }
    })
    .await
    .map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::TimedOut, "shell handshake timed out")
    })??;
    let handshake: ShellHandshake = serde_json::from_slice(&encoded)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    if !handshake.verify(&signing_key)
        || handshake.expires_at <= now
        || handshake.expires_at > now + 15 * 60
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "shell capability is invalid or expired",
        ));
    }
    let active = store
        .list_deployments(&handshake.project_id)
        .await
        .map_err(std::io::Error::other)?
        .into_iter()
        .find(|deployment| deployment.active);
    if active
        .as_ref()
        .and_then(|deployment| deployment.container_id.as_deref())
        != Some(handshake.container_id.as_str())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "shell container is no longer the active deployment",
        ));
    }
    match handshake.operation.clone() {
        WorkerOperation::Shell => {
            serve_interactive_shell(stream, store, docker_binary, handshake, now).await
        }
        operation => serve_file_operation(&mut stream, &docker_binary, &handshake, operation).await,
    }
}

/// Streams an interactive shell after the shared worker capability checks pass.
async fn serve_interactive_shell(
    mut stream: UnixStream,
    store: Store,
    docker_binary: String,
    handshake: ShellHandshake,
    now: i64,
) -> Result<(), std::io::Error> {
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;

    stream
        .write_all(b"Connected to the active Depot container. Session limit: 15 minutes.\r\n\r\n")
        .await?;
    let mut child = Command::new(docker_binary)
        .args([
            "exec",
            "-i",
            "-e",
            "TERM=xterm-256color",
            &handshake.container_id,
            "sh",
            "-i",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut child_stdin = child.stdin.take().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::BrokenPipe, "shell stdin unavailable")
    })?;
    let child_stdout = child.stdout.take().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::BrokenPipe, "shell stdout unavailable")
    })?;
    let child_stderr = child.stderr.take().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::BrokenPipe, "shell stderr unavailable")
    })?;
    let (mut socket_read, socket_write) = stream.into_split();
    let (output_tx, output_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(32);
    tokio::spawn(pump_shell_output(child_stdout, output_tx.clone()));
    tokio::spawn(pump_shell_output(child_stderr, output_tx));
    let output = tokio::spawn(write_shell_output(socket_write, output_rx));
    let input =
        tokio::spawn(async move { tokio::io::copy(&mut socket_read, &mut child_stdin).await });
    let remaining = u64::try_from(handshake.expires_at.saturating_sub(now)).unwrap_or(1);
    tokio::select! {
        _ = child.wait() => {}
        _ = input => { let _ = child.kill().await; }
        () = tokio::time::sleep(std::time::Duration::from_secs(remaining)) => {
            let _ = child.kill().await;
        }
    }
    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), output).await;
    let closed_at = time::OffsetDateTime::now_utc().unix_timestamp();
    let metadata = serde_json::json!({"session_id": handshake.session_id});
    let _ = store
        .record_project_audit(
            &handshake.actor_user_id,
            "project.shell.close",
            &handshake.project_id,
            &metadata.to_string(),
            closed_at,
        )
        .await;
    Ok(())
}

const FILE_TRANSFER_LIMIT: u64 = 32 * 1024 * 1024;
const FILE_LIST_LIMIT: u64 = 2 * 1024 * 1024;

/// Executes one fixed, signed file operation against the active container.
async fn serve_file_operation(
    stream: &mut UnixStream,
    docker_binary: &str,
    handshake: &ShellHandshake,
    operation: WorkerOperation,
) -> Result<(), std::io::Error> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::process::Command;

    match operation {
        WorkerOperation::List { path } => {
            let script = r#"for entry in "$1"/* "$1"/.[!.]* "$1"/..?*; do
  if [ ! -e "$entry" ] && [ ! -L "$entry" ]; then continue; fi
  name=${entry##*/}
  if [ -L "$entry" ]; then kind=symlink; size=0
  elif [ -d "$entry" ]; then kind=directory; size=0
  else kind=file; size=$(wc -c < "$entry" 2>/dev/null || printf 0); fi
  printf '%s\000%s\000%s\000' "$kind" "$size" "$name"
done"#;
            let output = bounded_docker_output(
                Command::new(docker_binary).args([
                    "exec",
                    &handshake.container_id,
                    "sh",
                    "-c",
                    script,
                    "sh",
                    &path,
                ]),
                FILE_LIST_LIMIT,
            )
            .await?;
            let entries = parse_file_entries(&output)?;
            write_broker_response(stream, &serde_json::json!({"ok": true, "entries": entries}))
                .await?;
        }
        WorkerOperation::Download { path } => {
            let script = r#"test -f "$1" && cat -- "$1""#;
            let output = bounded_docker_output(
                Command::new(docker_binary).args([
                    "exec",
                    &handshake.container_id,
                    "sh",
                    "-c",
                    script,
                    "sh",
                    &path,
                ]),
                FILE_TRANSFER_LIMIT,
            )
            .await?;
            write_broker_response(
                stream,
                &serde_json::json!({"ok": true, "size": output.len()}),
            )
            .await?;
            stream.write_all(&output).await?;
        }
        WorkerOperation::Upload { path, size } => {
            if size > FILE_TRANSFER_LIMIT {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "container upload exceeds the worker limit",
                ));
            }
            let script = r#"umask 077; cat > "$1""#;
            let mut child = Command::new(docker_binary)
                .args([
                    "exec",
                    "-i",
                    &handshake.container_id,
                    "sh",
                    "-c",
                    script,
                    "sh",
                    &path,
                ])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()?;
            let mut stdin = child.stdin.take().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "upload stdin unavailable")
            })?;
            let copied = tokio::io::copy(&mut stream.take(size), &mut stdin).await?;
            drop(stdin);
            if copied != size {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "container upload ended before its declared size",
                ));
            }
            let output = child.wait_with_output().await?;
            ensure_docker_success(output.status, &output.stderr)?;
            write_broker_response(stream, &serde_json::json!({"ok": true})).await?;
        }
        WorkerOperation::CreateDirectory { path } => {
            run_file_mutation(
                docker_binary,
                &handshake.container_id,
                &["mkdir", "--", &path],
            )
            .await?;
            write_broker_response(stream, &serde_json::json!({"ok": true})).await?;
        }
        WorkerOperation::Rename { path, destination } => {
            run_file_mutation(
                docker_binary,
                &handshake.container_id,
                &["mv", "--", &path, &destination],
            )
            .await?;
            write_broker_response(stream, &serde_json::json!({"ok": true})).await?;
        }
        WorkerOperation::Delete { path } => {
            run_file_mutation(
                docker_binary,
                &handshake.container_id,
                &["rm", "-rf", "--", &path],
            )
            .await?;
            write_broker_response(stream, &serde_json::json!({"ok": true})).await?;
        }
        WorkerOperation::Shell => unreachable!("shell operations are dispatched separately"),
    }
    Ok(())
}

/// Runs one bounded Docker command and rejects truncation or a non-zero exit.
async fn bounded_docker_output(
    command: &mut tokio::process::Command,
    limit: u64,
) -> Result<Vec<u8>, std::io::Error> {
    use tokio::io::AsyncReadExt;

    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Docker stdout unavailable")
    })?;
    let mut bytes = Vec::new();
    stdout.take(limit + 1).read_to_end(&mut bytes).await?;
    let output = child.wait_with_output().await?;
    ensure_docker_success(output.status, &output.stderr)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "container file response exceeds the worker limit",
        ));
    }
    Ok(bytes)
}

/// Converts NUL-delimited worker records into transport-safe file metadata.
fn parse_file_entries(bytes: &[u8]) -> Result<Vec<serde_json::Value>, std::io::Error> {
    let fields = bytes.split(|byte| *byte == 0).collect::<Vec<_>>();
    if !fields.last().is_some_and(|field| field.is_empty()) || (fields.len() - 1) % 3 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "container returned malformed directory metadata",
        ));
    }
    let mut entries = Vec::new();
    for record in fields[..fields.len().saturating_sub(1)].chunks_exact(3) {
        let kind = String::from_utf8_lossy(record[0]);
        let size = String::from_utf8_lossy(record[1])
            .parse::<u64>()
            .unwrap_or(0);
        let name = String::from_utf8_lossy(record[2]);
        entries.push(serde_json::json!({"name": name, "kind": kind, "size": size}));
    }
    Ok(entries)
}

/// Runs a fixed file mutation without accepting command structure from the caller.
async fn run_file_mutation(
    docker_binary: &str,
    container_id: &str,
    arguments: &[&str],
) -> Result<(), std::io::Error> {
    let output = tokio::process::Command::new(docker_binary)
        .arg("exec")
        .arg(container_id)
        .args(arguments)
        .output()
        .await?;
    ensure_docker_success(output.status, &output.stderr)
}

/// Maps Docker command failure to a bounded local broker error.
fn ensure_docker_success(
    status: std::process::ExitStatus,
    stderr: &[u8],
) -> Result<(), std::io::Error> {
    if status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(stderr);
    Err(std::io::Error::other(format!(
        "container file operation failed: {}",
        detail.trim().chars().take(300).collect::<String>()
    )))
}

/// Writes one newline-delimited broker response header.
async fn write_broker_response(
    stream: &mut UnixStream,
    value: &serde_json::Value,
) -> Result<(), std::io::Error> {
    use tokio::io::AsyncWriteExt;
    let mut encoded = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    encoded.push(b'\n');
    stream.write_all(&encoded).await
}

/// Reads one Docker output stream into the bounded broker channel.
async fn pump_shell_output<R>(mut reader: R, sender: tokio::sync::mpsc::Sender<Vec<u8>>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut buffer = vec![0_u8; 4 * 1024];
    loop {
        let count = match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        if sender.send(buffer[..count].to_vec()).await.is_err() {
            break;
        }
    }
}

/// Serializes Docker stdout and stderr chunks onto the local socket.
async fn write_shell_output(
    mut writer: tokio::net::unix::OwnedWriteHalf,
    mut output: tokio::sync::mpsc::Receiver<Vec<u8>>,
) {
    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = output.recv().await {
        if writer.write_all(&chunk).await.is_err() {
            break;
        }
    }
}

/// Records durable events and completion state around one claimed job attempt.
async fn process_job(
    store: &Store,
    worker_config: &depot_core::WorkerConfig,
    secrets: &depot_secrets::SecretCipher,
    github: Option<&depot_github::GithubAppClient>,
    worker_id: &str,
    job: depot_store::Job,
) -> Result<(), AppError> {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    store
        .append_job_event(&job.id, "info", "Worker claimed durable job.", now)
        .await?;
    match execute_job(store, worker_config, secrets, github, &job).await {
        Ok(result_json) => {
            let completed_at = time::OffsetDateTime::now_utc().unix_timestamp();
            let completion_message = match job.kind.as_str() {
                "inspect_repository" => "Repository inspection completed.",
                "runtime_logs" => "Runtime logs collected.",
                _ => "Deployment work completed.",
            };
            store
                .append_job_event(&job.id, "info", completion_message, completed_at)
                .await?;
            let cleanup = store
                .complete_job(&job.id, worker_id, Some(&result_json), completed_at)
                .await?;
            retire_containers(store, worker_config, cleanup.container_ids).await;
            retire_images(store, worker_config, cleanup.image_ids).await;
        }
        Err(message) => {
            let failed_at = time::OffsetDateTime::now_utc().unix_timestamp();
            store
                .append_job_event(&job.id, "error", &message, failed_at)
                .await?;
            store
                .fail_job(&job.id, worker_id, &message, 30, failed_at)
                .await?;
            tracing::warn!(job_id = %job.id, kind = %job.kind, error = %message, "job attempt failed");
        }
    }
    Ok(())
}

/// Reconciles terminal inactive containers left by an interrupted cleanup pass.
async fn cleanup_retired_containers(store: &Store, worker_config: &depot_core::WorkerConfig) {
    match store.containers_pending_cleanup().await {
        Ok(container_ids) => retire_containers(store, worker_config, container_ids).await,
        Err(error) => tracing::warn!(%error, "could not load containers pending cleanup"),
    }
    match store.images_pending_cleanup().await {
        Ok(image_ids) => retire_images(store, worker_config, image_ids).await,
        Err(error) => tracing::warn!(%error, "could not load images pending cleanup"),
    }
}

/// Removes switched-out containers and records successful retirement idempotently.
async fn retire_containers(
    store: &Store,
    worker_config: &depot_core::WorkerConfig,
    container_ids: Vec<String>,
) {
    for container_id in container_ids {
        match depot_runtime::remove_container(
            &worker_config.docker_binary,
            &container_id,
            std::time::Duration::from_secs(30),
        )
        .await
        {
            Ok(()) => {
                if let Err(error) = store
                    .mark_container_retired(
                        &container_id,
                        time::OffsetDateTime::now_utc().unix_timestamp(),
                    )
                    .await
                {
                    tracing::warn!(%error, %container_id, "could not persist container retirement");
                }
            }
            Err(error) => {
                tracing::warn!(%error, %container_id, "could not retire inactive container");
            }
        }
    }
}

/// Removes images beyond retention while preserving every active runtime image.
async fn retire_images(
    store: &Store,
    worker_config: &depot_core::WorkerConfig,
    image_ids: Vec<String>,
) {
    for image_id in image_ids {
        match depot_runtime::remove_image(
            &worker_config.docker_binary,
            &image_id,
            std::time::Duration::from_secs(30),
        )
        .await
        {
            Ok(()) => {
                if let Err(error) = store
                    .mark_image_retired(&image_id, time::OffsetDateTime::now_utc().unix_timestamp())
                    .await
                {
                    tracing::warn!(%error, %image_id, "could not persist image retirement");
                }
            }
            Err(error) => tracing::warn!(%error, %image_id, "could not retire old image"),
        }
    }
}

/// Executes a supported job kind and returns its bounded JSON result.
async fn execute_job(
    store: &Store,
    worker_config: &depot_core::WorkerConfig,
    secrets: &depot_secrets::SecretCipher,
    github: Option<&depot_github::GithubAppClient>,
    job: &depot_store::Job,
) -> Result<String, String> {
    match job.kind.as_str() {
        "inspect_repository" => {
            execute_inspection(store, worker_config, secrets, github, job).await
        }
        "deploy_project" => execute_deployment(store, worker_config, secrets, github, job).await,
        "runtime_logs" => execute_runtime_logs(store, worker_config, job).await,
        "reconcile_domains" => execute_domain_reconciliation(store, worker_config).await,
        _ => Err(format!("Unsupported job kind: {}", job.kind)),
    }
}

/// Reads bounded active-container output without granting Docker access to the API process.
async fn execute_runtime_logs(
    store: &Store,
    worker_config: &depot_core::WorkerConfig,
    job: &depot_store::Job,
) -> Result<String, String> {
    let project_id = job
        .project_id
        .as_deref()
        .ok_or_else(|| "Runtime log job is missing its project.".to_owned())?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let config = store
        .service_config(project_id)
        .await
        .map_err(|_| "Project runtime configuration could not be loaded.".to_owned())?;
    if config.project_kind == "static" {
        return serde_json::to_string(&serde_json::json!({
            "status": "static",
            "deployment_id": null,
            "logs": "",
            "fetched_at": now,
        }))
        .map_err(|_| "Runtime log result could not be encoded.".to_owned());
    }
    let deployments = store
        .list_deployments(project_id)
        .await
        .map_err(|_| "Project deployments could not be loaded.".to_owned())?;
    let Some(active) = deployments.into_iter().find(|deployment| deployment.active) else {
        return serde_json::to_string(&serde_json::json!({
            "status": "not_deployed",
            "deployment_id": null,
            "logs": "",
            "fetched_at": now,
        }))
        .map_err(|_| "Runtime log result could not be encoded.".to_owned());
    };
    let container_id = active
        .container_id
        .as_deref()
        .ok_or_else(|| "The active deployment has no runtime container.".to_owned())?;
    let logs = depot_runtime::container_logs(
        &worker_config.docker_binary,
        container_id,
        1_000,
        std::time::Duration::from_secs(10),
    )
    .await
    .map_err(|_| "Depot could not read logs from the active service.".to_owned())?;
    serde_json::to_string(&serde_json::json!({
        "status": "available",
        "deployment_id": active.id,
        "logs": logs,
        "fetched_at": now,
    }))
    .map_err(|_| "Runtime log result could not be encoded.".to_owned())
}

/// Sends the complete desired hostname snapshot and persists the agent acknowledgment.
async fn execute_domain_reconciliation(
    store: &Store,
    worker_config: &depot_core::WorkerConfig,
) -> Result<String, String> {
    let agent = worker_config
        .caddy_agent
        .as_ref()
        .ok_or_else(|| "Caddy agent client is not configured.".to_owned())?;
    let domains = store
        .all_domain_hostnames()
        .await
        .map_err(|_| "Desired domains could not be loaded.".to_owned())?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    match depot_caddy::send_snapshot(
        &agent.address,
        &agent.auth_token,
        &domains,
        std::time::Duration::from_secs(agent.timeout_seconds),
    )
    .await
    {
        Ok(()) => {
            store
                .mark_domains_active(now)
                .await
                .map_err(|_| "Caddy acknowledgment could not be persisted.".to_owned())?;
            serde_json::to_string(&serde_json::json!({ "accepted": domains.len() }))
                .map_err(|_| "Caddy reconciliation result could not be encoded.".to_owned())
        }
        Err(error) => {
            let message = error.to_string();
            store
                .mark_domains_error(&message, now)
                .await
                .map_err(|_| "Caddy reconciliation failure could not be persisted.".to_owned())?;
            Err(message)
        }
    }
}

/// Inspects one authoritative project source and returns its immutable build plan.
async fn execute_inspection(
    store: &Store,
    worker_config: &depot_core::WorkerConfig,
    secrets: &depot_secrets::SecretCipher,
    github: Option<&depot_github::GithubAppClient>,
    job: &depot_store::Job,
) -> Result<String, String> {
    let payload: InspectionJobPayload = serde_json::from_str(&job.payload_json)
        .map_err(|_| "Inspection job payload is invalid.".to_owned())?;
    let project_id = job
        .project_id
        .as_deref()
        .ok_or_else(|| "Inspection job has no project reference.".to_owned())?;
    let project = store
        .project_by_id(project_id)
        .await
        .map_err(|_| "Inspection project no longer exists.".to_owned())?;
    let mut ssh =
        load_project_ssh_credentials(store, secrets, project_id, &project.repository_url).await?;
    let git_credentials =
        ssh.as_ref()
            .map(|(private_key, known_hosts)| depot_git::GitSshCredentials {
                private_key,
                known_hosts,
            });
    let mut github_token = github_checkout_token(store, github, project_id).await?;
    let http_credentials = github_token
        .as_deref()
        .map(|token| depot_git::GitHttpCredentials { token });
    let inspection = depot_git::inspect_repository(
        &project.repository_url,
        &project.branch,
        std::path::Path::new(&worker_config.workspace_root),
        std::time::Duration::from_secs(worker_config.git_timeout_seconds),
        git_credentials,
        http_credentials,
    )
    .await;
    if let Some((private_key, _)) = &mut ssh {
        private_key.zeroize();
    }
    if let Some(token) = &mut github_token {
        token.zeroize();
    }
    let inspection = inspection.map_err(|error| error.to_string())?;
    let mut result = serde_json::to_value(&inspection)
        .map_err(|_| "Repository inspection result could not be encoded.".to_owned())?;
    if payload.deploy_after_inspection {
        let actor = payload
            .requested_by_user_id
            .as_deref()
            .ok_or_else(|| "Initial deployment has no requesting user.".to_owned())?;
        let deployment = store
            .create_initial_deployment(
                project_id,
                &inspection.commit_sha,
                actor,
                time::OffsetDateTime::now_utc().unix_timestamp(),
            )
            .await
            .map_err(|_| "Initial deployment could not be queued.".to_owned())?;
        result["initial_deployment_id"] = serde_json::Value::String(deployment.id);
    }
    serde_json::to_string(&result)
        .map_err(|_| "Repository inspection result could not be encoded.".to_owned())
}

/// Checks out the recorded SHA and builds a Dockerfile image from fixed arguments.
async fn execute_deployment(
    store: &Store,
    worker_config: &depot_core::WorkerConfig,
    secrets: &depot_secrets::SecretCipher,
    github: Option<&depot_github::GithubAppClient>,
    job: &depot_store::Job,
) -> Result<String, String> {
    let payload: DeploymentJobPayload = serde_json::from_str(&job.payload_json)
        .map_err(|_| "Deployment job payload is invalid.".to_owned())?;
    let deployment = store
        .deployment_by_id(&payload.deployment_id)
        .await
        .map_err(|_| "Deployment record no longer exists.".to_owned())?;
    let project_id = job
        .project_id
        .as_deref()
        .filter(|id| *id == deployment.project_id)
        .ok_or_else(|| "Deployment job project does not match its record.".to_owned())?;
    let project = store
        .project_by_id(project_id)
        .await
        .map_err(|_| "Deployment project no longer exists.".to_owned())?;
    let config = store
        .service_config(project_id)
        .await
        .map_err(|_| "Deployment configuration is unavailable.".to_owned())?;
    if config.builder == "railpack" && config.project_kind != "static" {
        return Err("Railpack builds are not implemented yet.".to_owned());
    }
    let mut ssh =
        load_project_ssh_credentials(store, secrets, project_id, &project.repository_url).await?;
    let git_credentials =
        ssh.as_ref()
            .map(|(private_key, known_hosts)| depot_git::GitSshCredentials {
                private_key,
                known_hosts,
            });
    let mut github_token = github_checkout_token(store, github, project_id).await?;
    let http_credentials = github_token
        .as_deref()
        .map(|token| depot_git::GitHttpCredentials { token });
    let checkout = depot_git::checkout_repository(
        &project.repository_url,
        &project.branch,
        &deployment.commit_sha,
        std::path::Path::new(&worker_config.workspace_root),
        std::time::Duration::from_secs(worker_config.git_timeout_seconds),
        git_credentials,
        http_credentials,
    )
    .await;
    if let Some((private_key, _)) = &mut ssh {
        private_key.zeroize();
    }
    if let Some(token) = &mut github_token {
        token.zeroize();
    }
    let checkout = checkout.map_err(|error| error.to_string())?;
    if config.project_kind == "static" {
        let output_directory = config
            .static_output_dir
            .as_deref()
            .ok_or_else(|| "Static projects require an output directory.".to_owned())?;
        let built = depot_build::build_static(depot_build::StaticBuildRequest {
            docker_binary: &worker_config.docker_binary,
            railpack_binary: &worker_config.railpack_binary,
            checkout_path: checkout.path(),
            context_path: &config.context_path,
            dockerfile_path: &config.dockerfile_path,
            builder: &config.builder,
            output_directory,
            static_root: Path::new(&worker_config.static_root),
            project_id,
            release_id: &deployment.id,
            timeout: std::time::Duration::from_secs(worker_config.docker_timeout_seconds),
        })
        .await
        .map_err(|error| error.to_string())?;
        store
            .record_static_release(
                &deployment.id,
                &built.release_path.to_string_lossy(),
                time::OffsetDateTime::now_utc().unix_timestamp(),
            )
            .await
            .map_err(|_| "Static release identity could not be persisted.".to_owned())?;
        return serde_json::to_string(&serde_json::json!({
            "static_release": built.release_path,
            "logs": built.logs,
        }))
        .map_err(|_| "Static deployment result could not be encoded.".to_owned());
    }
    let image_tag = format!("depot/{}:{}", project.slug, &checkout.commit_sha()[..12]);
    let build = depot_build::build_dockerfile(depot_build::DockerBuildRequest {
        docker_binary: &worker_config.docker_binary,
        checkout_path: checkout.path(),
        context_path: &config.context_path,
        dockerfile_path: &config.dockerfile_path,
        image_tag: &image_tag,
        timeout: std::time::Duration::from_secs(worker_config.docker_timeout_seconds),
    })
    .await
    .map_err(|error| error.to_string())?;
    let container_port = config
        .exposed_port
        .and_then(|port| u16::try_from(port).ok())
        .ok_or_else(|| "Service deployments require a valid exposed port.".to_owned())?;
    let mut environment = Vec::new();
    for variable in store
        .encrypted_project_variables(project_id)
        .await
        .map_err(|_| "Runtime variables could not be loaded.".to_owned())?
    {
        let context = format!("depot:v1:{project_id}:{}", variable.name);
        let value = secrets
            .decrypt(&variable.ciphertext, &variable.nonce, context.as_bytes())
            .map_err(|_| "Runtime variables could not be decrypted.".to_owned())?;
        environment.push((variable.name, value));
    }
    let running = depot_runtime::start_container(depot_runtime::StartContainerRequest {
        docker_binary: &worker_config.docker_binary,
        deployment_id: &deployment.id,
        image_id: &build.image_id,
        container_port,
        timeout: std::time::Duration::from_secs(30),
        environment: &environment,
    })
    .await
    .map_err(|error| error.to_string())?;
    for (_, value) in &mut environment {
        value.zeroize();
    }
    drop(environment);
    let healthcheck_path = config.healthcheck_path.as_deref().unwrap_or("/");
    if let Err(error) = depot_runtime::wait_for_http_ready(
        running.host_port,
        healthcheck_path,
        std::time::Duration::from_secs(60),
    )
    .await
    {
        if let Err(cleanup_error) = depot_runtime::remove_container(
            &worker_config.docker_binary,
            &running.container_id,
            std::time::Duration::from_secs(30),
        )
        .await
        {
            tracing::warn!(%cleanup_error, container_id = %running.container_id, "could not remove unready candidate container");
        }
        return Err(error.to_string());
    }
    if store
        .record_deployment_runtime(
            &deployment.id,
            &build.image_id,
            &running.container_id,
            running.host_port,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        )
        .await
        .is_err()
    {
        if let Err(cleanup_error) = depot_runtime::remove_container(
            &worker_config.docker_binary,
            &running.container_id,
            std::time::Duration::from_secs(30),
        )
        .await
        {
            tracing::warn!(%cleanup_error, container_id = %running.container_id, "could not remove unpersisted candidate container");
        }
        return Err("Verified runtime identity could not be persisted.".to_owned());
    }
    serde_json::to_string(&serde_json::json!({
        "image_id": build.image_id,
        "image_tag": build.image_tag,
        "container_id": running.container_id,
        "host_port": running.host_port,
        "logs": build.logs,
    }))
    .map_err(|_| "Deployment result could not be encoded.".to_owned())
}

/// Decrypts SSH material only for SSH repositories and fails closed when absent.
async fn load_project_ssh_credentials(
    store: &Store,
    secrets: &depot_secrets::SecretCipher,
    project_id: &str,
    repository_url: &str,
) -> Result<Option<(String, String)>, String> {
    if !repository_url.starts_with("ssh://") {
        return Ok(None);
    }
    let credential = store
        .project_ssh_credential(project_id)
        .await
        .map_err(|_| "SSH deploy credentials could not be loaded.".to_owned())?
        .ok_or_else(|| "This SSH repository has no deploy credentials configured.".to_owned())?;
    let context = format!("depot:ssh:v1:{project_id}");
    let private_key = secrets
        .decrypt(
            &credential.private_key_ciphertext,
            &credential.private_key_nonce,
            context.as_bytes(),
        )
        .map_err(|_| "SSH deploy credentials could not be decrypted.".to_owned())?;
    Ok(Some((private_key, credential.known_hosts)))
}

/// Mints an installation token only for projects imported through the GitHub App.
async fn github_checkout_token(
    store: &Store,
    github: Option<&depot_github::GithubAppClient>,
    project_id: &str,
) -> Result<Option<String>, String> {
    let Some(repository) = store
        .github_repository_for_project(project_id)
        .await
        .map_err(|_| "GitHub repository identity could not be loaded.".to_owned())?
    else {
        return Ok(None);
    };
    let github =
        github.ok_or_else(|| "This project requires the GitHub App integration.".to_owned())?;
    let installation_id = u64::try_from(repository.installation_id)
        .map_err(|_| "GitHub installation identity is invalid.".to_owned())?;
    github
        .checkout_token(installation_id)
        .await
        .map(Some)
        .map_err(|_| "GitHub could not issue a checkout credential.".to_owned())
}

/// Installs structured logging with a safe, operator-configurable filter.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("depot=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// /// Lightweight gateway liveness check.
///
/// Intentionally independent of hostname routing and authentication.
/// Confirms that the Depot HTTP server is accepting requests.
async fn gateway_health() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// Runs the initial combined API and embedded frontend HTTP listener.
async fn serve(config: Config) -> Result<(), AppError> {
    let store = Store::connect(&config.database.url).await?;
    let secrets = load_secret_cipher(&config)?;
    let mut master_key = depot_secrets::load_key(Path::new(&config.security.master_key_file))?;
    let shell_key = depot_core::derive_shell_signing_key(&master_key);
    master_key.zeroize();
    let github = load_github_integration(&config)?;
    let shell = depot_api::ShellBroker {
        socket_path: Arc::from(config.worker.shell_socket.clone()),
        public_origin: Arc::from(config.security.public_origin.clone()),
        signing_key: Arc::new(shell_key),
    };

    let dashboard_hostname = config
        .security
        .public_origin
        .parse::<Uri>()
        .ok()
        .and_then(|origin| origin.host().map(str::to_ascii_lowercase))
        .map(Arc::<str>::from)
        .expect("Depot public_origin must contain a hostname");

    // This router ONLY handles Depot's management API and dashboard.
    let management_router = Router::new()
        .route("/healthz", get(gateway_health))
        .route("/api/v1/system/health", get(system_health))
        .merge(depot_api::router_with_shell_and_domains(
            store.clone(),
            Some(secrets.clone()),
            github.clone(),
            Some(shell.clone()),
            config.domains.default_suffix.clone(),
        ))
        .fallback(|request: Request<Body>| async move {
            if request.uri().path().starts_with("/api/") {
                return StatusCode::NOT_FOUND.into_response();
            }

            serve_spa(request.uri()).await
        });

    // Run hostname routing BEFORE Depot's API routes can match.
    let gateway_store = store.clone();

    let app = management_router.layer(middleware::from_fn(
        move |request: Request<Body>, next: Next| {
            let store = gateway_store.clone();
            let dashboard = dashboard_hostname.clone();

            async move {
                // Health checks must work without a hostname, so they are handled before routing.
                if request.uri().path() == "/healthz" {
                    return next.run(request).await;
                }

                let Some(hostname) = request_hostname(request.headers()) else {
                    return StatusCode::BAD_REQUEST.into_response();
                };

                if hostname == dashboard.as_ref() {
                    // Depot's own hostname: use the management router.
                    return next.run(request).await;
                }

                // Application hostname: bypass Depot's API entirely.
                gateway_or_spa(store, request).await
            }
        },
    ));

    let listener = TcpListener::bind(config.server.bind)
        .await
        .map_err(AppError::Bind)?;

    info!(bind = %config.server.bind, "Depot HTTP listener started");
    axum::serve(listener, app).await.map_err(AppError::Serve)
}

/// Loads operator-owned GitHub App secrets once without retaining file paths in API state.
fn load_github_integration(
    config: &Config,
) -> Result<Option<depot_api::GithubIntegration>, AppError> {
    let Some(github) = &config.github else {
        return Ok(None);
    };
    let private_key = std::fs::read(&github.private_key_file)
        .map_err(|_| AppError::GithubConfig("the private key file is unreadable".to_owned()))?;
    let webhook_secret = std::fs::read_to_string(&github.webhook_secret_file)
        .map_err(|_| AppError::GithubConfig("the webhook secret file is unreadable".to_owned()))?;
    let webhook_secret = webhook_secret.trim();
    if webhook_secret.len() < 32 {
        return Err(AppError::GithubConfig(
            "the webhook secret is too short".to_owned(),
        ));
    }
    let client = depot_github::GithubAppClient::new(github.app_id, &github.app_slug, &private_key)
        .map_err(|error| AppError::GithubConfig(error.to_string()))?;
    Ok(Some(depot_api::GithubIntegration {
        client,
        webhook_secret: std::sync::Arc::from(webhook_secret.as_bytes()),
    }))
}

/// Decrypts dashboard-managed App credentials for the job currently being claimed.
async fn load_dashboard_github_client(
    store: &Store,
    secrets: &depot_secrets::SecretCipher,
) -> Result<Option<depot_github::GithubAppClient>, String> {
    const PRIVATE_KEY_CONTEXT: &[u8] = b"depot:github-app:v1:private-key";
    let Some(configuration) = store
        .github_app_configuration()
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let private_key = secrets
        .decrypt(
            &configuration.private_key_ciphertext,
            &configuration.private_key_nonce,
            PRIVATE_KEY_CONTEXT,
        )
        .map_err(|error| error.to_string())?;
    let app_id = u64::try_from(configuration.app_id)
        .map_err(|_| "dashboard GitHub App ID is invalid".to_owned())?;
    depot_github::GithubAppClient::new(app_id, &configuration.app_slug, private_key.as_bytes())
        .map(Some)
        .map_err(|error| error.to_string())
}

/// Reports API process liveness; dependency readiness will use a separate endpoint.
async fn system_health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "alive",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// Routes an application request to its deployed service or static release.
///
/// This is called only for non-dashboard hostnames, so application
/// paths can never conflict with Depot's management API.
async fn gateway_or_spa(store: Store, request: Request<Body>) -> Response {
    let Some(hostname) = request_hostname(request.headers()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    match store.gateway_target(&hostname).await {
        Ok(Some(depot_store::GatewayTarget::Service { host_port, .. })) => {
            proxy_to_runtime(request, host_port).await
        }

        Ok(Some(depot_store::GatewayTarget::Static { release_path, .. })) => {
            serve_static_release(request.uri(), Path::new(&release_path)).await
        }

        Ok(None) => managed_not_found_response(),

        Err(error) => {
            tracing::error!(
                %hostname,
                %error,
                "gateway target lookup failed"
            );

            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

/// Serves a safe branded response when a routed application hostname has no live target.
fn managed_not_found_response() -> Response {
    const PAGE: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>Nothing deployed here · Depot</title>
  <style>
    :root{color-scheme:dark;font-family:ui-sans-serif,system-ui,sans-serif;background:#0d0d0d;color:#f2f2f2}
    *{box-sizing:border-box}body{margin:0;min-height:100vh;display:grid;place-items:center;padding:2rem}
    main{width:min(100%,42rem);border-top:1px solid #c6f547;padding-top:1.5rem}
    small{color:#c6f547;font:700 .72rem ui-monospace,monospace;letter-spacing:.12em;text-transform:uppercase}
    h1{margin:1rem 0 .75rem;font-size:clamp(2rem,7vw,4.5rem);line-height:.95;letter-spacing:-.05em}
    p{max-width:34rem;margin:0;color:#a4a4a4;font-size:1rem;line-height:1.65}
    footer{margin-top:3rem;color:#666;font:700 .68rem ui-monospace,monospace;text-transform:uppercase}
  </style>
</head>
<body><main><small>404 · Depot</small><h1>Nothing is deployed here.</h1><p>This hostname is managed by Depot, but it does not currently point to a live deployment.</p><footer>Check the project domain and deployment status.</footer></main></body>
</html>"#;
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff")
        .header("referrer-policy", "no-referrer")
        .header(
            "content-security-policy",
            "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'",
        )
        .body(Body::from(PAGE))
        .expect("managed not-found response headers are valid")
}

/// Serves one validated immutable static release with a safe SPA fallback.
async fn serve_static_release(uri: &Uri, release_root: &Path) -> Response {
    let Ok(root) = tokio::fs::canonicalize(release_root).await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let requested = uri.path().trim_start_matches('/');
    if requested
        .split('/')
        .any(|part| part == ".." || part.contains('\\'))
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let requested = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };
    let candidate = root.join(requested);
    let selected = match tokio::fs::canonicalize(&candidate).await {
        Ok(path) if path.starts_with(&root) && path.is_file() => path,
        _ if !requested.contains('.') => root.join("index.html"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let Ok(selected) = tokio::fs::canonicalize(selected).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !selected.starts_with(&root) || !selected.is_file() {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tokio::fs::read(&selected).await {
        Ok(bytes) => static_artifact_response(&selected, bytes),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Returns a static artifact with MIME and cache policy derived from its extension.
fn static_artifact_response(path: &Path, bytes: Vec<u8>) -> Response {
    let content_type = mime_guess::from_path(path).first_or_octet_stream();
    let cache_control = if path.file_name().is_some_and(|name| name == "index.html") {
        "no-cache"
    } else {
        "public, max-age=3600"
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type.as_ref())
        .header(header::CACHE_CONTROL, cache_control)
        .header("x-content-type-options", "nosniff")
        .body(Body::from(bytes))
        .expect("static artifact response headers are valid")
}

/// Extracts a lowercase DNS hostname while rejecting malformed Host authorities.
fn request_hostname(headers: &HeaderMap) -> Option<String> {
    let authority = headers.get(header::HOST)?.to_str().ok()?;
    let authority = authority.parse::<axum::http::uri::Authority>().ok()?;
    let hostname = authority.host().trim_end_matches('.').to_ascii_lowercase();
    (!hostname.is_empty()).then_some(hostname)
}

/// Proxies one request to a database-selected loopback port using an HTTP/1 connection.
async fn proxy_to_runtime(mut request: Request<Body>, host_port: u16) -> Response {
    strip_hop_by_hop_headers(request.headers_mut());
    let stream = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, host_port)),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        _ => return StatusCode::BAD_GATEWAY.into_response(),
    };
    let (mut sender, connection) = match http1::handshake(TokioIo::new(stream)).await {
        Ok(parts) => parts,
        Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
    };
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::debug!(%error, "gateway upstream connection closed");
        }
    });
    match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        sender.send_request(request),
    )
    .await
    {
        Ok(Ok(upstream)) => {
            let (mut parts, body) = upstream.into_parts();
            strip_hop_by_hop_headers(&mut parts.headers);
            Response::from_parts(parts, Body::new(body))
        }
        _ => StatusCode::BAD_GATEWAY.into_response(),
    }
}

/// Removes connection-specific headers which cannot cross proxy hops safely.
fn strip_hop_by_hop_headers(headers: &mut HeaderMap) {
    for name in [
        header::CONNECTION,
        header::PROXY_AUTHENTICATE,
        header::PROXY_AUTHORIZATION,
        header::TE,
        header::TRAILER,
        header::TRANSFER_ENCODING,
        header::UPGRADE,
    ] {
        headers.remove(name);
    }
    headers.remove("keep-alive");
}

/// Serves a requested embedded asset or the SPA shell for client-side routes.
async fn serve_spa(uri: &Uri) -> Response {
    let requested = uri.path().trim_start_matches('/');
    let asset_name = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };

    if let Some(asset) = WebAssets::get(asset_name) {
        return embedded_response(asset_name, asset.data.into_owned());
    }

    if requested.contains('.') {
        return StatusCode::NOT_FOUND.into_response();
    }

    match WebAssets::get("index.html") {
        Some(asset) => embedded_response("index.html", asset.data.into_owned()),
        None => (StatusCode::SERVICE_UNAVAILABLE, "Depot UI is not embedded").into_response(),
    }
}

/// Builds an HTTP response with conservative HTML and immutable hashed-asset caching.
fn embedded_response(asset_name: &str, bytes: Vec<u8>) -> Response {
    let content_type = content_type(asset_name);
    let cache_control = if asset_name == "index.html" {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, cache_control)
        .body(Body::from(bytes))
        .expect("static response headers are valid")
}

/// Returns a safe content type for the small set of emitted Vite asset extensions.
fn content_type(asset_name: &str) -> &'static str {
    match asset_name.rsplit('.').next() {
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("woff2") => "font/woff2",
        _ => "text/html; charset=utf-8",
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};

    use super::{
        content_type, managed_not_found_response, parse_file_entries, parse_user_selection,
        request_hostname, serve_static_release,
    };

    /// Keeps recovery selection deliberately one-based and bounded to listed users.
    #[test]
    fn password_recovery_user_selection_is_bounded() {
        assert_eq!(parse_user_selection(" 2\n", 3), Some(1));
        assert_eq!(parse_user_selection("0", 3), None);
        assert_eq!(parse_user_selection("4", 3), None);
        assert_eq!(parse_user_selection("1 extra", 3), None);
    }

    /// Keeps missing managed deployments distinct from the operator dashboard SPA.
    #[test]
    fn managed_not_found_page_is_html_and_not_cacheable() {
        let response = managed_not_found_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
    }

    /// Ensures unknown embedded files cannot select an executable content type.
    #[test]
    fn unknown_assets_use_html_content_type() {
        assert_eq!(content_type("unknown.bin"), "text/html; charset=utf-8");
    }

    /// Ensures gateway host parsing normalizes valid authorities and rejects malformed ones.
    #[test]
    fn gateway_hostname_parses_authority_safely() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HOST,
            HeaderValue::from_static("App.Example.COM:443"),
        );
        assert_eq!(
            request_hostname(&headers).as_deref(),
            Some("app.example.com")
        );
        headers.insert(header::HOST, HeaderValue::from_static("invalid host"));
        assert!(request_hostname(&headers).is_none());
    }

    /// Parses whitespace-bearing file names without treating them as command structure.
    #[test]
    fn container_file_records_are_nul_delimited() {
        let entries =
            parse_file_entries(b"directory\x00\x00my folder\x00file\x0012\x00notes.txt\x00")
                .expect("records are valid");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["name"], "my folder");
        assert_eq!(entries[1]["size"], 12);
        assert!(parse_file_entries(b"file\x001\x00missing terminator").is_err());
    }

    /// Serves files and SPA routes only from the selected immutable release root.
    #[tokio::test]
    async fn static_release_serving_has_safe_fallbacks() {
        let root =
            std::env::temp_dir().join(format!("depot-static-serve-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(root.join("assets")).expect("release root is created");
        std::fs::write(root.join("index.html"), "home").expect("index writes");
        std::fs::write(root.join("assets/app.js"), "app").expect("asset writes");
        let asset =
            serve_static_release(&"/assets/app.js".parse::<Uri>().expect("URI"), &root).await;
        assert_eq!(asset.status(), StatusCode::OK);
        let route =
            serve_static_release(&"/dashboard/settings".parse::<Uri>().expect("URI"), &root).await;
        assert_eq!(route.status(), StatusCode::OK);
        let traversal =
            serve_static_release(&"/../secret".parse::<Uri>().expect("URI"), &root).await;
        assert_eq!(traversal.status(), StatusCode::BAD_REQUEST);
        std::fs::remove_dir_all(root).expect("release root is removed");
    }
}
