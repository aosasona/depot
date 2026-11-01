//! Shared configuration and domain-level invariants for Depot processes.

use std::{fs, net::SocketAddr, path::Path};

use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;
use url::Url;

/// Complete configuration consumed by the API and worker process modes.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub security: SecurityConfig,
    #[serde(default)]
    pub domains: DomainConfig,
    pub worker: WorkerConfig,
    #[serde(default)]
    pub github: Option<GithubAppConfig>,
}

/// Optional installation-wide defaults for newly created project hostnames.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainConfig {
    #[serde(default)]
    pub default_suffix: Option<String>,
}

/// Operator-owned GitHub App configuration shared by API and workers.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubAppConfig {
    pub app_id: u64,
    pub app_slug: String,
    pub private_key_file: String,
    pub webhook_secret_file: String,
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
    #[serde(default = "default_static_root")]
    pub static_root: String,
    pub git_timeout_seconds: u64,
    #[serde(default = "default_docker_binary")]
    pub docker_binary: String,
    #[serde(default = "default_railpack_binary")]
    pub railpack_binary: String,
    #[serde(default = "default_shell_socket")]
    pub shell_socket: String,
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

/// Returns the conventional Railpack CLI name for operator PATH resolution.
fn default_railpack_binary() -> String {
    "railpack".to_owned()
}

/// Returns the group-restricted local socket shared by the API and worker.
fn default_shell_socket() -> String {
    "/var/lib/depot/worker-shell.sock".to_owned()
}

/// Signed short-lived request passed from the authenticated API to the Docker worker.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ShellHandshake {
    pub session_id: String,
    pub project_id: String,
    pub container_id: String,
    pub actor_user_id: String,
    #[serde(default)]
    pub operation: WorkerOperation,
    pub expires_at: i64,
    pub nonce: String,
    pub signature: String,
}

/// Enumerates the only container operations accepted by the privileged worker broker.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerOperation {
    #[default]
    Shell,
    List {
        path: String,
    },
    Download {
        path: String,
    },
    Upload {
        path: String,
        size: u64,
    },
    CreateDirectory {
        path: String,
    },
    Rename {
        path: String,
        destination: String,
    },
    Delete {
        path: String,
    },
}

/// Domain-separates the local shell MAC key from the secret-encryption key.
pub fn derive_shell_signing_key(master_key: &[u8; 32]) -> [u8; 32] {
    let mut digest = Sha256::new();
    use sha2::Digest;
    digest.update(b"depot:worker-shell:v1:");
    digest.update(master_key);
    digest.finalize().into()
}

/// Accepts one canonical absolute container path without traversal or control bytes.
pub fn valid_container_path(value: &str, allow_root: bool) -> bool {
    if value.is_empty()
        || value.len() > 4096
        || !value.starts_with('/')
        || value.bytes().any(|byte| byte.is_ascii_control())
        || value.contains("//")
        || (value.len() > 1 && value.ends_with('/'))
        || (!allow_root && value == "/")
    {
        return false;
    }
    value
        .split('/')
        .skip(1)
        .all(|component| !component.is_empty() && component != "." && component != "..")
        || (allow_root && value == "/")
}

impl ShellHandshake {
    /// Signs every capability field while deliberately excluding the signature itself.
    pub fn sign(&mut self, key: &[u8; 32]) {
        let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts a 256-bit key");
        mac.update(self.canonical().as_bytes());
        self.signature = encode_hex(&mac.finalize().into_bytes());
    }

    /// Authenticates the complete capability using constant-time MAC verification.
    pub fn verify(&self, key: &[u8; 32]) -> bool {
        let Some(signature) = decode_hex_32(&self.signature) else {
            return false;
        };
        let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts a 256-bit key");
        mac.update(self.canonical().as_bytes());
        mac.verify_slice(&signature).is_ok()
    }

    /// Produces an unambiguous length-prefixed capability representation.
    fn canonical(&self) -> String {
        let fields = [
            self.session_id.as_str(),
            self.project_id.as_str(),
            self.container_id.as_str(),
            self.actor_user_id.as_str(),
            self.nonce.as_str(),
        ];
        let mut value = format!("{}|", self.expires_at);
        for field in fields {
            value.push_str(&format!("{}:{field}|", field.len()));
        }
        let operation = self.operation.canonical();
        value.push_str(&format!("{}:{operation}|", operation.len()));
        value
    }
}

impl WorkerOperation {
    /// Produces a stable length-prefixed representation for capability signing.
    fn canonical(&self) -> String {
        /// Appends one unambiguous string field to a signed operation value.
        fn field(value: &mut String, field: &str) {
            value.push_str(&format!("{}:{field}|", field.len()));
        }

        let mut value = String::new();
        match self {
            Self::Shell => value.push_str("shell|"),
            Self::List { path } => {
                value.push_str("list|");
                field(&mut value, path);
            }
            Self::Download { path } => {
                value.push_str("download|");
                field(&mut value, path);
            }
            Self::Upload { path, size } => {
                value.push_str(&format!("upload|{size}|"));
                field(&mut value, path);
            }
            Self::CreateDirectory { path } => {
                value.push_str("create_directory|");
                field(&mut value, path);
            }
            Self::Rename { path, destination } => {
                value.push_str("rename|");
                field(&mut value, path);
                field(&mut value, destination);
            }
            Self::Delete { path } => {
                value.push_str("delete|");
                field(&mut value, path);
            }
        }
        value
    }
}

/// Encodes authentication bytes without introducing another protocol dependency.
fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

