//! Narrow GitHub App authentication and repository discovery client.

use std::sync::Arc;

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

const API_VERSION: &str = "2026-03-10";

/// Operator-owned GitHub App identity and signing material.
#[derive(Clone)]
pub struct GithubAppClient {
    app_id: u64,
    app_slug: Arc<str>,
    key: Arc<EncodingKey>,
    http: Client,
}

impl std::fmt::Debug for GithubAppClient {
    /// Omits private signing material from diagnostic output.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GithubAppClient")
            .field("app_id", &self.app_id)
            .field("app_slug", &self.app_slug)
            .finish_non_exhaustive()
    }
}

/// GitHub App installation metadata safe to persist and show to operators.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Installation {
    pub id: u64,
    pub account: InstallationAccount,
    pub repository_selection: String,
}

/// Account owning one GitHub App installation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InstallationAccount {
    pub login: String,
    #[serde(rename = "type")]
    pub account_type: String,
}

/// Repository metadata needed for project creation and source checkout.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Repository {
    pub id: u64,
    pub name: String,
    pub full_name: String,
    pub private: bool,
    pub clone_url: String,
    pub default_branch: String,
    pub archived: bool,
}

/// Short-lived installation credential retained only in process memory.
#[derive(Debug, Deserialize)]
struct InstallationToken {
    token: String,
}

/// Paginated repository response returned by installation authentication.
#[derive(Debug, Deserialize)]
struct RepositoryPage {
    repositories: Vec<Repository>,
}

/// Minimal claims GitHub accepts for app-level RS256 authentication.
#[derive(Debug, Serialize)]
struct AppClaims {
    iat: i64,
    exp: i64,
    iss: String,
}

/// Safe GitHub integration failures without credential or response-body leakage.
#[derive(Debug, Error)]
pub enum GithubError {
    #[error("GitHub App private key is invalid")]
    InvalidPrivateKey,
    #[error("GitHub App authentication could not be signed")]
    Jwt,
    #[error("GitHub API request failed: {0}")]
    Request(#[source] reqwest::Error),
    #[error("GitHub API rejected the request with status {0}")]
    Rejected(StatusCode),
    #[error("GitHub returned an invalid response")]
    InvalidResponse,
    #[error("repository is not accessible to this GitHub App installation")]
    RepositoryUnavailable,
}

impl GithubAppClient {
    /// Creates a client from an app ID, public app slug, and RSA PEM bytes.
    pub fn new(app_id: u64, app_slug: &str, private_key_pem: &[u8]) -> Result<Self, GithubError> {
        let key = EncodingKey::from_rsa_pem(private_key_pem)
            .map_err(|_| GithubError::InvalidPrivateKey)?;
        let http = Client::builder()
            .user_agent("depot-self-hosted")
            .build()
            .map_err(GithubError::Request)?;
        Ok(Self {
            app_id,
            app_slug: Arc::from(app_slug),
            key: Arc::new(key),
            http,
        })
    }

    /// Returns GitHub's installation page for this configured application.
    pub fn installation_url(&self) -> String {
        format!(
            "https://github.com/apps/{}/installations/new",
            self.app_slug
        )
    }

    /// Verifies and returns one installation owned by the configured app.
    pub async fn installation(&self, installation_id: u64) -> Result<Installation, GithubError> {
        self.app_get(&format!("/app/installations/{installation_id}"))
            .await
    }

    /// Lists every repository explicitly visible to one linked installation.
    pub async fn repositories(&self, installation_id: u64) -> Result<Vec<Repository>, GithubError> {
        let token = self.installation_token(installation_id).await?;
        let mut repositories = Vec::new();
        for page in 1..=100 {
            let response: RepositoryPage = self
                .installation_get(
                    &token,
                    &format!("/installation/repositories?per_page=100&page={page}"),
                )
                .await?;
            let count = response.repositories.len();
            repositories.extend(response.repositories);
            if count < 100 {
                break;
            }
        }
        repositories.sort_by(|left, right| left.full_name.cmp(&right.full_name));
        Ok(repositories)
    }

    /// Resolves one selected repository only after installation-scoped verification.
    pub async fn repository(
        &self,
        installation_id: u64,
        repository_id: u64,
    ) -> Result<Repository, GithubError> {
        let token = self.installation_token(installation_id).await?;
        self.installation_get(&token, &format!("/repositories/{repository_id}"))
            .await
            .map_err(|error| match error {
                GithubError::Rejected(StatusCode::NOT_FOUND | StatusCode::FORBIDDEN) => {
                    GithubError::RepositoryUnavailable
                }
                error => error,
            })
    }

    /// Mints a short-lived token for private checkout without persisting it.
    pub async fn checkout_token(&self, installation_id: u64) -> Result<String, GithubError> {
        self.installation_token(installation_id).await
    }

    /// Exchanges an app JWT for an installation-scoped one-hour credential.
    async fn installation_token(&self, installation_id: u64) -> Result<String, GithubError> {
        let jwt = self.jwt()?;
        let response = self
            .http
            .post(format!(
                "https://api.github.com/app/installations/{installation_id}/access_tokens"
            ))
            .headers(api_headers(&jwt)?)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(GithubError::Request)?;
        decode_response::<InstallationToken>(response)
            .await
            .map(|token| token.token)
    }

    /// Performs one app-authenticated GET against a fixed GitHub API origin.
    async fn app_get<T: DeserializeOwned>(&self, path: &str) -> Result<T, GithubError> {
        let jwt = self.jwt()?;
        let response = self
            .http
            .get(format!("https://api.github.com{path}"))
            .headers(api_headers(&jwt)?)
            .send()
            .await
            .map_err(GithubError::Request)?;
        decode_response(response).await
    }

    /// Performs one installation-authenticated GET against GitHub's fixed API origin.
    async fn installation_get<T: DeserializeOwned>(
        &self,
        token: &str,
        path: &str,
    ) -> Result<T, GithubError> {
        let response = self
            .http
            .get(format!("https://api.github.com{path}"))
            .headers(api_headers(token)?)
            .send()
            .await
            .map_err(GithubError::Request)?;
        decode_response(response).await
    }

    /// Signs a nine-minute JWT with a small clock-skew allowance.
    fn jwt(&self) -> Result<String, GithubError> {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        encode(
            &Header::new(Algorithm::RS256),
            &AppClaims {
                iat: now - 60,
                exp: now + 540,
                iss: self.app_id.to_string(),
            },
            &self.key,
        )
        .map_err(|_| GithubError::Jwt)
    }
}

/// Builds GitHub's versioned API headers without logging the bearer credential.
fn api_headers(token: &str) -> Result<reqwest::header::HeaderMap, GithubError> {
    use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue};
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(
        "x-github-api-version",
        HeaderValue::from_static(API_VERSION),
    );
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| GithubError::InvalidResponse)?,
    );
    Ok(headers)
}

/// Decodes only successful JSON responses and discards rejected response bodies.
async fn decode_response<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, GithubError> {
    if !response.status().is_success() {
        return Err(GithubError::Rejected(response.status()));
    }
    response
        .json()
        .await
        .map_err(|_| GithubError::InvalidResponse)
}
