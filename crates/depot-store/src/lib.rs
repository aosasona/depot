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
    pub latest_inspection_job_id: Option<String>,
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

/// Non-secret build and runtime settings attached one-to-one to a project.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceConfig {
    pub project_id: String,
    pub project_kind: String,
    pub builder: String,
    pub context_path: String,
    pub dockerfile_path: String,
    pub static_output_dir: Option<String>,
    pub exposed_port: Option<i64>,
    pub healthcheck_path: Option<String>,
    pub updated_at: i64,
}

/// Validated mutable build and runtime settings ready for persistence.
#[derive(Debug)]
pub struct UpdateServiceConfig<'a> {
    pub project_kind: &'a str,
    pub builder: &'a str,
    pub context_path: &'a str,
    pub dockerfile_path: &'a str,
    pub static_output_dir: Option<&'a str>,
    pub exposed_port: Option<i64>,
    pub healthcheck_path: Option<&'a str>,
    pub actor_user_id: &'a str,
    pub now_unix: i64,
}

/// A durable unit of background work claimed by one worker lease at a time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub project_id: Option<String>,
    pub payload_json: String,
    pub attempts: i64,
    pub max_attempts: i64,
}

/// Validated durable job data supplied by an API or reconciliation producer.
#[derive(Debug)]
pub struct NewJob<'a> {
    pub kind: &'a str,
    pub project_id: Option<&'a str>,
    pub payload_json: &'a str,
    pub deduplication_key: &'a str,
    pub max_attempts: i64,
    pub now_unix: i64,
}

