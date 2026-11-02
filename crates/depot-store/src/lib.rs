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

/// Immutable source identity and current lifecycle state for one deployment attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Deployment {
    pub id: String,
    pub project_id: String,
    pub job_id: String,
    pub commit_sha: String,
    pub trigger_kind: String,
    pub status: String,
    pub failure_reason: Option<String>,
    pub image_id: Option<String>,
    pub container_id: Option<String>,
    pub host_port: Option<i64>,
    pub static_release_path: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
    pub active: bool,
}

/// Validated values used to atomically create a manual deployment and build job.
#[derive(Debug)]
pub struct NewDeployment<'a> {
    pub project_id: &'a str,
    pub commit_sha: &'a str,
    pub created_by_user_id: &'a str,
    pub trigger_kind: &'a str,
    pub now_unix: i64,
}

/// Non-secret metadata for a project-scoped deployment credential.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeployToken {
    pub id: String,
    pub project_id: String,
    pub label: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

/// Secret digest and metadata required to persist a new deploy token.
#[derive(Debug)]
pub struct NewDeployToken<'a> {
    pub project_id: &'a str,
    pub label: &'a str,
    pub token_hash: &'a [u8],
    pub created_by_user_id: &'a str,
    pub now_unix: i64,
}

/// Project identity resolved from a valid deploy token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeployTokenIdentity {
    pub token_id: String,
    pub project_id: String,
    pub created_by_user_id: String,
}

/// Masked metadata for one encrypted runtime variable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectVariable {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub is_secret: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Encrypted variable payload loaded only by the trusted worker boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptedProjectVariable {
    pub name: String,
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
}

/// Authenticated encrypted value ready for insertion or replacement.
#[derive(Debug)]
pub struct UpsertProjectVariable<'a> {
    pub project_id: &'a str,
    pub name: &'a str,
    pub ciphertext: &'a [u8],
    pub nonce: &'a [u8],
    pub is_secret: bool,
    pub actor_user_id: &'a str,
    pub now_unix: i64,
}

/// Encrypted GitHub webhook configuration loaded only for signature validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GithubWebhook {
    pub id: String,
    pub project_id: String,
    pub repository_full_name: String,
    pub secret_ciphertext: Vec<u8>,
    pub secret_nonce: Vec<u8>,
    pub created_by_user_id: String,
}

/// Values needed to create or rotate one project's GitHub webhook.
#[derive(Debug)]
pub struct UpsertGithubWebhook<'a> {
    pub project_id: &'a str,
    pub repository_full_name: &'a str,
    pub endpoint_token_hash: &'a [u8],
    pub secret_ciphertext: &'a [u8],
    pub secret_nonce: &'a [u8],
    pub actor_user_id: &'a str,
    pub now_unix: i64,
}

/// Non-secret metadata for a GitHub App installation linked to this Depot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GithubInstallation {
    pub installation_id: i64,
    pub account_login: String,
    pub account_type: String,
    pub repository_selection: String,
}

/// Encrypted singleton GitHub App credentials configured through the dashboard.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GithubAppConfiguration {
    pub app_id: i64,
    pub app_slug: String,
    pub private_key_ciphertext: Vec<u8>,
    pub private_key_nonce: Vec<u8>,
    pub webhook_secret_ciphertext: Vec<u8>,
    pub webhook_secret_nonce: Vec<u8>,
}

/// Values used to atomically create or rotate the dashboard-managed GitHub App.
#[derive(Debug)]
pub struct UpsertGithubAppConfiguration<'a> {
    pub app_id: i64,
    pub app_slug: &'a str,
    pub private_key_ciphertext: &'a [u8],
    pub private_key_nonce: &'a [u8],
    pub webhook_secret_ciphertext: &'a [u8],
    pub webhook_secret_nonce: &'a [u8],
    pub actor_user_id: &'a str,
    pub now_unix: i64,
}

/// Repository identity bound to a project without any persisted access token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GithubProjectRepository {
    pub project_id: String,
    pub installation_id: i64,
    pub repository_id: i64,
    pub repository_full_name: String,
    pub private: bool,
}

/// Project and actor resolved from an exact GitHub installation/repository pair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GithubAppWebhookTarget {
    pub project_id: String,
    pub branch: String,
    pub actor_user_id: String,
}

/// Encrypted SSH key and trusted-host data loaded only by the worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSshCredential {
    pub private_key_ciphertext: Vec<u8>,
    pub private_key_nonce: Vec<u8>,
    pub known_hosts: String,
}

/// Values needed to create or rotate one project's SSH deploy credential.
#[derive(Debug)]
pub struct UpsertProjectSshCredential<'a> {
    pub project_id: &'a str,
    pub private_key_ciphertext: &'a [u8],
    pub private_key_nonce: &'a [u8],
    pub known_hosts: &'a str,
    pub actor_user_id: &'a str,
    pub now_unix: i64,
}

/// One project hostname and its routing reconciliation state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Domain {
    pub id: String,
    pub project_id: String,
    pub hostname: String,
    pub routing_status: String,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Verified active release selected for an exact validated incoming hostname.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GatewayTarget {
    Service {
        deployment_id: String,
        host_port: u16,
    },
    Static {
        deployment_id: String,
        release_path: String,
    },
}

/// Runtime containers safe to retire after an atomic deployment transition.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeploymentCleanup {
    pub container_ids: Vec<String>,
    pub image_ids: Vec<String>,
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
    #[error("the requested deployment does not exist")]
    DeploymentNotFound,
    #[error("deployment runtime artifacts must be recorded before completion")]
    DeploymentRuntimeMissing,
    #[error("a domain with that hostname already exists")]
    DomainAlreadyExists,
    #[error("the requested domain does not exist")]
    DomainNotFound,
}

