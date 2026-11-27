//! HTML pages for errors the proxy generates itself.
//!
//! [`ErrorPageTemplate`] renders `400`, `403`, `502`, `504`, and the other statuses the
//! engine answers with when it refuses or cannot complete a request. It is never applied to
//! responses that come from an upstream origin server: those are streamed back unchanged.
//!
//! A template is plain HTML with four placeholders, each replaced by an HTML-escaped value:
//!
//! * `{{status}}`: the numeric status code (`502`);
//! * `{{reason}}`: the canonical reason phrase (`Bad Gateway`);
//! * `{{message}}`: the proxy's short explanation (`upstream connection failed`);
//! * `{{target}}`: the requested `host` or `host:port`, or empty when unknown.
//!
//! Placeholders are replaced in a single pass, so a value that itself contains `{{...}}` is
//! never expanded again.

use std::fmt;
use std::io;
use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{HeaderValue, CACHE_CONTROL, CONTENT_TYPE};
use hyper::{Response, StatusCode};

use super::ProxyBody;

/// The built-in page: unbranded, no external resources, no scripts, light and dark styles.
/// An empty target hides its line (`small:empty`).
const BUILTIN: &str = concat!(
    "<!DOCTYPE html><html><head><meta charset=\"utf-8\">",
    "<meta name=\"viewport\" content=\"width=device-width\">",
    "<title>{{status}} {{reason}}</title><style>",
    "body{margin:0;min-height:100vh;display:grid;place-items:center;",
    "font:15px/1.5 system-ui;background:#eee;color:#222}",
    "main{max-width:28em;margin:1em;padding:2em;border-radius:8px;background:#fff}",
    "h1{margin:0 0 .5em;font-size:1.2em}p{margin:0}",
    "small{display:block;margin-top:1em;color:#777;word-break:break-all}",
    "small:empty{display:none}",
    "@media(prefers-color-scheme:dark){body{background:#111;color:#ddd}",
    "main{background:#2a2a2a}small{color:#999}}",
    "</style></head><body><main><h1>{{status}} {{reason}}</h1><p>{{message}}</p>",
    "<small>{{target}}</small></main></body></html>",
);

/// An HTML template for proxy-generated error responses. Cheap to clone.
///
/// [`Default`] is the built-in page ([`ErrorPageTemplate::builtin`]).
#[derive(Clone)]
pub struct ErrorPageTemplate {
    html: Arc<String>,
}

impl Default for ErrorPageTemplate {
    fn default() -> Self {
        Self::builtin()
    }
}

impl fmt::Debug for ErrorPageTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ErrorPageTemplate")
            .field("builtin", &self.is_builtin())
            .field("len", &self.html.len())
            .finish()
    }
}

impl ErrorPageTemplate {
    /// The built-in, unbranded page.
    pub fn builtin() -> Self {
        Self::from_template(BUILTIN)
    }

    /// A custom template (see the module docs for its placeholders).
    pub fn from_template(html: impl Into<String>) -> Self {
        Self {
            html: Arc::new(html.into()),
        }
    }

    /// A custom template read from a UTF-8 file.
    pub fn from_file(path: &Path) -> io::Result<Self> {
        std::fs::read_to_string(path).map(Self::from_template)
    }

    /// Whether this is the built-in page.
    pub fn is_builtin(&self) -> bool {
        self.html.as_str() == BUILTIN
    }

    /// The raw template text.
    pub fn as_str(&self) -> &str {
        &self.html
    }

