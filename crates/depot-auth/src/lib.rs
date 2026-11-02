//! Authentication primitives and secure first-run initialization.

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use depot_store::{
    BootstrapAdmin, DeployToken, DeployTokenIdentity, NewDeployToken, NewSession, NewUser,
    SessionUser, Store, StoreError, UserRole, UserSummary,
};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;
use time::{Duration, OffsetDateTime};

const BOOTSTRAP_TOKEN_BYTES: usize = 32;
const PASSWORD_SALT_BYTES: usize = 16;
const MINIMUM_PASSWORD_LENGTH: usize = 12;
const SESSION_TOKEN_BYTES: usize = 32;
const CSRF_TOKEN_BYTES: usize = 32;
const DEPLOY_TOKEN_BYTES: usize = 32;

/// A newly issued plaintext bootstrap token shown once to the operator.
#[derive(Debug)]
pub struct IssuedBootstrapToken {
    pub token: String,
    pub expires_at: OffsetDateTime,
}

/// Plaintext credentials returned once for delivery to the authenticated browser.
#[derive(Debug)]
pub struct IssuedSession {
    pub token: String,
    pub csrf_token: String,
    pub expires_at: OffsetDateTime,
    pub user: SessionUser,
}

/// A newly issued deploy credential whose plaintext is returned only once.
#[derive(Debug)]
pub struct IssuedDeployToken {
    pub token: String,
    pub metadata: DeployToken,
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
    #[error("email or password is incorrect")]
    InvalidCredentials,
    #[error("the session is invalid or expired")]
    InvalidSession,
    #[error("the CSRF token is invalid")]
    InvalidCsrf,
    #[error("the deploy token is invalid or revoked")]
    InvalidDeployToken,
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Issues a project-scoped deployment credential and stores only its digest.
pub async fn issue_deploy_token(
    store: &Store,
    project_id: &str,
    label: &str,
    actor_user_id: &str,
) -> Result<IssuedDeployToken, AuthError> {
    let token = format!(
        "dpt_{}",
        encode_hex(&rand::random::<[u8; DEPLOY_TOKEN_BYTES]>())
    );
    let now_unix = OffsetDateTime::now_utc().unix_timestamp();
    let metadata = store
        .create_deploy_token(NewDeployToken {
            project_id,
            label,
            token_hash: &hash_token(&token),
            created_by_user_id: actor_user_id,
            now_unix,
        })
        .await?;
    Ok(IssuedDeployToken { token, metadata })
}

/// Authenticates and records use of a project-scoped deployment credential.
pub async fn authenticate_deploy_token(
    store: &Store,
    token: &str,
) -> Result<DeployTokenIdentity, AuthError> {
    store
        .use_deploy_token(
            &hash_token(token),
            OffsetDateTime::now_utc().unix_timestamp(),
        )
        .await?
        .ok_or(AuthError::InvalidDeployToken)
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

/// Validates and hashes credentials before an administrator creates a local account.
pub async fn create_managed_user(
    store: &Store,
    actor_user_id: &str,
    email: &str,
    password: String,
    role: UserRole,
) -> Result<UserSummary, AuthError> {
    let email = normalize_email(email)?;
    if password.chars().count() < MINIMUM_PASSWORD_LENGTH {
        return Err(AuthError::WeakPassword);
    }
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|_| AuthError::PasswordHash)??;
    store
        .create_user(NewUser {
            email: &email,
            password_hash: &password_hash,
            role,
            actor_user_id,
            now_unix: OffsetDateTime::now_utc().unix_timestamp(),
        })
        .await
        .map_err(AuthError::from)
}

/// Verifies local credentials and creates an opaque, revocable server-side session.
pub async fn login(
    store: &Store,
    email: &str,
    password: String,
    lifetime: Duration,
) -> Result<IssuedSession, AuthError> {
    let email = normalize_email(email)?;
    let candidate = store.auth_user_by_email(&email).await?;
    let verification_hash = candidate.as_ref().map(|user| user.password_hash.clone());
    let password_matches = tokio::task::spawn_blocking(move || match verification_hash {
        Some(encoded) => verify_password(&password, &encoded),
        None => {
            let _ = hash_password(&password);
            false
        }
    })
    .await
    .map_err(|_| AuthError::PasswordHash)?;
    let user = candidate
        .filter(|_| password_matches)
        .ok_or(AuthError::InvalidCredentials)?;

    let now = OffsetDateTime::now_utc();
    let expires_at = now + lifetime;
    let token = encode_hex(&rand::random::<[u8; SESSION_TOKEN_BYTES]>());
    let csrf_token = encode_hex(&rand::random::<[u8; CSRF_TOKEN_BYTES]>());
    let session_id = uuid::Uuid::now_v7().to_string();
    store
        .create_session(NewSession {
            id: &session_id,
            user_id: &user.id,
            token_hash: &hash_token(&token),
            csrf_hash: &hash_token(&csrf_token),
            expires_at_unix: expires_at.unix_timestamp(),
            now_unix: now.unix_timestamp(),
        })
        .await?;
    Ok(IssuedSession {
        token,
        csrf_token,
        expires_at,
        user: SessionUser {
            id: user.id,
            email: user.email,
            role: user.role,
        },
    })
}

/// Resolves an opaque bearer token into a non-secret active user identity.
pub async fn authenticate_session(store: &Store, token: &str) -> Result<SessionUser, AuthError> {
    store
        .session_user(
            &hash_token(token),
            OffsetDateTime::now_utc().unix_timestamp(),
        )
        .await?
        .map(|(user, _)| user)
        .ok_or(AuthError::InvalidSession)
}

/// Validates CSRF possession against the digest bound to the active session.
pub async fn validate_csrf(
    store: &Store,
    token: &str,
    csrf_token: &str,
) -> Result<SessionUser, AuthError> {
    let (user, expected_hash) = store
        .session_user(
            &hash_token(token),
            OffsetDateTime::now_utc().unix_timestamp(),
        )
        .await?
        .ok_or(AuthError::InvalidSession)?;
    let presented_hash = hash_token(csrf_token);
    if expected_hash.len() != presented_hash.len()
        || !bool::from(expected_hash.ct_eq(presented_hash.as_slice()))
    {
        return Err(AuthError::InvalidCsrf);
    }
    Ok(user)
}

/// Revokes the active session after its bound CSRF credential has been verified.
pub async fn logout(store: &Store, token: &str, csrf_token: &str) -> Result<(), AuthError> {
    validate_csrf(store, token, csrf_token).await?;
    store
        .revoke_session(
            &hash_token(token),
            OffsetDateTime::now_utc().unix_timestamp(),
        )
        .await?;
    Ok(())
}

/// Verifies the current credential, hashes its replacement, and revokes all sessions.
pub async fn change_password(
    store: &Store,
    user: &SessionUser,
    current_password: String,
    new_password: String,
) -> Result<(), AuthError> {
    if new_password.chars().count() < MINIMUM_PASSWORD_LENGTH {
        return Err(AuthError::WeakPassword);
    }
    let candidate = store.auth_user_by_email(&user.email).await?;
    let verification_hash = candidate
        .as_ref()
        .filter(|candidate| candidate.id == user.id)
        .map(|candidate| candidate.password_hash.clone());
    let password_matches = tokio::task::spawn_blocking(move || match verification_hash {
        Some(encoded) => verify_password(&current_password, &encoded),
        None => {
            let _ = hash_password(&current_password);
            false
        }
    })
    .await
    .map_err(|_| AuthError::PasswordHash)?;
    if !password_matches {
        return Err(AuthError::InvalidCredentials);
    }
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&new_password))
        .await
        .map_err(|_| AuthError::PasswordHash)??;
    store
        .update_user_password(
            &user.id,
            &password_hash,
            Some(&user.id),
            "user.password_change",
            OffsetDateTime::now_utc().unix_timestamp(),
        )
        .await?;
    Ok(())
}

