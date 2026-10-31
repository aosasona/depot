//! Depot's single executable and process-mode entry point.

use std::path::{Path, PathBuf};

use axum::{
    Json, Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use clap::{Parser, Subcommand};
use depot_auth::issue_bootstrap_token;
use depot_core::Config;
use depot_store::Store;
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
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
    /// Reconciles an authenticated desired-domain snapshot on the Caddy host.
    CaddyAgent {
        /// Host-owned Caddy agent TOML configuration path.
        #[arg(long, env = "DEPOT_CADDY_AGENT_CONFIG")]
        config: PathBuf,
        /// JSON file containing the complete desired hostname array.
        #[arg(long)]
        domains_file: Option<PathBuf>,
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
        Command::CaddyAgent {
            config,
            domains_file,
        } => caddy_agent(&config, domains_file.as_deref()).await,
    }
}

/// Applies one complete desired-domain snapshot through the restricted host reconciler.
async fn caddy_agent(config_path: &Path, domains_file: Option<&Path>) -> Result<(), AppError> {
    let config = depot_caddy::CaddyAgentConfig::load(config_path)?;
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
            let completion_message = if job.kind == "inspect_repository" {
                "Repository inspection completed."
            } else {
                "Deployment work completed."
            };
            store
                .append_job_event(&job.id, "info", completion_message, completed_at)
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
    match job.kind.as_str() {
        "inspect_repository" => execute_inspection(store, worker_config, job).await,
        "deploy_project" => execute_deployment(store, worker_config, job).await,
        _ => Err(format!("Unsupported job kind: {}", job.kind)),
    }
}

/// Inspects one authoritative project source and returns its immutable build plan.
async fn execute_inspection(
    store: &Store,
    worker_config: &depot_core::WorkerConfig,
    job: &depot_store::Job,
) -> Result<String, String> {
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

/// Checks out the recorded SHA and builds a Dockerfile image without shell invocation.
async fn execute_deployment(
    store: &Store,
    worker_config: &depot_core::WorkerConfig,
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
    if config.project_kind == "static" {
        return Err("Static artifact builds are not implemented yet.".to_owned());
    }
    if config.builder == "railpack" {
        return Err("Railpack builds are not implemented yet.".to_owned());
    }
    let checkout = depot_git::checkout_repository(
        &project.repository_url,
        &project.branch,
        &deployment.commit_sha,
        std::path::Path::new(&worker_config.workspace_root),
        std::time::Duration::from_secs(worker_config.git_timeout_seconds),
    )
    .await
    .map_err(|error| error.to_string())?;
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
    let running = depot_runtime::start_container(depot_runtime::StartContainerRequest {
        docker_binary: &worker_config.docker_binary,
        deployment_id: &deployment.id,
        image_id: &build.image_id,
        container_port,
        timeout: std::time::Duration::from_secs(30),
    })
    .await
    .map_err(|error| error.to_string())?;
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

/// Installs structured logging with a safe, operator-configurable filter.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("depot=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Runs the initial combined API and embedded frontend HTTP listener.
async fn serve(config: Config) -> Result<(), AppError> {
    let store = Store::connect(&config.database.url).await?;
    let gateway_store = store.clone();
    let app = Router::new()
        .route("/api/v1/system/health", get(system_health))
        .merge(depot_api::router(store.clone()))
        .fallback(move |request| gateway_or_spa(gateway_store.clone(), request));
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

/// Routes exact project hostnames to healthy loopback runtimes, otherwise serves Depot's SPA.
async fn gateway_or_spa(store: Store, request: Request<Body>) -> Response {
    if request.uri().path().starts_with("/api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let hostname = request_hostname(request.headers());
    if let Some(hostname) = hostname {
        match store.gateway_target(&hostname).await {
            Ok(Some(target)) => return proxy_to_runtime(request, target.host_port).await,
            Ok(None) => {}
            Err(error) => {
                tracing::error!(%error, "gateway target lookup failed");
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
        }
    }
    serve_spa(request.uri()).await
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
    use axum::http::{HeaderMap, HeaderValue, header};

    use super::{content_type, request_hostname};

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
}
