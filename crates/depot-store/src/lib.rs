//! SQLite persistence for Depot's control-plane state.

use std::{str::FromStr, time::Duration};

use sqlx::{
    Row, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow},
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
    /// Parses the database representation while rejecting unknown privilege values.
    pub fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "admin" => Ok(Self::Admin),
            "member" => Ok(Self::Member),
            _ => Err(StoreError::InvalidRole(value.to_owned())),
        }
    }
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

/// Authentication data loaded only by the trusted auth service.
#[derive(Debug)]
pub struct AuthUser {
    pub id: String,
    pub email: String,
    pub password_hash: String,
    pub role: UserRole,
}

/// Non-secret user identity attached to a valid server-side session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionUser {
    pub id: String,
    pub email: String,
    pub role: UserRole,
}

/// Digests and timestamps needed to persist a newly issued session.
#[derive(Debug)]
pub struct NewSession<'a> {
    pub id: &'a str,
    pub user_id: &'a str,
    pub token_hash: &'a [u8],
    pub csrf_hash: &'a [u8],
    pub expires_at_unix: i64,
    pub now_unix: i64,
}

/// Non-secret user fields returned by administration queries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserSummary {
    pub id: String,
    pub email: String,
    pub role: UserRole,
    pub active: bool,
    pub created_at: i64,
}

/// Validated local user data ready for durable insertion.
#[derive(Debug)]
pub struct NewUser<'a> {
    pub email: &'a str,
    pub password_hash: &'a str,
    pub role: UserRole,
    pub actor_user_id: &'a str,
    pub now_unix: i64,
}

/// Non-secret project fields shared by API and worker boundaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSummary {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub owner_user_id: String,
    pub repository_url: String,
    pub branch: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Validated data required to create a project.
