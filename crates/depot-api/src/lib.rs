//! Axum routes and transport-safe errors for Depot's HTTP API.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use axum_extra::extract::{
    CookieJar,
    cookie::{Cookie, SameSite},
};
use depot_auth::{
    AuthError, authenticate_deploy_token, authenticate_session, create_first_admin,
    create_managed_user, issue_deploy_token, login as authenticate_login, logout as revoke_login,
    validate_csrf,
};
use depot_core::{
    ProjectValidationError, validate_domain_hostname, validate_project_slug,
    validate_repository_url,
};
use depot_secrets::SecretCipher;
use depot_store::{
    Deployment, Domain, JobEvent, JobStatus, NewDeployment, NewJob, NewProject, ProjectSummary,
    ProjectVariable, ServiceConfig, SessionUser, Store, StoreError, UpdateServiceConfig,
    UpsertProjectVariable, UserRole, UserSummary,
};
use serde::{Deserialize, Serialize};
use tracing::error;

const SESSION_COOKIE: &str = "depot_session";
const CSRF_COOKIE: &str = "depot_csrf";
const CSRF_HEADER: &str = "x-csrf-token";
const SESSION_HOURS: i64 = 12;

/// Shared dependencies available to API request handlers.
#[derive(Clone, Debug)]
struct ApiState {
    store: Store,
    secrets: Option<SecretCipher>,
}

/// Request body accepted only while the Depot instance is uninitialized.
#[derive(Debug, Deserialize)]
struct BootstrapRequest {
    token: String,
    email: String,
    password: String,
}

/// Public setup state used to select the bootstrap or login screen.
#[derive(Debug, Serialize)]
struct BootstrapStatus {
    initialized: bool,
}

/// Dependency-aware readiness response for operators and the dashboard.
#[derive(Debug, Serialize)]
struct ReadinessResponse {
    status: &'static str,
    database: &'static str,
}

/// Minimal identity returned after successful first-admin creation.
#[derive(Debug, Serialize)]
struct BootstrapResponse {
    user_id: String,
}

/// Credentials accepted by the local account login endpoint.
#[derive(Debug, Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

/// Safe user identity returned to the authenticated browser.
#[derive(Debug, Serialize)]
struct UserResponse {
    id: String,
    email: String,
    role: &'static str,
}

/// User administration representation without password or session fields.
#[derive(Debug, Serialize)]
struct AdminUserResponse {
    id: String,
    email: String,
    role: &'static str,
    active: bool,
    created_at: i64,
}

/// Admin-only request for creating a local account with an initial password.
#[derive(Debug, Deserialize)]
struct CreateUserRequest {
    email: String,
    password: String,
    role: String,
}

/// Admin-only mutable fields supported for an existing account.
#[derive(Debug, Deserialize)]
struct UpdateUserRequest {
    role: String,
    active: bool,
}

/// Project representation returned to every authenticated workspace member.
#[derive(Debug, Serialize)]
struct ProjectResponse {
    id: String,
    slug: String,
    name: String,
    owner_user_id: String,
    repository_url: String,
    branch: String,
    created_at: i64,
    updated_at: i64,
    latest_inspection_job_id: Option<String>,
}

/// Authenticated request to create a project from a Git source.
#[derive(Debug, Deserialize)]
struct CreateProjectRequest {
    slug: String,
    name: String,
    repository_url: String,
    branch: String,
}

/// Mutable project metadata; stable slug and ownership are intentionally omitted.
#[derive(Debug, Deserialize)]
struct UpdateProjectRequest {
    name: String,
    repository_url: String,
    branch: String,
}

/// Project build and runtime configuration returned to authenticated members.
#[derive(Debug, Serialize)]
struct ServiceConfigResponse {
    project_id: String,
    project_kind: String,
    builder: String,
    context_path: String,
    dockerfile_path: String,
    static_output_dir: Option<String>,
    exposed_port: Option<i64>,
    healthcheck_path: Option<String>,
    updated_at: i64,
}

/// Complete mutable service configuration accepted as one coherent replacement.
#[derive(Debug, Deserialize)]
struct UpdateServiceConfigRequest {
    project_kind: String,
    builder: String,
    context_path: String,
    dockerfile_path: String,
    static_output_dir: Option<String>,
    exposed_port: Option<i64>,
    healthcheck_path: Option<String>,
}

/// Accepted inspection job identifier returned after durable enqueueing.
#[derive(Debug, Serialize)]
struct EnqueuedJobResponse {
    job_id: String,
}

/// Authenticated durable job status with parsed result and ordered events.
#[derive(Debug, Serialize)]
struct JobStatusResponse {
    id: String,
    kind: String,
    project_id: Option<String>,
    state: String,
    attempts: i64,
    max_attempts: i64,
    result: Option<serde_json::Value>,
    last_error: Option<String>,
    created_at: i64,
    updated_at: i64,
    events: Vec<JobEventResponse>,
}

/// One sanitized ordered event attached to a job status response.
#[derive(Debug, Serialize)]
struct JobEventResponse {
    sequence: i64,
    level: String,
    message: String,
    created_at: i64,
}

/// Stable JSON error shape shared by all API endpoints.
#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

/// Safe error details which never contain database or credential values.
#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
}

