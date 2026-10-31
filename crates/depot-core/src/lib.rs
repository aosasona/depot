//! Shared configuration and domain-level invariants for Depot processes.

use std::{fs, net::SocketAddr, path::Path};

use serde::Deserialize;
use thiserror::Error;
use url::Url;

/// Complete configuration consumed by the API and worker process modes.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub security: SecurityConfig,
    pub worker: WorkerConfig,
}

/// Listener configuration for the combined v0.1 API and private gateway process.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub bind: SocketAddr,
}

/// SQLite connection configuration kept explicit for future backup tooling.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    pub url: String,
}

/// Browser trust boundary used for origin checks and externally visible links.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityConfig {
    pub public_origin: String,
    pub master_key_file: String,
}

/// Filesystem and timeout limits applied to isolated background work.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerConfig {
    pub workspace_root: String,
    pub git_timeout_seconds: u64,
    #[serde(default = "default_docker_binary")]
    pub docker_binary: String,
    #[serde(default = "default_docker_timeout_seconds")]
    pub docker_timeout_seconds: u64,
    pub caddy_agent: Option<CaddyAgentClientConfig>,
}

/// Private authenticated channel used by the worker for Caddy reconciliation.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaddyAgentClientConfig {
    pub address: String,
    pub auth_token: String,
    #[serde(default = "default_caddy_agent_timeout_seconds")]
    pub timeout_seconds: u64,
}

/// Returns the conventional Docker CLI name for operator PATH resolution.
fn default_docker_binary() -> String {
    "docker".to_owned()
}

/// Returns a conservative default wall-clock limit for application image builds.
const fn default_docker_timeout_seconds() -> u64 {
    1_800
}

/// Returns a short bounded timeout for private Caddy-agent requests.
const fn default_caddy_agent_timeout_seconds() -> u64 {
    30
}

/// Configuration load and validation failures with actionable field context.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not read configuration {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("configuration TOML is invalid: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("database.url must be a SQLite URL")]
    InvalidDatabaseUrl,
    #[error("security.public_origin must contain only an http(s) origin without a path")]
    InvalidPublicOrigin,
    #[error("security.master_key_file must not be empty")]
    InvalidMasterKeyFile,
    #[error("worker workspace and timeout configuration is invalid")]
    InvalidWorkerConfig,
}

/// Accepts an IP address or DNS name with an explicit TCP port and no URL syntax.
pub fn validate_host_port(value: &str) -> bool {
    let Ok(url) = Url::parse(&format!("http://{value}")) else {
        return false;
    };
    url.host_str().is_some()
        && url.port().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
}

/// Validation failures for project identity and Git source configuration.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum ProjectValidationError {
    #[error("project slug must contain 2-63 lowercase letters, numbers, or hyphens")]
    InvalidSlug,
    #[error("repository URL must use https or ssh and include a public host")]
    InvalidRepositoryUrl,
    #[error("repository URL cannot target localhost or a private IP address")]
    PrivateRepositoryHost,
    #[error("domain hostname must be a public lowercase DNS name")]
    InvalidDomainHostname,
}

/// Validates a stable project slug suitable for database and filesystem identity.
pub fn validate_project_slug(slug: &str) -> Result<(), ProjectValidationError> {
    let valid_length = (2..=63).contains(&slug.len());
    let valid_edges = !slug.starts_with('-') && !slug.ends_with('-');
    let valid_chars = slug
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid_length && valid_edges && valid_chars {
        Ok(())
    } else {
        Err(ProjectValidationError::InvalidSlug)
    }
}

/// Rejects unsafe Git URLs before any worker attempts an outbound connection.
pub fn validate_repository_url(value: &str) -> Result<(), ProjectValidationError> {
    let url = Url::parse(value).map_err(|_| ProjectValidationError::InvalidRepositoryUrl)?;
    let valid_username = url.username().is_empty()
        || (url.scheme() == "ssh" && url.username().eq_ignore_ascii_case("git"));
    if !matches!(url.scheme(), "https" | "ssh")
        || url.host_str().is_none()
        || !valid_username
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ProjectValidationError::InvalidRepositoryUrl);
    }
    let host = url.host_str().expect("host presence checked");
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err(ProjectValidationError::PrivateRepositoryHost);
    }
    if let Ok(address) = host.parse::<std::net::IpAddr>() {
        let is_private = match address {
            std::net::IpAddr::V4(address) => {
                address.is_private()
                    || address.is_loopback()
                    || address.is_link_local()
                    || address.is_unspecified()
            }
            std::net::IpAddr::V6(address) => {
                address.is_loopback() || address.is_unspecified() || address.is_unique_local()
            }
        };
        if is_private {
            return Err(ProjectValidationError::PrivateRepositoryHost);
        }
    }
    Ok(())
}

/// Normalizes and validates one public DNS hostname without schemes, ports, or wildcards.
pub fn validate_domain_hostname(value: &str) -> Result<String, ProjectValidationError> {
    let hostname = value.trim().trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<_> = hostname.split('.').collect();
    let valid_label = |label: &&str| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    };
    if hostname.len() > 253
        || labels.len() < 2
        || !labels.iter().all(valid_label)
        || hostname == "localhost"
        || hostname.ends_with(".localhost")
        || hostname.parse::<std::net::IpAddr>().is_ok()
    {
        return Err(ProjectValidationError::InvalidDomainHostname);
    }
    Ok(hostname)
}

