//! Authentication primitives and secure first-run initialization.

use argon2::{Algorithm, Argon2, Params, Version, password_hash::PasswordHasher};
use depot_store::{BootstrapAdmin, Store, StoreError};
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::{Duration, OffsetDateTime};

const BOOTSTRAP_TOKEN_BYTES: usize = 32;
const PASSWORD_SALT_BYTES: usize = 16;
const MINIMUM_PASSWORD_LENGTH: usize = 12;

/// A newly issued plaintext bootstrap token shown once to the operator.
#[derive(Debug)]
pub struct IssuedBootstrapToken {
    pub token: String,
    pub expires_at: OffsetDateTime,
}

/// Authentication failures safe for callers to classify without leaking secrets.
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("password must contain at least {MINIMUM_PASSWORD_LENGTH} characters")]
    WeakPassword,
    #[error("email address is invalid")]
    InvalidEmail,
    #[error("password hashing failed")]
    PasswordHash,
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Generates a one-time setup credential and persists only its SHA-256 digest.
pub async fn issue_bootstrap_token(
    store: &Store,
    lifetime: Duration,
) -> Result<IssuedBootstrapToken, AuthError> {
    let now = OffsetDateTime::now_utc();
    let expires_at = now + lifetime;
    let token = encode_hex(&rand::random::<[u8; BOOTSTRAP_TOKEN_BYTES]>());
    let token_hash = hash_token(&token);
    store
        .replace_bootstrap_token(
            &token_hash,
            expires_at.unix_timestamp(),
            now.unix_timestamp(),
        )
        .await?;
    Ok(IssuedBootstrapToken { token, expires_at })
}

/// Validates setup input, hashes the password off-thread, and creates the first admin.
pub async fn create_first_admin(
    store: &Store,
    token: &str,
    email: &str,
    password: String,
) -> Result<String, AuthError> {
    let email = normalize_email(email)?;
    if password.chars().count() < MINIMUM_PASSWORD_LENGTH {
        return Err(AuthError::WeakPassword);
    }
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|_| AuthError::PasswordHash)??;
    let token_hash = hash_token(token);
    store
        .create_first_admin(BootstrapAdmin {
            email: &email,
            password_hash: &password_hash,
            token_hash: &token_hash,
            now_unix: OffsetDateTime::now_utc().unix_timestamp(),
        })
        .await
        .map_err(AuthError::from)
}

/// Applies a deliberately conservative email normalization for local identities.
fn normalize_email(email: &str) -> Result<String, AuthError> {
    let normalized = email.trim().to_lowercase();
    let valid = normalized.len() <= 254
        && !normalized.contains(char::is_whitespace)
        && normalized
            .split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'));
    valid.then_some(normalized).ok_or(AuthError::InvalidEmail)
}

/// Hashes a password with explicit Argon2id parameters and a fresh random salt.
fn hash_password(password: &str) -> Result<String, AuthError> {
    let salt_bytes = rand::random::<[u8; PASSWORD_SALT_BYTES]>();
    let params = Params::new(19_456, 2, 1, None).map_err(|_| AuthError::PasswordHash)?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_with_salt(password.as_bytes(), &salt_bytes)
        .map(|hash| hash.to_string())
        .map_err(|_| AuthError::PasswordHash)
}

/// Produces the fixed-size digest stored instead of a bearer setup token.
fn hash_token(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// Encodes random bytes without adding another secret-handling dependency.
fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use argon2::{Argon2, PasswordHash, PasswordVerifier};

    use super::{AuthError, encode_hex, hash_password, normalize_email};

    /// Ensures passwords are stored as verifiable Argon2id PHC strings.
    #[test]
    fn password_hash_uses_argon2id_and_verifies() {
        let encoded = hash_password("a sufficiently long password").expect("password hashes");
        let parsed = PasswordHash::new(&encoded).expect("PHC string parses");
        assert_eq!(parsed.algorithm.as_str(), "argon2id");
        Argon2::default()
            .verify_password(b"a sufficiently long password", &parsed)
            .expect("password verifies");
    }

    /// Ensures identity normalization is predictable and rejects malformed values.
    #[test]
    fn email_normalization_is_conservative() {
        assert_eq!(
            normalize_email(" Admin@Example.COM ").expect("email is valid"),
            "admin@example.com"
        );
        assert!(matches!(
            normalize_email("admin"),
            Err(AuthError::InvalidEmail)
        ));
    }

    /// Ensures the local encoder preserves byte order and leading zeroes.
    #[test]
    fn hexadecimal_encoding_is_stable() {
        assert_eq!(encode_hex(&[0, 15, 16, 255]), "000f10ff");
    }
}
