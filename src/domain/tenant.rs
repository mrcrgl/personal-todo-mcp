//! Who a request acts for, and what makes their repository usable.

use serde::{Deserialize, Serialize};

/// The identity every data access is scoped to. Only ever built from a
/// validated access token, never from tool arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tenant {
    pub user_id: u64,
    pub login: String,
    pub installation_id: u64,
}

/// Why an installation cannot be used as a todo repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryProblem {
    NoRepository,
    TooManyRepositories { count: u64 },
    PublicRepository { name: String },
}

impl std::fmt::Display for RepositoryProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRepository => write!(
                f,
                "The GitHub App has no repository selected. Grant it access to one private repository."
            ),
            Self::TooManyRepositories { count } => write!(
                f,
                "The GitHub App can access {count} repositories, but exactly one is required. \
                 Select only your dedicated todo repository."
            ),
            // Anyone can open issues on a public repository, so its "todos"
            // would be text written by strangers and read by the user's agent.
            Self::PublicRepository { name } => write!(
                f,
                "Repository {name} is public, so anyone could add todos to it. \
                 Make it private or select a private repository."
            ),
        }
    }
}
