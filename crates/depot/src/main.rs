//! Depot's single executable and process-mode entry point.

use std::net::SocketAddr;

use axum::{
    Json, Router,
    body::Body,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use clap::{Parser, Subcommand};
use depot_auth::issue_bootstrap_token;
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
        /// SQLite URL for the Depot control-plane database.
        #[arg(long, env = "DEPOT_DATABASE_URL", default_value = "sqlite://depot.db")]
        database_url: String,
        /// Number of minutes before the displayed setup token expires.
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(i64).range(1..=120))]
        expires_in_minutes: i64,
    },
    /// Serves the API, private gateway, and embedded web application.
    Serve {
        /// Address used by the initial combined HTTP listener.
        #[arg(long, env = "DEPOT_BIND", default_value = "127.0.0.1:8080")]
        bind: SocketAddr,
        /// SQLite URL for the Depot control-plane database.
        #[arg(long, env = "DEPOT_DATABASE_URL", default_value = "sqlite://depot.db")]
        database_url: String,
    },
    /// Prints environment checks required by the current foundation.
    Doctor,
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
}

/// Parses the command line and runs the selected Depot process mode.
#[tokio::main]
async fn main() -> Result<(), AppError> {
    init_tracing();

    match Cli::parse().command {
        Command::Setup {
            database_url,
            expires_in_minutes,
        } => setup(&database_url, expires_in_minutes).await,
        Command::Serve { bind, database_url } => serve(bind, &database_url).await,
        Command::Doctor => {
            println!(
                "depot {}: foundation checks passed",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
    }
}

/// Initializes storage and prints a short-lived bootstrap token exactly once.
async fn setup(database_url: &str, expires_in_minutes: i64) -> Result<(), AppError> {
    let store = Store::connect(database_url).await?;
    let issued = issue_bootstrap_token(&store, Duration::minutes(expires_in_minutes)).await?;
    println!("Depot first-admin setup token (shown once):");
    println!("{}", issued.token);
    println!(
        "Expires at Unix timestamp: {}",
        issued.expires_at.unix_timestamp()
    );
    Ok(())
}

/// Installs structured logging with a safe, operator-configurable filter.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("depot=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Runs the initial combined API and embedded frontend HTTP listener.
async fn serve(bind: SocketAddr, database_url: &str) -> Result<(), AppError> {
    let store = Store::connect(database_url).await?;
    let app = Router::new()
        .route("/api/v1/system/health", get(system_health))
        .merge(depot_api::router(store))
        .fallback(get(serve_spa));
    let listener = TcpListener::bind(bind).await.map_err(AppError::Bind)?;

    info!(%bind, "Depot HTTP listener started");
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
