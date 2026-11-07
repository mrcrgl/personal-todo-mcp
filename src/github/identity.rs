//! `IdentityProvider` on GitHub: user sign-in, installation lookup, and the
//! repository check.

use std::sync::Arc;

use url::Url;

use crate::domain::tenant::Tenant;
use crate::github::api::GitHubApi;
use crate::github::installation::{InstallationAccessProvider, Scope};
use crate::github::models::{OAuthTokenRow, UserInstallationRow, UserInstallationsRow, UserRow};
use crate::oauth::identity::{
    BoxFut, GrantedTokens, IdentityError, IdentityProvider, IdentityUser,
};
use crate::todos::store::StoreError;

const USER_AGENT: &str = "personal-todo-mcp";
const INSTALLATIONS_PAGE_SIZE: &str = "100";

pub struct GitHubIdentity {
    http: reqwest::Client,
    api: GitHubApi,
    web_url: String,
    app_slug: String,
    client_id: String,
    client_secret: String,
    installations: Arc<InstallationAccessProvider>,
}

impl GitHubIdentity {
    pub fn new(
        api_url: String,
        web_url: String,
        app_slug: String,
        client_id: String,
        client_secret: String,
        installations: Arc<InstallationAccessProvider>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            api: GitHubApi::new(api_url),
            web_url,
            app_slug,
            client_id,
            client_secret,
            installations,
        }
    }

    async fn token_request(&self, params: &[(&str, &str)]) -> Result<GrantedTokens, IdentityError> {
        let url = format!("{}/login/oauth/access_token", self.web_url);
        let response = self
            .http
            .post(url)
            .header("accept", "application/json")
            .header("user-agent", USER_AGENT)
            .form(params)
            .send()
            .await
            .map_err(|e| IdentityError::Unavailable(e.without_url().to_string()))?;
        if response.status().is_server_error() {
            return Err(IdentityError::Unavailable(format!(
                "GitHub answered {}",
                response.status()
            )));
        }
        let row: OAuthTokenRow = response
            .json()
            .await
            .map_err(|e| IdentityError::Unavailable(format!("unreadable token response: {e}")))?;
        tokens_from_row(row)
    }
}

/// GitHub's token endpoint can report failure in the body of a 200, so the
/// fields decide the outcome rather than the status.
fn tokens_from_row(row: OAuthTokenRow) -> Result<GrantedTokens, IdentityError> {
    if let Some(error) = row.error {
        return Err(match error.as_str() {
            "bad_verification_code" | "bad_refresh_token" => IdentityError::Rejected,
            _ => IdentityError::Unavailable(format!(
                "{error}: {}",
                row.error_description.unwrap_or_default()
            )),
        });
    }
    let access_token = row
        .access_token
        .ok_or_else(|| IdentityError::Unavailable("token response without a token".into()))?;
    let refresh_token = row.refresh_token.ok_or_else(|| {
        IdentityError::Unavailable(
            "GitHub issued no refresh token: enable user-to-server token expiration on the App"
                .into(),
        )
    })?;
    Ok(GrantedTokens {
        access_token,
        refresh_token,
    })
}

/// Only an installation on the user's own account counts. An organization
/// installation is excluded because its token could exceed what the user may
/// do in that organization's repository.
fn personal_installation(rows: &[UserInstallationRow], user_id: u64) -> Option<u64> {
    rows.iter()
        .find(|row| {
            row.target_type == "User" && row.account.id == user_id && row.suspended_at.is_none()
        })
        .map(|row| row.id)
}

fn identity_error(error: StoreError) -> IdentityError {
    match error {
        StoreError::InstallationGone => IdentityError::Rejected,
        other => IdentityError::Unavailable(other.to_string()),
    }
}

impl IdentityProvider for GitHubIdentity {
    fn authorize_url(&self, state: &str, redirect_uri: &str) -> String {
        let mut url = Url::parse(&format!("{}/login/oauth/authorize", self.web_url))
            .expect("GitHub web URL is validated at startup");
        url.query_pairs_mut()
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("state", state)
            .append_pair("scope", "offline_access");
        url.into()
    }