#[derive(Debug)]
pub struct NewProject<'a> {
    pub slug: &'a str,
    pub name: &'a str,
    pub owner_user_id: &'a str,
    pub repository_url: &'a str,
    pub branch: &'a str,
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
    #[error("database contains an unsupported user role: {0}")]
    InvalidRole(String),
    #[error("a user with that email already exists")]
    EmailAlreadyExists,
    #[error("the last active administrator cannot be demoted or disabled")]
    LastActiveAdmin,
    #[error("the requested user does not exist")]
    UserNotFound,
    #[error("a project with that slug already exists")]
    ProjectSlugAlreadyExists,
    #[error("the requested project does not exist")]
    ProjectNotFound,
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

    /// Verifies the SQLite pool can execute a minimal query for readiness checks.
    pub async fn ping(&self) -> Result<(), StoreError> {
        sqlx::query_scalar::<_, i64>("SELECT 1")
            .fetch_one(&self.pool)
            .await?;
        Ok(())
    }

    /// Loads an active user by normalized email for password verification.
    pub async fn auth_user_by_email(&self, email: &str) -> Result<Option<AuthUser>, StoreError> {
        let row = sqlx::query(
            "SELECT id, email, password_hash, role FROM users WHERE email = ? AND active = 1",
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let role: String = row.try_get("role")?;
            Ok(AuthUser {
                id: row.try_get("id")?,
                email: row.try_get("email")?,
                password_hash: row.try_get("password_hash")?,
                role: UserRole::parse(&role)?,
            })
        })
        .transpose()
    }

    /// Persists a new opaque session after invalidating expired rows for its user.
    pub async fn create_session(&self, input: NewSession<'_>) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("DELETE FROM sessions WHERE user_id = ? AND expires_at < ?")
            .bind(input.user_id)
            .bind(input.now_unix)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO sessions
             (id, token_hash, user_id, csrf_hash, expires_at, created_at, last_seen_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(input.id)
        .bind(input.token_hash)
        .bind(input.user_id)
        .bind(input.csrf_hash)
        .bind(input.expires_at_unix)
        .bind(input.now_unix)
        .bind(input.now_unix)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Resolves a live session and returns its user plus the expected CSRF digest.
    pub async fn session_user(
        &self,
        token_hash: &[u8],
        now_unix: i64,
    ) -> Result<Option<(SessionUser, Vec<u8>)>, StoreError> {
        let row = sqlx::query(
            "SELECT users.id, users.email, users.role, sessions.csrf_hash
             FROM sessions
             JOIN users ON users.id = sessions.user_id
             WHERE sessions.token_hash = ? AND sessions.revoked_at IS NULL
               AND sessions.expires_at >= ? AND users.active = 1",
        )
        .bind(token_hash)
        .bind(now_unix)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let role: String = row.try_get("role")?;
            Ok((
                SessionUser {
                    id: row.try_get("id")?,
                    email: row.try_get("email")?,
                    role: UserRole::parse(&role)?,
                },
                row.try_get("csrf_hash")?,
            ))
        })
        .transpose()
    }

    /// Revokes one session by bearer-token digest and records its termination time.
    pub async fn revoke_session(&self, token_hash: &[u8], now_unix: i64) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE sessions SET revoked_at = ?
             WHERE token_hash = ? AND revoked_at IS NULL",
        )
        .bind(now_unix)
        .bind(token_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Lists users in stable creation order without exposing credential fields.
    pub async fn list_users(&self) -> Result<Vec<UserSummary>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, email, role, active, created_at FROM users ORDER BY created_at, email",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let role: String = row.try_get("role")?;
                Ok(UserSummary {
                    id: row.try_get("id")?,
                    email: row.try_get("email")?,
                    role: UserRole::parse(&role)?,
                    active: row.try_get::<i64, _>("active")? != 0,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect()
    }

    /// Creates a local account and its audit event in one transaction.
    pub async fn create_user(&self, input: NewUser<'_>) -> Result<UserSummary, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE email = ?")
            .bind(input.email)
            .fetch_one(&mut *transaction)
            .await?;
        if exists > 0 {
            return Err(StoreError::EmailAlreadyExists);
        }
        let id = Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, active, created_at, updated_at)
             VALUES (?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(&id)
        .bind(input.email)
        .bind(input.password_hash)
        .bind(input.role.as_str())
        .bind(input.now_unix)
        .bind(input.now_unix)
        .execute(&mut *transaction)
        .await?;
        insert_user_audit(
            &mut transaction,
            input.actor_user_id,
            "user.create",
            &id,
            input.now_unix,
        )
        .await?;
        transaction.commit().await?;
        Ok(UserSummary {
            id,
            email: input.email.to_owned(),
            role: input.role,
            active: true,
            created_at: input.now_unix,
        })
    }

    /// Updates role and activation while preserving at least one active administrator.
    pub async fn update_user(
        &self,
        user_id: &str,
        role: UserRole,
        active: bool,
        actor_user_id: &str,
        now_unix: i64,
    ) -> Result<UserSummary, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query("SELECT email, role, active, created_at FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(StoreError::UserNotFound)?;
        let old_role = UserRole::parse(&row.try_get::<String, _>("role")?)?;
        let old_active = row.try_get::<i64, _>("active")? != 0;
        if old_role == UserRole::Admin && old_active && (role != UserRole::Admin || !active) {
            let admin_count = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM users WHERE role = 'admin' AND active = 1",
            )
            .fetch_one(&mut *transaction)
            .await?;
            if admin_count <= 1 {
                return Err(StoreError::LastActiveAdmin);
            }
        }
        sqlx::query("UPDATE users SET role = ?, active = ?, updated_at = ? WHERE id = ?")
            .bind(role.as_str())
            .bind(active)
            .bind(now_unix)
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;
        if !active {
            sqlx::query(
                "UPDATE sessions SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL",
            )
            .bind(now_unix)
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;
        }
        insert_user_audit(
            &mut transaction,
            actor_user_id,
            "user.update",
            user_id,
            now_unix,
        )
        .await?;
        transaction.commit().await?;
        Ok(UserSummary {
            id: user_id.to_owned(),
            email: row.try_get("email")?,
            role,
            active,
            created_at: row.try_get("created_at")?,
        })
    }

    /// Lists all projects in the shared v0.1 workspace by recent creation.
    pub async fn list_projects(&self) -> Result<Vec<ProjectSummary>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, slug, name, owner_user_id, repository_url, branch,
                    created_at, updated_at
             FROM projects ORDER BY created_at DESC, name",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(project_from_row).collect()
    }

    /// Loads one project by opaque identifier.
    pub async fn project_by_id(&self, project_id: &str) -> Result<ProjectSummary, StoreError> {
        let row = sqlx::query(
            "SELECT id, slug, name, owner_user_id, repository_url, branch,
                    created_at, updated_at
             FROM projects WHERE id = ?",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StoreError::ProjectNotFound)?;
        project_from_row(row)
    }

    /// Creates a project and an audit event in one transaction.
    pub async fn create_project(
        &self,
        input: NewProject<'_>,
    ) -> Result<ProjectSummary, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM projects WHERE slug = ?")
            .bind(input.slug)
            .fetch_one(&mut *transaction)
            .await?;
        if exists > 0 {
            return Err(StoreError::ProjectSlugAlreadyExists);
        }
        let id = Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO projects
             (id, slug, name, owner_user_id, repository_url, branch, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(input.slug)
        .bind(input.name)
        .bind(input.owner_user_id)
        .bind(input.repository_url)
        .bind(input.branch)
        .bind(input.now_unix)
        .bind(input.now_unix)
        .execute(&mut *transaction)
        .await?;
        insert_project_audit(
            &mut transaction,
            input.owner_user_id,
            "project.create",
            &id,
            input.now_unix,
        )
        .await?;
        transaction.commit().await?;
        Ok(ProjectSummary {
            id,
            slug: input.slug.to_owned(),
            name: input.name.to_owned(),
            owner_user_id: input.owner_user_id.to_owned(),
            repository_url: input.repository_url.to_owned(),
            branch: input.branch.to_owned(),
            created_at: input.now_unix,
            updated_at: input.now_unix,
        })
    }

    /// Updates mutable project metadata while retaining stable ownership and identity.
    pub async fn update_project(
        &self,
        project_id: &str,
        name: &str,
        repository_url: &str,
        branch: &str,
        actor_user_id: &str,
        now_unix: i64,
    ) -> Result<ProjectSummary, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE projects SET name = ?, repository_url = ?, branch = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(name)
        .bind(repository_url)
        .bind(branch)
        .bind(now_unix)
        .bind(project_id)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::ProjectNotFound);
        }
        insert_project_audit(
            &mut transaction,
            actor_user_id,
            "project.update",
            project_id,
            now_unix,
        )
        .await?;
        transaction.commit().await?;
        self.project_by_id(project_id).await
    }

    /// Deletes a project which has no retained deployment children and records the action.
    pub async fn delete_project(
        &self,
        project_id: &str,
        actor_user_id: &str,
        now_unix: i64,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query("DELETE FROM projects WHERE id = ?")
            .bind(project_id)
            .execute(&mut *transaction)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::ProjectNotFound);
        }
        insert_project_audit(
            &mut transaction,
            actor_user_id,
            "project.delete",
            project_id,
            now_unix,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
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

/// Appends a user-management audit event inside the caller's transaction.
async fn insert_user_audit(
    transaction: &mut Transaction<'_, Sqlite>,
    actor_user_id: &str,
    action: &str,
    target_user_id: &str,
    now_unix: i64,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO audit_log (actor_user_id, action, target_type, target_id, created_at)
         VALUES (?, ?, 'user', ?, ?)",
    )
    .bind(actor_user_id)
    .bind(action)
    .bind(target_user_id)
    .bind(now_unix)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Converts a selected SQLite project row into its domain representation.
fn project_from_row(row: SqliteRow) -> Result<ProjectSummary, StoreError> {
    Ok(ProjectSummary {
        id: row.try_get("id")?,
        slug: row.try_get("slug")?,
        name: row.try_get("name")?,
        owner_user_id: row.try_get("owner_user_id")?,
        repository_url: row.try_get("repository_url")?,
        branch: row.try_get("branch")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// Appends a project lifecycle audit event inside the caller's transaction.
async fn insert_project_audit(
    transaction: &mut Transaction<'_, Sqlite>,
    actor_user_id: &str,
    action: &str,
    project_id: &str,
    now_unix: i64,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO audit_log (actor_user_id, action, target_type, target_id, created_at)
         VALUES (?, ?, 'project', ?, ?)",
    )
    .bind(actor_user_id)
    .bind(action)
    .bind(project_id)
    .bind(now_unix)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{BootstrapAdmin, NewUser, Store, StoreError, UserRole};

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

    /// Proves an instance can never lose its final active administrator.
    #[tokio::test]
    async fn last_active_admin_cannot_be_disabled_or_demoted() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let token_hash = [1_u8; 32];
        store
            .replace_bootstrap_token(&token_hash, 200, 100)
            .await
            .expect("token is created");
        let admin_id = store
            .create_first_admin(BootstrapAdmin {
                email: "admin@example.com",
                password_hash: "password hash",
                token_hash: &token_hash,
                now_unix: 150,
            })
            .await
            .expect("admin is created");
        let error = store
            .update_user(&admin_id, UserRole::Member, true, &admin_id, 160)
            .await
            .expect_err("last admin is protected");
        assert!(matches!(error, StoreError::LastActiveAdmin));

        store
            .create_user(NewUser {
                email: "second@example.com",
                password_hash: "password hash",
                role: UserRole::Admin,
                actor_user_id: &admin_id,
                now_unix: 170,
            })
            .await
            .expect("second admin is created");
        let updated = store
            .update_user(&admin_id, UserRole::Member, true, &admin_id, 180)
            .await
            .expect("admin can be demoted when another remains");
        assert_eq!(updated.role, UserRole::Member);
    }
}