/// Replaces a selected local password from the trusted server console.
pub async fn recover_password(
    store: &Store,
    user_id: &str,
    new_password: String,
) -> Result<(), AuthError> {
    if new_password.chars().count() < MINIMUM_PASSWORD_LENGTH {
        return Err(AuthError::WeakPassword);
    }
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&new_password))
        .await
        .map_err(|_| AuthError::PasswordHash)??;
    store
        .update_user_password(
            user_id,
            &password_hash,
            None,
            "user.password_recovery",
            OffsetDateTime::now_utc().unix_timestamp(),
        )
        .await?;
    Ok(())
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

/// Verifies a candidate against a PHC string and treats malformed hashes as failure.
fn verify_password(password: &str, encoded: &str) -> bool {
    PasswordHash::new(encoded).ok().is_some_and(|hash| {
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
    })
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
    use argon2::{Argon2, PasswordVerifier, password_hash::phc::PasswordHash};

    use super::{AuthError, encode_hex, hash_password, normalize_email, verify_password};

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

    /// Ensures incorrect passwords and malformed stored hashes fail closed.
    #[test]
    fn password_verification_fails_closed() {
        let encoded = hash_password("a sufficiently long password").expect("password hashes");
        assert!(!verify_password("wrong password", &encoded));
        assert!(!verify_password(
            "a sufficiently long password",
            "not-a-phc-string"
        ));
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