    fn install_url(&self) -> String {
        format!("{}/apps/{}/installations/new", self.web_url, self.app_slug)
    }

    fn installation_settings_url(&self, installation_id: u64) -> String {
        format!("{}/settings/installations/{installation_id}", self.web_url)
    }

    fn exchange_code<'a>(
        &'a self,
        code: &'a str,
        redirect_uri: &'a str,
    ) -> BoxFut<'a, GrantedTokens> {
        Box::pin(self.token_request_for_code(code, redirect_uri))
    }

    fn refresh<'a>(&'a self, refresh_token: &'a str) -> BoxFut<'a, GrantedTokens> {
        Box::pin(self.token_request_for_refresh(refresh_token))
    }

    fn user<'a>(&'a self, access_token: &'a str) -> BoxFut<'a, IdentityUser> {
        Box::pin(async move {
            let row: UserRow = self
                .api
                .get(access_token, &["user"], &[])
                .await
                .map_err(identity_error)?;
            Ok(IdentityUser {
                id: row.id,
                login: row.login,
            })
        })
    }

    fn personal_installation<'a>(
        &'a self,
        access_token: &'a str,
        user_id: u64,
    ) -> BoxFut<'a, Option<u64>> {
        Box::pin(async move {
            let row: UserInstallationsRow = self
                .api
                .get(
                    access_token,
                    &["user", "installations"],
                    &[("per_page", INSTALLATIONS_PAGE_SIZE.to_string())],
                )
                .await
                .map_err(identity_error)?;
            Ok(personal_installation(&row.installations, user_id))
        })
    }

    fn check_repository<'a>(&'a self, tenant: &'a Tenant) -> BoxFut<'a, ()> {
        // Checked with the todo scope: signing in must not depend on a
        // permission only the file tools need.
        Box::pin(async move {
            self.installations
                .access(tenant.installation_id, Scope::Issues)
                .await
                .map(drop)
                .map_err(|error| match error {
                    StoreError::Repository(problem) => IdentityError::Repository(problem),
                    other => identity_error(other),
                })
        })
    }
}

impl GitHubIdentity {
    async fn token_request_for_code(
        &self,
        code: &str,
        redirect_uri: &str,
    ) -> Result<GrantedTokens, IdentityError> {
        self.token_request(&[
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("code", code),
            ("redirect_uri", redirect_uri),
        ])
        .await
    }

    async fn token_request_for_refresh(
        &self,
        refresh_token: &str,
    ) -> Result<GrantedTokens, IdentityError> {
        self.token_request(&[
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::models::AccountRow;

    fn installation(id: u64, target_type: &str, account: u64) -> UserInstallationRow {
        UserInstallationRow {
            id,
            target_type: target_type.into(),
            suspended_at: None,
            account: AccountRow { id: account },
        }
    }

    #[test]
    fn only_the_users_own_account_qualifies() {
        let rows = [
            installation(1, "Organization", 500),
            installation(2, "User", 99),
            installation(3, "User", 42),
        ];
        assert_eq!(personal_installation(&rows, 42), Some(3));
        assert_eq!(personal_installation(&rows[..2], 42), None);
    }

    #[test]
    fn suspended_installations_do_not_qualify() {
        let mut row = installation(3, "User", 42);
        row.suspended_at = Some("2026-10-30T00:00:00Z".into());
        assert_eq!(personal_installation(&[row], 42), None);
    }

    #[test]
    fn error_in_a_success_response_is_still_a_failure() {
        let row = OAuthTokenRow {
            access_token: None,
            refresh_token: None,
            error: Some("bad_refresh_token".into()),
            error_description: None,
        };
        assert!(matches!(tokens_from_row(row), Err(IdentityError::Rejected)));
    }

    #[test]
    fn missing_refresh_token_explains_the_app_setting() {
        let row = OAuthTokenRow {
            access_token: Some("ghu_x".into()),
            refresh_token: None,
            error: None,
            error_description: None,
        };
        let Err(IdentityError::Unavailable(message)) = tokens_from_row(row) else {
            panic!("expected a configuration error");
        };
        assert!(message.contains("token expiration"));
    }
}
