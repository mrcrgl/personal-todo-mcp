//! Per-tenant GitHub access: App JWT → installation token + the one repository
//! that installation may use.
//!
//! The cache TTL bounds how long a change to the installation goes unnoticed.
//! It is far shorter than the token's one-hour lifetime on purpose: repository
//! selection is re-validated on every refill.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use moka::future::Cache;
use reqwest::Method;
use serde_json::json;

use crate::domain::tenant::RepositoryProblem;
use crate::github::api::{GitHubApi, decorate, transport_error};
use crate::github::models::{InstallationRepositoriesRow, InstallationTokenRow, RepositoryRow};
use crate::todos::store::StoreError;

const CACHE_TTL: Duration = Duration::from_secs(600);
const CACHE_CAPACITY: u64 = 10_000;

#[derive(serde::Serialize)]
struct AppClaims {
    iat: u64,
    exp: u64,
    iss: String,
}

/// What a tenant's requests run with. Holds a live token, hence no `Debug`.
pub struct InstallationAccess {
    pub token: String,
    pub repository: RepositoryRow,
}

pub struct InstallationAccessProvider {
    api: GitHubApi,
    api_url: String,
    http: reqwest::Client,
    app_id: String,
    key: jsonwebtoken::EncodingKey,
    cache: Cache<u64, Arc<InstallationAccess>>,
}

impl InstallationAccessProvider {
    pub fn new(api_url: String, app_id: String, private_key_pem: &str) -> Result<Self, String> {
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(private_key_pem.as_bytes())
            .map_err(|e| format!("GitHub App private key unusable: {e}"))?;
        Ok(Self {
            api: GitHubApi::new(api_url.clone()),
            api_url,
            http: reqwest::Client::new(),
            app_id,
            key,
            cache: Cache::builder()
                .max_capacity(CACHE_CAPACITY)
                .time_to_live(CACHE_TTL)
                .build(),
        })
    }

    /// Cached per installation; concurrent misses share one refill.
    pub async fn access(
        &self,
        installation_id: u64,
    ) -> Result<Arc<InstallationAccess>, StoreError> {
        self.cache
            .try_get_with(installation_id, self.fetch(installation_id))
            .await
            .map_err(|shared: Arc<StoreError>| clone_error(&shared))
    }

    /// Drops a cached entry GitHub has just refused.
    pub async fn invalidate(&self, installation_id: u64) {
        self.cache.invalidate(&installation_id).await;
    }

    async fn fetch(&self, installation_id: u64) -> Result<Arc<InstallationAccess>, StoreError> {
        let token = self.mint_token(installation_id).await?;
        let listed: InstallationRepositoriesRow = self
            .api
            .get(
                &token,
                &["installation", "repositories"],
                &[("per_page", "2".to_string())],
            )
            .await?;
        let repository = single_private_repository(listed).map_err(StoreError::Repository)?;
        Ok(Arc::new(InstallationAccess { token, repository }))
    }

    async fn mint_token(&self, installation_id: u64) -> Result<String, StoreError> {
        let jwt = build_app_jwt(&self.app_id, &self.key, unix_now())?;
        let url = format!(
            "{}/app/installations/{installation_id}/access_tokens",
            self.api_url
        );
        // Asks for less than the App may hold: a leaked token can do no more
        // than what the todo tools need.
        let response = decorate(self.http.request(Method::POST, url))
            .bearer_auth(jwt)
            .json(&json!({"permissions": {"issues": "write", "metadata": "read"}}))
            .send()
            .await
            .map_err(transport_error)?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(StoreError::InstallationGone);
        }
        if !status.is_success() {
            return Err(StoreError::Rejected {
                status: status.as_u16(),
                message: "installation token exchange failed".into(),
            });
        }
        let row: InstallationTokenRow = response
            .json()
            .await
            .map_err(|e| StoreError::Unavailable(format!("unreadable token response: {e}")))?;
        Ok(row.token)
    }
}