    /// The page for `status`, with every placeholder replaced by an HTML-escaped value.
    pub fn render(&self, status: StatusCode, message: &str, target: Option<&str>) -> String {
        let code = status.as_str();
        let reason = status.canonical_reason().unwrap_or("Error");
        let target = target.unwrap_or("");
        let mut out = String::with_capacity(self.html.len() + message.len() + target.len());
        let mut rest = self.html.as_str();
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let tail = &rest[start..];
            let value = [
                ("{{status}}", code),
                ("{{reason}}", reason),
                ("{{message}}", message),
                ("{{target}}", target),
            ]
            .into_iter()
            .find(|(name, _)| tail.starts_with(name));
            match value {
                Some((name, value)) => {
                    escape_into(&mut out, value);
                    rest = &tail[name.len()..];
                }
                None => {
                    out.push_str("{{");
                    rest = &tail[2..];
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// A complete `text/html` response for `status`, marked `Cache-Control: no-store`.
    pub fn response(
        &self,
        status: StatusCode,
        message: &str,
        target: Option<&str>,
    ) -> Response<ProxyBody> {
        let body = Full::new(Bytes::from(self.render(status, message, target)))
            .map_err(|never| match never {})
            .boxed();
        let mut resp = Response::new(body);
        *resp.status_mut() = status;
        let headers = resp.headers_mut();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
        resp
    }
}

/// Append `s` to `out`, escaping `&`, `<`, `>`, `"`, and `'`.
fn escape_into(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

/// `host[:port]` of an authority with any `userinfo@` removed, so credentials never reach a
/// page.
pub(crate) fn page_target(authority: &str) -> Option<&str> {
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    (!host.is_empty()).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn body_of(resp: Response<ProxyBody>) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn builtin_is_small_unbranded_and_self_contained() {
        let html = ErrorPageTemplate::builtin().as_str().to_string();
        assert!(
            (500..=720).contains(&html.len()),
            "built-in page is {} bytes",
            html.len()
        );
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("prefers-color-scheme:dark"));
        let lower = html.to_ascii_lowercase();
        for banned in [
            "<script", "http://", "https://", "src=", "href=", "@import", "conduit",
        ] {
            assert!(!lower.contains(banned), "built-in page contains {banned}");
        }
        assert!(ErrorPageTemplate::default().is_builtin());
        assert!(!ErrorPageTemplate::from_template("x").is_builtin());
    }

    #[tokio::test]
    async fn builtin_response_has_headers_status_and_escaped_values() {
        let page = ErrorPageTemplate::builtin();
        let resp = page.response(
            StatusCode::BAD_GATEWAY,
            "upstream <b>failed</b> & \"quoted\" 'single'",
            Some("<script>alert(1)</script>.example:443"),
        );
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(resp.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(resp.headers()[CACHE_CONTROL], "no-store");
        let body = body_of(resp).await;
        assert!(body.contains("<h1>502 Bad Gateway</h1>"), "{body}");
        assert!(body.contains("<title>502 Bad Gateway</title>"));
        assert!(body.contains(
            "<p>upstream &lt;b&gt;failed&lt;/b&gt; &amp; &quot;quoted&quot; &#39;single&#39;</p>"
        ));
        assert!(body.contains("<small>&lt;script&gt;alert(1)&lt;/script&gt;.example:443</small>"));
        assert!(!body.contains("<script"));
        assert!(!body.contains("<b>"));
        assert!(!body.contains("{{"));
    }

    #[test]
    fn missing_target_renders_empty() {
        let body = ErrorPageTemplate::builtin().render(StatusCode::BAD_REQUEST, "bad", None);
        assert!(body.contains("<small></small>"));
        assert!(body.contains("400 Bad Request"));
    }

    #[test]
    fn custom_template_substitutes_every_placeholder_once() {
        let page = ErrorPageTemplate::from_template(
            "[{{status}}|{{reason}}|{{message}}|{{target}}|{{status}}|{{unknown}}|{{]",
        );
        assert_eq!(
            page.render(
                StatusCode::GATEWAY_TIMEOUT,
                "slow {{target}}",
                Some("a.example:8080")
            ),
            "[504|Gateway Timeout|slow {{target}}|a.example:8080|504|{{unknown}}|{{]"
        );
        assert_eq!(
            page.render(StatusCode::FORBIDDEN, "no", None),
            "[403|Forbidden|no||403|{{unknown}}|{{]"
        );
    }

    #[test]
    fn unknown_status_has_a_generic_reason() {
        let status = StatusCode::from_u16(599).unwrap();
        let page = ErrorPageTemplate::from_template("{{status}} {{reason}}");
        assert_eq!(page.render(status, "", None), "599 Error");
    }

    #[test]
    fn from_file_reads_the_template() {
        let dir = std::env::temp_dir().join(format!("rc-error-page-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("page.html");
        std::fs::write(
            &path,
            "<p>{{status}} {{reason}}: {{message}} @ {{target}}</p>",
        )
        .unwrap();
        let page = ErrorPageTemplate::from_file(&path).unwrap();
        assert_eq!(
            page.render(StatusCode::FORBIDDEN, "denied", Some("x.example")),
            "<p>403 Forbidden: denied @ x.example</p>"
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(ErrorPageTemplate::from_file(&path).is_err());
    }

    #[test]
    fn page_target_strips_userinfo() {
        assert_eq!(page_target("user:pw@host:8080"), Some("host:8080"));
        assert_eq!(page_target("host"), Some("host"));
        assert_eq!(page_target(""), None);
        assert_eq!(page_target("user@"), None);
    }
}
