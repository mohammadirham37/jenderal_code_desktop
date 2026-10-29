//! Provider contract and shared plumbing (PRD §12.1).
//!
//! Every adapter (OpenAI-compatible, Anthropic, Google, mock) implements
//! [`AiProvider`] against the internal, provider-neutral types from
//! `jcode-common`. The runtime normalizes everything else.

pub mod credentials;
pub mod mock;

pub use credentials::{
    probe_keyring, CredentialStore, FileCredentialStore, KeyringCredentialStore,
    SwitchableCredentialStore, KEYCHAIN_SERVICE,
};
pub use mock::MockProvider;

use anyhow::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;
use jcode_common::{ModelInfo, ModelRequest, StreamEvent};

/// The uniform internal provider contract.
#[async_trait]
pub trait AiProvider: Send + Sync {
    /// Stable provider id (the config id from storage).
    fn id(&self) -> &str;

    /// Human-readable provider name.
    fn name(&self) -> &str;

    /// List models available to the configured credentials.
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;

    /// Start a streaming completion. The stream is `'static`; the caller
    /// owns it and can drop it to cancel.
    async fn stream(&self, request: ModelRequest) -> Result<BoxStream<'static, StreamEvent>>;
}

/// Common preset metadata used by the Provider Center UI to prefill forms.
pub struct ProviderPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: jcode_common::ProviderKind,
    pub base_url: &'static str,
    pub needs_key: bool,
}

pub fn provider_presets() -> Vec<ProviderPreset> {
    use jcode_common::ProviderKind::*;
    vec![
        ProviderPreset { id: "openai", label: "OpenAI", kind: OpenAiCompatible, base_url: "https://api.openai.com/v1", needs_key: true },
        ProviderPreset { id: "anthropic", label: "Anthropic", kind: Anthropic, base_url: "https://api.anthropic.com", needs_key: true },
        ProviderPreset { id: "google", label: "Google Gemini", kind: Google, base_url: "https://generativelanguage.googleapis.com", needs_key: true },
        ProviderPreset { id: "deepseek", label: "DeepSeek", kind: OpenAiCompatible, base_url: "https://api.deepseek.com/v1", needs_key: true },
        ProviderPreset { id: "mistral", label: "Mistral", kind: OpenAiCompatible, base_url: "https://api.mistral.ai/v1", needs_key: true },
        ProviderPreset { id: "xai", label: "xAI (Grok)", kind: OpenAiCompatible, base_url: "https://api.x.ai/v1", needs_key: true },
        ProviderPreset { id: "groq", label: "Groq", kind: OpenAiCompatible, base_url: "https://api.groq.com/openai/v1", needs_key: true },
        ProviderPreset { id: "ollama", label: "Ollama (local)", kind: OpenAiCompatible, base_url: "http://localhost:11434/v1", needs_key: false },
        ProviderPreset { id: "lmstudio", label: "LM Studio (local)", kind: OpenAiCompatible, base_url: "http://localhost:1234/v1", needs_key: false },
        ProviderPreset { id: "custom", label: "Custom OpenAI-compatible", kind: OpenAiCompatible, base_url: "", needs_key: false },
        ProviderPreset { id: "mock", label: "Mock (demo, no network)", kind: Mock, base_url: "", needs_key: false },
    ]
}

/// SSE parsing helper shared by the HTTP adapters: yields the `data:` payload
/// of each event, skipping comments and empty lines.
pub fn parse_sse_data_line(line: &str) -> Option<&str> {
    let line = line.strip_prefix("data:")?;
    Some(line.trim_start())
}

/// Shared HTTP client for all provider adapters.
///
/// One connection pool for the whole process, with sane timeouts and
/// redirects disabled: following a redirect would forward provider auth
/// headers (`x-api-key`, `x-goog-api-key`) to whatever host the redirect
/// points at. There is deliberately no overall request timeout, because SSE
/// streams legitimately run for minutes; `read_timeout` bounds the gap
/// between received bytes instead.
pub fn http_client() -> reqwest::Client {
    use std::sync::OnceLock;
    use std::time::Duration;
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .read_timeout(Duration::from_secs(120))
                .pool_idle_timeout(Duration::from_secs(90))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_else(|_| reqwest::Client::new())
        })
        .clone()
}

/// Overall timeout for short, non-streaming provider calls (e.g. list models).
pub const NON_STREAMING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Incremental SSE line splitter that works on raw bytes.
///
/// Network chunks can split a multi-byte UTF-8 character; decoding each
/// chunk separately would corrupt it. This buffers bytes and only decodes
/// complete `\n`-terminated lines. Trailing `\r` is stripped.
#[derive(Debug, Default)]
pub struct SseLineBuffer {
    buf: Vec<u8>,
}

impl SseLineBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a chunk and return every complete line now available.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        let scan_from = self.buf.len();
        self.buf.extend_from_slice(chunk);
        let mut start = 0usize;
        let mut i = scan_from;
        while let Some(off) = self.buf[i..].iter().position(|&b| b == b'\n') {
            let end = i + off;
            lines.push(decode_line(&self.buf[start..end]));
            start = end + 1;
            i = start;
        }
        if start > 0 {
            self.buf.drain(..start);
        }
        lines
    }

    /// Return the trailing unterminated line, if any (call at end of stream).
    pub fn finish(&mut self) -> Option<String> {
        if self.buf.is_empty() {
            return None;
        }
        let line = decode_line(&self.buf);
        self.buf.clear();
        Some(line)
    }
}

fn decode_line(bytes: &[u8]) -> String {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        // Only a genuinely invalid complete line gets lossy decoding.
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_data_line_parsing() {
        assert_eq!(parse_sse_data_line("data: hello"), Some("hello"));
        assert_eq!(parse_sse_data_line("data:[DONE]"), Some("[DONE]"));
        assert_eq!(parse_sse_data_line(": keepalive"), None);
        assert_eq!(parse_sse_data_line("event: x"), None);
    }

    #[test]
    fn sse_line_buffer_handles_utf8_split_across_chunks() {
        // "€" is 3 bytes (E2 82 AC); split it across chunks.
        let full = "data: a€b\r\ndata: 日本\n".as_bytes();
        let euro = full.iter().position(|&b| b == 0xE2).unwrap();
        let mut b = SseLineBuffer::new();
        assert!(b.push(&full[..euro + 1]).is_empty());
        assert!(b.push(&full[euro + 1..euro + 2]).is_empty());
        let lines = b.push(&full[euro + 2..full.len() - 3]);
        assert_eq!(lines, vec!["data: a€b".to_string()]);
        let lines = b.push(&full[full.len() - 3..]);
        assert_eq!(lines, vec!["data: 日本".to_string()]);
        assert_eq!(b.finish(), None);
    }

    #[test]
    fn sse_line_buffer_byte_by_byte_and_trailing() {
        let text = "data: ☃ one\n\ndata: two";
        let mut b = SseLineBuffer::new();
        let mut lines = Vec::new();
        for byte in text.as_bytes() {
            lines.extend(b.push(std::slice::from_ref(byte)));
        }
        assert_eq!(lines, vec!["data: ☃ one".to_string(), String::new()]);
        assert_eq!(b.finish().as_deref(), Some("data: two"));
    }

    #[test]
    fn sse_line_buffer_invalid_line_is_lossy() {
        let mut b = SseLineBuffer::new();
        let lines = b.push(b"data: \xff\n");
        assert_eq!(lines, vec!["data: \u{FFFD}".to_string()]);
    }

    #[test]
    fn http_client_is_shared() {
        let _a = http_client();
        let _b = http_client();
    }
}
