//! Depot's single executable and process-mode entry point.

use std::path::{Path, PathBuf};

use axum::{
    Json, Router,
    body::Body,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use clap::{Parser, Subcommand};
use depot_auth::issue_bootstrap_token;
use depot_core::Config;
use depot_store::Store;
use rust_embed::Embed;
use serde::Serialize;
use thiserror::Error;
use time::Duration;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;

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
    /// Claims and executes durable background jobs from the shared SQLite queue.
    Worker {
        /// TOML configuration path; local development defaults apply when omitted.
        #[arg(long, env = "DEPOT_CONFIG")]
        config: Option<PathBuf>,
        /// Claims at most one available job and exits, primarily for supervision checks.
        #[arg(long)]
        once: bool,
    },
}

/// Describes a successful liveness response without overstating dependency health.
#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
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
        Command::Serve { config } => serve(load_config(config.as_deref())?).await,
        Command::Migrate { config } => migrate(load_config(config.as_deref())?).await,
        Command::Doctor { config } => doctor(load_config(config.as_deref())?).await,
        Command::Worker { config, once } => worker(load_config(config.as_deref())?, once).await,
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
    println!("Depot {} is ready to start.", env!("CARGO_PKG_VERSION"));
    Ok(())
}

/// Runs the durable worker claim loop with bounded polling and graceful shutdown.
async fn worker(config: Config, once: bool) -> Result<(), AppError> {
    let store = Store::connect(&config.database.url).await?;
    let worker_id = format!("worker-{}", uuid::Uuid::now_v7());
    info!(%worker_id, "Depot worker started");
    loop {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let claimed = store.claim_job(&worker_id, now, 60).await?;
        if let Some(job) = claimed {
            process_job(&store, &config.worker, &worker_id, job).await?;
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

/// Records durable events and completion state around one claimed job attempt.
async fn process_job(
    store: &Store,
    worker_config: &depot_core::WorkerConfig,
    worker_id: &str,
    job: depot_store::Job,
) -> Result<(), AppError> {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    store
        .append_job_event(&job.id, "info", "Worker claimed durable job.", now)
        .await?;
    match execute_job(store, worker_config, &job).await {
        Ok(result_json) => {
            let completed_at = time::OffsetDateTime::now_utc().unix_timestamp();
            store
                .append_job_event(
                    &job.id,
                    "info",
                    "Repository inspection completed.",
                    completed_at,
                )
                .await?;
            store
                .complete_job(&job.id, worker_id, Some(&result_json), completed_at)
                .await?;
        }
        Err(message) => {
            let failed_at = time::OffsetDateTime::now_utc().unix_timestamp();
            store
                .append_job_event(&job.id, "error", &message, failed_at)
                .await?;
            store
                .fail_job(&job.id, worker_id, &message, 30, failed_at)
                .await?;
            tracing::warn!(job_id = %job.id, kind = %job.kind, "job attempt failed");
        }
    }
    Ok(())
}

/// Executes a supported job kind and returns its bounded JSON result.
async fn execute_job(
    store: &Store,
    worker_config: &depot_core::WorkerConfig,
    job: &depot_store::Job,
) -> Result<String, String> {
    if job.kind != "inspect_repository" {
        return Err(format!("Unsupported job kind: {}", job.kind));
    }
    let project_id = job
        .project_id
        .as_deref()
        .ok_or_else(|| "Inspection job has no project reference.".to_owned())?;
    let project = store
        .project_by_id(project_id)
        .await
        .map_err(|_| "Inspection project no longer exists.".to_owned())?;
    let inspection = depot_git::inspect_repository(
        &project.repository_url,
        &project.branch,
        std::path::Path::new(&worker_config.workspace_root),
        std::time::Duration::from_secs(worker_config.git_timeout_seconds),
    )
    .await
    .map_err(|error| error.to_string())?;
    serde_json::to_string(&inspection)
        .map_err(|_| "Repository inspection result could not be encoded.".to_owned())
}

/// Installs structured logging with a safe, operator-configurable filter.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("depot=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Runs the initial combined API and embedded frontend HTTP listener.
async fn serve(config: Config) -> Result<(), AppError> {
    let store = Store::connect(&config.database.url).await?;
    let app = Router::new()
        .route("/api/v1/system/health", get(system_health))
        .merge(depot_api::router(store))
        .fallback(get(serve_spa));
    let listener = TcpListener::bind(config.server.bind)
        .await
        .map_err(AppError::Bind)?;

    info!(bind = %config.server.bind, "Depot HTTP listener started");
    axum::serve(listener, app).await.map_err(AppError::Serve)
}

/// Reports API process liveness; dependency readiness will use a separate endpoint.
async fn system_health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "alive",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// Serves a requested embedded asset or the SPA shell for client-side routes.
async fn serve_spa(uri: Uri) -> Response {
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
    use super::content_type;

    /// Ensures unknown embedded files cannot select an executable content type.
    #[test]
    fn unknown_assets_use_html_content_type() {
        assert_eq!(content_type("unknown.bin"), "text/html; charset=utf-8");
    }
}
