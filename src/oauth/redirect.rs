//! Rules for client redirect URIs.

use url::{Host, Url};

/// Schemes that execute or read locally instead of handing off to an app.
const FORBIDDEN_SCHEMES: &[&str] = &[
    "about",
    "blob",
    "data",
    "file",
    "ftp",
    "javascript",
    "vbscript",
    "ws",
    "wss",
];

/// Accepts `https`, plain `http` on loopback (RFC 8252 §7.3), and the private
/// schemes native clients register (RFC 8252 §7.1).
pub fn is_registrable(uri: &str) -> bool {
    let Ok(url) = Url::parse(uri) else {
        return false;
    };
    if url.fragment().is_some() || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    match url.scheme() {
        "https" => url.host().is_some(),
        "http" => is_loopback(&url),
        scheme => !FORBIDDEN_SCHEMES.contains(&scheme),
    }
}

/// Exact match, except that a loopback port may differ: native clients bind an
/// ephemeral port at request time (RFC 8252 §7.3).
pub fn matches_registered(registered: &str, requested: &str) -> bool {
    if registered == requested {
        return true;
    }
    let (Ok(mut registered), Ok(mut requested)) = (Url::parse(registered), Url::parse(requested))
    else {
        return false;
    };
    if registered.scheme() != "http" || !is_loopback(&registered) || !is_loopback(&requested) {
        return false;
    }
    if registered.set_port(None).is_err() || requested.set_port(None).is_err() {
        return false;
    }
    registered == requested
}

/// Who the user is authorizing, for the consent page. Derived from the
/// redirect URI because that is where tokens go, unlike the self-declared name.
pub fn display_target(uri: &str) -> String {
    let Ok(url) = Url::parse(uri) else {
        return uri.to_string();
    };
    if is_loopback(&url) {
        return "an app on this computer".to_string();
    }
    match (url.scheme(), url.host_str()) {
        ("https", Some(host)) => host.to_string(),
        (scheme, Some(host)) => format!("{scheme}://{host}"),
        (scheme, None) => format!("{scheme}:"),
    }
}

/// Adds query parameters to a redirect URI.
pub fn with_params(uri: &str, params: &[(&str, &str)]) -> Result<String, url::ParseError> {
    let mut url = Url::parse(uri)?;
    url.query_pairs_mut().extend_pairs(params);
    Ok(url.into())
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain == "localhost",
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_loopback_and_private_schemes_can_register() {
        for uri in [
            "https://app.example.com/callback",
            "http://localhost:33418/callback",
            "http://127.0.0.1/callback",
            "http://[::1]:8080/callback",
            "cursor://oauth/callback",
            "com.example.app:/callback",
        ] {
            assert!(is_registrable(uri), "{uri}");
        }
    }

    #[test]
    fn unsafe_redirects_cannot_register() {
        for uri in [
            "http://app.example.com/callback",
            "javascript:alert(1)",
            "data:text/html,x",
            "file:///etc/passwd",
            "https://app.example.com/callback#frag",
            "https://user:pw@app.example.com/callback",
            "not a uri",
            "",
        ] {
            assert!(!is_registrable(uri), "{uri}");
        }
    }

    #[test]
    fn only_loopback_may_vary_its_port() {
        assert!(matches_registered(
            "http://localhost:1000/cb",
            "http://localhost:2000/cb"
        ));
        assert!(!matches_registered(
            "http://localhost:1000/cb",
            "http://localhost:2000/other"
        ));
        assert!(!matches_registered(
            "https://app.example.com:1000/cb",
            "https://app.example.com:2000/cb"
        ));
        assert!(!matches_registered(
            "https://app.example.com/cb",
            "https://evil.example.com/cb"
        ));
    }

    #[test]
    fn a_loopback_registration_does_not_cover_other_hosts() {
        assert!(!matches_registered(
            "http://localhost:1000/cb",
            "http://evil.example.com:1000/cb"
        ));
    }

    #[test]
    fn consent_target_names_where_the_token_goes() {
        assert_eq!(
            display_target("https://app.example.com/cb"),
            "app.example.com"
        );
        assert_eq!(
            display_target("http://localhost:1/cb"),
            "an app on this computer"
        );
        assert_eq!(display_target("cursor://oauth/cb"), "cursor://oauth");
        assert_eq!(display_target("com.example.app:/cb"), "com.example.app:");
    }

    #[test]
    fn params_keep_the_existing_query() {
        let url = with_params("https://a.example/cb?x=1", &[("code", "a b")]).unwrap();
        assert_eq!(url, "https://a.example/cb?x=1&code=a+b");
    }
}