/// Internal handler errors mapped to deliberate public responses.
#[derive(Debug)]
enum ApiError {
    Auth(AuthError),
    Store(StoreError),
    Forbidden,
    ProjectValidation(ProjectValidationError),
    InvalidProjectField(&'static str),
    ProjectNotInspected,
    InvalidDeployRequest,
    InvalidVariable,
    SecretsUnavailable,
}

/// Durable deployment history returned to authenticated workspace members.
#[derive(Debug, Serialize)]
struct DeploymentResponse {
    id: String,
    project_id: String,
    job_id: String,
    commit_sha: String,
    trigger_kind: String,
    status: String,
    failure_reason: Option<String>,
    image_id: Option<String>,
    container_id: Option<String>,
    host_port: Option<i64>,
    created_at: i64,
    updated_at: i64,
    finished_at: Option<i64>,
    active: bool,
}

/// Desired project hostname and its current routing reconciliation state.
#[derive(Debug, Serialize)]
struct DomainResponse {
    id: String,
    project_id: String,
    hostname: String,
    routing_status: String,
    last_error: Option<String>,
    created_at: i64,
    updated_at: i64,
}

/// Authenticated request to attach one public hostname to a project.
#[derive(Debug, Deserialize)]
struct CreateDomainRequest {
    hostname: String,
}

/// Label used to identify a project deployment credential after issuance.
#[derive(Debug, Deserialize)]
struct CreateDeployTokenRequest {
    label: String,
}

/// One-time response containing a new project deployment credential.
#[derive(Debug, Serialize)]
struct DeployTokenResponse {
    id: String,
    project_id: String,
    label: String,
    token: String,
    created_at: i64,
}

/// Commit selected by a CI system for an authenticated deployment.
#[derive(Debug, Deserialize)]
struct HookDeployRequest {
    commit_sha: String,
}

/// Write-only value and classification for one runtime environment entry.
#[derive(Debug, Deserialize)]
struct UpsertVariableRequest {
    name: String,
    value: String,
    is_secret: bool,
}

/// Masked runtime-variable metadata which never contains a stored value.
#[derive(Debug, Serialize)]
struct VariableResponse {
    id: String,
    project_id: String,
    name: String,
    is_secret: bool,
    created_at: i64,
    updated_at: i64,
}

/// Builds the versioned authentication routes backed by the supplied store.
pub fn router(store: Store) -> Router {
    router_with_secrets(store, None)
}

/// Builds API routes with project secret encryption enabled for production use.
pub fn router_with_secrets(store: Store, secrets: Option<SecretCipher>) -> Router {
    Router::new()
        .route(
            "/api/v1/auth/bootstrap",
            get(bootstrap_status).post(bootstrap_admin),
        )
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/me", get(current_user))
        .route("/api/v1/system/readiness", get(system_readiness))
        .route("/api/v1/users", get(list_users).post(create_user))
        .route("/api/v1/users/{user_id}", axum::routing::patch(update_user))
        .route("/api/v1/projects", get(list_projects).post(create_project))
        .route(
            "/api/v1/projects/{project_id}",
            get(get_project)
                .patch(update_project)
                .delete(delete_project),
        )
        .route(
            "/api/v1/projects/{project_id}/inspect",
            post(enqueue_project_inspection),
        )
        .route(
            "/api/v1/projects/{project_id}/config",
            get(get_service_config).put(update_service_config),
        )
        .route(
            "/api/v1/projects/{project_id}/deployments",
            get(list_deployments).post(create_deployment),
        )
        .route(
            "/api/v1/projects/{project_id}/deploy-tokens",
            post(create_deploy_token),
        )
        .route(
            "/api/v1/projects/{project_id}/deploy-tokens/{token_id}",
            axum::routing::delete(revoke_deploy_token),
        )
        .route(
            "/api/v1/projects/{project_id}/variables",
            get(list_variables).put(upsert_variable),
        )
        .route(
            "/api/v1/projects/{project_id}/variables/{variable_id}",
            axum::routing::delete(delete_variable),
        )
        .route("/api/v1/deploy", post(hook_deploy))
        .route("/api/v1/deployments/{deployment_id}", get(get_deployment))
        .route(
            "/api/v1/deployments/{deployment_id}/rollback",
            post(rollback_deployment),
        )
        .route(
            "/api/v1/projects/{project_id}/domains",
            get(list_domains).post(create_domain),
        )
        .route(
            "/api/v1/domains/{domain_id}",
            axum::routing::delete(delete_domain),
        )
        .route("/api/v1/jobs/{job_id}", get(get_job_status))
        .with_state(ApiState { store, secrets })
}

/// Lists masked variable metadata for one authenticated project.
async fn list_variables(
    State(state): State<ApiState>,
    jar: CookieJar,
    axum::extract::Path(project_id): axum::extract::Path<String>,
) -> Result<Json<Vec<VariableResponse>>, ApiError> {
    require_user(&state.store, &jar).await?;
    let variables = state.store.list_project_variables(&project_id).await?;
    Ok(Json(variables.into_iter().map(variable_response).collect()))
}

/// Encrypts and replaces one validated runtime value after CSRF verification.
async fn upsert_variable(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: HeaderMap,
    axum::extract::Path(project_id): axum::extract::Path<String>,
    Json(request): Json<UpsertVariableRequest>,
) -> Result<Json<VariableResponse>, ApiError> {
    let actor = require_mutation(&state.store, &jar, &headers).await?;
    let name = validate_variable_name(&request.name)?;
    validate_variable_value(&request.value)?;
    let existing = state.store.list_project_variables(&project_id).await?;
    if existing.len() >= 128 && !existing.iter().any(|variable| variable.name == name) {
        return Err(ApiError::InvalidVariable);
    }
    let cipher = state.secrets.as_ref().ok_or(ApiError::SecretsUnavailable)?;
    let context = variable_context(&project_id, name);
    let encrypted = cipher
        .encrypt(&request.value, context.as_bytes())
        .map_err(|_| ApiError::SecretsUnavailable)?;
    let variable = state
        .store
        .upsert_project_variable(UpsertProjectVariable {
            project_id: &project_id,
            name,
            ciphertext: &encrypted.ciphertext,
            nonce: &encrypted.nonce,
            is_secret: request.is_secret,
            actor_user_id: &actor.id,
            now_unix: time::OffsetDateTime::now_utc().unix_timestamp(),
        })
        .await?;
    Ok(Json(variable_response(variable)))
}

/// Deletes one encrypted runtime value after session and CSRF verification.
async fn delete_variable(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: HeaderMap,
    axum::extract::Path((project_id, variable_id)): axum::extract::Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    require_mutation(&state.store, &jar, &headers).await?;
    state
        .store
        .delete_project_variable(&project_id, &variable_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Accepts portable POSIX-style environment names with bounded length.
fn validate_variable_name(value: &str) -> Result<&str, ApiError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_uppercase() || (index > 0 && byte.is_ascii_digit())
        });
    valid.then_some(value).ok_or(ApiError::InvalidVariable)
}

/// Rejects values Docker env files cannot represent safely or reasonably bound.
fn validate_variable_value(value: &str) -> Result<(), ApiError> {
    (!value.contains(['\n', '\r', '\0']) && value.len() <= 32_768)
        .then_some(())
        .ok_or(ApiError::InvalidVariable)
}

/// Produces authenticated context which prevents ciphertext row substitution.
fn variable_context(project_id: &str, name: &str) -> String {
    format!("depot:v1:{project_id}:{name}")
}

/// Enqueues one inspection per current project revision using an active dedup key.
async fn enqueue_project_inspection(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: HeaderMap,
    axum::extract::Path(project_id): axum::extract::Path<String>,
) -> Result<(StatusCode, Json<EnqueuedJobResponse>), ApiError> {
    require_mutation(&state.store, &jar, &headers).await?;
    let project = state.store.project_by_id(&project_id).await?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let deduplication_key = format!("inspect:{}:{}", project.id, project.updated_at);
    let job = state
        .store
        .enqueue_job(NewJob {
            kind: "inspect_repository",
            project_id: Some(&project.id),
            payload_json: "{}",
            deduplication_key: &deduplication_key,
            max_attempts: 3,
            now_unix: now,
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(EnqueuedJobResponse { job_id: job.id }),
    ))
}

/// Returns current durable state and ordered events for an authenticated user.
async fn get_job_status(
    State(state): State<ApiState>,
    jar: CookieJar,
    axum::extract::Path(job_id): axum::extract::Path<String>,
) -> Result<Json<JobStatusResponse>, ApiError> {
    require_user(&state.store, &jar).await?;
    let status = state.store.job_status(&job_id).await?;
    let events = state.store.job_events(&job_id).await?;
    Ok(Json(job_status_response(status, events)))
}

/// Lists projects for any authenticated member of the shared v0.1 workspace.
async fn list_projects(
    State(state): State<ApiState>,
    jar: CookieJar,
) -> Result<Json<Vec<ProjectResponse>>, ApiError> {
    require_user(&state.store, &jar).await?;
    let projects = state.store.list_projects().await?;
    Ok(Json(projects.into_iter().map(project_response).collect()))
}

