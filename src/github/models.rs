//! GitHub wire formats. Private to the adapter; `mapping` converts them to
//! domain types.

use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct IssueRow {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub state: String,
    #[serde(default)]
    pub state_reason: Option<String>,
    #[serde(default)]
    pub labels: Vec<LabelRow>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub html_url: String,
    /// Present only on pull requests, which GitHub serves from the same API.
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct LabelRow {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct CommentRow {
    pub user: Option<UserRow>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UserRow {
    pub id: u64,
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct InstallationTokenRow {
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct InstallationRepositoriesRow {
    pub total_count: u64,
    pub repositories: Vec<RepositoryRow>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepositoryRow {
    pub name: String,
    pub full_name: String,
    pub private: bool,
    pub html_url: String,
    pub owner: RepositoryOwnerRow,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepositoryOwnerRow {
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct UserInstallationsRow {
    pub installations: Vec<UserInstallationRow>,
}

#[derive(Debug, Deserialize)]
pub struct UserInstallationRow {
    pub id: u64,
    pub target_type: String,
    #[serde(default)]
    pub suspended_at: Option<String>,
    pub account: AccountRow,
}

#[derive(Debug, Deserialize)]
pub struct AccountRow {
    pub id: u64,
}

/// Body of `/login/oauth/access_token`, which reports failures as fields.
#[derive(Debug, Deserialize)]
pub struct OAuthTokenRow {
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}
