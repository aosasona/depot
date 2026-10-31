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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, ConfigError};

    /// Ensures the built-in development configuration satisfies all validation.
    #[test]
    fn local_defaults_are_valid() {
        Config::local_default()
            .validate()
            .expect("defaults are valid");
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
}
