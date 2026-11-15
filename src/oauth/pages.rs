//! The few HTML pages of the authorization flow. Every dynamic value passes
//! through `escape`: client names are chosen by whoever registers a client.

const STYLE: &str = "body{font:16px/1.5 system-ui,sans-serif;max-width:32rem;margin:4rem auto;\
padding:0 1rem;color:#1f2328}h1{font-size:1.4rem}.target{font-weight:600}\
.actions{display:flex;gap:.75rem;margin-top:1.5rem}\
button,a.button{font:inherit;padding:.5rem 1rem;border:1px solid #d1d9e0;border-radius:6px;\
background:#f6f8fa;color:inherit;text-decoration:none;cursor:pointer}\
button.primary,a.button.primary{background:#1f883d;border-color:#1f883d;color:#fff}\
footer{margin-top:3rem;font-size:.875rem;color:#59636e}footer a{color:inherit}";

/// Renders the pages. Holds what every page shows regardless of the flow:
/// the operator's legal links, which differ per deployment.
pub struct Pages {
    footer: String,
}

impl Pages {
    /// Links are shown only when the operator configured them.
    pub fn new(legal_notice_url: Option<&str>, privacy_policy_url: Option<&str>) -> Self {
        let links: Vec<String> = [
            (legal_notice_url, "Legal notice"),
            (privacy_policy_url, "Privacy policy"),
        ]
        .into_iter()
        .filter_map(|(url, label)| {
            url.map(|url| format!("<a href=\"{}\">{label}</a>", escape(url)))
        })
        .collect();
        let footer = if links.is_empty() {
            String::new()
        } else {
            format!("<footer>{}</footer>", links.join(" · "))
        };
        Self { footer }
    }

    fn layout(&self, title: &str, body: &str) -> String {
        format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
             <title>{title}</title><style>{STYLE}</style></head>\
             <body><h1>{title}</h1>{body}{footer}</body></html>",
            title = escape(title),
            footer = self.footer,
        )
    }

    pub fn consent(&self, client_name: Option<&str>, target: &str, consent_token: &str) -> String {
        let name = client_name
            .map(|name| format!("<p>The client calls itself “{}”.</p>", escape(name)))
            .unwrap_or_default();
        self.layout(
            "Connect your todos?",
            &format!(
                "<p><span class=\"target\">{target}</span> wants to read and change the todos in \
                 your GitHub todo repository.</p>{name}\
                 <p>Only continue if you started this yourself. You will sign in with GitHub next.</p>\
                 <form method=\"post\" action=\"/authorize/consent\">\
                 <input type=\"hidden\" name=\"consent\" value=\"{token}\">\
                 <div class=\"actions\">\
                 <button class=\"primary\" name=\"decision\" value=\"allow\">Continue with GitHub</button>\
                 <button name=\"decision\" value=\"deny\">Cancel</button>\
                 </div></form>",
                target = escape(target),
                token = escape(consent_token),
            ),
        )
    }

    pub fn problem(&self, message: &str) -> String {
        self.layout(
            "Connection failed",
            &format!(
                "<p>{}</p><p>Start again from your MCP client.</p>",
                escape(message)
            ),
        )
    }

    /// A step the user has to take on GitHub before the flow can continue.
    pub fn blocked(
        &self,
        message: &str,
        action_url: &str,
        action_label: &str,
        retry_url: &str,
    ) -> String {
        self.layout(
            "One more step",
            &format!(
                "<p>{}</p><div class=\"actions\">\
                 <a class=\"button primary\" href=\"{}\" target=\"_blank\" rel=\"noopener\">{}</a>\
                 <a class=\"button\" href=\"{}\">Check again</a></div>",
                escape(message),
                escape(action_url),
                escape(action_label),
                escape(retry_url),
            ),
        )
    }

    pub fn installed(&self) -> String {
        self.layout(
            "GitHub App installed",
            "<p>Now add this server in your MCP client to connect it to your todo repository.</p>",
        )
    }

    pub fn landing(&self, mcp_url: &str) -> String {
        self.layout(
            "Personal Todo MCP",
            &format!(
                "<p>Manage your todos from any MCP client. They stay in a private GitHub repository \
                 you own.</p><p>Add this server URL to your MCP client:</p><p><code>{}</code></p>\
                 <p><a href=\"https://github.com/mrcrgl/personal-todo-mcp\">Source and documentation</a></p>",
                escape(mcp_url),
            ),
        )
    }
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages() -> Pages {
        Pages::new(None, None)
    }

    #[test]
    fn client_controlled_text_is_escaped() {
        let page = pages().consent(
            Some("<script>alert(1)</script>"),
            "evil.example\"><img src=x>",
            "tok",
        );
        assert!(!page.contains("<script>"));
        assert!(!page.contains("<img"));
        assert!(page.contains("&lt;script&gt;"));
    }

    #[test]
    fn consent_names_the_target() {
        let page = pages().consent(None, "app.example.com", "tok");
        assert!(page.contains("app.example.com"));
        assert!(page.contains("value=\"tok\""));
    }

    #[test]
    fn blocked_links_are_escaped_into_attributes() {
        let page = pages().blocked("m", "https://x/?a=1&b=2", "Fix", "/retry");
        assert!(page.contains("https://x/?a=1&amp;b=2"));
    }

    #[test]
    fn without_legal_links_there_is_no_footer() {
        assert!(!pages().landing("https://t/mcp").contains("<footer>"));
    }

    #[test]
    fn configured_legal_links_appear_on_every_page() {
        let pages = Pages::new(
            Some("https://example.com/imprint"),
            Some("https://example.com/privacy"),
        );
        for page in [
            pages.landing("https://t/mcp"),
            pages.consent(None, "t", "tok"),
            pages.problem("m"),
            pages.blocked("m", "https://x", "Fix", "/retry"),
            pages.installed(),
        ] {
            assert!(page.contains("<a href=\"https://example.com/imprint\">Legal notice</a>"));
            assert!(page.contains("<a href=\"https://example.com/privacy\">Privacy policy</a>"));
        }
    }

    #[test]
    fn a_single_legal_link_is_shown_alone() {
        let page = Pages::new(None, Some("https://example.com/privacy")).installed();
        assert!(page.contains("Privacy policy"));
        assert!(!page.contains("Legal notice"));
    }

    #[test]
    fn legal_links_are_escaped_into_attributes() {
        let page = Pages::new(Some("https://example.com/?a=1&b=\"x\""), None).installed();
        assert!(page.contains("href=\"https://example.com/?a=1&amp;b=&quot;x&quot;\""));
    }
}
