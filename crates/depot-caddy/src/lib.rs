//! Restricted generation and atomic reconciliation of Depot-owned Caddy routes.

use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
use depot_core::{validate_domain_hostname, validate_host_port};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::process::Command;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

/// Host-side agent configuration with one fixed gateway and managed directory.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaddyAgentConfig {
    pub bind: SocketAddr,
    pub auth_token: String,
    pub gateway_upstream: String,
    pub allowed_domain_suffixes: Vec<String>,
    pub managed_directory: PathBuf,
    pub caddyfile: PathBuf,
    #[serde(default = "default_caddy_binary")]
    pub caddy_binary: String,
    #[serde(default = "default_command_timeout_seconds")]
    pub command_timeout_seconds: u64,
}

/// Failures which preserve the previously working generated configuration.
#[derive(Debug, Error)]
pub enum CaddyError {
    #[error("could not read Caddy agent configuration: {0}")]
    ConfigRead(#[source] std::io::Error),
    #[error("Caddy agent configuration is invalid: {0}")]
    ConfigParse(#[from] toml::de::Error),
    #[error("Caddy agent configuration has unsafe or incomplete fields")]
    InvalidConfig,
    #[error("domain is malformed or outside the configured suffix allowlist: {0}")]
    DomainRejected(String),
    #[error("could not update the managed Caddy configuration: {0}")]
    Filesystem(#[source] std::io::Error),
    #[error("Caddy command could not be started: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("Caddy command exceeded its configured timeout")]
    Timeout,
    #[error("generated Caddy configuration failed validation")]
    ValidationFailed,
    #[error("Caddy reload failed; the previous generated configuration was restored")]
    ReloadFailed,
    #[error("could not contact the private Caddy agent: {0}")]
    ClientIo(#[source] std::io::Error),
    #[error("private Caddy agent request exceeded its timeout")]
    ClientTimeout,
    #[error("private Caddy agent rejected the desired domain snapshot")]
    ClientRejected,
    #[error("could not bind Caddy agent listener: {0}")]
    Bind(#[source] std::io::Error),
    #[error("Caddy agent listener stopped unexpectedly: {0}")]
    Serve(#[source] std::io::Error),
}

impl CaddyAgentConfig {
    /// Loads and validates the host-owned agent configuration from disk.
    pub fn load(path: &Path) -> Result<Self, CaddyError> {
        let source = fs::read_to_string(path).map_err(CaddyError::ConfigRead)?;
        let config: Self = toml::from_str(&source)?;
        config.validate()?;
        Ok(config)
    }

    /// Rejects broad filesystem roots, empty suffix policy, and unreasonable timeouts.
    pub fn validate(&self) -> Result<(), CaddyError> {
        let unsafe_directory = self.managed_directory.as_os_str().is_empty()
            || self.managed_directory == Path::new("/")
            || self.managed_directory == Path::new(".");
        if unsafe_directory
            || self.allowed_domain_suffixes.is_empty()
            || !validate_host_port(&self.gateway_upstream)
            || self.auth_token.len() < 32
            || !self.auth_token.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
            })
            || self.caddy_binary.trim().is_empty()
            || self.command_timeout_seconds == 0
            || self.command_timeout_seconds > 300
        {
            return Err(CaddyError::InvalidConfig);
        }
        for suffix in &self.allowed_domain_suffixes {
            validate_domain_hostname(suffix).map_err(|_| CaddyError::InvalidConfig)?;
        }
        Ok(())
    }
}

/// Complete desired-domain snapshot accepted by the authenticated agent endpoint.
#[derive(Debug, Deserialize)]
struct ReconcileRequest {
    domains: Vec<String>,
}

/// Minimal acknowledgment which does not overstate DNS or certificate readiness.
#[derive(Debug, Serialize)]
struct ReconcileResponse {
    accepted: usize,
}

#[derive(Debug, Serialize)]
struct AgentHealthResponse {
    status: &'static str,
}

/// Shared agent state serializes filesystem and Caddy side effects.
#[derive(Clone, Debug)]
struct AgentState {
    config: CaddyAgentConfig,
    lock: std::sync::Arc<tokio::sync::Mutex<()>>,
}

/// Serves the private authenticated reconciliation endpoint until shutdown.
pub async fn serve(config: CaddyAgentConfig) -> Result<(), CaddyError> {
    config.validate()?;
    let bind = config.bind;
    let state = AgentState {
        config,
        lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
    };
    let app = Router::new()
        .route("/v1/health", get(health_handler))
        .route("/v1/reconcile", post(reconcile_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(CaddyError::Bind)?;
    axum::serve(listener, app).await.map_err(CaddyError::Serve)
}

/// Validates agent policy and the complete operator-owned Caddy configuration.
pub async fn check(config: &CaddyAgentConfig) -> Result<(), CaddyError> {
    config.validate()?;
    if run_caddy(config, "validate").await? {
        Ok(())
    } else {
        Err(CaddyError::ValidationFailed)
    }
}

/// Sends a complete desired-domain snapshot to one fixed private agent address.
pub async fn send_snapshot(
    address: &str,
    auth_token: &str,
    domains: &[String],
    timeout: Duration,
) -> Result<(), CaddyError> {
    if auth_token.len() < 32
        || !auth_token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~'))
    {
        return Err(CaddyError::ClientRejected);
    }
    let operation = async {
        let body = serde_json::to_vec(&serde_json::json!({ "domains": domains }))
            .expect("string-array JSON serialization is infallible");
        let mut stream = TcpStream::connect(address)
            .await
            .map_err(CaddyError::ClientIo)?;
        let request = format!(
            "POST /v1/reconcile HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {auth_token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(CaddyError::ClientIo)?;
        stream
            .write_all(&body)
            .await
            .map_err(CaddyError::ClientIo)?;
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .map_err(CaddyError::ClientIo)?;
        if response.starts_with(b"HTTP/1.1 200") {
            Ok(())
        } else {
            Err(CaddyError::ClientRejected)
        }
    };
    tokio::time::timeout(timeout, operation)
        .await
        .map_err(|_| CaddyError::ClientTimeout)?
}

async fn health_handler(
    State(state): State<AgentState>,
    headers: HeaderMap,
) -> Result<Json<AgentHealthResponse>, StatusCode> {
    if !valid_bearer_token(&headers, &state.config.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(AgentHealthResponse { status: "ok" }))
}

/// Authenticates and applies one complete desired-domain snapshot serially.
async fn reconcile_handler(
    State(state): State<AgentState>,
    headers: HeaderMap,
    Json(request): Json<ReconcileRequest>,
) -> Result<Json<ReconcileResponse>, StatusCode> {
    if !valid_bearer_token(&headers, &state.config.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let _guard = state.lock.lock().await;
    reconcile(&state.config, &request.domains)
        .await
        .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    Ok(Json(ReconcileResponse {
        accepted: request.domains.len(),
    }))
}

/// Compares the complete bearer token in constant time after exact scheme parsing.
fn valid_bearer_token(headers: &HeaderMap, expected: &str) -> bool {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    value.len() == expected.len() && value.as_bytes().ct_eq(expected.as_bytes()).into()
}

/// Atomically replaces only Depot's generated import and restores it on command failure.
pub async fn reconcile(
    config: &CaddyAgentConfig,
    requested_domains: &[String],
) -> Result<(), CaddyError> {
    config.validate()?;
    let generated = generate_routes(config, requested_domains)?;
    fs::create_dir_all(&config.managed_directory).map_err(CaddyError::Filesystem)?;
    let managed = config.managed_directory.join("routes.caddy");
    let temporary = config.managed_directory.join(".routes.caddy.tmp");
    let backup = config.managed_directory.join(".routes.caddy.previous");
    fs::write(&temporary, generated).map_err(CaddyError::Filesystem)?;
    let had_previous = managed.exists();
    if had_previous {
        fs::rename(&managed, &backup).map_err(CaddyError::Filesystem)?;
    }
    if let Err(error) = fs::rename(&temporary, &managed) {
        if had_previous {
            let _ = fs::rename(&backup, &managed);
        }
        return Err(CaddyError::Filesystem(error));
    }
    if !run_caddy(config, "validate").await? {
        restore_previous(&managed, &backup, had_previous)?;
        return Err(CaddyError::ValidationFailed);
    }
    if !run_caddy(config, "reload").await? {
        restore_previous(&managed, &backup, had_previous)?;
        let _ = run_caddy(config, "reload").await;
        return Err(CaddyError::ReloadFailed);
    }
    if had_previous {
        fs::remove_file(&backup).map_err(CaddyError::Filesystem)?;
    }
    Ok(())
}

/// Generates deterministic minimal routes after normalization, allowlisting, and deduplication.
pub fn generate_routes(
    config: &CaddyAgentConfig,
    requested_domains: &[String],
) -> Result<String, CaddyError> {
    let mut domains = requested_domains
        .iter()
        .map(|domain| {
            let normalized = validate_domain_hostname(domain)
                .map_err(|_| CaddyError::DomainRejected(domain.clone()))?;
            let allowed = config
                .allowed_domain_suffixes
                .iter()
                .any(|suffix| normalized == *suffix || normalized.ends_with(&format!(".{suffix}")));
            allowed
                .then_some(normalized)
                .ok_or_else(|| CaddyError::DomainRejected(domain.clone()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    domains.sort();
    domains.dedup();
    let upstream = &config.gateway_upstream;
    Ok(domains
        .into_iter()
        .map(|domain| format!("{domain} {{\n\treverse_proxy {upstream}\n}}\n"))
        .collect())
}

/// Invokes only the fixed validate or reload Caddy command against the main configuration.
async fn run_caddy(config: &CaddyAgentConfig, action: &str) -> Result<bool, CaddyError> {
    let mut command = Command::new(&config.caddy_binary);
    command
        .arg(action)
        .args(["--config"])
        .arg(&config.caddyfile)
        .args(["--adapter", "caddyfile"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let status = tokio::time::timeout(
        Duration::from_secs(config.command_timeout_seconds),
        command.status(),
    )
    .await
    .map_err(|_| CaddyError::Timeout)?
    .map_err(CaddyError::Spawn)?;
    Ok(status.success())
}

/// Restores exactly the previously managed file after failed validation or reload.
fn restore_previous(managed: &Path, backup: &Path, had_previous: bool) -> Result<(), CaddyError> {
    if managed.exists() {
        fs::remove_file(managed).map_err(CaddyError::Filesystem)?;
    }
    if had_previous {
        fs::rename(backup, managed).map_err(CaddyError::Filesystem)?;
    }
    Ok(())
}

/// Returns the conventional Caddy binary name resolved through the service PATH.
fn default_caddy_binary() -> String {
    "caddy".to_owned()
}

/// Returns a bounded default for local Caddy validation and reload operations.
const fn default_command_timeout_seconds() -> u64 {
    30
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{CaddyAgentConfig, CaddyError, check, generate_routes, reconcile};

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    /// Returns a process-unique root so parallel and retried CI tests cannot share backups.
    fn isolated_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "depot-caddy-{label}-{}-{}",
            std::process::id(),
            TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// Builds a safe isolated configuration for pure generation tests.
    fn config() -> CaddyAgentConfig {
        CaddyAgentConfig {
            bind: "127.0.0.1:9080".parse().expect("bind parses"),
            auth_token: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            gateway_upstream: "gateway.internal:8080".to_owned(),
            allowed_domain_suffixes: vec!["example.com".to_owned()],
            managed_directory: PathBuf::from("/etc/caddy/depot"),
            caddyfile: PathBuf::from("/etc/caddy/Caddyfile"),
            caddy_binary: "caddy".to_owned(),
            command_timeout_seconds: 30,
        }
    }

    /// Proves generated routes are deterministic, deduplicated, and fixed-upstream only.
    #[test]
    fn routes_are_safe_deterministic_and_deduplicated() {
        let output = generate_routes(
            &config(),
            &[
                "B.example.com".to_owned(),
                "b.example.com".to_owned(),
                "a.example.com".to_owned(),
            ],
        )
        .expect("routes generate");
        assert_eq!(output.matches("reverse_proxy").count(), 2);
        assert!(output.starts_with("a.example.com"));
        assert!(output.contains("reverse_proxy gateway.internal:8080"));
    }

    /// Proves unrelated and injection-shaped hostnames cannot enter generated configuration.
    #[test]
    fn routes_reject_domains_outside_allowlist() {
        assert!(generate_routes(&config(), &["attacker.test".to_owned()]).is_err());
        assert!(generate_routes(&config(), &["bad.example.com {".to_owned()]).is_err());
    }

    /// Proves bearer authentication requires an exact token and scheme.
    #[test]
    fn bearer_token_requires_exact_secret() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        );
        assert!(super::valid_bearer_token(
            &headers,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ));
        assert!(!super::valid_bearer_token(
            &headers,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        ));
    }

    /// Proves a setup check validates Caddy without rewriting managed routes.
    #[tokio::test]
    async fn configuration_check_does_not_reconcile_routes() {
        let root = isolated_root("check");
        let managed_directory = root.join("managed");
        fs::create_dir_all(&managed_directory).expect("managed directory is created");
        let routes = managed_directory.join("routes.caddy");
        fs::write(&routes, "manual test sentinel").expect("managed routes write");
        let fake_caddy = root.join("fake-caddy");
        fs::write(&fake_caddy, "#!/bin/sh\nexit 0\n").expect("fake Caddy writes");
        fs::set_permissions(&fake_caddy, fs::Permissions::from_mode(0o700))
            .expect("fake Caddy is executable");
        let mut config = config();
        config.managed_directory = managed_directory;
        config.caddyfile = root.join("Caddyfile");
        config.caddy_binary = fake_caddy.to_string_lossy().into_owned();
        check(&config).await.expect("configuration checks");
        assert_eq!(
            fs::read_to_string(routes).expect("managed routes read"),
            "manual test sentinel"
        );
        fs::remove_dir_all(root).expect("test root is removed");
    }

    /// Proves a failed reload restores the exact previously managed route file.
    #[tokio::test]
    async fn failed_reload_restores_previous_configuration() {
        let root = isolated_root("reload");
        let managed_directory = root.join("managed");
        fs::create_dir_all(&managed_directory).expect("managed directory is created");
        fs::write(managed_directory.join("routes.caddy"), "previous contents")
            .expect("previous config writes");
        let fake_caddy = root.join("fake-caddy");
        fs::write(
            &fake_caddy,
            "#!/bin/sh\nif [ \"$1\" = \"validate\" ]; then exit 0; fi\nexit 1\n",
        )
        .expect("fake Caddy writes");
        fs::set_permissions(&fake_caddy, fs::Permissions::from_mode(0o700))
            .expect("fake Caddy is executable");
        let mut config = config();
        config.managed_directory = managed_directory.clone();
        config.caddyfile = root.join("Caddyfile");
        config.caddy_binary = fake_caddy.to_string_lossy().into_owned();
        let error = reconcile(&config, &["app.example.com".to_owned()])
            .await
            .expect_err("reload fails");
        assert!(
            matches!(error, CaddyError::ReloadFailed),
            "unexpected reconciliation error: {error:?}"
        );
        assert_eq!(
            fs::read_to_string(managed_directory.join("routes.caddy"))
                .expect("restored config reads"),
            "previous contents"
        );
        fs::remove_dir_all(root).expect("test root is removed");
    }
}
