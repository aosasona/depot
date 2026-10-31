//! Axum routes and transport-safe errors for Depot's HTTP API.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use depot_auth::{AuthError, create_first_admin};
use depot_store::{Store, StoreError};
use serde::{Deserialize, Serialize};
use tracing::error;

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

/// Minimal identity returned after successful first-admin creation.
#[derive(Debug, Serialize)]
struct BootstrapResponse {
    user_id: String,
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
}

/// Builds the versioned authentication routes backed by the supplied store.
pub fn router(store: Store) -> Router {
    Router::new()
        .route(
            "/api/v1/auth/bootstrap",
            get(bootstrap_status).post(bootstrap_admin),
        )
        .with_state(ApiState { store })
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
    use depot_auth::issue_bootstrap_token;
    use depot_store::Store;
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
}