impl Config {
    /// Loads strict TOML and validates security-sensitive values before startup.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.display().to_string(),
            source,
        })?;
        let config: Self = toml::from_str(&contents)?;
        config.validate()?;
        Ok(config)
    }

    /// Returns development-safe local defaults when no config file is supplied.
    pub fn local_default() -> Self {
        Self {
            server: ServerConfig {
                bind: "127.0.0.1:8080"
                    .parse()
                    .expect("static socket address is valid"),
            },
            database: DatabaseConfig {
                url: "sqlite://depot.db".to_owned(),
            },
            security: SecurityConfig {
                public_origin: "http://127.0.0.1:8080".to_owned(),
                master_key_file: ".tmp/master.key".to_owned(),
            },
            worker: WorkerConfig {
                workspace_root: ".tmp/workspaces".to_owned(),
                git_timeout_seconds: 120,
                docker_binary: default_docker_binary(),
                docker_timeout_seconds: default_docker_timeout_seconds(),
                caddy_agent: None,
            },
        }
    }

    /// Rejects values which would weaken persistence or browser-origin boundaries.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !self.database.url.starts_with("sqlite:") {
            return Err(ConfigError::InvalidDatabaseUrl);
        }
        let origin = Url::parse(&self.security.public_origin)
            .map_err(|_| ConfigError::InvalidPublicOrigin)?;
        let valid_scheme = matches!(origin.scheme(), "http" | "https");
        let is_origin_only = origin.path() == "/"
            && origin.query().is_none()
            && origin.fragment().is_none()
            && !origin.cannot_be_a_base();
        if !valid_scheme || !is_origin_only || origin.host_str().is_none() {
            return Err(ConfigError::InvalidPublicOrigin);
        }
        if self.security.master_key_file.trim().is_empty() {
            return Err(ConfigError::InvalidMasterKeyFile);
        }
        if self.worker.workspace_root.trim().is_empty()
            || self.worker.git_timeout_seconds == 0
            || self.worker.git_timeout_seconds > 1_800
            || self.worker.docker_binary.trim().is_empty()
            || self.worker.docker_timeout_seconds == 0
            || self.worker.docker_timeout_seconds > 7_200
        {
            return Err(ConfigError::InvalidWorkerConfig);
        }
        if self.worker.caddy_agent.as_ref().is_some_and(|agent| {
            !validate_host_port(&agent.address)
                || agent.auth_token.len() < 32
                || !agent.auth_token.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
                })
                || agent.timeout_seconds == 0
                || agent.timeout_seconds > 300
        }) {
            return Err(ConfigError::InvalidWorkerConfig);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Config, ConfigError, ProjectValidationError, validate_domain_hostname, validate_host_port,
        validate_project_slug, validate_repository_url,
    };

    /// Ensures the built-in development configuration satisfies all validation.
    #[test]
    fn local_defaults_are_valid() {
        Config::local_default()
            .validate()
            .expect("defaults are valid");
    }

    #[test]
    fn host_port_accepts_dns_and_ip_but_rejects_url_syntax() {
        assert!(validate_host_port("caddy.internal:9080"));
        assert!(validate_host_port("10.0.0.5:9080"));
        assert!(validate_host_port("[fd00::5]:9080"));
        assert!(!validate_host_port("caddy.internal"));
        assert!(!validate_host_port("https://caddy.internal:9080"));
        assert!(!validate_host_port("user@caddy.internal:9080"));
    }

    /// Ensures public origins cannot smuggle paths into origin validation.
    #[test]
    fn public_origin_rejects_paths() {
        let mut config = Config::local_default();
        config.security.public_origin = "https://depot.example.com/unsafe".to_owned();
        assert!(matches!(
            config.validate(),
            Err(ConfigError::InvalidPublicOrigin)
        ));
    }

    /// Ensures non-SQLite connection strings cannot enter this single-node release.
    #[test]
    fn database_rejects_other_engines() {
        let mut config = Config::local_default();
        config.database.url = "postgres://localhost/depot".to_owned();
        assert!(matches!(
            config.validate(),
            Err(ConfigError::InvalidDatabaseUrl)
        ));
    }

    /// Ensures project slugs remain safe across URLs, SQLite, and release paths.
    #[test]
    fn project_slug_rejects_path_like_values() {
        validate_project_slug("my-service").expect("ordinary slug is valid");
        assert_eq!(
            validate_project_slug("../escape"),
            Err(ProjectValidationError::InvalidSlug)
        );
        assert_eq!(
            validate_project_slug("Uppercase"),
            Err(ProjectValidationError::InvalidSlug)
        );
    }

    /// Ensures repository validation blocks local network literals and credentials.
    #[test]
    fn repository_url_rejects_private_targets() {
        validate_repository_url("https://github.com/example/project.git")
            .expect("public HTTPS repository is valid");
        assert_eq!(
            validate_repository_url("https://127.0.0.1/repository.git"),
            Err(ProjectValidationError::PrivateRepositoryHost)
        );
        assert_eq!(
            validate_repository_url("https://user:secret@example.com/repository.git"),
            Err(ProjectValidationError::InvalidRepositoryUrl)
        );
    }

    /// Ensures domain input is normalized while local and malformed names are rejected.
    #[test]
    fn domain_hostname_is_normalized_and_public() {
        assert_eq!(
            validate_domain_hostname(" App.Example.COM. ").expect("hostname is valid"),
            "app.example.com"
        );
        assert!(matches!(
            validate_domain_hostname("localhost"),
            Err(ProjectValidationError::InvalidDomainHostname)
        ));
        assert!(validate_domain_hostname("https://example.com").is_err());
    }
}