/// Loads one project for any authenticated workspace member.
async fn get_project(
    State(state): State<ApiState>,
    jar: CookieJar,
    axum::extract::Path(project_id): axum::extract::Path<String>,
) -> Result<Json<ProjectResponse>, ApiError> {
    require_user(&state.store, &jar).await?;
    Ok(Json(project_response(
        state.store.project_by_id(&project_id).await?,
    )))
}

/// Loads one project's build and runtime settings for an authenticated member.
async fn get_service_config(
    State(state): State<ApiState>,
    jar: CookieJar,
    axum::extract::Path(project_id): axum::extract::Path<String>,
) -> Result<Json<ServiceConfigResponse>, ApiError> {
    require_user(&state.store, &jar).await?;
    Ok(Json(service_config_response(
        state.store.service_config(&project_id).await?,
    )))
}

/// Lists immutable deployment attempts for one authenticated project.
async fn list_deployments(
    State(state): State<ApiState>,
    jar: CookieJar,
    axum::extract::Path(project_id): axum::extract::Path<String>,
) -> Result<Json<Vec<DeploymentResponse>>, ApiError> {
    require_user(&state.store, &jar).await?;
    let deployments = state.store.list_deployments(&project_id).await?;
    Ok(Json(
        deployments.into_iter().map(deployment_response).collect(),
    ))
}

/// Creates a manual deployment from the latest successfully inspected commit.
async fn create_deployment(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: HeaderMap,
    axum::extract::Path(project_id): axum::extract::Path<String>,
) -> Result<(StatusCode, Json<DeploymentResponse>), ApiError> {
    let actor = require_mutation(&state.store, &jar, &headers).await?;
    let project = state.store.project_by_id(&project_id).await?;
    let inspection_job_id = project
        .latest_inspection_job_id
        .ok_or(ApiError::ProjectNotInspected)?;
    let inspection = state.store.job_status(&inspection_job_id).await?;
    let result = inspection
        .result_json
        .filter(|_| inspection.state == "succeeded")
        .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
        .ok_or(ApiError::ProjectNotInspected)?;
    let commit_sha = result
        .get("commit_sha")
        .and_then(serde_json::Value::as_str)
        .filter(|value| value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or(ApiError::ProjectNotInspected)?;
    let deployment = state
        .store
        .create_deployment(NewDeployment {
            project_id: &project_id,
            commit_sha,
            created_by_user_id: &actor.id,
            trigger_kind: "manual",
            now_unix: time::OffsetDateTime::now_utc().unix_timestamp(),
        })
        .await?;
    Ok((StatusCode::ACCEPTED, Json(deployment_response(deployment))))
}

/// Creates a one-time-visible project credential after session and CSRF checks.
async fn create_deploy_token(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: HeaderMap,
    axum::extract::Path(project_id): axum::extract::Path<String>,
    Json(request): Json<CreateDeployTokenRequest>,
) -> Result<(StatusCode, Json<DeployTokenResponse>), ApiError> {
    let actor = require_mutation(&state.store, &jar, &headers).await?;
    let label = request.label.trim();
    if label.is_empty() || label.len() > 80 {
        return Err(ApiError::InvalidDeployRequest);
    }
    let issued = issue_deploy_token(&state.store, &project_id, label, &actor.id).await?;
    Ok((
        StatusCode::CREATED,
        Json(DeployTokenResponse {
            id: issued.metadata.id,
            project_id: issued.metadata.project_id,
            label: issued.metadata.label,
            token: issued.token,
            created_at: issued.metadata.created_at,
        }),
    ))
}

/// Revokes a project deploy credential after session and CSRF checks.
async fn revoke_deploy_token(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: HeaderMap,
    axum::extract::Path((project_id, token_id)): axum::extract::Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    require_mutation(&state.store, &jar, &headers).await?;
    state.store.project_by_id(&project_id).await?;
    state
        .store
        .revoke_deploy_token(
            &project_id,
            &token_id,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Enqueues an exact commit using a project-scoped bearer credential.
async fn hook_deploy(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<HookDeployRequest>,
) -> Result<(StatusCode, Json<DeploymentResponse>), ApiError> {
    let token = bearer_token(&headers).ok_or(AuthError::InvalidDeployToken)?;
    let identity = authenticate_deploy_token(&state.store, token).await?;
    let commit_sha = request.commit_sha.trim().to_ascii_lowercase();
    if commit_sha.len() != 40 || !commit_sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiError::InvalidDeployRequest);
    }
    let deployment = state
        .store
        .create_deployment(NewDeployment {
            project_id: &identity.project_id,
            commit_sha: &commit_sha,
            created_by_user_id: &identity.created_by_user_id,
            trigger_kind: "webhook",
            now_unix: time::OffsetDateTime::now_utc().unix_timestamp(),
        })
        .await?;
    Ok((StatusCode::ACCEPTED, Json(deployment_response(deployment))))
}

/// Extracts a strict HTTP bearer token without accepting alternate schemes.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty() && !token.contains(char::is_whitespace))
}

/// Loads one deployment attempt for an authenticated workspace member.
async fn get_deployment(
    State(state): State<ApiState>,
    jar: CookieJar,
    axum::extract::Path(deployment_id): axum::extract::Path<String>,
) -> Result<Json<DeploymentResponse>, ApiError> {
    require_user(&state.store, &jar).await?;
    Ok(Json(deployment_response(
        state.store.deployment_by_id(&deployment_id).await?,
    )))
}

/// Rebuilds and promotes a retained successful commit through the normal safe path.
async fn rollback_deployment(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: HeaderMap,
    axum::extract::Path(deployment_id): axum::extract::Path<String>,
) -> Result<(StatusCode, Json<DeploymentResponse>), ApiError> {
    let actor = require_mutation(&state.store, &jar, &headers).await?;
    let target = state.store.deployment_by_id(&deployment_id).await?;
    if target.status != "succeeded" {
        return Err(ApiError::InvalidDeployRequest);
    }
    let deployment = state
        .store
        .create_deployment(NewDeployment {
            project_id: &target.project_id,
            commit_sha: &target.commit_sha,
            created_by_user_id: &actor.id,
            trigger_kind: "manual",
            now_unix: time::OffsetDateTime::now_utc().unix_timestamp(),
        })
        .await?;
    Ok((StatusCode::ACCEPTED, Json(deployment_response(deployment))))
}

/// Lists desired hostnames for one authenticated project.
async fn list_domains(
    State(state): State<ApiState>,
    jar: CookieJar,
    axum::extract::Path(project_id): axum::extract::Path<String>,
) -> Result<Json<Vec<DomainResponse>>, ApiError> {
    require_user(&state.store, &jar).await?;
    let domains = state.store.list_domains(&project_id).await?;
    Ok(Json(domains.into_iter().map(domain_response).collect()))
}

