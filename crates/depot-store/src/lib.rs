//! SQLite persistence for Depot's control-plane state.

use std::{str::FromStr, time::Duration};

use sqlx::{
    Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use thiserror::Error;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// A user's workspace-wide role in Depot v0.1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserRole {
    Admin,
    Member,
}

impl UserRole {
    /// Returns the stable database representation of this role.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

/// Data required to atomically create the first administrator.
#[derive(Debug)]
pub struct BootstrapAdmin<'a> {
    pub email: &'a str,
    pub password_hash: &'a str,
    pub token_hash: &'a [u8],
    pub now_unix: i64,
}

/// Durable access to Depot's SQLite database.
#[derive(Clone, Debug)]
pub struct Store {
    pool: SqlitePool,
}

/// Persistence failures with stable distinctions for API error mapping.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database URL is invalid: {0}")]
    InvalidUrl(#[source] sqlx::Error),
    #[error("database operation failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error("database migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("the instance already has an administrator")]
    AlreadyInitialized,
    #[error("the bootstrap token is invalid or expired")]
    InvalidBootstrapToken,
}

impl Store {
    /// Opens SQLite with WAL, foreign keys, and a bounded contention timeout.
    pub async fn connect(database_url: &str) -> Result<Self, StoreError> {
        let options = SqliteConnectOptions::from_str(database_url)
            .map_err(StoreError::InvalidUrl)?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    /// Opens an isolated single-connection database for integration tests.
    pub async fn connect_in_memory() -> Result<Self, StoreError> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")?
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Memory);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    /// Reports whether initial setup has committed an active administrator.
    pub async fn is_initialized(&self) -> Result<bool, StoreError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM users WHERE role = 'admin' AND active = 1",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(count > 0)
    }

    /// Replaces any unused token with one newly issued by an explicit setup command.
    pub async fn replace_bootstrap_token(
        &self,
        token_hash: &[u8],
        expires_at_unix: i64,
        now_unix: i64,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;
        ensure_uninitialized(&mut transaction).await?;
        sqlx::query("DELETE FROM bootstrap_tokens WHERE consumed_at IS NULL")
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO bootstrap_tokens (token_hash, expires_at, created_at) VALUES (?, ?, ?)",
        )
        .bind(token_hash)
        .bind(expires_at_unix)
        .bind(now_unix)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Consumes a valid setup token and creates the first administrator atomically.
    pub async fn create_first_admin(
        &self,
        input: BootstrapAdmin<'_>,
    ) -> Result<String, StoreError> {
        let mut transaction = self.pool.begin().await?;
        ensure_uninitialized(&mut transaction).await?;

        let token_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM bootstrap_tokens
             WHERE token_hash = ? AND consumed_at IS NULL AND expires_at >= ?",
        )
        .bind(input.token_hash)
        .bind(input.now_unix)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(StoreError::InvalidBootstrapToken)?;

        let user_id = Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, active, created_at, updated_at)
             VALUES (?, ?, ?, 'admin', 1, ?, ?)",
        )
        .bind(&user_id)
        .bind(input.email)
        .bind(input.password_hash)
        .bind(input.now_unix)
        .bind(input.now_unix)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("UPDATE bootstrap_tokens SET consumed_at = ? WHERE id = ?")
            .bind(input.now_unix)
            .bind(token_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO audit_log (actor_user_id, action, target_type, target_id, created_at)
             VALUES (?, 'instance.bootstrap', 'user', ?, ?)",
        )
        .bind(&user_id)
        .bind(&user_id)
        .bind(input.now_unix)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(user_id)
    }
}

/// Rejects setup operations once any administrator exists.
async fn ensure_uninitialized(transaction: &mut Transaction<'_, Sqlite>) -> Result<(), StoreError> {
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM users WHERE role = 'admin' AND active = 1",
    )
    .fetch_one(&mut **transaction)
    .await?;
    if count > 0 {
        return Err(StoreError::AlreadyInitialized);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{BootstrapAdmin, Store, StoreError};

    /// Proves a setup token can initialize an instance exactly once.
    #[tokio::test]
    async fn bootstrap_is_single_use_and_closes_setup() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let token_hash = [1_u8; 32];
        store
            .replace_bootstrap_token(&token_hash, 200, 100)
            .await
            .expect("token is created");
        let user_id = store
            .create_first_admin(BootstrapAdmin {
                email: "admin@example.com",
                password_hash: "password hash",
                token_hash: &token_hash,
                now_unix: 150,
            })
            .await
            .expect("admin is created");
        assert!(!user_id.is_empty());

        let error = store
            .replace_bootstrap_token(&[2_u8; 32], 300, 200)
            .await
            .expect_err("initialized instances reject setup");
        assert!(matches!(error, StoreError::AlreadyInitialized));
    }

    /// Proves expired setup credentials cannot create an administrator.
    #[tokio::test]
    async fn expired_bootstrap_token_is_rejected() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let token_hash = [1_u8; 32];
        store
            .replace_bootstrap_token(&token_hash, 100, 50)
            .await
            .expect("token is created");
        let error = store
            .create_first_admin(BootstrapAdmin {
                email: "admin@example.com",
                password_hash: "password hash",
                token_hash: &token_hash,
                now_unix: 101,
            })
            .await
            .expect_err("expired token is rejected");
        assert!(matches!(error, StoreError::InvalidBootstrapToken));
    }
}