/// Decodes an exact 256-bit hexadecimal signature.
fn decode_hex_32(encoded: &str) -> Option<[u8; 32]> {
    if encoded.len() != 64 {
        return None;
    }
    let mut decoded = [0_u8; 32];
    for (index, pair) in encoded.as_bytes().chunks_exact(2).enumerate() {
        let high = decode_nibble(pair[0])?;
        let low = decode_nibble(pair[1])?;
        decoded[index] = (high << 4) | low;
    }
    Some(decoded)
}

/// Converts one ASCII hexadecimal character into its four-bit value.
const fn decode_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

/// Returns the production static artifact root used by packaged installations.
fn default_static_root() -> String {
    "/var/lib/depot/static".to_owned()
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
    #[error("domains.default_suffix must be a valid public DNS suffix")]
    InvalidDefaultDomainSuffix,
    #[error("worker workspace and timeout configuration is invalid")]
    InvalidWorkerConfig,
    #[error("github app configuration is invalid")]
    InvalidGithubConfig,
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
            domains: DomainConfig::default(),
            worker: WorkerConfig {
                workspace_root: ".tmp/workspaces".to_owned(),
                static_root: ".tmp/static".to_owned(),
                git_timeout_seconds: 120,
                docker_binary: default_docker_binary(),
                railpack_binary: default_railpack_binary(),
                shell_socket: ".tmp/worker-shell.sock".to_owned(),
                docker_timeout_seconds: default_docker_timeout_seconds(),
                caddy_agent: None,
            },
            github: None,
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
        if self.domains.default_suffix.as_ref().is_some_and(|suffix| {
            !matches!(validate_domain_hostname(suffix), Ok(normalized) if normalized == *suffix)
                || validate_domain_hostname(&format!("{}.{}", "a".repeat(63), suffix)).is_err()
        }) {
            return Err(ConfigError::InvalidDefaultDomainSuffix);
        }
        if self.worker.workspace_root.trim().is_empty()
            || self.worker.static_root.trim().is_empty()
            || matches!(self.worker.static_root.as_str(), "/" | ".")
            || self.worker.git_timeout_seconds == 0
            || self.worker.git_timeout_seconds > 1_800
            || self.worker.docker_binary.trim().is_empty()
            || self.worker.railpack_binary.trim().is_empty()
            || self.worker.shell_socket.trim().is_empty()
            || !self.worker.shell_socket.ends_with(".sock")
            || Path::new(&self.worker.shell_socket).parent()
                != Path::new(&self.worker.workspace_root).parent()
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
        if self.github.as_ref().is_some_and(|github| {
            github.app_id == 0
                || github.app_slug.is_empty()
                || github.private_key_file.trim().is_empty()
                || github.webhook_secret_file.trim().is_empty()
                || github.private_key_file == github.webhook_secret_file
                || !github
                    .app_slug
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        }) {
            return Err(ConfigError::InvalidGithubConfig);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Config, ConfigError, ProjectValidationError, ShellHandshake, WorkerOperation,
        valid_container_path, validate_domain_hostname, validate_host_port, validate_project_slug,
        validate_repository_url,
    };

    /// Proves shell capabilities cannot be altered after the API signs them.
    #[test]
    fn shell_handshake_authenticates_every_capability_field() {
        let key = [19_u8; 32];
        let mut handshake = ShellHandshake {
            session_id: "session".to_owned(),
            project_id: "project".to_owned(),
            container_id: "container".to_owned(),
            actor_user_id: "user".to_owned(),
            operation: WorkerOperation::Shell,
            expires_at: 100,
            nonce: "nonce".to_owned(),
            signature: String::new(),
        };
        handshake.sign(&key);
        assert!(handshake.verify(&key));
        handshake.operation = WorkerOperation::Delete {
            path: "/app/data".to_owned(),
        };
        assert!(!handshake.verify(&key));
        handshake.operation = WorkerOperation::Shell;
        handshake.sign(&key);
        handshake.container_id = "other".to_owned();
        assert!(!handshake.verify(&key));
    }

    /// Ensures container file capabilities cannot traverse or target root mutations.
    #[test]
    fn container_paths_are_absolute_and_canonical() {
        assert!(valid_container_path("/app/config.json", false));
        assert!(valid_container_path("/", true));
        assert!(!valid_container_path("/", false));
        assert!(!valid_container_path("app/config.json", false));
        assert!(!valid_container_path("/app/../etc/passwd", false));
        assert!(!valid_container_path("/app//config.json", false));
        assert!(!valid_container_path("/app/config\n.json", false));
    }

    /// Ensures the built-in development configuration satisfies all validation.
    #[test]
    fn local_defaults_are_valid() {
        Config::local_default()
            .validate()
            .expect("defaults are valid");
    }

    #[test]
    fn default_domain_suffix_is_normalized_and_public() {
        let mut config = Config::local_default();
        config.domains.default_suffix = Some("apps.example.com".to_owned());
        config.validate().expect("public suffix is valid");

        config.domains.default_suffix = Some("Apps.Example.com.".to_owned());
        assert!(matches!(
            config.validate(),
            Err(ConfigError::InvalidDefaultDomainSuffix)
        ));
    }

    /// Prevents the stale-socket cleanup from ever targeting Docker or system sockets.
    #[test]
    fn shell_socket_must_share_the_workspace_parent() {
        let mut config = Config::local_default();
        config.worker.shell_socket = "/var/run/docker.sock".to_owned();
        assert!(matches!(
            config.validate(),
            Err(ConfigError::InvalidWorkerConfig)
        ));
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