/// Validates and attaches a globally unique public hostname to one project.
async fn create_domain(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: HeaderMap,
    axum::extract::Path(project_id): axum::extract::Path<String>,
    Json(request): Json<CreateDomainRequest>,
) -> Result<(StatusCode, Json<DomainResponse>), ApiError> {
    let actor = require_mutation(&state.store, &jar, &headers).await?;
    let hostname = validate_domain_hostname(&request.hostname)?;
    let domain = state
        .store
        .create_domain(
            &project_id,
            &hostname,
            &actor.id,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        )
        .await?;
    state
        .store
        .enqueue_domain_reconciliation(time::OffsetDateTime::now_utc().unix_timestamp())
        .await?;
    Ok((StatusCode::CREATED, Json(domain_response(domain))))
}

/// Removes a desired hostname after session-bound CSRF verification.
async fn delete_domain(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: HeaderMap,
    axum::extract::Path(domain_id): axum::extract::Path<String>,
) -> Result<StatusCode, ApiError> {
    let actor = require_mutation(&state.store, &jar, &headers).await?;
    state
        .store
        .delete_domain(
            &domain_id,
            &actor.id,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        )
        .await?;
    state
        .store
        .enqueue_domain_reconciliation(time::OffsetDateTime::now_utc().unix_timestamp())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Validates and replaces project service settings after CSRF verification.
async fn update_service_config(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: HeaderMap,
    axum::extract::Path(project_id): axum::extract::Path<String>,
    Json(request): Json<UpdateServiceConfigRequest>,
) -> Result<Json<ServiceConfigResponse>, ApiError> {
    let actor = require_mutation(&state.store, &jar, &headers).await?;
    validate_service_config(&request)?;
    let config = state
        .store
        .update_service_config(
            &project_id,
            UpdateServiceConfig {
                project_kind: &request.project_kind,
                builder: &request.builder,
                context_path: request.context_path.trim(),
                dockerfile_path: request.dockerfile_path.trim(),
                static_output_dir: trimmed_optional(request.static_output_dir.as_deref()),
                exposed_port: request.exposed_port,
                healthcheck_path: trimmed_optional(request.healthcheck_path.as_deref()),
                actor_user_id: &actor.id,
                now_unix: time::OffsetDateTime::now_utc().unix_timestamp(),
            },
        )
        .await?;
    Ok(Json(service_config_response(config)))
}

/// Creates a validated project and attributes ownership to the current user.
async fn create_project(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: HeaderMap,
    Json(request): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<ProjectResponse>), ApiError> {
    let actor = require_mutation(&state.store, &jar, &headers).await?;
    validate_project_input(
        Some(&request.slug),
        &request.name,
        &request.repository_url,
        &request.branch,
    )?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let project = state
        .store
        .create_project(NewProject {
            slug: &request.slug,
            name: request.name.trim(),
            owner_user_id: &actor.id,
            repository_url: &request.repository_url,
            branch: request.branch.trim(),
            now_unix: now,
        })
        .await?;
    Ok((StatusCode::CREATED, Json(project_response(project))))
}

/// Updates mutable Git source metadata after session-bound CSRF validation.
async fn update_project(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: HeaderMap,
    axum::extract::Path(project_id): axum::extract::Path<String>,
    Json(request): Json<UpdateProjectRequest>,
) -> Result<Json<ProjectResponse>, ApiError> {
    let actor = require_mutation(&state.store, &jar, &headers).await?;
    validate_project_input(
        None,
        &request.name,
        &request.repository_url,
        &request.branch,
    )?;
    let project = state
        .store
        .update_project(
            &project_id,
            request.name.trim(),
            &request.repository_url,
            request.branch.trim(),
            &actor.id,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        )
        .await?;
    Ok(Json(project_response(project)))
}

/// Deletes a project after CSRF verification; deployed-child guards arrive with releases.
async fn delete_project(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: HeaderMap,
    axum::extract::Path(project_id): axum::extract::Path<String>,
) -> Result<StatusCode, ApiError> {
    let actor = require_mutation(&state.store, &jar, &headers).await?;
    state
        .store
        .delete_project(
            &project_id,
            &actor.id,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Distinguishes process liveness from readiness to serve control-plane requests.
async fn system_readiness(
    State(state): State<ApiState>,
) -> Result<Json<ReadinessResponse>, ApiError> {
    state.store.ping().await?;
    Ok(Json(ReadinessResponse {
        status: "ready",
        database: "ready",
    }))
}

/// Verifies credentials and returns secure cookies bound to a server-side session.
async fn login(
    State(state): State<ApiState>,
    jar: CookieJar,
    Json(request): Json<LoginRequest>,
) -> Result<(CookieJar, Json<UserResponse>), ApiError> {
    let issued = authenticate_login(
        &state.store,
        &request.email,
        request.password,
        time::Duration::hours(SESSION_HOURS),
    )
    .await?;
    let jar = jar
        .add(session_cookie(issued.token))
        .add(csrf_cookie(issued.csrf_token));
    Ok((jar, Json(user_response(issued.user))))
}

/// Returns the identity attached to the active opaque session cookie.
async fn current_user(
    State(state): State<ApiState>,
    jar: CookieJar,
) -> Result<Json<UserResponse>, ApiError> {
    let token = session_token(&jar)?;
    let user = authenticate_session(&state.store, token).await?;
    Ok(Json(user_response(user)))
}

/// Validates the session-bound CSRF header, revokes the session, and expires cookies.
async fn logout(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: HeaderMap,
) -> Result<(CookieJar, StatusCode), ApiError> {
    let token = session_token(&jar)?;
    let csrf_token = headers
        .get(CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or(AuthError::InvalidCsrf)?;
    revoke_login(&state.store, token, csrf_token).await?;
    Ok((
        jar.remove(removal_cookie(SESSION_COOKIE))
            .remove(removal_cookie(CSRF_COOKIE)),
        StatusCode::NO_CONTENT,
    ))
}

/// Extracts the opaque bearer cookie without logging or copying it unnecessarily.
fn session_token(jar: &CookieJar) -> Result<&str, ApiError> {
    jar.get(SESSION_COOKIE)
        .map(Cookie::value)
        .ok_or_else(|| AuthError::InvalidSession.into())
}

/// Creates the HttpOnly bearer cookie using strict cross-site and transport policy.
fn session_cookie(token: String) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, token))
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Strict)
        .max_age(time::Duration::hours(SESSION_HOURS))
        .build()
}

/// Creates the readable CSRF cookie whose value must be echoed in a request header.
fn csrf_cookie(token: String) -> Cookie<'static> {
    Cookie::build((CSRF_COOKIE, token))
        .path("/")
        .http_only(false)
        .secure(true)
        .same_site(SameSite::Strict)
        .max_age(time::Duration::hours(SESSION_HOURS))
        .build()
}

