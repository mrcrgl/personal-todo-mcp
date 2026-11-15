//! Process configuration. Deliberately not `Debug`: it holds secrets.

use url::Url;

const DEFAULT_GITHUB_API_URL: &str = "https://api.github.com";
const DEFAULT_GITHUB_WEB_URL: &str = "https://github.com";

pub struct Config {
    pub port: u16,
    /// Externally visible origin, without a trailing slash. Issuer, resource
    /// identifier, and redirect URIs are all derived from it.
    pub public_base_url: String,
    /// Hostnames accepted in the `Host` header by the MCP transport.
    pub allowed_hosts: Vec<String>,
    pub github_app_id: String,
    pub github_app_slug: String,
    pub github_client_id: String,
    pub github_client_secret: String,
    pub github_private_key_path: String,
    pub github_api_url: String,
    pub github_web_url: String,
    /// Base64url-encoded 32-byte keys. The first seals; all of them open.
    pub token_keys: Vec<String>,
    /// The operator's legal notice (Impressum), linked from every page.
    pub legal_notice_url: Option<String>,
    /// The operator's privacy policy, linked from every page.
    pub privacy_policy_url: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(&|k| std::env::var(k).ok())
    }

    /// Env access goes through a lookup fn so tests don't mutate process env.
    pub fn from_lookup(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let port = match get("PORT") {
            Some(p) => p
                .parse::<u16>()
                .map_err(|_| format!("PORT is not a valid port: {p}"))?,
            None => 10080,
        };
        let require = |k: &str, hint: &str| {
            get(k)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("{k} must be set ({hint})"))
        };

        let public_base_url = normalize_base_url(&require(
            "PUBLIC_BASE_URL",
            "the externally visible origin, e.g. https://todo.example.com",
        )?)?;
        let host = Url::parse(&public_base_url)
            .ok()
            .and_then(|url| {
                url.host_str().map(|host| match url.port() {
                    Some(port) => vec![host.to_string(), format!("{host}:{port}")],
                    None => vec![host.to_string()],
                })
            })
            .ok_or("PUBLIC_BASE_URL has no host")?;

        let token_keys: Vec<String> = require(
            "TOKEN_KEYS",
            "comma-separated base64url 32-byte keys, newest first",
        )?
        .split(',')
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(String::from)
        .collect();

        Ok(Self {
            port,
            public_base_url,
            allowed_hosts: host,
            github_app_id: require("GH_APP_ID", "numeric GitHub App id")?,
            github_app_slug: require("GH_APP_SLUG", "the App's URL name")?,
            github_client_id: require("GH_APP_CLIENT_ID", "the App's OAuth client id")?,
            github_client_secret: require("GH_APP_CLIENT_SECRET", "the App's OAuth client secret")?,
            github_private_key_path: require(
                "GH_APP_PRIVATE_KEY_PATH",
                "path to the App's private key .pem",
            )?,
            github_api_url: get("GH_API_URL")
                .unwrap_or_else(|| DEFAULT_GITHUB_API_URL.into())
                .trim_end_matches('/')
                .to_string(),
            github_web_url: get("GH_WEB_URL")
                .unwrap_or_else(|| DEFAULT_GITHUB_WEB_URL.into())
                .trim_end_matches('/')
                .to_string(),
            token_keys,
            legal_notice_url: optional_https_url("LEGAL_NOTICE_URL", get("LEGAL_NOTICE_URL"))?,
            privacy_policy_url: optional_https_url(
                "PRIVACY_POLICY_URL",
                get("PRIVACY_POLICY_URL"),
            )?,
        })
    }
}

/// An absolute `https` URL, or nothing when unset or empty.
fn optional_https_url(name: &str, value: Option<String>) -> Result<Option<String>, String> {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let url = Url::parse(&value).map_err(|e| format!("{name} is not a URL: {e}"))?;
    if url.scheme() != "https" {
        return Err(format!("{name} must be an https URL"));
    }
    Ok(Some(url.to_string()))
}

