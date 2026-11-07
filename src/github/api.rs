//! Thin REST client shared by the adapter's components.

use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::todos::store::StoreError;

/// GitHub rejects requests without a User-Agent.
const USER_AGENT: &str = "personal-todo-mcp";
const API_VERSION: &str = "2022-11-28";

#[derive(Clone)]
pub struct GitHubApi {
    http: reqwest::Client,
    base_url: String,
}

impl GitHubApi {
    pub fn new(base_url: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url,
        }
    }

    pub async fn get<T: DeserializeOwned>(
        &self,
        token: &str,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<T, StoreError> {
        let response = self.send(Method::GET, token, segments, query, None).await?;
        decode(response).await
    }

    pub async fn post<T: DeserializeOwned>(
        &self,
        token: &str,
        segments: &[&str],
        body: &Value,
    ) -> Result<T, StoreError> {
        let response = self
            .send(Method::POST, token, segments, &[], Some(body))
            .await?;
        decode(response).await
    }

    pub async fn patch<T: DeserializeOwned>(
        &self,
        token: &str,
        segments: &[&str],
        body: &Value,
    ) -> Result<T, StoreError> {
        let response = self
            .send(Method::PATCH, token, segments, &[], Some(body))
            .await?;
        decode(response).await
    }

    pub async fn put<T: DeserializeOwned>(
        &self,
        token: &str,
        segments: &[&str],
        body: &Value,
    ) -> Result<T, StoreError> {
        let response = self
            .send(Method::PUT, token, segments, &[], Some(body))
            .await?;
        decode(response).await
    }

    pub async fn delete(&self, token: &str, segments: &[&str]) -> Result<(), StoreError> {
        self.send(Method::DELETE, token, segments, &[], None)
            .await
            .map(drop)
    }

    /// For deletions GitHub needs details for, such as a file's `sha`.
    pub async fn delete_with_body(
        &self,
        token: &str,
        segments: &[&str],
        body: &Value,
    ) -> Result<(), StoreError> {
        self.send(Method::DELETE, token, segments, &[], Some(body))
            .await
            .map(drop)
    }

    /// Like `post`, but an "already exists" validation failure counts as
    /// success, which makes creating a label idempotent.
    pub async fn post_idempotent(
        &self,
        token: &str,
        segments: &[&str],
        body: &Value,
    ) -> Result<(), StoreError> {
        match self
            .send(Method::POST, token, segments, &[], Some(body))
            .await
        {
            Ok(_) => Ok(()),
            Err(StoreError::Rejected { status: 422, .. }) => Ok(()),
            Err(other) => Err(other),
        }
    }

    async fn send(
        &self,
        method: Method,
        token: &str,
        segments: &[&str],
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Response, StoreError> {
        let mut url = url::Url::parse(&self.base_url)
            .map_err(|e| StoreError::Unavailable(format!("invalid GitHub API URL: {e}")))?;
        url.path_segments_mut()
            .map_err(|()| StoreError::Unavailable("invalid GitHub API URL".into()))?
            .pop_if_empty()
            .extend(segments);
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
        }

        let mut request = decorate(self.http.request(method, url)).bearer_auth(token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(transport_error)?;
        check_status(response).await
    }
}

/// Headers every GitHub call carries.
pub fn decorate(request: RequestBuilder) -> RequestBuilder {
    request
        .header("accept", "application/vnd.github+json")
        .header("x-github-api-version", API_VERSION)
        .header("user-agent", USER_AGENT)
}

/// `without_url` because a request URL can carry a code or token in its query.
pub fn transport_error(error: reqwest::Error) -> StoreError {
    StoreError::Unavailable(error.without_url().to_string())
}

async fn check_status(response: Response) -> Result<Response, StoreError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    if is_rate_limited(&response) {
        return Err(StoreError::RateLimited);
    }
    if status == StatusCode::UNAUTHORIZED {
        return Err(StoreError::InstallationGone);
    }
    let body: Value = response.json().await.unwrap_or(Value::Null);
    let message = body["message"]
        .as_str()
        .unwrap_or("no details given")
        .to_string();
    Err(StoreError::Rejected {
        status: status.as_u16(),
        message,
    })
}

fn is_rate_limited(response: &Response) -> bool {
    let status = response.status();
    let exhausted = response
        .headers()
        .get("x-ratelimit-remaining")
        .is_some_and(|remaining| remaining == "0");
    status == StatusCode::TOO_MANY_REQUESTS
        || (status == StatusCode::FORBIDDEN
            && (exhausted || response.headers().contains_key("retry-after")))
}

async fn decode<T: DeserializeOwned>(response: Response) -> Result<T, StoreError> {
    response
        .json()
        .await
        .map_err(|e| StoreError::Unavailable(format!("unreadable GitHub response: {e}")))
}
