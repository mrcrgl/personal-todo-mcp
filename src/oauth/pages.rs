//! The few HTML pages of the authorization flow. Every dynamic value passes
//! through `escape`: client names are chosen by whoever registers a client.

const STYLE: &str = "body{font:16px/1.5 system-ui,sans-serif;max-width:32rem;margin:4rem auto;\
padding:0 1rem;color:#1f2328}h1{font-size:1.4rem}.target{font-weight:600}\
.actions{display:flex;gap:.75rem;margin-top:1.5rem}\
button,a.button{font:inherit;padding:.5rem 1rem;border:1px solid #d1d9e0;border-radius:6px;\
background:#f6f8fa;color:inherit;text-decoration:none;cursor:pointer}\
button.primary,a.button.primary{background:#1f883d;border-color:#1f883d;color:#fff}";

fn layout(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{title}</title><style>{STYLE}</style></head><body><h1>{title}</h1>{body}</body></html>",
        title = escape(title),
    )
}

pub fn consent(client_name: Option<&str>, target: &str, consent_token: &str) -> String {
    let name = client_name
        .map(|name| format!("<p>The client calls itself “{}”.</p>", escape(name)))
        .unwrap_or_default();
    layout(
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

pub fn problem(message: &str) -> String {
    layout(
        "Connection failed",
        &format!(
            "<p>{}</p><p>Start again from your MCP client.</p>",
            escape(message)
        ),
    )
}

/// A step the user has to take on GitHub before the flow can continue.
pub fn blocked(message: &str, action_url: &str, action_label: &str, retry_url: &str) -> String {
    layout(
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

pub fn installed() -> String {
    layout(
        "GitHub App installed",
        "<p>Now add this server in your MCP client to connect it to your todo repository.</p>",
    )
}

pub fn landing(mcp_url: &str) -> String {
    layout(
        "Personal Todo MCP",
        &format!(
            "<p>Manage your todos from any MCP client. They stay in a private GitHub repository \
             you own.</p><p>Add this server URL to your MCP client:</p><p><code>{}</code></p>\
             <p><a href=\"https://github.com/mrcrgl/personal-todo-mcp\">Source and documentation</a></p>",
            escape(mcp_url),
        ),
    )
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

    #[test]
    fn client_controlled_text_is_escaped() {
        let page = consent(
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
        let page = consent(None, "app.example.com", "tok");
        assert!(page.contains("app.example.com"));
        assert!(page.contains("value=\"tok\""));
    }

    #[test]
    fn blocked_links_are_escaped_into_attributes() {
        let page = blocked("m", "https://x/?a=1&b=2", "Fix", "/retry");
        assert!(page.contains("https://x/?a=1&amp;b=2"));
    }
}