fn single_private_repository(
    listed: InstallationRepositoriesRow,
) -> Result<RepositoryRow, RepositoryProblem> {
    if listed.total_count > 1 {
        return Err(RepositoryProblem::TooManyRepositories {
            count: listed.total_count,
        });
    }
    let repository = listed
        .repositories
        .into_iter()
        .next()
        .ok_or(RepositoryProblem::NoRepository)?;
    if !repository.private {
        return Err(RepositoryProblem::PublicRepository {
            name: repository.full_name,
        });
    }
    Ok(repository)
}

/// `try_get_with` shares one error across waiters, so it has to be rebuilt.
fn clone_error(error: &StoreError) -> StoreError {
    match error {
        StoreError::NotFound(id) => StoreError::NotFound(*id),
        StoreError::InstallationGone => StoreError::InstallationGone,
        StoreError::Repository(problem) => StoreError::Repository(problem.clone()),
        StoreError::RateLimited => StoreError::RateLimited,
        StoreError::Rejected { status, message } => StoreError::Rejected {
            status: *status,
            message: message.clone(),
        },
        StoreError::Unavailable(message) => StoreError::Unavailable(message.clone()),
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before 1970")
        .as_secs()
}

/// App JWT per GitHub's spec: iat 60s in the past (clock drift), exp max
/// 10 minutes out, iss = app id.
fn build_app_jwt(
    app_id: &str,
    key: &jsonwebtoken::EncodingKey,
    now: u64,
) -> Result<String, StoreError> {
    let claims = AppClaims {
        iat: now.saturating_sub(60),
        exp: now + 540,
        iss: app_id.to_string(),
    };
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        &claims,
        key,
    )
    .map_err(|e| StoreError::Unavailable(format!("App JWT signing failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::models::RepositoryOwnerRow;
    use base64::Engine as _;

    // Throwaway RSA key for tests only. Regenerate: openssl genrsa 2048
    const TEST_KEY_PEM: &str = include_str!("../../tests/fixtures/test-rsa.pem");

    fn repository(private: bool) -> RepositoryRow {
        RepositoryRow {
            name: "todos".into(),
            full_name: "alice/todos".into(),
            private,
            html_url: "https://github.test/alice/todos".into(),
            owner: RepositoryOwnerRow {
                login: "alice".into(),
            },
        }
    }

    fn listed(total_count: u64, repositories: Vec<RepositoryRow>) -> InstallationRepositoriesRow {
        InstallationRepositoriesRow {
            total_count,
            repositories,
        }
    }

    #[test]
    fn app_jwt_has_github_shaped_claims() {
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(TEST_KEY_PEM.as_bytes()).unwrap();
        let jwt = build_app_jwt("12345", &key, 1_700_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let claims: serde_json::Value =
            serde_json::from_slice(&b64.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["iss"], "12345");
        assert_eq!(claims["iat"], 1_700_000_000u64 - 60);
        assert_eq!(claims["exp"], 1_700_000_000u64 + 540);
    }

    #[test]
    fn bad_key_is_a_clear_error() {
        let err = InstallationAccessProvider::new("https://x".into(), "1".into(), "not a pem")
            .err()
            .unwrap();
        assert!(err.contains("private key"));
    }

    #[test]
    fn exactly_one_private_repository_is_accepted() {
        assert!(single_private_repository(listed(1, vec![repository(true)])).is_ok());
    }

    #[test]
    fn other_repository_selections_are_refused() {
        assert_eq!(
            single_private_repository(listed(0, vec![])).err(),
            Some(RepositoryProblem::NoRepository)
        );
        assert_eq!(
            single_private_repository(listed(3, vec![repository(true)])).err(),
            Some(RepositoryProblem::TooManyRepositories { count: 3 })
        );
        assert_eq!(
            single_private_repository(listed(1, vec![repository(false)])).err(),
            Some(RepositoryProblem::PublicRepository {
                name: "alice/todos".into()
            })
        );
    }
}
