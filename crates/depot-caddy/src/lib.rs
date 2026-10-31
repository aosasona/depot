//! Restricted generation and atomic reconciliation of Depot-owned Caddy routes.

use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use depot_core::validate_domain_hostname;
use serde::Deserialize;
use thiserror::Error;
use tokio::process::Command;

/// Host-side agent configuration with one fixed gateway and managed directory.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaddyAgentConfig {
    pub gateway_upstream: SocketAddr,
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
    let upstream = config.gateway_upstream;
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
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{CaddyAgentConfig, CaddyError, generate_routes, reconcile};

    /// Builds a safe isolated configuration for pure generation tests.
    fn config() -> CaddyAgentConfig {
        CaddyAgentConfig {
            gateway_upstream: "10.0.0.20:8080".parse().expect("upstream parses"),
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
        assert!(output.contains("reverse_proxy 10.0.0.20:8080"));
    }

    /// Proves unrelated and injection-shaped hostnames cannot enter generated configuration.
    #[test]
    fn routes_reject_domains_outside_allowlist() {
        assert!(generate_routes(&config(), &["attacker.test".to_owned()]).is_err());
        assert!(generate_routes(&config(), &["bad.example.com {".to_owned()]).is_err());
    }

    /// Proves a failed reload restores the exact previously managed route file.
    #[tokio::test]
    async fn failed_reload_restores_previous_configuration() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock follows epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("depot-caddy-test-{nonce}"));
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
        assert!(matches!(error, CaddyError::ReloadFailed));
        assert_eq!(
            fs::read_to_string(managed_directory.join("routes.caddy"))
                .expect("restored config reads"),
            "previous contents"
        );
        fs::remove_dir_all(root).expect("test root is removed");
    }
}
