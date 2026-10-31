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
    AuthError, authenticate_session, create_first_admin, create_managed_user,
    login as authenticate_login, logout as revoke_login, validate_csrf,
};
use depot_store::{SessionUser, Store, StoreError, UserRole, UserSummary};
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
}

/// Builds the versioned authentication routes backed by the supplied store.
pub fn router(store: Store) -> Router {
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
        .with_state(ApiState { store })
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
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "You do not have permission to perform this action.",
            ),
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
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use depot_auth::{create_first_admin, create_managed_user, issue_bootstrap_token, login};
    use depot_store::{Store, UserRole};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use time::Duration;
    use tower::ServiceExt;

    use super::router;

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
}