fn normalize_base_url(value: &str) -> Result<String, String> {
    let url = Url::parse(value).map_err(|e| format!("PUBLIC_BASE_URL is not a URL: {e}"))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err("PUBLIC_BASE_URL must use https outside of localhost".into());
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err("PUBLIC_BASE_URL must be an origin without a path".into());
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &[(&str, &str)] = &[
        ("PUBLIC_BASE_URL", "https://todo.example.com/"),
        ("GH_APP_ID", "12345"),
        ("GH_APP_SLUG", "personal-todos"),
        ("GH_APP_CLIENT_ID", "Iv1.abc"),
        ("GH_APP_CLIENT_SECRET", "secret"),
        ("GH_APP_PRIVATE_KEY_PATH", "/k.pem"),
        ("TOKEN_KEYS", "new, old"),
    ];

    fn config(pairs: &[(&str, &str)]) -> Result<Config, String> {
        Config::from_lookup(&|k| {
            pairs
                .iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| v.to_string())
        })
    }

    fn with(key: &'static str, value: &'static str) -> Vec<(&'static str, &'static str)> {
        let mut pairs: Vec<_> = BASE.iter().copied().filter(|(k, _)| *k != key).collect();
        pairs.push((key, value));
        pairs
    }

    #[test]
    fn defaults_point_at_github_and_derive_the_host() {
        let cfg = config(BASE).unwrap();
        assert_eq!(cfg.public_base_url, "https://todo.example.com");
        assert_eq!(cfg.allowed_hosts, ["todo.example.com"]);
        assert_eq!(cfg.github_api_url, "https://api.github.com");
        assert_eq!(cfg.token_keys, ["new", "old"]);
        assert_eq!(cfg.port, 10080);
    }

    #[test]
    fn missing_settings_name_the_variable() {
        let pairs: Vec<_> = BASE
            .iter()
            .copied()
            .filter(|(k, _)| *k != "GH_APP_SLUG")
            .collect();
        assert!(config(&pairs).err().unwrap().contains("GH_APP_SLUG"));
    }

    #[test]
    fn plain_http_is_only_accepted_on_loopback() {
        assert!(config(&with("PUBLIC_BASE_URL", "http://todo.example.com")).is_err());
        let local = config(&with("PUBLIC_BASE_URL", "http://localhost:10080")).unwrap();
        assert_eq!(local.public_base_url, "http://localhost:10080");
        assert_eq!(local.allowed_hosts, ["localhost", "localhost:10080"]);
    }

    #[test]
    fn legal_links_are_optional() {
        let cfg = config(BASE).unwrap();
        assert_eq!(cfg.legal_notice_url, None);
        assert_eq!(cfg.privacy_policy_url, None);
        let empty = config(&with("LEGAL_NOTICE_URL", "")).unwrap();
        assert_eq!(empty.legal_notice_url, None);
    }

    #[test]
    fn legal_links_are_read() {
        let mut pairs = with("LEGAL_NOTICE_URL", "https://example.com/imprint");
        pairs.push(("PRIVACY_POLICY_URL", "https://example.com/privacy"));
        let cfg = config(&pairs).unwrap();
        assert_eq!(
            cfg.legal_notice_url.as_deref(),
            Some("https://example.com/imprint")
        );
        assert_eq!(
            cfg.privacy_policy_url.as_deref(),
            Some("https://example.com/privacy")
        );
    }

    #[test]
    fn legal_links_must_be_absolute_https_urls() {
        for (key, value) in [
            ("LEGAL_NOTICE_URL", "http://example.com/imprint"),
            ("LEGAL_NOTICE_URL", "/imprint"),
            ("PRIVACY_POLICY_URL", "javascript:alert(1)"),
        ] {
            let error = config(&with(key, value)).err().unwrap();
            assert!(error.contains(key), "{error}");
        }
    }

    #[test]
    fn base_url_with_a_path_is_rejected() {
        assert!(config(&with("PUBLIC_BASE_URL", "https://example.com/todo")).is_err());
    }
}
