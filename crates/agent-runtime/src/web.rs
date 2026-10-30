//! `web_fetch`: download a web page for the model and turn HTML into
//! readable text. Permission (approval, http(s) only) is handled by the
//! policy engine before this runs.

use anyhow::{bail, Context, Result};
use futures::StreamExt;
use regex::Regex;
use std::sync::OnceLock;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;

pub struct Fetched {
    pub final_url: String,
    pub content_type: String,
    pub text: String,
    pub bytes: usize,
}

pub async fn fetch(url: &str, raw: bool) -> Result<Fetched> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        bail!("only http(s) URLs can be fetched");
    }
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("Mozilla/5.0 (compatible; JenderalCode)")
        .build()?;
    let resp = client.get(url).send().await.context("request failed")?;
    let status = resp.status();
    let final_url = resp.url().to_string();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !status.is_success() {
        bail!("HTTP {status} for {final_url}");
    }
    let textual = content_type.is_empty()
        || content_type.starts_with("text/")
        || ["json", "xml", "javascript", "yaml", "markdown"].iter().any(|t| content_type.contains(t));
    if !textual {
        bail!("{final_url} is not a text page ({content_type})");
    }
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading response")?;
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            body.extend_from_slice(&chunk[..MAX_BODY_BYTES - body.len()]);
            break;
        }
        body.extend_from_slice(&chunk);
    }
    let bytes = body.len();
    let body = String::from_utf8_lossy(&body).into_owned();
    let is_html = content_type.contains("html") || (content_type.is_empty() && body.trim_start().starts_with('<'));
    let text = if is_html && !raw { html_to_text(&body) } else { body };
    Ok(Fetched { final_url, content_type, text, bytes })
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid regex"))
}

/// Readable text from HTML: drops scripts/styles/navigation chrome, keeps
/// headings, list items, links (as `text (url)`) and code blocks.
pub fn html_to_text(html: &str) -> String {
    static DROP: OnceLock<Regex> = OnceLock::new();
    static COMMENT: OnceLock<Regex> = OnceLock::new();
    static HEADING: OnceLock<Regex> = OnceLock::new();
    static LINK: OnceLock<Regex> = OnceLock::new();
    static BLOCK: OnceLock<Regex> = OnceLock::new();
    static LI: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    static ENTITY: OnceLock<Regex> = OnceLock::new();
    static SPACES: OnceLock<Regex> = OnceLock::new();
    static BLANKS: OnceLock<Regex> = OnceLock::new();

    let s = re(&COMMENT, r"(?s)<!--.*?-->").replace_all(html, "");
    let s = re(&DROP, r"(?is)<(script|style|noscript|svg|head|nav|footer|iframe|template)\b[^>]*>.*?</(script|style|noscript|svg|head|nav|footer|iframe|template)>")
        .replace_all(&s, "");
    let s = re(&HEADING, r"(?is)<h([1-6])\b[^>]*>(.*?)</h[1-6]>").replace_all(&s, |c: &regex::Captures| {
        let level = c[1].parse::<usize>().unwrap_or(1);
        format!("\n\n{} {}\n\n", "#".repeat(level), c[2].trim())
    });
    let s = re(&LINK, r#"(?is)<a\b[^>]*?href\s*=\s*["']([^"']+)["'][^>]*>(.*?)</a>"#).replace_all(&s, |c: &regex::Captures| {
        let href = &c[1];
        let text = c[2].trim();
        if href.starts_with('#') || href.starts_with("javascript:") || text.is_empty() {
            text.to_string()
        } else {
            format!("{text} ({href})")
        }
    });
    let s = re(&LI, r"(?i)<li\b[^>]*>").replace_all(&s, "\n- ");
    let s = re(&BLOCK, r"(?i)<(br|/p|p|/div|div|/tr|tr|/ul|/ol|/table|pre|/pre|/section|/article|/header|/blockquote)\b[^>]*>")
        .replace_all(&s, "\n");
    let s = re(&TAG, r"(?s)<[^>]+>").replace_all(&s, "");
    let s = re(&ENTITY, r"&(#x[0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);").replace_all(&s, |c: &regex::Captures| {
        let e = &c[1];
        let decoded = match e {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ if e.starts_with("#x") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            _ if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        decoded.map(String::from).unwrap_or_else(|| c[0].to_string())
    });
    let s = re(&SPACES, r"[ \t\x0B\f\r]+").replace_all(&s, " ");
    let lines: Vec<&str> = s.lines().map(str::trim).collect();
    let joined = lines.join("\n");
    re(&BLANKS, r"\n{3,}").replace_all(&joined, "\n\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_becomes_readable_text() {
        let html = r#"<html><head><title>T</title><style>p{}</style></head><body>
            <nav>menu</nav>
            <h1>Pricing</h1>
            <p>Model &amp; price: <b>$1</b>&nbsp;per&#32;token</p>
            <ul><li>One</li><li><a href="https://x.dev/a">Docs</a></li></ul>
            <script>alert(1)</script><!-- hidden -->
            </body></html>"#;
        let t = html_to_text(html);
        assert!(t.contains("# Pricing"), "{t}");
        assert!(t.contains("Model & price: $1 per token"), "{t}");
        assert!(t.contains("- One"));
        assert!(t.contains("Docs (https://x.dev/a)"));
        for gone in ["menu", "alert", "hidden", "p{}", "<"] {
            assert!(!t.contains(gone), "{gone} in {t}");
        }
    }

    #[tokio::test]
    async fn rejects_non_http_urls() {
        assert!(fetch("file:///etc/passwd", false).await.is_err());
    }
}