impl Store {
    /// Loads dashboard-managed GitHub App credentials without decrypting them.
    pub async fn github_app_configuration(
        &self,
    ) -> Result<Option<GithubAppConfiguration>, StoreError> {
        let row = sqlx::query(
            "SELECT app_id, app_slug, private_key_ciphertext, private_key_nonce,
                    webhook_secret_ciphertext, webhook_secret_nonce
             FROM github_app_configuration WHERE singleton = 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(GithubAppConfiguration {
                app_id: row.try_get("app_id")?,
                app_slug: row.try_get("app_slug")?,
                private_key_ciphertext: row.try_get("private_key_ciphertext")?,
                private_key_nonce: row.try_get("private_key_nonce")?,
                webhook_secret_ciphertext: row.try_get("webhook_secret_ciphertext")?,
                webhook_secret_nonce: row.try_get("webhook_secret_nonce")?,
            })
        })
        .transpose()
    }

    /// Atomically stores encrypted GitHub App material and its configuring actor.
    pub async fn upsert_github_app_configuration(
        &self,
        input: UpsertGithubAppConfiguration<'_>,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO github_app_configuration
             (singleton, app_id, app_slug, private_key_ciphertext, private_key_nonce,
              webhook_secret_ciphertext, webhook_secret_nonce,
              configured_by_user_id, created_at, updated_at)
             VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(singleton) DO UPDATE SET
               app_id = excluded.app_id,
               app_slug = excluded.app_slug,
               private_key_ciphertext = excluded.private_key_ciphertext,
               private_key_nonce = excluded.private_key_nonce,
               webhook_secret_ciphertext = excluded.webhook_secret_ciphertext,
               webhook_secret_nonce = excluded.webhook_secret_nonce,
               configured_by_user_id = excluded.configured_by_user_id,
               updated_at = excluded.updated_at",
        )
        .bind(input.app_id)
        .bind(input.app_slug)
        .bind(input.private_key_ciphertext)
        .bind(input.private_key_nonce)
        .bind(input.webhook_secret_ciphertext)
        .bind(input.webhook_secret_nonce)
        .bind(input.actor_user_id)
        .bind(input.now_unix)
        .bind(input.now_unix)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Links a verified GitHub App installation while retaining its original actor.
    pub async fn upsert_github_installation(
        &self,
        installation: &GithubInstallation,
        actor_user_id: &str,
        now_unix: i64,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO github_installations
             (installation_id, account_login, account_type, repository_selection,
              linked_by_user_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(installation_id) DO UPDATE SET
               account_login = excluded.account_login,
               account_type = excluded.account_type,
               repository_selection = excluded.repository_selection,
               updated_at = excluded.updated_at",
        )
        .bind(installation.installation_id)
        .bind(&installation.account_login)
        .bind(&installation.account_type)
        .bind(&installation.repository_selection)
        .bind(actor_user_id)
        .bind(now_unix)
        .bind(now_unix)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Lists linked installations without exposing app credentials.
    pub async fn github_installations(&self) -> Result<Vec<GithubInstallation>, StoreError> {
        let rows = sqlx::query(
            "SELECT installation_id, account_login, account_type, repository_selection
             FROM github_installations ORDER BY account_login, installation_id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(GithubInstallation {
                    installation_id: row.try_get("installation_id")?,
                    account_login: row.try_get("account_login")?,
                    account_type: row.try_get("account_type")?,
                    repository_selection: row.try_get("repository_selection")?,
                })
            })
            .collect()
    }

    /// Returns whether an installation was explicitly linked by an authenticated user.
    pub async fn has_github_installation(&self, installation_id: i64) -> Result<bool, StoreError> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM github_installations WHERE installation_id = ?",
        )
        .bind(installation_id)
        .fetch_one(&self.pool)
        .await?
            > 0)
    }

    /// Creates a project and binds its verified GitHub repository in one transaction.
    pub async fn create_github_project(
        &self,
        input: NewProject<'_>,
        repository: &GithubProjectRepository,
    ) -> Result<ProjectSummary, StoreError> {
        self.create_github_project_with_domain(input, repository, None)
            .await
    }

    /// Creates a GitHub project and optional generated hostname atomically.
    pub async fn create_github_project_with_domain(
        &self,
        input: NewProject<'_>,
        repository: &GithubProjectRepository,
        default_hostname: Option<&str>,
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
            "INSERT INTO service_configs (project_id, created_at, updated_at) VALUES (?, ?, ?)",
        )
        .bind(&id)
        .bind(input.now_unix)
        .bind(input.now_unix)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO github_project_repositories
             (project_id, installation_id, repository_id, repository_full_name,
              private, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(repository.installation_id)
        .bind(repository.repository_id)
        .bind(&repository.repository_full_name)
        .bind(repository.private)
        .bind(input.now_unix)
        .bind(input.now_unix)
        .execute(&mut *transaction)
        .await?;
        if let Some(hostname) = default_hostname {
            insert_project_domain(
                &mut transaction,
                &id,
                hostname,
                input.owner_user_id,
                input.now_unix,
            )
            .await?;
        }
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

    /// Loads the GitHub repository identity used for short-lived worker authentication.
    pub async fn github_repository_for_project(
        &self,
        project_id: &str,
    ) -> Result<Option<GithubProjectRepository>, StoreError> {
        let row = sqlx::query(
            "SELECT project_id, installation_id, repository_id, repository_full_name, private
             FROM github_project_repositories WHERE project_id = ?",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(GithubProjectRepository {
                project_id: row.try_get("project_id")?,
                installation_id: row.try_get("installation_id")?,
                repository_id: row.try_get("repository_id")?,
                repository_full_name: row.try_get("repository_full_name")?,
                private: row.try_get("private")?,
            })
        })
        .transpose()
    }

    /// Resolves a global GitHub App delivery to exactly one imported project.
    pub async fn github_app_webhook_target(
        &self,
        installation_id: i64,
        repository_id: i64,
    ) -> Result<Option<GithubAppWebhookTarget>, StoreError> {
        let row = sqlx::query(
            "SELECT projects.id AS project_id, projects.branch,
                    github_installations.linked_by_user_id AS actor_user_id
             FROM github_project_repositories
             JOIN projects ON projects.id = github_project_repositories.project_id
             JOIN github_installations ON github_installations.installation_id = github_project_repositories.installation_id
             WHERE github_project_repositories.installation_id = ?
               AND github_project_repositories.repository_id = ?",
        )
        .bind(installation_id)
        .bind(repository_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(GithubAppWebhookTarget {
                project_id: row.try_get("project_id")?,
                branch: row.try_get("branch")?,
                actor_user_id: row.try_get("actor_user_id")?,
            })
        })
        .transpose()
    }

    /// Deduplicates one global App delivery and atomically enqueues its deployment.
    pub async fn enqueue_github_app_delivery(
        &self,
        target: &GithubAppWebhookTarget,
        installation_id: i64,
        repository_id: i64,
        delivery_id: &str,
        commit_sha: &str,
        now_unix: i64,
    ) -> Result<Option<Deployment>, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO github_app_deliveries
             (installation_id, repository_id, delivery_id, commit_sha, received_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(installation_id)
        .bind(repository_id)
        .bind(delivery_id)
        .bind(commit_sha)
        .bind(now_unix)
        .execute(&mut *transaction)
        .await?;
        if inserted.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(None);
        }
        let deployment_id = Uuid::now_v7().to_string();
        let job_id = Uuid::now_v7().to_string();
        let payload_json = format!(r#"{{"deployment_id":"{deployment_id}"}}"#);
        sqlx::query(
            "INSERT INTO jobs
             (id, kind, project_id, payload_json, state, deduplication_key,
              attempts, max_attempts, available_at, created_at, updated_at)
             VALUES (?, 'deploy_project', ?, ?, 'queued', ?, 0, 3, ?, ?, ?)",
        )
        .bind(&job_id)
        .bind(&target.project_id)
        .bind(payload_json)
        .bind(format!("deploy:{deployment_id}"))
        .bind(now_unix)
        .bind(now_unix)
        .bind(now_unix)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO deployments
             (id, project_id, job_id, commit_sha, trigger_kind, status,
              created_by_user_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'webhook', 'queued', ?, ?, ?)",
        )
        .bind(&deployment_id)
        .bind(&target.project_id)
        .bind(&job_id)
        .bind(commit_sha)
        .bind(&target.actor_user_id)
        .bind(now_unix)
        .bind(now_unix)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE github_app_deliveries SET deployment_id = ?
             WHERE installation_id = ? AND delivery_id = ?",
        )
        .bind(&deployment_id)
        .bind(installation_id)
        .bind(delivery_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        self.deployment_by_id(&deployment_id).await.map(Some)
    }
    /// Creates or rotates an encrypted SSH deploy key and its trusted hosts.
    pub async fn upsert_project_ssh_credential(
        &self,
        input: UpsertProjectSshCredential<'_>,
    ) -> Result<(), StoreError> {
        self.project_by_id(input.project_id).await?;
        sqlx::query(
            "INSERT INTO project_ssh_credentials
             (project_id, private_key_ciphertext, private_key_nonce, known_hosts,
              created_by_user_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(project_id) DO UPDATE SET
               private_key_ciphertext = excluded.private_key_ciphertext,
               private_key_nonce = excluded.private_key_nonce,
               known_hosts = excluded.known_hosts,
               created_by_user_id = excluded.created_by_user_id,
               updated_at = excluded.updated_at",
        )
        .bind(input.project_id)
        .bind(input.private_key_ciphertext)
        .bind(input.private_key_nonce)
        .bind(input.known_hosts)
        .bind(input.actor_user_id)
        .bind(input.now_unix)
        .bind(input.now_unix)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Loads one encrypted SSH credential without exposing it through the API.
    pub async fn project_ssh_credential(
        &self,
        project_id: &str,
    ) -> Result<Option<ProjectSshCredential>, StoreError> {
        let row = sqlx::query(
            "SELECT private_key_ciphertext, private_key_nonce, known_hosts
             FROM project_ssh_credentials WHERE project_id = ?",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(ProjectSshCredential {
                private_key_ciphertext: row.try_get("private_key_ciphertext")?,
                private_key_nonce: row.try_get("private_key_nonce")?,
                known_hosts: row.try_get("known_hosts")?,
            })
        })
        .transpose()
    }

    /// Creates or rotates one encrypted project webhook and invalidates its old URL.
    pub async fn upsert_github_webhook(
        &self,
        input: UpsertGithubWebhook<'_>,
    ) -> Result<String, StoreError> {
        self.project_by_id(input.project_id).await?;
        let id = Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO github_webhooks
             (id, project_id, endpoint_token_hash, secret_ciphertext, secret_nonce,
              repository_full_name, created_by_user_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(project_id) DO UPDATE SET
               endpoint_token_hash = excluded.endpoint_token_hash,
               secret_ciphertext = excluded.secret_ciphertext,
               secret_nonce = excluded.secret_nonce,
               repository_full_name = excluded.repository_full_name,
               created_by_user_id = excluded.created_by_user_id,
               updated_at = excluded.updated_at",
        )
        .bind(&id)
        .bind(input.project_id)
        .bind(input.endpoint_token_hash)
        .bind(input.secret_ciphertext)
        .bind(input.secret_nonce)
        .bind(input.repository_full_name)
        .bind(input.actor_user_id)
        .bind(input.now_unix)
        .bind(input.now_unix)
        .execute(&self.pool)
        .await?;
        Ok(
            sqlx::query_scalar::<_, String>("SELECT id FROM github_webhooks WHERE project_id = ?")
                .bind(input.project_id)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    /// Resolves an opaque webhook endpoint token to encrypted validation state.
    pub async fn github_webhook_by_token_hash(
        &self,
        token_hash: &[u8],
    ) -> Result<Option<GithubWebhook>, StoreError> {
        let row = sqlx::query(
            "SELECT github_webhooks.id, github_webhooks.project_id,
                    github_webhooks.repository_full_name,
                    github_webhooks.secret_ciphertext, github_webhooks.secret_nonce,
                    github_webhooks.created_by_user_id
             FROM github_webhooks
             JOIN users ON users.id = github_webhooks.created_by_user_id
             WHERE endpoint_token_hash = ? AND users.active = 1",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(GithubWebhook {
                id: row.try_get("id")?,
                project_id: row.try_get("project_id")?,
                repository_full_name: row.try_get("repository_full_name")?,
                secret_ciphertext: row.try_get("secret_ciphertext")?,
                secret_nonce: row.try_get("secret_nonce")?,
                created_by_user_id: row.try_get("created_by_user_id")?,
            })
        })
        .transpose()
    }

    /// Deduplicates one verified delivery and atomically creates its deployment job.
    pub async fn enqueue_github_delivery(
        &self,
        webhook: &GithubWebhook,
        delivery_id: &str,
        commit_sha: &str,
        now_unix: i64,
    ) -> Result<Option<Deployment>, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO github_webhook_deliveries
             (webhook_id, delivery_id, commit_sha, received_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(&webhook.id)
        .bind(delivery_id)
        .bind(commit_sha)
        .bind(now_unix)
        .execute(&mut *transaction)
        .await?;
        if inserted.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(None);
        }
        let deployment_id = Uuid::now_v7().to_string();
        let job_id = Uuid::now_v7().to_string();
        let payload_json = format!(r#"{{"deployment_id":"{deployment_id}"}}"#);
        sqlx::query(
            "INSERT INTO jobs
             (id, kind, project_id, payload_json, state, deduplication_key,
              attempts, max_attempts, available_at, created_at, updated_at)
             VALUES (?, 'deploy_project', ?, ?, 'queued', ?, 0, 3, ?, ?, ?)",
        )
        .bind(&job_id)
        .bind(&webhook.project_id)
        .bind(payload_json)
        .bind(format!("deploy:{deployment_id}"))
        .bind(now_unix)
        .bind(now_unix)
        .bind(now_unix)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO deployments
             (id, project_id, job_id, commit_sha, trigger_kind, status,
              created_by_user_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'webhook', 'queued', ?, ?, ?)",
        )
        .bind(&deployment_id)
        .bind(&webhook.project_id)
        .bind(&job_id)
        .bind(commit_sha)
        .bind(&webhook.created_by_user_id)
        .bind(now_unix)
        .bind(now_unix)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE github_webhook_deliveries SET deployment_id = ?
             WHERE webhook_id = ? AND delivery_id = ?",
        )
        .bind(&deployment_id)
        .bind(&webhook.id)
        .bind(delivery_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        self.deployment_by_id(&deployment_id).await.map(Some)
    }

    /// Inserts or replaces one encrypted runtime variable without exposing its value.
    pub async fn upsert_project_variable(
        &self,
        input: UpsertProjectVariable<'_>,
    ) -> Result<ProjectVariable, StoreError> {
        self.project_by_id(input.project_id).await?;
        let id = Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO project_variables
             (id, project_id, name, ciphertext, nonce, is_secret,
              created_by_user_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(project_id, name) DO UPDATE SET
               ciphertext = excluded.ciphertext,
               nonce = excluded.nonce,
               is_secret = excluded.is_secret,
               created_by_user_id = excluded.created_by_user_id,
               updated_at = excluded.updated_at",
        )
        .bind(&id)
        .bind(input.project_id)
        .bind(input.name)
        .bind(input.ciphertext)
        .bind(input.nonce)
        .bind(input.is_secret)
        .bind(input.actor_user_id)
        .bind(input.now_unix)
        .bind(input.now_unix)
        .execute(&self.pool)
        .await?;
        self.project_variable_by_name(input.project_id, input.name)
            .await
    }

    /// Lists masked runtime variable metadata for an authenticated project view.
    pub async fn list_project_variables(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProjectVariable>, StoreError> {
        self.project_by_id(project_id).await?;
        let rows = sqlx::query(
            "SELECT id, project_id, name, is_secret, created_at, updated_at
             FROM project_variables WHERE project_id = ? ORDER BY name",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(project_variable_from_row).collect()
    }

    /// Loads encrypted values only for the worker immediately before runtime creation.
    pub async fn encrypted_project_variables(
        &self,
        project_id: &str,
    ) -> Result<Vec<EncryptedProjectVariable>, StoreError> {
        self.project_by_id(project_id).await?;
        let rows = sqlx::query(
            "SELECT name, ciphertext, nonce FROM project_variables
             WHERE project_id = ? ORDER BY name",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(EncryptedProjectVariable {
                    name: row.try_get("name")?,
                    ciphertext: row.try_get("ciphertext")?,
                    nonce: row.try_get("nonce")?,
                })
            })
            .collect()
    }

    /// Deletes one runtime variable by project and opaque identifier.
    pub async fn delete_project_variable(
        &self,
        project_id: &str,
        variable_id: &str,
    ) -> Result<bool, StoreError> {
        self.project_by_id(project_id).await?;
        let result = sqlx::query("DELETE FROM project_variables WHERE id = ? AND project_id = ?")
            .bind(variable_id)
            .bind(project_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Loads one masked variable after an upsert.
    async fn project_variable_by_name(
        &self,
        project_id: &str,
        name: &str,
    ) -> Result<ProjectVariable, StoreError> {
        let row = sqlx::query(
            "SELECT id, project_id, name, is_secret, created_at, updated_at
             FROM project_variables WHERE project_id = ? AND name = ?",
        )
        .bind(project_id)
        .bind(name)
        .fetch_one(&self.pool)
        .await?;
        project_variable_from_row(row)
    }

    /// Persists a project deploy credential while retaining only its digest.
    pub async fn create_deploy_token(
        &self,
        input: NewDeployToken<'_>,
    ) -> Result<DeployToken, StoreError> {
        self.project_by_id(input.project_id).await?;
        let id = Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO project_deploy_tokens
             (id, project_id, label, token_hash, created_by_user_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(input.project_id)
        .bind(input.label)
        .bind(input.token_hash)
        .bind(input.created_by_user_id)
        .bind(input.now_unix)
        .execute(&self.pool)
        .await?;
        Ok(DeployToken {
            id,
            project_id: input.project_id.to_owned(),
            label: input.label.to_owned(),
            created_at: input.now_unix,
            last_used_at: None,
        })
    }

    /// Resolves and marks one active deploy credential as used.
    pub async fn use_deploy_token(
        &self,
        token_hash: &[u8],
        now_unix: i64,
    ) -> Result<Option<DeployTokenIdentity>, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT project_deploy_tokens.id, project_deploy_tokens.project_id,
                    project_deploy_tokens.created_by_user_id
             FROM project_deploy_tokens
             JOIN users ON users.id = project_deploy_tokens.created_by_user_id
             WHERE token_hash = ? AND revoked_at IS NULL AND users.active = 1",
        )
        .bind(token_hash)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(row) = row else {
            transaction.rollback().await?;
            return Ok(None);
        };
        let identity = DeployTokenIdentity {
            token_id: row.try_get("id")?,
            project_id: row.try_get("project_id")?,
            created_by_user_id: row.try_get("created_by_user_id")?,
        };
        sqlx::query("UPDATE project_deploy_tokens SET last_used_at = ? WHERE id = ?")
            .bind(now_unix)
            .bind(&identity.token_id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(Some(identity))
    }

    /// Revokes one deploy credential belonging to the selected project.
    pub async fn revoke_deploy_token(
        &self,
        project_id: &str,
        token_id: &str,
        now_unix: i64,
    ) -> Result<bool, StoreError> {
        let result = sqlx::query(
            "UPDATE project_deploy_tokens SET revoked_at = ?
             WHERE id = ? AND project_id = ? AND revoked_at IS NULL",
        )
        .bind(now_unix)
        .bind(token_id)
        .bind(project_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

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

    /// Replaces one active user's password and revokes every outstanding session atomically.
    pub async fn update_user_password(
        &self,
        user_id: &str,
        password_hash: &str,
        actor_user_id: Option<&str>,
        action: &str,
        now_unix: i64,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ? AND active = 1",
        )
        .bind(password_hash)
        .bind(now_unix)
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::UserNotFound);
        }
        sqlx::query("UPDATE sessions SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL")
            .bind(now_unix)
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;
        insert_user_audit(&mut transaction, actor_user_id, action, user_id, now_unix).await?;
        transaction.commit().await?;
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
            Some(input.actor_user_id),
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
            Some(actor_user_id),
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
        self.create_project_with_domain(input, None).await
    }

    /// Creates a project and optional generated hostname atomically.
    pub async fn create_project_with_domain(
        &self,
        input: NewProject<'_>,
        default_hostname: Option<&str>,
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
        if let Some(hostname) = default_hostname {
            insert_project_domain(
                &mut transaction,
                &id,
                hostname,
                input.owner_user_id,
                input.now_unix,
            )
            .await?;
        }
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

    /// Lists desired hostnames and reconciliation state for one project.
    pub async fn list_domains(&self, project_id: &str) -> Result<Vec<Domain>, StoreError> {
        self.project_by_id(project_id).await?;
        let rows = sqlx::query(
            "SELECT id, project_id, hostname, routing_status, last_error, created_at, updated_at
             FROM domains WHERE project_id = ? ORDER BY created_at, hostname",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(domain_from_row).collect()
    }

    /// Adds one globally unique project hostname and records the requesting actor.
    pub async fn create_domain(
        &self,
        project_id: &str,
        hostname: &str,
        actor_user_id: &str,
        now_unix: i64,
    ) -> Result<Domain, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let project_exists =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM projects WHERE id = ?")
                .bind(project_id)
                .fetch_one(&mut *transaction)
                .await?;
        if project_exists == 0 {
            return Err(StoreError::ProjectNotFound);
        }
        let exists = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM domains WHERE hostname = ? COLLATE NOCASE",
        )
        .bind(hostname)
        .fetch_one(&mut *transaction)
        .await?;
        if exists > 0 {
            return Err(StoreError::DomainAlreadyExists);
        }
        let id = Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO domains
             (id, project_id, hostname, routing_status, created_at, updated_at)
             VALUES (?, ?, ?, 'pending', ?, ?)",
        )
        .bind(&id)
        .bind(project_id)
        .bind(hostname)
        .bind(now_unix)
        .bind(now_unix)
        .execute(&mut *transaction)
        .await?;
        insert_project_audit(
            &mut transaction,
            actor_user_id,
            "domain.create",
            project_id,
            now_unix,
        )
        .await?;
        transaction.commit().await?;
        Ok(Domain {
            id,
            project_id: project_id.to_owned(),
            hostname: hostname.to_owned(),
            routing_status: "pending".to_owned(),
            last_error: None,
            created_at: now_unix,
            updated_at: now_unix,
        })
    }

    /// Removes one desired hostname and records the project-level audit event.
    pub async fn delete_domain(
        &self,
        domain_id: &str,
        actor_user_id: &str,
        now_unix: i64,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;
        let project_id =
            sqlx::query_scalar::<_, String>("SELECT project_id FROM domains WHERE id = ?")
                .bind(domain_id)
                .fetch_optional(&mut *transaction)
                .await?
                .ok_or(StoreError::DomainNotFound)?;
        sqlx::query("DELETE FROM domains WHERE id = ?")
            .bind(domain_id)
            .execute(&mut *transaction)
            .await?;
        insert_project_audit(
            &mut transaction,
            actor_user_id,
            "domain.delete",
            &project_id,
            now_unix,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Resolves a hostname only to the explicitly active verified project runtime.
    pub async fn gateway_target(
        &self,
        hostname: &str,
    ) -> Result<Option<GatewayTarget>, StoreError> {
        let row = sqlx::query(
            "SELECT deployments.id, deployments.host_port,
                    deployments.static_release_path
             FROM domains
             JOIN deployments ON deployments.project_id = domains.project_id
             WHERE domains.hostname = ? COLLATE NOCASE
               AND deployments.status = 'succeeded'
               AND deployments.active = 1
               AND (deployments.static_release_path IS NOT NULL
                    OR deployments.host_port IS NOT NULL)
             LIMIT 1",
        )
        .bind(hostname)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let deployment_id = row.try_get("id")?;
            if row
                .try_get::<Option<String>, _>("static_release_path")?
                .is_some()
            {
                Ok(GatewayTarget::Static {
                    deployment_id,
                    release_path: row.try_get("static_release_path")?,
                })
            } else {
                let port: i64 = row.try_get("host_port")?;
                let host_port = u16::try_from(port).map_err(|_| {
                    StoreError::Database(sqlx::Error::Protocol(
                        "stored deployment port is invalid".to_owned(),
                    ))
                })?;
                Ok(GatewayTarget::Service {
                    deployment_id,
                    host_port,
                })
            }
        })
        .transpose()
    }

    /// Enqueues an independent full-domain reconciliation snapshot.
    pub async fn enqueue_domain_reconciliation(&self, now_unix: i64) -> Result<Job, StoreError> {
        let deduplication_key = format!("reconcile:domains:{}", Uuid::now_v7());
        self.enqueue_job(NewJob {
            kind: "reconcile_domains",
            project_id: None,
            payload_json: "{}",
            deduplication_key: &deduplication_key,
            max_attempts: 5,
            now_unix,
        })
        .await
    }

    /// Lists the complete canonical desired hostname snapshot for the Caddy agent.
    pub async fn all_domain_hostnames(&self) -> Result<Vec<String>, StoreError> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT hostname FROM domains ORDER BY hostname COLLATE NOCASE",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Acknowledges that the agent accepted the current complete desired snapshot.
    pub async fn mark_domains_active(&self, now_unix: i64) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE domains SET routing_status = 'active', last_error = NULL, updated_at = ?",
        )
        .bind(now_unix)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Fails only unacknowledged routes because Caddy retains its last valid snapshot.
    pub async fn mark_domains_error(&self, message: &str, now_unix: i64) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE domains SET routing_status = 'error', last_error = ?, updated_at = ?
             WHERE routing_status != 'active'",
        )
        .bind(message)
        .bind(now_unix)
        .execute(&self.pool)
        .await?;
        Ok(())
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

    /// Atomically creates one manual deployment attempt and its durable build job.
    pub async fn create_deployment(
        &self,
        input: NewDeployment<'_>,
    ) -> Result<Deployment, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let deployment_id = Uuid::now_v7().to_string();
        let job_id = Uuid::now_v7().to_string();
        let deduplication_key = format!("deploy:{deployment_id}");
        let payload_json = format!(r#"{{"deployment_id":"{deployment_id}"}}"#);
        sqlx::query(
            "INSERT INTO jobs
             (id, kind, project_id, payload_json, state, deduplication_key,
              attempts, max_attempts, available_at, created_at, updated_at)
             VALUES (?, 'deploy_project', ?, ?, 'queued', ?, 0, 3, ?, ?, ?)",
        )
        .bind(&job_id)
        .bind(input.project_id)
        .bind(payload_json)
        .bind(deduplication_key)
        .bind(input.now_unix)
        .bind(input.now_unix)
        .bind(input.now_unix)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO deployments
             (id, project_id, job_id, commit_sha, trigger_kind, status,
              created_by_user_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'queued', ?, ?, ?)",
        )
        .bind(&deployment_id)
        .bind(input.project_id)
        .bind(&job_id)
        .bind(input.commit_sha)
        .bind(input.trigger_kind)
        .bind(input.created_by_user_id)
        .bind(input.now_unix)
        .bind(input.now_unix)
        .execute(&mut *transaction)
        .await?;
        insert_project_audit(
            &mut transaction,
            input.created_by_user_id,
            "deployment.create",
            input.project_id,
            input.now_unix,
        )
        .await?;
        transaction.commit().await?;
        self.deployment_by_id(&deployment_id).await
    }

    /// Creates at most one initial deployment for a resolved project commit.
    ///
    /// Inspection jobs may be retried after their deployment was queued. Reusing
    /// the durable record keeps that retry from producing duplicate builds.
    pub async fn create_initial_deployment(
        &self,
        project_id: &str,
        commit_sha: &str,
        actor_user_id: &str,
        now_unix: i64,
    ) -> Result<Deployment, StoreError> {
        let existing = sqlx::query(
            "SELECT id, project_id, job_id, commit_sha, trigger_kind, status,
                    failure_reason, image_id, container_id, host_port,
                    static_release_path, created_at, updated_at, finished_at, active
             FROM deployments
             WHERE project_id = ? AND commit_sha = ? AND trigger_kind = 'manual'
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(project_id)
        .bind(commit_sha)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = existing {
            return deployment_from_row(row);
        }
        self.create_deployment(NewDeployment {
            project_id,
            commit_sha,
            created_by_user_id: actor_user_id,
            trigger_kind: "manual",
            now_unix,
        })
        .await
    }

    /// Lists deployment history for one project from newest to oldest.
    pub async fn list_deployments(&self, project_id: &str) -> Result<Vec<Deployment>, StoreError> {
        self.project_by_id(project_id).await?;
        let rows = sqlx::query(
            "SELECT id, project_id, job_id, commit_sha, trigger_kind, status,
                    failure_reason, image_id, container_id, host_port, static_release_path,
                    created_at, updated_at, finished_at, active
             FROM deployments WHERE project_id = ? ORDER BY created_at DESC, id DESC",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(deployment_from_row).collect()
    }

    /// Loads one deployment attempt by its opaque identifier.
    pub async fn deployment_by_id(&self, deployment_id: &str) -> Result<Deployment, StoreError> {
        let row = sqlx::query(
            "SELECT id, project_id, job_id, commit_sha, trigger_kind, status,
                    failure_reason, image_id, container_id, host_port, static_release_path,
                    created_at, updated_at, finished_at, active
             FROM deployments WHERE id = ?",
        )
        .bind(deployment_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StoreError::DeploymentNotFound)?;
        deployment_from_row(row)
    }

    /// Persists verified runtime artifacts before the linked job completes successfully.
    pub async fn record_deployment_runtime(
        &self,
        deployment_id: &str,
        image_id: &str,
        container_id: &str,
        host_port: u16,
        now_unix: i64,
    ) -> Result<(), StoreError> {
        let result = sqlx::query(
            "UPDATE deployments SET image_id = ?, container_id = ?, host_port = ?, updated_at = ?
             WHERE id = ? AND status = 'building'",
        )
        .bind(image_id)
        .bind(container_id)
        .bind(i64::from(host_port))
        .bind(now_unix)
        .bind(deployment_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::DeploymentNotFound);
        }
        Ok(())
    }

    /// Persists one immutable static release before atomic deployment promotion.
    pub async fn record_static_release(
        &self,
        deployment_id: &str,
        release_path: &str,
        now_unix: i64,
    ) -> Result<(), StoreError> {
        let result = sqlx::query(
            "UPDATE deployments SET static_release_path = ?, updated_at = ?
             WHERE id = ? AND status = 'building'",
        )
        .bind(release_path)
        .bind(now_unix)
        .bind(deployment_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::DeploymentNotFound);
        }
        Ok(())
    }

    /// Claims the oldest eligible job, including work whose previous lease expired.
    pub async fn claim_job(
        &self,
        worker_id: &str,
        now_unix: i64,
        lease_seconds: i64,
    ) -> Result<Option<Job>, StoreError> {
        let mut transaction = self.pool.begin().await?;
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
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(row) = row {
            let job = job_from_row(row)?;
            sqlx::query(
                "UPDATE deployments SET status = 'building', failure_reason = NULL, updated_at = ?
                 WHERE job_id = ?",
            )
            .bind(now_unix)
            .bind(&job.id)
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await?;
            Ok(Some(job))
        } else {
            transaction.commit().await?;
            Ok(None)
        }
    }

    /// Marks a leased job successful only when the caller still owns its lease.
    pub async fn complete_job(
        &self,
        job_id: &str,
        worker_id: &str,
        result_json: Option<&str>,
        now_unix: i64,
    ) -> Result<DeploymentCleanup, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let missing_runtime = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM deployments
             WHERE deployments.job_id = ?
               AND deployments.static_release_path IS NULL
               AND (deployments.image_id IS NULL
                    OR deployments.container_id IS NULL
                    OR deployments.host_port IS NULL)",
        )
        .bind(job_id)
        .fetch_one(&mut *transaction)
        .await?;
        if missing_runtime > 0 {
            return Err(StoreError::DeploymentRuntimeMissing);
        }
        let result = sqlx::query(
            "UPDATE jobs SET state = 'succeeded', lease_owner = NULL,
                    lease_expires_at = NULL, result_json = ?, updated_at = ?
             WHERE id = ? AND state = 'running' AND lease_owner = ?",
        )
        .bind(result_json)
        .bind(now_unix)
        .bind(job_id)
        .bind(worker_id)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::JobLeaseLost);
        }
        let deployment = sqlx::query(
            "SELECT id, project_id, created_at, container_id
             FROM deployments WHERE job_id = ?",
        )
        .bind(job_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let mut cleanup = DeploymentCleanup::default();
        if let Some(deployment) = deployment {
            let deployment_id: String = deployment.try_get("id")?;
            let project_id: String = deployment.try_get("project_id")?;
            let created_at: i64 = deployment.try_get("created_at")?;
            let newer_exists = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM deployments
                 WHERE project_id = ? AND (created_at > ? OR (created_at = ? AND id > ?))",
            )
            .bind(&project_id)
            .bind(created_at)
            .bind(created_at)
            .bind(&deployment_id)
            .fetch_one(&mut *transaction)
            .await?
                > 0;
            if newer_exists {
                if let Some(container_id) = deployment.try_get("container_id")? {
                    cleanup.container_ids.push(container_id);
                }
            } else {
                cleanup.container_ids = sqlx::query_scalar::<_, String>(
                    "SELECT container_id FROM deployments
                     WHERE project_id = ? AND active = 1 AND id != ?
                       AND container_id IS NOT NULL",
                )
                .bind(&project_id)
                .bind(&deployment_id)
                .fetch_all(&mut *transaction)
                .await?;
                sqlx::query("UPDATE deployments SET active = 0 WHERE project_id = ?")
                    .bind(&project_id)
                    .execute(&mut *transaction)
                    .await?;
            }
            sqlx::query(
                "UPDATE deployments SET status = 'succeeded', failure_reason = NULL,
                        active = ?, updated_at = ?, finished_at = ? WHERE id = ?",
            )
            .bind(!newer_exists)
            .bind(now_unix)
            .bind(now_unix)
            .bind(&deployment_id)
            .execute(&mut *transaction)
            .await?;
            cleanup.image_ids = sqlx::query_scalar::<_, String>(
                "SELECT DISTINCT ranked.image_id FROM (
                   SELECT image_id,
                          ROW_NUMBER() OVER (
                            ORDER BY active DESC, finished_at DESC, id DESC
                          ) AS release_rank
                   FROM deployments
                   WHERE project_id = ? AND status = 'succeeded'
                     AND image_id IS NOT NULL AND image_retired_at IS NULL
                 ) AS ranked
                 WHERE ranked.release_rank > 5
                   AND NOT EXISTS (
                     SELECT 1 FROM deployments active_deployment
                     WHERE active_deployment.active = 1
                       AND active_deployment.image_id = ranked.image_id
                   )",
            )
            .bind(&project_id)
            .fetch_all(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(cleanup)
    }

    /// Lists inactive terminal containers left behind by interrupted cleanup.
    pub async fn containers_pending_cleanup(&self) -> Result<Vec<String>, StoreError> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT container_id FROM deployments
             WHERE active = 0 AND status IN ('succeeded', 'failed')
               AND container_id IS NOT NULL AND retired_at IS NULL ORDER BY finished_at",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Records that a retired runtime container was removed from Docker.
    pub async fn mark_container_retired(
        &self,
        container_id: &str,
        now_unix: i64,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE deployments SET retired_at = ?, updated_at = ?
             WHERE container_id = ? AND active = 0",
        )
        .bind(now_unix)
        .bind(now_unix)
        .bind(container_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Lists old inactive images beyond the five-release runtime retention bound.
    pub async fn images_pending_cleanup(&self) -> Result<Vec<String>, StoreError> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT ranked.image_id FROM (
               SELECT image_id,
                      ROW_NUMBER() OVER (
                        PARTITION BY project_id
                        ORDER BY active DESC, finished_at DESC, id DESC
                      ) AS release_rank
               FROM deployments
               WHERE status = 'succeeded' AND image_id IS NOT NULL
                 AND image_retired_at IS NULL
             ) AS ranked
             WHERE ranked.release_rank > 5
               AND NOT EXISTS (
                 SELECT 1 FROM deployments active_deployment
                 WHERE active_deployment.active = 1
                   AND active_deployment.image_id = ranked.image_id
               )",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Records successful removal of an inactive retained Docker image.
    pub async fn mark_image_retired(
        &self,
        image_id: &str,
        now_unix: i64,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE deployments SET image_retired_at = ?, updated_at = ?
             WHERE image_id = ? AND active = 0",
        )
        .bind(now_unix)
        .bind(now_unix)
        .bind(image_id)
        .execute(&self.pool)
        .await?;
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
        let mut transaction = self.pool.begin().await?;
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
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::JobLeaseLost);
        }
        sqlx::query(
            "UPDATE deployments
             SET status = CASE
                    WHEN (SELECT state FROM jobs WHERE id = ?) = 'failed' THEN 'failed'
                    ELSE 'queued'
                 END,
                 failure_reason = CASE
                    WHEN (SELECT state FROM jobs WHERE id = ?) = 'failed' THEN ?
                    ELSE NULL
                 END,
                 updated_at = ?,
                 finished_at = CASE
                    WHEN (SELECT state FROM jobs WHERE id = ?) = 'failed' THEN ?
                    ELSE NULL
                 END
             WHERE job_id = ?",
        )
        .bind(job_id)
        .bind(job_id)
        .bind(error)
        .bind(now_unix)
        .bind(job_id)
        .bind(now_unix)
        .bind(job_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
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

    /// Records a security-relevant project action without retaining sensitive content.
    pub async fn record_project_audit(
        &self,
        actor_user_id: &str,
        action: &str,
        project_id: &str,
        metadata_json: &str,
        now_unix: i64,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO audit_log
             (actor_user_id, action, target_type, target_id, metadata_json, created_at)
             VALUES (?, ?, 'project', ?, ?, ?)",
        )
        .bind(actor_user_id)
        .bind(action)
        .bind(project_id)
        .bind(metadata_json)
        .bind(now_unix)
        .execute(&self.pool)
        .await?;
        Ok(())
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
    actor_user_id: Option<&str>,
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

/// Maps one domain row into its reconciliation representation.
fn domain_from_row(row: SqliteRow) -> Result<Domain, StoreError> {
    Ok(Domain {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        hostname: row.try_get("hostname")?,
        routing_status: row.try_get("routing_status")?,
        last_error: row.try_get("last_error")?,
        created_at: row.try_get("created_at")?,
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

/// Maps one deployment history row into its domain representation.
fn deployment_from_row(row: SqliteRow) -> Result<Deployment, StoreError> {
    Ok(Deployment {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        job_id: row.try_get("job_id")?,
        commit_sha: row.try_get("commit_sha")?,
        trigger_kind: row.try_get("trigger_kind")?,
        status: row.try_get("status")?,
        failure_reason: row.try_get("failure_reason")?,
        image_id: row.try_get("image_id")?,
        container_id: row.try_get("container_id")?,
        host_port: row.try_get("host_port")?,
        static_release_path: row.try_get("static_release_path")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        finished_at: row.try_get("finished_at")?,
        active: row.try_get("active")?,
    })
}

/// Converts one masked project-variable row without loading encrypted bytes.
fn project_variable_from_row(row: SqliteRow) -> Result<ProjectVariable, StoreError> {
    Ok(ProjectVariable {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        name: row.try_get("name")?,
        is_secret: row.try_get("is_secret")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// Adds a generated project hostname inside the project-creation transaction.
async fn insert_project_domain(
    transaction: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    hostname: &str,
    actor_user_id: &str,
    now_unix: i64,
) -> Result<(), StoreError> {
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM domains WHERE hostname = ? COLLATE NOCASE",
    )
    .bind(hostname)
    .fetch_one(&mut **transaction)
    .await?;
    if exists > 0 {
        return Err(StoreError::DomainAlreadyExists);
    }
    sqlx::query(
        "INSERT INTO domains
         (id, project_id, hostname, routing_status, created_at, updated_at)
         VALUES (?, ?, ?, 'pending', ?, ?)",
    )
    .bind(Uuid::now_v7().to_string())
    .bind(project_id)
    .bind(hostname)
    .bind(now_unix)
    .bind(now_unix)
    .execute(&mut **transaction)
    .await?;
    insert_project_audit(
        transaction,
        actor_user_id,
        "domain.create",
        project_id,
        now_unix,
    )
    .await
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
        BootstrapAdmin, GatewayTarget, NewDeployment, NewJob, NewProject, NewUser, Store,
        StoreError, UpdateServiceConfig, UpsertGithubAppConfiguration, UpsertGithubWebhook,
        UpsertProjectVariable, UserRole,
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

    /// Ensures generated domains cannot be omitted or partially persisted.
    #[tokio::test]
    async fn project_and_default_domain_are_created_atomically() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let admin_id = create_test_admin(&store).await;
        let project = store
            .create_project_with_domain(
                NewProject {
                    slug: "notes",
                    name: "Notes",
                    owner_user_id: &admin_id,
                    repository_url: "https://github.com/example/notes.git",
                    branch: "main",
                    now_unix: 200,
                },
                Some("notes.apps.example.com"),
            )
            .await
            .expect("project is created");
        let domains = store.list_domains(&project.id).await.expect("domains load");
        assert_eq!(domains.len(), 1);
        assert_eq!(domains[0].hostname, "notes.apps.example.com");
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

    /// Proves project domains are unique, listable, and removable.
    #[tokio::test]
    async fn project_domains_enforce_global_hostname_ownership() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let admin_id = create_test_admin(&store).await;
        let project = store
            .create_project(NewProject {
                slug: "domain-owner",
                name: "Domain owner",
                owner_user_id: &admin_id,
                repository_url: "https://github.com/example/project.git",
                branch: "main",
                now_unix: 200,
            })
            .await
            .expect("project is created");
        let domain = store
            .create_domain(&project.id, "app.example.com", &admin_id, 210)
            .await
            .expect("domain is created");
        assert_eq!(domain.routing_status, "pending");
        assert_eq!(
            store.list_domains(&project.id).await.expect("domains list"),
            vec![domain.clone()]
        );
        let reconciliation = store
            .enqueue_domain_reconciliation(210)
            .await
            .expect("reconciliation enqueues");
        let claimed = store
            .claim_job("routing-worker", 210, 60)
            .await
            .expect("claim succeeds")
            .expect("reconciliation is available");
        assert_eq!(claimed.id, reconciliation.id);
        assert_eq!(claimed.kind, "reconcile_domains");
        store
            .mark_domains_error("agent unavailable", 211)
            .await
            .expect("failure records");
        assert_eq!(
            store.list_domains(&project.id).await.expect("domains list")[0].routing_status,
            "error"
        );
        store
            .mark_domains_active(212)
            .await
            .expect("acknowledgment records");
        assert_eq!(
            store.list_domains(&project.id).await.expect("domains list")[0].routing_status,
            "active"
        );
        let duplicate = store
            .create_domain(&project.id, "app.example.com", &admin_id, 211)
            .await
            .expect_err("duplicate is rejected");
        assert!(matches!(duplicate, StoreError::DomainAlreadyExists));
        store
            .delete_domain(&domain.id, &admin_id, 220)
            .await
            .expect("domain deletes");
        assert!(
            store
                .list_domains(&project.id)
                .await
                .expect("domains list")
                .is_empty()
        );
    }

    /// Keeps acknowledged routes active when a newer desired snapshot cannot apply.
    #[tokio::test]
    async fn domain_failure_only_marks_unacknowledged_routes() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let admin_id = create_test_admin(&store).await;
        let project = store
            .create_project(NewProject {
                slug: "stable-routes",
                name: "Stable routes",
                owner_user_id: &admin_id,
                repository_url: "https://github.com/example/project.git",
                branch: "main",
                now_unix: 300,
            })
            .await
            .expect("project is created");
        store
            .create_domain(&project.id, "active.example.com", &admin_id, 301)
            .await
            .expect("active domain is created");
        store
            .mark_domains_active(302)
            .await
            .expect("first snapshot is acknowledged");
        store
            .create_domain(&project.id, "pending.example.com", &admin_id, 303)
            .await
            .expect("pending domain is created");
        store
            .mark_domains_error("agent unavailable", 304)
            .await
            .expect("failure is recorded");

        let domains = store.list_domains(&project.id).await.expect("domains list");
        assert_eq!(domains[0].routing_status, "active");
        assert_eq!(domains[1].routing_status, "error");
    }

    /// Proves deployment history and its durable build job are created atomically.
    #[tokio::test]
    async fn deployment_creation_persists_immutable_history_and_job() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let admin_id = create_test_admin(&store).await;
        let project = store
            .create_project(NewProject {
                slug: "deployable",
                name: "Deployable",
                owner_user_id: &admin_id,
                repository_url: "https://github.com/example/project.git",
                branch: "main",
                now_unix: 200,
            })
            .await
            .expect("project is created");
        let commit_sha = "0123456789abcdef0123456789abcdef01234567";
        let deployment = store
            .create_deployment(NewDeployment {
                project_id: &project.id,
                commit_sha,
                created_by_user_id: &admin_id,
                trigger_kind: "manual",
                now_unix: 210,
            })
            .await
            .expect("deployment is created");
        assert_eq!(deployment.commit_sha, commit_sha);
        assert_eq!(deployment.status, "queued");
        assert_eq!(deployment.trigger_kind, "manual");
        assert!(!deployment.active);
        assert_eq!(
            store
                .list_deployments(&project.id)
                .await
                .expect("history loads"),
            vec![deployment.clone()]
        );

        let claimed = store
            .claim_job("worker", 211, 60)
            .await
            .expect("claim succeeds")
            .expect("build job exists");
        assert_eq!(claimed.id, deployment.job_id);
        assert_eq!(claimed.kind, "deploy_project");
        assert!(claimed.payload_json.contains(&deployment.id));
        assert_eq!(
            store
                .deployment_by_id(&deployment.id)
                .await
                .expect("deployment reloads")
                .status,
            "building"
        );
        store
            .fail_job(&claimed.id, "worker", "build failed", 10, 212)
            .await
            .expect("retry records");
        assert_eq!(
            store
                .deployment_by_id(&deployment.id)
                .await
                .expect("deployment reloads")
                .status,
            "queued"
        );
        store
            .claim_job("worker", 223, 60)
            .await
            .expect("retry claim succeeds")
            .expect("retry is available");
        store
            .record_deployment_runtime(
                &deployment.id,
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                43123,
                223,
            )
            .await
            .expect("runtime identity records");
        store
            .complete_job(&claimed.id, "worker", Some("{}"), 224)
            .await
            .expect("deployment completes");
        let completed = store
            .deployment_by_id(&deployment.id)
            .await
            .expect("deployment reloads");
        assert_eq!(completed.status, "succeeded");
        assert_eq!(completed.finished_at, Some(224));
        assert_eq!(completed.host_port, Some(43123));
        assert!(completed.active);
        store
            .create_domain(&project.id, "app.example.com", &admin_id, 225)
            .await
            .expect("domain is created");
        let target = store
            .gateway_target("APP.EXAMPLE.COM")
            .await
            .expect("target lookup succeeds")
            .expect("healthy target exists");
        assert_eq!(
            target,
            GatewayTarget::Service {
                deployment_id: deployment.id,
                host_port: 43123,
            }
        );
    }

    /// Proves a retried launch inspection cannot duplicate its first deployment.
    #[tokio::test]
    async fn initial_deployment_is_idempotent_per_commit() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let admin_id = create_test_admin(&store).await;
        let project = store
            .create_project(NewProject {
                slug: "first-launch",
                name: "First launch",
                owner_user_id: &admin_id,
                repository_url: "https://github.com/example/project.git",
                branch: "main",
                now_unix: 200,
            })
            .await
            .expect("project is created");
        let commit_sha = "0123456789abcdef0123456789abcdef01234567";
        let first = store
            .create_initial_deployment(&project.id, commit_sha, &admin_id, 210)
            .await
            .expect("first deployment queues");
        let retry = store
            .create_initial_deployment(&project.id, commit_sha, &admin_id, 211)
            .await
            .expect("retry reuses deployment");

        assert_eq!(retry.id, first.id);
        assert_eq!(
            store
                .list_deployments(&project.id)
                .await
                .expect("deployments load")
                .len(),
            1
        );
    }

    /// Proves promotion retires the previous runtime and rejects stale completion order.
    #[tokio::test]
    async fn deployment_promotion_is_single_active_and_stale_safe() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let admin_id = create_test_admin(&store).await;
        let project = store
            .create_project(NewProject {
                slug: "promotion-test",
                name: "Promotion test",
                owner_user_id: &admin_id,
                repository_url: "https://github.com/example/project.git",
                branch: "main",
                now_unix: 100,
            })
            .await
            .expect("project is created");

        let first = create_test_deployment(&store, &project.id, &admin_id, '1', 200).await;
        let first_job = store
            .claim_job("worker-a", 201, 60)
            .await
            .expect("claim succeeds")
            .expect("first job is claimed");
        record_test_runtime(&store, &first.id, "container-first", 40_001, 202).await;
        let cleanup = store
            .complete_job(&first_job.id, "worker-a", Some("{}"), 203)
            .await
            .expect("first deployment promotes");
        assert!(cleanup.container_ids.is_empty());

        let second = create_test_deployment(&store, &project.id, &admin_id, '2', 210).await;
        let second_job = store
            .claim_job("worker-b", 211, 60)
            .await
            .expect("claim succeeds")
            .expect("second job is claimed");
        record_test_runtime(&store, &second.id, "container-second", 40_002, 212).await;
        let cleanup = store
            .complete_job(&second_job.id, "worker-b", Some("{}"), 213)
            .await
            .expect("second deployment promotes");
        assert_eq!(cleanup.container_ids, vec!["container-first"]);
        assert!(
            !store
                .deployment_by_id(&first.id)
                .await
                .expect("loads")
                .active
        );
        assert!(
            store
                .deployment_by_id(&second.id)
                .await
                .expect("loads")
                .active
        );

        let stale = create_test_deployment(&store, &project.id, &admin_id, '3', 220).await;
        let newest = create_test_deployment(&store, &project.id, &admin_id, '4', 221).await;
        let stale_job = store
            .claim_job("worker-c", 222, 60)
            .await
            .expect("claim succeeds")
            .expect("stale job is claimed");
        let newest_job = store
            .claim_job("worker-d", 222, 60)
            .await
            .expect("claim succeeds")
            .expect("newest job is claimed");
        record_test_runtime(&store, &stale.id, "container-stale", 40_003, 223).await;
        record_test_runtime(&store, &newest.id, "container-newest", 40_004, 223).await;
        let cleanup = store
            .complete_job(&newest_job.id, "worker-d", Some("{}"), 224)
            .await
            .expect("newest deployment promotes");
        assert_eq!(cleanup.container_ids, vec!["container-second"]);
        let cleanup = store
            .complete_job(&stale_job.id, "worker-c", Some("{}"), 225)
            .await
            .expect("stale job completes without promotion");
        assert_eq!(cleanup.container_ids, vec!["container-stale"]);
        assert!(
            !store
                .deployment_by_id(&stale.id)
                .await
                .expect("loads")
                .active
        );
        assert!(
            store
                .deployment_by_id(&newest.id)
                .await
                .expect("loads")
                .active
        );

        let fifth = create_test_deployment(&store, &project.id, &admin_id, '5', 230).await;
        let fifth_job = store
            .claim_job("worker-e", 231, 60)
            .await
            .expect("claim succeeds")
            .expect("fifth job is claimed");
        record_test_runtime(&store, &fifth.id, "container-fifth", 40_005, 232).await;
        let cleanup = store
            .complete_job(&fifth_job.id, "worker-e", Some("{}"), 233)
            .await
            .expect("fifth deployment promotes");
        assert!(cleanup.image_ids.is_empty());

        let sixth = create_test_deployment(&store, &project.id, &admin_id, '6', 240).await;
        let sixth_job = store
            .claim_job("worker-f", 241, 60)
            .await
            .expect("claim succeeds")
            .expect("sixth job is claimed");
        record_test_runtime(&store, &sixth.id, "container-sixth", 40_006, 242).await;
        let cleanup = store
            .complete_job(&sixth_job.id, "worker-f", Some("{}"), 243)
            .await
            .expect("sixth deployment promotes");
        assert_eq!(cleanup.image_ids, vec!["sha256:container-first"]);
    }

    /// Creates one test deployment with a distinct repeated hexadecimal SHA.
    async fn create_test_deployment(
        store: &Store,
        project_id: &str,
        admin_id: &str,
        digit: char,
        now_unix: i64,
    ) -> super::Deployment {
        let commit_sha: String = std::iter::repeat_n(digit, 40).collect();
        store
            .create_deployment(NewDeployment {
                project_id,
                commit_sha: &commit_sha,
                created_by_user_id: admin_id,
                trigger_kind: "manual",
                now_unix,
            })
            .await
            .expect("deployment is created")
    }

    /// Records deterministic candidate runtime values for promotion tests.
    async fn record_test_runtime(
        store: &Store,
        deployment_id: &str,
        container_id: &str,
        host_port: u16,
        now_unix: i64,
    ) {
        store
            .record_deployment_runtime(
                deployment_id,
                &format!("sha256:{container_id}"),
                container_id,
                host_port,
                now_unix,
            )
            .await
            .expect("runtime records");
    }

    /// Proves runtime values remain encrypted while list queries return metadata only.
    #[tokio::test]
    async fn project_variables_persist_only_encrypted_values() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let admin_id = create_test_admin(&store).await;
        let project = store
            .create_project(NewProject {
                slug: "variable-test",
                name: "Variable test",
                owner_user_id: &admin_id,
                repository_url: "https://github.com/example/project.git",
                branch: "main",
                now_unix: 100,
            })
            .await
            .expect("project is created");
        let ciphertext = b"authenticated ciphertext";
        store
            .upsert_project_variable(UpsertProjectVariable {
                project_id: &project.id,
                name: "DATABASE_URL",
                ciphertext,
                nonce: &[7_u8; 24],
                is_secret: true,
                actor_user_id: &admin_id,
                now_unix: 200,
            })
            .await
            .expect("variable is stored");
        let metadata = store
            .list_project_variables(&project.id)
            .await
            .expect("metadata lists");
        assert_eq!(metadata.len(), 1);
        assert_eq!(metadata[0].name, "DATABASE_URL");
        assert!(metadata[0].is_secret);
        let encrypted = store
            .encrypted_project_variables(&project.id)
            .await
            .expect("worker payload loads");
        assert_eq!(encrypted[0].ciphertext, ciphertext);
        assert_eq!(encrypted[0].nonce, [7_u8; 24]);
    }

    /// Proves only encrypted GitHub App material is persisted and rotations replace it.
    #[tokio::test]
    async fn github_app_configuration_round_trips_encrypted_material() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let admin_id = create_test_admin(&store).await;
        store
            .upsert_github_app_configuration(UpsertGithubAppConfiguration {
                app_id: 42,
                app_slug: "depot-test",
                private_key_ciphertext: b"encrypted private key",
                private_key_nonce: &[3_u8; 24],
                webhook_secret_ciphertext: b"encrypted webhook secret",
                webhook_secret_nonce: &[4_u8; 24],
                actor_user_id: &admin_id,
                now_unix: 200,
            })
            .await
            .expect("GitHub App configuration stores");
        let configuration = store
            .github_app_configuration()
            .await
            .expect("configuration loads")
            .expect("configuration exists");
        assert_eq!(configuration.app_id, 42);
        assert_eq!(configuration.app_slug, "depot-test");
        assert_eq!(
            configuration.private_key_ciphertext,
            b"encrypted private key"
        );
        assert_eq!(configuration.private_key_nonce, [3_u8; 24]);
        assert_eq!(
            configuration.webhook_secret_ciphertext,
            b"encrypted webhook secret"
        );
        assert_eq!(configuration.webhook_secret_nonce, [4_u8; 24]);
    }

    /// Proves verified GitHub delivery IDs enqueue at most one deployment.
    #[tokio::test]
    async fn github_deliveries_are_atomically_deduplicated() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let admin_id = create_test_admin(&store).await;
        let project = store
            .create_project(NewProject {
                slug: "webhook-test",
                name: "Webhook test",
                owner_user_id: &admin_id,
                repository_url: "https://github.com/aosasona/depot.git",
                branch: "main",
                now_unix: 100,
            })
            .await
            .expect("project is created");
        store
            .upsert_github_webhook(UpsertGithubWebhook {
                project_id: &project.id,
                repository_full_name: "aosasona/depot",
                endpoint_token_hash: &[8_u8; 32],
                secret_ciphertext: b"ciphertext",
                secret_nonce: &[9_u8; 24],
                actor_user_id: &admin_id,
                now_unix: 110,
            })
            .await
            .expect("webhook is created");
        let webhook = store
            .github_webhook_by_token_hash(&[8_u8; 32])
            .await
            .expect("lookup succeeds")
            .expect("webhook exists");
        let commit_sha = "0123456789abcdef0123456789abcdef01234567";
        let deployment = store
            .enqueue_github_delivery(&webhook, "delivery-1", commit_sha, 120)
            .await
            .expect("delivery enqueues")
            .expect("deployment is new");
        assert_eq!(deployment.trigger_kind, "webhook");
        assert!(
            store
                .enqueue_github_delivery(&webhook, "delivery-1", commit_sha, 121)
                .await
                .expect("duplicate is accepted")
                .is_none()
        );
        assert_eq!(
            store
                .list_deployments(&project.id)
                .await
                .expect("history loads")
                .len(),
            1
        );
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