/// Creates an immediate-expiry cookie scoped identically to authentication cookies.
fn removal_cookie(name: &'static str) -> Cookie<'static> {
    Cookie::build(name)
        .path("/")
        .secure(true)
        .same_site(SameSite::Strict)
        .build()
}

/// Maps the store identity to an API value with a stable role representation.
fn user_response(user: SessionUser) -> UserResponse {
    UserResponse {
        id: user.id,
        email: user.email,
        role: user.role.as_str(),
    }
}

/// Requires any active authenticated account for a shared-workspace read.
async fn require_user(store: &Store, jar: &CookieJar) -> Result<SessionUser, ApiError> {
    authenticate_session(store, session_token(jar)?)
        .await
        .map_err(ApiError::from)
}

/// Requires an active session and its bound CSRF value for workspace mutations.
async fn require_mutation(
    store: &Store,
    jar: &CookieJar,
    headers: &HeaderMap,
) -> Result<SessionUser, ApiError> {
    let token = session_token(jar)?;
    let csrf_token = headers
        .get(CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or(AuthError::InvalidCsrf)?;
    validate_csrf(store, token, csrf_token)
        .await
        .map_err(ApiError::from)
}

/// Applies project identity, Git target, and compact metadata validation.
fn validate_project_input(
    slug: Option<&str>,
    name: &str,
    repository_url: &str,
    branch: &str,
) -> Result<(), ApiError> {
    if let Some(slug) = slug {
        validate_project_slug(slug)?;
    }
    if name.trim().is_empty() || name.trim().len() > 100 {
        return Err(ApiError::InvalidProjectField(
            "Project name must contain 1-100 characters.",
        ));
    }
    if branch.trim().is_empty() || branch.trim().len() > 255 || branch.chars().any(char::is_control)
    {
        return Err(ApiError::InvalidProjectField(
            "Branch must contain 1-255 printable characters.",
        ));
    }
    validate_repository_url(repository_url)?;
    Ok(())
}

/// Rejects unsafe paths and contradictory project service settings.
fn validate_service_config(request: &UpdateServiceConfigRequest) -> Result<(), ApiError> {
    if !matches!(request.project_kind.as_str(), "auto" | "service" | "static") {
        return Err(ApiError::InvalidProjectField("Project kind is invalid."));
    }
    if !matches!(request.builder.as_str(), "auto" | "dockerfile" | "railpack") {
        return Err(ApiError::InvalidProjectField("Builder is invalid."));
    }
    for (value, message) in [
        (&request.context_path, "Build context path is invalid."),
        (&request.dockerfile_path, "Dockerfile path is invalid."),
    ] {
        if !is_safe_relative_path(value) {
            return Err(ApiError::InvalidProjectField(message));
        }
    }
    if let Some(path) = trimmed_optional(request.static_output_dir.as_deref())
        && !is_safe_relative_path(path)
    {
        return Err(ApiError::InvalidProjectField(
            "Static output path is invalid.",
        ));
    }
    if request.project_kind == "static"
        && trimmed_optional(request.static_output_dir.as_deref()).is_none()
    {
        return Err(ApiError::InvalidProjectField(
            "Static projects require an output directory.",
        ));
    }
    if request
        .exposed_port
        .is_some_and(|port| !(1..=65_535).contains(&port))
    {
        return Err(ApiError::InvalidProjectField("Exposed port is invalid."));
    }
    if let Some(path) = trimmed_optional(request.healthcheck_path.as_deref())
        && (!path.starts_with('/') || path.len() > 255 || path.chars().any(char::is_control))
    {
        return Err(ApiError::InvalidProjectField(
            "Health-check path is invalid.",
        ));
    }
    Ok(())
}

/// Accepts bounded repository-relative paths without traversal or absolute roots.
fn is_safe_relative_path(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('/')
        && !value.starts_with('\\')
        && !value.chars().any(char::is_control)
        && !value.split(['/', '\\']).any(|segment| segment == "..")
}

/// Normalizes blank optional form values to absent database fields.
fn trimmed_optional(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// Maps a stored project into the stable API contract.
fn project_response(project: ProjectSummary) -> ProjectResponse {
    ProjectResponse {
        id: project.id,
        slug: project.slug,
        name: project.name,
        owner_user_id: project.owner_user_id,
        repository_url: project.repository_url,
        branch: project.branch,
        created_at: project.created_at,
        updated_at: project.updated_at,
        latest_inspection_job_id: project.latest_inspection_job_id,
    }
}

/// Maps stored service settings into the stable authenticated API contract.
fn service_config_response(config: ServiceConfig) -> ServiceConfigResponse {
    ServiceConfigResponse {
        project_id: config.project_id,
        project_kind: config.project_kind,
        builder: config.builder,
        context_path: config.context_path,
        dockerfile_path: config.dockerfile_path,
        static_output_dir: config.static_output_dir,
        exposed_port: config.exposed_port,
        healthcheck_path: config.healthcheck_path,
        updated_at: config.updated_at,
    }
}

/// Maps a durable deployment into its stable authenticated API contract.
fn deployment_response(deployment: Deployment) -> DeploymentResponse {
    DeploymentResponse {
        id: deployment.id,
        project_id: deployment.project_id,
        job_id: deployment.job_id,
        commit_sha: deployment.commit_sha,
        trigger_kind: deployment.trigger_kind,
        status: deployment.status,
        failure_reason: deployment.failure_reason,
        image_id: deployment.image_id,
        container_id: deployment.container_id,
        host_port: deployment.host_port,
        created_at: deployment.created_at,
        updated_at: deployment.updated_at,
        finished_at: deployment.finished_at,
        active: deployment.active,
    }
}

/// Converts masked variable metadata without ever loading its encrypted value.
fn variable_response(variable: ProjectVariable) -> VariableResponse {
    VariableResponse {
        id: variable.id,
        project_id: variable.project_id,
        name: variable.name,
        is_secret: variable.is_secret,
        created_at: variable.created_at,
        updated_at: variable.updated_at,
    }
}

/// Maps one desired hostname into the stable authenticated API contract.
fn domain_response(domain: Domain) -> DomainResponse {
    DomainResponse {
        id: domain.id,
        project_id: domain.project_id,
        hostname: domain.hostname,
        routing_status: domain.routing_status,
        last_error: domain.last_error,
        created_at: domain.created_at,
        updated_at: domain.updated_at,
    }
}

/// Parses trusted worker JSON while keeping malformed results non-fatal to status reads.
fn job_status_response(status: JobStatus, events: Vec<JobEvent>) -> JobStatusResponse {
    JobStatusResponse {
        id: status.id,
        kind: status.kind,
        project_id: status.project_id,
        state: status.state,
        attempts: status.attempts,
        max_attempts: status.max_attempts,
        result: status
            .result_json
            .and_then(|value| serde_json::from_str(&value).ok()),
        last_error: status.last_error,
        created_at: status.created_at,
        updated_at: status.updated_at,
        events: events
            .into_iter()
            .map(|event| JobEventResponse {
                sequence: event.sequence,
                level: event.level,
                message: event.message,
                created_at: event.created_at,
            })
            .collect(),
    }
}

/// Lists local accounts for active administrators only.
async fn list_users(
    State(state): State<ApiState>,
    jar: CookieJar,
) -> Result<Json<Vec<AdminUserResponse>>, ApiError> {
    require_admin(&state.store, &jar).await?;
    let users = state.store.list_users().await?;
    Ok(Json(users.into_iter().map(admin_user_response).collect()))
}

/// Creates a local account after both session-role and CSRF verification.
async fn create_user(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: HeaderMap,
    Json(request): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<AdminUserResponse>), ApiError> {
    let actor = require_admin_mutation(&state.store, &jar, &headers).await?;
    let role = UserRole::parse(&request.role)?;
    let user = create_managed_user(
        &state.store,
        &actor.id,
        &request.email,
        request.password,
        role,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(admin_user_response(user))))
}

/// Changes role or activation while retaining the last active administrator.
async fn update_user(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: HeaderMap,
    axum::extract::Path(user_id): axum::extract::Path<String>,
    Json(request): Json<UpdateUserRequest>,
) -> Result<Json<AdminUserResponse>, ApiError> {
    let actor = require_admin_mutation(&state.store, &jar, &headers).await?;
    let role = UserRole::parse(&request.role)?;
    let user = state
        .store
        .update_user(
            &user_id,
            role,
            request.active,
            &actor.id,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        )
        .await?;
    Ok(Json(admin_user_response(user)))
}

/// Requires an active authenticated administrator for a read operation.
async fn require_admin(store: &Store, jar: &CookieJar) -> Result<SessionUser, ApiError> {
    let user = authenticate_session(store, session_token(jar)?).await?;
    if user.role != UserRole::Admin {
        return Err(ApiError::Forbidden);
    }
    Ok(user)
}

/// Requires administrator role and session-bound CSRF for a state change.
async fn require_admin_mutation(
    store: &Store,
    jar: &CookieJar,
    headers: &HeaderMap,
) -> Result<SessionUser, ApiError> {
    let token = session_token(jar)?;
    let csrf_token = headers
        .get(CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or(AuthError::InvalidCsrf)?;
    let user = validate_csrf(store, token, csrf_token).await?;
    if user.role != UserRole::Admin {
        return Err(ApiError::Forbidden);
    }
    Ok(user)
}

/// Maps a store user summary into the public administration contract.
fn admin_user_response(user: UserSummary) -> AdminUserResponse {
    AdminUserResponse {
        id: user.id,
        email: user.email,
        role: user.role.as_str(),
        active: user.active,
        created_at: user.created_at,
    }
}

/// Reports only whether setup remains available, without exposing token state.
async fn bootstrap_status(
    State(state): State<ApiState>,
) -> Result<Json<BootstrapStatus>, ApiError> {
    Ok(Json(BootstrapStatus {
        initialized: state.store.is_initialized().await?,
    }))
}

/// Consumes an operator-issued token and atomically creates the first administrator.
async fn bootstrap_admin(
    State(state): State<ApiState>,
    Json(request): Json<BootstrapRequest>,
) -> Result<(StatusCode, Json<BootstrapResponse>), ApiError> {
    let user_id = create_first_admin(
        &state.store,
        &request.token,
        &request.email,
        request.password,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(BootstrapResponse { user_id })))
}

impl From<AuthError> for ApiError {
    /// Preserves authentication error categories for safe HTTP mapping.
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}

impl From<StoreError> for ApiError {
    /// Preserves store error categories for safe HTTP mapping.
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<ProjectValidationError> for ApiError {
    /// Preserves project validation categories for stable client error mapping.
    fn from(error: ProjectValidationError) -> Self {
        Self::ProjectValidation(error)
    }
}

impl IntoResponse for ApiError {
    /// Converts internal failures to stable status codes without leaking sensitive details.
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Auth(AuthError::WeakPassword) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "weak_password",
                "Password must contain at least 12 characters.",
            ),
            Self::Auth(AuthError::InvalidEmail) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_email",
                "Enter a valid email address.",
            ),
            Self::Auth(AuthError::InvalidCredentials) => (
                StatusCode::UNAUTHORIZED,
                "invalid_credentials",
                "Email or password is incorrect.",
            ),
            Self::Auth(AuthError::InvalidSession) => (
                StatusCode::UNAUTHORIZED,
                "invalid_session",
                "Your session is invalid or expired.",
            ),
            Self::Auth(AuthError::InvalidCsrf) => (
                StatusCode::FORBIDDEN,
                "invalid_csrf",
                "The request could not be verified.",
            ),
            Self::Auth(AuthError::InvalidDeployToken) => (
                StatusCode::UNAUTHORIZED,
                "invalid_deploy_token",
                "The deploy token is invalid or revoked.",
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "You do not have permission to perform this action.",
            ),
            Self::ProjectValidation(ProjectValidationError::InvalidSlug) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_project_slug",
                "Use 2-63 lowercase letters, numbers, or hyphens for the slug.",
            ),
            Self::ProjectValidation(ProjectValidationError::InvalidRepositoryUrl) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_repository_url",
                "Use a valid HTTPS or SSH repository URL without embedded credentials.",
            ),
            Self::ProjectValidation(ProjectValidationError::PrivateRepositoryHost) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "private_repository_host",
                "Repository URLs cannot target localhost or private IP addresses.",
            ),
            Self::ProjectValidation(ProjectValidationError::InvalidDomainHostname) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_domain_hostname",
                "Enter a public DNS hostname without a scheme, port, or wildcard.",
            ),
            Self::InvalidProjectField(message) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "invalid_project", message)
            }
            Self::Store(StoreError::EmailAlreadyExists)
            | Self::Auth(AuthError::Store(StoreError::EmailAlreadyExists)) => (
                StatusCode::CONFLICT,
                "email_already_exists",
                "A user with that email already exists.",
            ),
            Self::Store(StoreError::LastActiveAdmin)
            | Self::Auth(AuthError::Store(StoreError::LastActiveAdmin)) => (
                StatusCode::CONFLICT,
                "last_active_admin",
                "The last active administrator cannot be changed.",
            ),
            Self::Store(StoreError::UserNotFound)
            | Self::Auth(AuthError::Store(StoreError::UserNotFound)) => (
                StatusCode::NOT_FOUND,
                "user_not_found",
                "The requested user does not exist.",
            ),
            Self::Store(StoreError::ProjectSlugAlreadyExists)
            | Self::Auth(AuthError::Store(StoreError::ProjectSlugAlreadyExists)) => (
                StatusCode::CONFLICT,
                "project_slug_exists",
                "A project with that slug already exists.",
            ),
            Self::Store(StoreError::ProjectNotFound)
            | Self::Auth(AuthError::Store(StoreError::ProjectNotFound)) => (
                StatusCode::NOT_FOUND,
                "project_not_found",
                "The requested project does not exist.",
            ),
            Self::Store(StoreError::JobNotFound) => (
                StatusCode::NOT_FOUND,
                "job_not_found",
                "The requested job does not exist.",
            ),
            Self::Store(StoreError::DeploymentNotFound) => (
                StatusCode::NOT_FOUND,
                "deployment_not_found",
                "The requested deployment does not exist.",
            ),
            Self::Store(StoreError::DeploymentRuntimeMissing) => (
                StatusCode::CONFLICT,
                "deployment_runtime_missing",
                "The deployment runtime is not ready to complete.",
            ),
            Self::Store(StoreError::DomainAlreadyExists) => (
                StatusCode::CONFLICT,
                "domain_already_exists",
                "That hostname is already attached to a project.",
            ),
            Self::Store(StoreError::DomainNotFound) => (
                StatusCode::NOT_FOUND,
                "domain_not_found",
                "The requested domain does not exist.",
            ),
            Self::ProjectNotInspected => (
                StatusCode::CONFLICT,
                "project_not_inspected",
                "Inspect the current project revision before deploying it.",
            ),
            Self::InvalidDeployRequest => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_deploy_request",
                "Provide a 40-character Git commit SHA and a valid token label.",
            ),
            Self::InvalidVariable => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_variable",
                "Use an uppercase environment name and a value without line breaks.",
            ),
            Self::SecretsUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "secrets_unavailable",
                "Project secret encryption is not configured.",
            ),
            Self::Store(StoreError::InvalidRole(_))
            | Self::Auth(AuthError::Store(StoreError::InvalidRole(_))) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_role",
                "Choose a supported user role.",
            ),
            Self::Auth(AuthError::Store(StoreError::AlreadyInitialized))
            | Self::Store(StoreError::AlreadyInitialized) => (
                StatusCode::CONFLICT,
                "already_initialized",
                "This Depot instance is already initialized.",
            ),
            Self::Auth(AuthError::Store(StoreError::InvalidBootstrapToken))
            | Self::Store(StoreError::InvalidBootstrapToken) => (
                StatusCode::UNAUTHORIZED,
                "invalid_bootstrap_token",
                "The setup token is invalid or expired.",
            ),
            error => {
                error!(?error, "API request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "Depot could not complete the request.",
                )
            }
        };
        (
            status,
            Json(ErrorEnvelope {
                error: ErrorBody { code, message },
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use depot_auth::{
        create_first_admin, create_managed_user, issue_bootstrap_token, issue_deploy_token, login,
    };
    use depot_secrets::SecretCipher;
    use depot_store::{NewProject, Store, UserRole};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use time::Duration;
    use tower::ServiceExt;

    use super::{
        UpdateServiceConfigRequest, router, router_with_secrets, validate_service_config,
        validate_variable_name, validate_variable_value,
    };

    /// Parses a JSON response body for focused API assertions.
    async fn json_body(response: axum::response::Response) -> Value {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("response body reads")
            .to_bytes();
        serde_json::from_slice(&bytes).expect("response is JSON")
    }

    /// Converts response Set-Cookie fields into one request Cookie header.
    fn request_cookie_header(response: &axum::response::Response) -> String {
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok()?.split(';').next())
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Proves a project bearer credential can deploy an exact SHA and fails after revocation.
    #[tokio::test]
    async fn deploy_hook_uses_revocable_project_token() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let bootstrap = issue_bootstrap_token(&store, Duration::minutes(5))
            .await
            .expect("bootstrap token issues");
        let admin_id = create_first_admin(
            &store,
            &bootstrap.token,
            "admin@example.com",
            "a secure initial password".to_owned(),
        )
        .await
        .expect("admin is created");
        let project = store
            .create_project(NewProject {
                slug: "hook-test",
                name: "Hook test",
                owner_user_id: &admin_id,
                repository_url: "https://github.com/example/project.git",
                branch: "main",
                now_unix: 100,
            })
            .await
            .expect("project is created");
        let issued = issue_deploy_token(&store, &project.id, "GitHub Actions", &admin_id)
            .await
            .expect("deploy token issues");
        let token_id = issued.metadata.id.clone();
        let app = router(store.clone());
        let commit_sha = "0123456789abcdef0123456789abcdef01234567";
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/v1/deploy")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {}", issued.token))
                    .body(Body::from(json!({ "commit_sha": commit_sha }).to_string()))
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = json_body(response).await;
        assert_eq!(body["commit_sha"], commit_sha);
        assert_eq!(body["trigger_kind"], "webhook");

        assert!(
            store
                .revoke_deploy_token(&project.id, &token_id, 200)
                .await
                .expect("token revokes")
        );
        let response = app
            .oneshot(
                Request::post("/api/v1/deploy")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {}", issued.token))
                    .body(Body::from(json!({ "commit_sha": commit_sha }).to_string()))
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// Proves repository-relative build paths cannot escape the checked-out source.
    #[test]
    fn service_config_rejects_traversal_and_incomplete_static_settings() {
        let traversal = UpdateServiceConfigRequest {
            project_kind: "service".to_owned(),
            builder: "dockerfile".to_owned(),
            context_path: "../secrets".to_owned(),
            dockerfile_path: "Dockerfile".to_owned(),
            static_output_dir: None,
            exposed_port: Some(8080),
            healthcheck_path: Some("/health".to_owned()),
        };
        assert!(validate_service_config(&traversal).is_err());

        let incomplete_static = UpdateServiceConfigRequest {
            project_kind: "static".to_owned(),
            builder: "railpack".to_owned(),
            context_path: ".".to_owned(),
            dockerfile_path: "Dockerfile".to_owned(),
            static_output_dir: Some("  ".to_owned()),
            exposed_port: None,
            healthcheck_path: None,
        };
        assert!(validate_service_config(&incomplete_static).is_err());
    }

    /// Proves environment syntax rejects shell-like names and multiline values.
    #[test]
    fn runtime_variables_use_bounded_portable_syntax() {
        assert_eq!(
            validate_variable_name("DATABASE_URL").expect("valid"),
            "DATABASE_URL"
        );
        assert!(validate_variable_name("lowercase").is_err());
        assert!(validate_variable_name("1INVALID").is_err());
        assert!(validate_variable_name("NAME=value").is_err());
        validate_variable_value("postgres://database/app").expect("ordinary value is valid");
        assert!(validate_variable_value("first\nsecond").is_err());
    }

    /// Proves variable writes encrypt values and all API reads remain masked.
    #[tokio::test]
    async fn runtime_variable_api_never_returns_stored_values() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let bootstrap = issue_bootstrap_token(&store, Duration::minutes(5))
            .await
            .expect("bootstrap token issues");
        let admin_id = create_first_admin(
            &store,
            &bootstrap.token,
            "admin@example.com",
            "a secure initial password".to_owned(),
        )
        .await
        .expect("admin is created");
        let session = login(
            &store,
            "admin@example.com",
            "a secure initial password".to_owned(),
            Duration::hours(1),
        )
        .await
        .expect("login succeeds");
        let project = store
            .create_project(NewProject {
                slug: "secret-api",
                name: "Secret API",
                owner_user_id: &admin_id,
                repository_url: "https://github.com/example/project.git",
                branch: "main",
                now_unix: 100,
            })
            .await
            .expect("project is created");
        let key_path = std::env::temp_dir().join(format!(
            "depot-api-secret-key-{}",
            time::OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        fs::write(&key_path, "11".repeat(32)).expect("key writes");
        let cipher = SecretCipher::load(&key_path).expect("key loads");
        fs::remove_file(key_path).expect("temporary key is removed");
        let app = router_with_secrets(store.clone(), Some(cipher.clone()));
        let cookie = format!("depot_session={}", session.token);
        let response = app
            .clone()
            .oneshot(
                Request::put(format!("/api/v1/projects/{}/variables", project.id))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .header("x-csrf-token", &session.csrf_token)
                    .body(Body::from(
                        json!({
                            "name": "DATABASE_URL",
                            "value": "super-secret-value",
                            "is_secret": true
                        })
                        .to_string(),
                    ))
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["name"], "DATABASE_URL");
        assert!(body.get("value").is_none());

        let response = app
            .oneshot(
                Request::get(format!("/api/v1/projects/{}/variables", project.id))
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        let body = json_body(response).await;
        assert!(!body.to_string().contains("super-secret-value"));
        let encrypted = store
            .encrypted_project_variables(&project.id)
            .await
            .expect("encrypted value loads");
        assert_ne!(encrypted[0].ciphertext, b"super-secret-value");
        let context = format!("depot:v1:{}:DATABASE_URL", project.id);
        assert_eq!(
            cipher
                .decrypt(
                    &encrypted[0].ciphertext,
                    &encrypted[0].nonce,
                    context.as_bytes()
                )
                .expect("ciphertext decrypts"),
            "super-secret-value"
        );
    }

    /// Proves setup status flips only after valid first-admin creation.
    #[tokio::test]
    async fn valid_bootstrap_initializes_instance_once() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let issued = issue_bootstrap_token(&store, Duration::minutes(5))
            .await
            .expect("token issues");
        let app = router(store);

        let before = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auth/bootstrap")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(json_body(before).await["initialized"], false);

        let request_body = json!({
            "token": issued.token,
            "email": "admin@example.com",
            "password": "correct horse battery staple"
        });
        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/bootstrap")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(request_body.to_string()))
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(created.status(), StatusCode::CREATED);

        let after = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auth/bootstrap")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(json_body(after).await["initialized"], true);
    }

    /// Proves malformed setup credentials return a safe non-specific error.
    #[tokio::test]
    async fn invalid_bootstrap_token_is_rejected() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let app = router(store);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/bootstrap")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "token": "wrong",
                            "email": "admin@example.com",
                            "password": "correct horse battery staple"
                        })
                        .to_string(),
                    ))
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(response).await["error"]["code"],
            "invalid_bootstrap_token"
        );
    }

    /// Proves login cookies authorize identity lookup and CSRF protects revocation.
    #[tokio::test]
    async fn login_me_and_logout_enforce_session_and_csrf() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let issued = issue_bootstrap_token(&store, Duration::minutes(5))
            .await
            .expect("token issues");
        create_first_admin(
            &store,
            &issued.token,
            "admin@example.com",
            "correct horse battery staple".to_owned(),
        )
        .await
        .expect("admin is created");
        let app = router(store);

        let login = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "email": "admin@example.com",
                            "password": "correct horse battery staple"
                        })
                        .to_string(),
                    ))
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(login.status(), StatusCode::OK);
        let set_cookies: Vec<String> = login
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().expect("cookie is ASCII").to_owned())
            .collect();
        assert!(set_cookies.iter().any(|value| {
            value.starts_with("depot_session=")
                && value.contains("HttpOnly")
                && value.contains("Secure")
        }));
        let cookie_header = request_cookie_header(&login);
        let csrf_token = set_cookies
            .iter()
            .find_map(|value| value.strip_prefix("depot_csrf="))
            .and_then(|value| value.split(';').next())
            .expect("CSRF cookie exists")
            .to_owned();

        let me = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auth/me")
                    .header(header::COOKIE, &cookie_header)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(me.status(), StatusCode::OK);
        assert_eq!(json_body(me).await["role"], "admin");

        let forged_logout = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/logout")
                    .header(header::COOKIE, &cookie_header)
                    .header("x-csrf-token", "forged")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(forged_logout.status(), StatusCode::FORBIDDEN);

        let logout = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/logout")
                    .header(header::COOKIE, &cookie_header)
                    .header("x-csrf-token", csrf_token)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(logout.status(), StatusCode::NO_CONTENT);

        let revoked = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auth/me")
                    .header(header::COOKIE, cookie_header)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
    }

    /// Proves a valid member session cannot access administrator user APIs.
    #[tokio::test]
    async fn member_cannot_list_users() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let issued = issue_bootstrap_token(&store, Duration::minutes(5))
            .await
            .expect("token issues");
        let admin_id = create_first_admin(
            &store,
            &issued.token,
            "admin@example.com",
            "correct horse battery staple".to_owned(),
        )
        .await
        .expect("admin is created");
        create_managed_user(
            &store,
            &admin_id,
            "member@example.com",
            "another correct battery staple".to_owned(),
            UserRole::Member,
        )
        .await
        .expect("member is created");
        let member_session = login(
            &store,
            "member@example.com",
            "another correct battery staple".to_owned(),
            Duration::hours(1),
        )
        .await
        .expect("member logs in");
        let app = router(store);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/users")
                    .header(
                        header::COOKIE,
                        format!("depot_session={}", member_session.token),
                    )
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    /// Proves an authenticated member can create, read, update, and delete a project.
    #[tokio::test]
    async fn project_crud_requires_session_and_csrf() {
        let store = Store::connect_in_memory().await.expect("store opens");
        let issued = issue_bootstrap_token(&store, Duration::minutes(5))
            .await
            .expect("token issues");
        create_first_admin(
            &store,
            &issued.token,
            "admin@example.com",
            "correct horse battery staple".to_owned(),
        )
        .await
        .expect("admin is created");
        let session = login(
            &store,
            "admin@example.com",
            "correct horse battery staple".to_owned(),
            Duration::hours(1),
        )
        .await
        .expect("admin logs in");
        let cookie = format!("depot_session={}", session.token);
        let app = router(store);

        let create = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/projects")
                    .header(header::COOKIE, &cookie)
                    .header("x-csrf-token", &session.csrf_token)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "slug": "first-project",
                            "name": "First project",
                            "repository_url": "https://github.com/example/project.git",
                            "branch": "main"
                        })
                        .to_string(),
                    ))
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(create.status(), StatusCode::CREATED);
        let project = json_body(create).await;
        let project_id = project["id"].as_str().expect("project has id");

        let update = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/v1/projects/{project_id}"))
                    .header(header::COOKIE, &cookie)
                    .header("x-csrf-token", &session.csrf_token)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "name": "Updated project",
                            "repository_url": "https://github.com/example/project.git",
                            "branch": "stable"
                        })
                        .to_string(),
                    ))
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(json_body(update).await["branch"], "stable");

        let list = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(json_body(list).await.as_array().map(Vec::len), Some(1));

        let inspection = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/projects/{project_id}/inspect"))
                    .header(header::COOKIE, &cookie)
                    .header("x-csrf-token", &session.csrf_token)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(inspection.status(), StatusCode::ACCEPTED);
        let inspection_body = json_body(inspection).await;
        let job_id = inspection_body["job_id"].as_str().expect("job has id");
        let job_status = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/jobs/{job_id}"))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(json_body(job_status).await["state"], "queued");

        let delete = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/projects/{project_id}"))
                    .header(header::COOKIE, cookie)
                    .header("x-csrf-token", session.csrf_token)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(delete.status(), StatusCode::NO_CONTENT);
    }
}