/// Safe job state and result fields exposed to authenticated API consumers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobStatus {
    pub id: String,
    pub kind: String,
    pub project_id: Option<String>,
    pub state: String,
    pub attempts: i64,
    pub max_attempts: i64,
    pub result_json: Option<String>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One ordered, sanitized event emitted during durable job processing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobEvent {
    pub sequence: i64,
    pub level: String,
    pub message: String,
    pub created_at: i64,
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
    #[error("the requested job does not exist or its lease is no longer owned")]
    JobLeaseLost,
    #[error("the requested job does not exist")]
    JobNotFound,
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
            "SELECT projects.id, slug, name, owner_user_id, repository_url, branch,
                    projects.created_at, projects.updated_at,
                    (SELECT jobs.id FROM jobs WHERE jobs.project_id = projects.id
                     AND jobs.kind = 'inspect_repository'
                     ORDER BY jobs.created_at DESC LIMIT 1) AS latest_inspection_job_id
             FROM projects ORDER BY projects.created_at DESC, name",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(project_from_row).collect()
    }

    /// Loads one project by opaque identifier.
    pub async fn project_by_id(&self, project_id: &str) -> Result<ProjectSummary, StoreError> {
        let row = sqlx::query(
            "SELECT projects.id, slug, name, owner_user_id, repository_url, branch,
                    projects.created_at, projects.updated_at,
                    (SELECT jobs.id FROM jobs WHERE jobs.project_id = projects.id
                     AND jobs.kind = 'inspect_repository'
                     ORDER BY jobs.created_at DESC LIMIT 1) AS latest_inspection_job_id
             FROM projects WHERE projects.id = ?",
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
        sqlx::query(
            "INSERT INTO service_configs
             (project_id, created_at, updated_at) VALUES (?, ?, ?)",
        )
        .bind(&id)
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
            latest_inspection_job_id: None,
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

    /// Loads the build and runtime configuration for an existing project.
    pub async fn service_config(&self, project_id: &str) -> Result<ServiceConfig, StoreError> {
        let row = sqlx::query(
            "SELECT project_id, project_kind, builder, context_path, dockerfile_path,
                    static_output_dir, exposed_port, healthcheck_path, updated_at
             FROM service_configs WHERE project_id = ?",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StoreError::ProjectNotFound)?;
        service_config_from_row(row)
    }

    /// Replaces a project's validated service configuration and records its actor.
    pub async fn update_service_config(
        &self,
        project_id: &str,
        input: UpdateServiceConfig<'_>,
    ) -> Result<ServiceConfig, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE service_configs
             SET project_kind = ?, builder = ?, context_path = ?, dockerfile_path = ?,
                 static_output_dir = ?, exposed_port = ?, healthcheck_path = ?, updated_at = ?
             WHERE project_id = ?",
        )
        .bind(input.project_kind)
        .bind(input.builder)
        .bind(input.context_path)
        .bind(input.dockerfile_path)
        .bind(input.static_output_dir)
        .bind(input.exposed_port)
        .bind(input.healthcheck_path)
        .bind(input.now_unix)
        .bind(project_id)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::ProjectNotFound);
        }
        insert_project_audit(
            &mut transaction,
            input.actor_user_id,
            "project.config.update",
            project_id,
            input.now_unix,
        )
        .await?;
        transaction.commit().await?;
        self.service_config(project_id).await
    }

    /// Enqueues work once per active deduplication key and returns the durable job.
    pub async fn enqueue_job(&self, input: NewJob<'_>) -> Result<Job, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let candidate_id = Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT OR IGNORE INTO jobs
             (id, kind, project_id, payload_json, state, deduplication_key,
              attempts, max_attempts, available_at, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'queued', ?, 0, ?, ?, ?, ?)",
        )
        .bind(&candidate_id)
        .bind(input.kind)
        .bind(input.project_id)
        .bind(input.payload_json)
        .bind(input.deduplication_key)
        .bind(input.max_attempts)
        .bind(input.now_unix)
        .bind(input.now_unix)
        .bind(input.now_unix)
        .execute(&mut *transaction)
        .await?;
        let row = sqlx::query(
            "SELECT id, kind, project_id, payload_json, attempts, max_attempts
             FROM jobs WHERE deduplication_key = ? AND state IN ('queued', 'running')",
        )
        .bind(input.deduplication_key)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        job_from_row(row)
    }

    /// Claims the oldest eligible job, including work whose previous lease expired.
    pub async fn claim_job(
        &self,
        worker_id: &str,
        now_unix: i64,
        lease_seconds: i64,
    ) -> Result<Option<Job>, StoreError> {
        let row = sqlx::query(
            "UPDATE jobs
             SET state = 'running', lease_owner = ?, lease_expires_at = ?,
                 attempts = attempts + 1, updated_at = ?
             WHERE id = (
                 SELECT id FROM jobs
                 WHERE attempts < max_attempts
                   AND ((state = 'queued' AND available_at <= ?)
                        OR (state = 'running' AND lease_expires_at < ?))
                 ORDER BY created_at, id LIMIT 1
             )
             RETURNING id, kind, project_id, payload_json, attempts, max_attempts",
        )
        .bind(worker_id)
        .bind(now_unix + lease_seconds)
        .bind(now_unix)
        .bind(now_unix)
        .bind(now_unix)
        .fetch_optional(&self.pool)
        .await?;
        row.map(job_from_row).transpose()
    }

    /// Marks a leased job successful only when the caller still owns its lease.
    pub async fn complete_job(
        &self,
        job_id: &str,
        worker_id: &str,
        result_json: Option<&str>,
        now_unix: i64,
    ) -> Result<(), StoreError> {
        let result = sqlx::query(
            "UPDATE jobs SET state = 'succeeded', lease_owner = NULL,
                    lease_expires_at = NULL, result_json = ?, updated_at = ?
             WHERE id = ? AND state = 'running' AND lease_owner = ?",
        )
        .bind(result_json)
        .bind(now_unix)
        .bind(job_id)
        .bind(worker_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::JobLeaseLost);
        }
        Ok(())
    }

    /// Loads job status and result without exposing internal payload values.
    pub async fn job_status(&self, job_id: &str) -> Result<JobStatus, StoreError> {
        let row = sqlx::query(
            "SELECT id, kind, project_id, state, attempts, max_attempts,
                    result_json, last_error, created_at, updated_at
             FROM jobs WHERE id = ?",
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StoreError::JobNotFound)?;
        Ok(JobStatus {
            id: row.try_get("id")?,
            kind: row.try_get("kind")?,
            project_id: row.try_get("project_id")?,
            state: row.try_get("state")?,
            attempts: row.try_get("attempts")?,
            max_attempts: row.try_get("max_attempts")?,
            result_json: row.try_get("result_json")?,
            last_error: row.try_get("last_error")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }

    /// Lists ordered sanitized events for a durable job.
    pub async fn job_events(&self, job_id: &str) -> Result<Vec<JobEvent>, StoreError> {
        let rows = sqlx::query(
            "SELECT sequence, level, message, created_at
             FROM job_events WHERE job_id = ? ORDER BY sequence",
        )
        .bind(job_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(JobEvent {
                    sequence: row.try_get("sequence")?,
                    level: row.try_get("level")?,
                    message: row.try_get("message")?,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect()
    }

    /// Records a failure and either requeues with delay or exhausts retry attempts.
    pub async fn fail_job(
        &self,
        job_id: &str,
        worker_id: &str,
        error: &str,
        retry_delay_seconds: i64,
        now_unix: i64,
    ) -> Result<(), StoreError> {
        let result = sqlx::query(
            "UPDATE jobs
             SET state = CASE WHEN attempts < max_attempts THEN 'queued' ELSE 'failed' END,
                 available_at = CASE WHEN attempts < max_attempts THEN ? ELSE available_at END,
                 lease_owner = NULL, lease_expires_at = NULL, last_error = ?, updated_at = ?
             WHERE id = ? AND state = 'running' AND lease_owner = ?",
        )
        .bind(now_unix + retry_delay_seconds)
        .bind(error)
        .bind(now_unix)
        .bind(job_id)
        .bind(worker_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::JobLeaseLost);
        }
        Ok(())
    }

    /// Appends a monotonically ordered, bounded worker event for log streaming.
    pub async fn append_job_event(
        &self,
        job_id: &str,
        level: &str,
        message: &str,
        now_unix: i64,
    ) -> Result<i64, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let sequence = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM job_events WHERE job_id = ?",
        )
        .bind(job_id)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO job_events (job_id, sequence, level, message, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(job_id)
        .bind(sequence)
        .bind(level)
        .bind(message)
        .bind(now_unix)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(sequence)
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
        latest_inspection_job_id: row.try_get("latest_inspection_job_id")?,
    })
}

/// Maps one service configuration row into its domain representation.
fn service_config_from_row(row: SqliteRow) -> Result<ServiceConfig, StoreError> {
    Ok(ServiceConfig {
        project_id: row.try_get("project_id")?,
        project_kind: row.try_get("project_kind")?,
        builder: row.try_get("builder")?,
        context_path: row.try_get("context_path")?,
        dockerfile_path: row.try_get("dockerfile_path")?,
        static_output_dir: row.try_get("static_output_dir")?,
        exposed_port: row.try_get("exposed_port")?,
        healthcheck_path: row.try_get("healthcheck_path")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// Converts a selected SQLite job row into its lease-safe domain representation.
fn job_from_row(row: SqliteRow) -> Result<Job, StoreError> {
    Ok(Job {
        id: row.try_get("id")?,
        kind: row.try_get("kind")?,
        project_id: row.try_get("project_id")?,
        payload_json: row.try_get("payload_json")?,
        attempts: row.try_get("attempts")?,
        max_attempts: row.try_get("max_attempts")?,
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
    use super::{
        BootstrapAdmin, NewJob, NewProject, NewUser, Store, StoreError, UpdateServiceConfig,
        UserRole,
    };

    /// Creates one administrator in a fresh store for project-focused tests.
    async fn create_test_admin(store: &Store) -> String {
        let token_hash = [7_u8; 32];
        store
            .replace_bootstrap_token(&token_hash, 200, 100)
            .await
            .expect("token is created");
        store
            .create_first_admin(BootstrapAdmin {
                email: "admin@example.com",
                password_hash: "password hash",
                token_hash: &token_hash,
                now_unix: 150,
            })
            .await
            .expect("admin is created")
    }

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

    /// Proves every project receives defaults and configuration updates round-trip.
    #[tokio::test]
    async fn project_service_config_is_created_and_updated() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let admin_id = create_test_admin(&store).await;
        let project = store
            .create_project(NewProject {
                slug: "example",
                name: "Example",
                owner_user_id: &admin_id,
                repository_url: "https://github.com/example/project.git",
                branch: "main",
                now_unix: 200,
            })
            .await
            .expect("project is created");
        let defaults = store
            .service_config(&project.id)
            .await
            .expect("defaults load");
        assert_eq!(defaults.project_kind, "auto");
        assert_eq!(defaults.builder, "auto");
        assert_eq!(defaults.context_path, ".");

        let updated = store
            .update_service_config(
                &project.id,
                UpdateServiceConfig {
                    project_kind: "service",
                    builder: "dockerfile",
                    context_path: "apps/web",
                    dockerfile_path: "deploy/Dockerfile",
                    static_output_dir: None,
                    exposed_port: Some(8080),
                    healthcheck_path: Some("/health"),
                    actor_user_id: &admin_id,
                    now_unix: 210,
                },
            )
            .await
            .expect("configuration updates");
        assert_eq!(updated.exposed_port, Some(8080));
        assert_eq!(updated.healthcheck_path.as_deref(), Some("/health"));
        assert_eq!(updated.updated_at, 210);
    }

    /// Proves active deduplication and expired-lease recovery survive worker restarts.
    #[tokio::test]
    async fn jobs_deduplicate_and_recover_expired_leases() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let input = NewJob {
            kind: "inspect_repository",
            project_id: None,
            payload_json: "{}",
            deduplication_key: "inspect:project:commit",
            max_attempts: 3,
            now_unix: 100,
        };
        let first = store.enqueue_job(input).await.expect("job enqueues");
        let duplicate = store
            .enqueue_job(NewJob {
                kind: "inspect_repository",
                project_id: None,
                payload_json: "{}",
                deduplication_key: "inspect:project:commit",
                max_attempts: 3,
                now_unix: 101,
            })
            .await
            .expect("duplicate resolves");
        assert_eq!(first.id, duplicate.id);

        let claimed = store
            .claim_job("worker-a", 110, 30)
            .await
            .expect("claim succeeds")
            .expect("job is available");
        assert_eq!(claimed.attempts, 1);
        assert!(
            store
                .claim_job("worker-b", 120, 30)
                .await
                .expect("claim query succeeds")
                .is_none()
        );
        let recovered = store
            .claim_job("worker-b", 141, 30)
            .await
            .expect("recovery succeeds")
            .expect("expired job is available");
        assert_eq!(recovered.id, first.id);
        assert_eq!(recovered.attempts, 2);
        assert!(matches!(
            store.complete_job(&first.id, "worker-a", None, 142).await,
            Err(StoreError::JobLeaseLost)
        ));
        store
            .complete_job(&first.id, "worker-b", Some("{}"), 143)
            .await
            .expect("current lease owner completes");
    }

    /// Proves retries are delayed and stop once the configured attempt budget is spent.
    #[tokio::test]
    async fn failed_jobs_respect_backoff_and_attempt_limit() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let job = store
            .enqueue_job(NewJob {
                kind: "inspect_repository",
                project_id: None,
                payload_json: "{}",
                deduplication_key: "retry:test",
                max_attempts: 2,
                now_unix: 100,
            })
            .await
            .expect("job enqueues");
        store
            .claim_job("worker", 100, 30)
            .await
            .expect("claim succeeds");
        store
            .fail_job(&job.id, "worker", "temporary", 10, 101)
            .await
            .expect("failure records");
        assert!(
            store
                .claim_job("worker", 105, 30)
                .await
                .expect("claim query succeeds")
                .is_none()
        );
        store
            .claim_job("worker", 111, 30)
            .await
            .expect("retry claim succeeds")
            .expect("retry is available");
        store
            .fail_job(&job.id, "worker", "permanent", 10, 112)
            .await
            .expect("final failure records");
        assert!(
            store
                .claim_job("worker", 200, 30)
                .await
                .expect("claim query succeeds")
                .is_none()
        );
    }
}
