//! The phone's browser (docs/REMOTE_PROTOCOL.md §6.10): the phone shows a
//! dev server running on this computer by sending each HTTP request through
//! the encrypted channel; the desktop fetches it from `localhost` and sends
//! the response back in chunks that fit the relay's frame limit.
//!
//! Only loopback hosts are fetched (never the internet or the LAN), redirects
//! are passed back rather than followed, and the whole feature needs the
//! desktop's "Allow Full access & terminal from phone" (checked by the caller).

use base64::Engine;
use jcode_remote::protocol::AppError;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use url::Url;

/// Raw bytes per chunk: base64 (×4/3), JSON and sealing (×4/3 again) stay
/// well under the relay's 1 MiB frame.
pub const CHUNK_BYTES: usize = 384 * 1024;
/// Largest response body fetched for the phone.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
/// Largest request body the phone may send (it arrives in one frame).
const MAX_REQUEST_BODY: usize = 384 * 1024;
/// How long the rest of a large response waits for `http.read`.
const STREAM_TTL: Duration = Duration::from_secs(120);
const TIMEOUT: Duration = Duration::from_secs(60);

fn bad(msg: impl Into<String>) -> AppError {
    AppError::new("bad_request", msg)
}

/// Headers that describe one hop or that the phone recomputes.
const HOP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "transfer-encoding",
    "upgrade",
    "te",
    "trailer",
    "content-length",
    "host",
];

/// `url` if it is http(s) on a loopback host, else an error.
pub fn loopback_url(input: &str) -> Result<Url, AppError> {
    let url = Url::parse(input).map_err(|e| bad(format!("invalid URL: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(bad("only http(s) URLs can be opened"));
    }
    let loopback = match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost") || d.to_ascii_lowercase().ends_with(".localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if !loopback {
        return Err(AppError::new(
            "not_allowed",
            "The phone browser only opens pages served on this computer (localhost).",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(bad("URLs with credentials are not allowed"));
    }
    Ok(url)
}

struct Stream {
    rest: Vec<u8>,
    at: Instant,
}

/// Fetches for the phone and keeps the unsent part of large responses.
pub struct HttpProxy {
    client: reqwest::Client,
    streams: Mutex<HashMap<String, Stream>>,
}

impl Default for HttpProxy {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpProxy {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(TIMEOUT)
            // Never through a proxy: the target is this computer.
            .no_proxy()
            .build()
            .expect("http client");
        Self { client, streams: Mutex::new(HashMap::new()) }
    }

    fn streams(&self) -> std::sync::MutexGuard<'_, HashMap<String, Stream>> {
        self.streams.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `http.request {url, method?, headers?: [[name, value]], body?: b64}`
    /// → `{status, headers: [[name, value]], body: b64, stream_id?}`.
    pub async fn request(&self, params: &Value) -> Result<Value, AppError> {
        let url = loopback_url(params.get("url").and_then(|v| v.as_str()).ok_or_else(|| bad("missing url"))?)?;
        let method = params.get("method").and_then(|v| v.as_str()).unwrap_or("GET");
        let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| bad("invalid method"))?;
        let mut req = self.client.request(method, url.clone());
        if let Some(hs) = params.get("headers").and_then(|v| v.as_array()) {
            for h in hs.iter().take(100) {
                let (Some(name), Some(value)) = (h.get(0).and_then(|v| v.as_str()), h.get(1).and_then(|v| v.as_str())) else {
                    continue;
                };
                let lower = name.to_ascii_lowercase();
                if HOP_HEADERS.contains(&lower.as_str()) || lower == "accept-encoding" {
                    continue;
                }
                req = req.header(name, value);
            }
        }
        // Uncompressed bodies: chunks stay meaningful and sizes honest.
        req = req.header("accept-encoding", "identity");
        if let Some(b) = params.get("body").and_then(|v| v.as_str()) {
            let body = base64::engine::general_purpose::STANDARD.decode(b).map_err(|_| bad("invalid body"))?;
            if body.len() > MAX_REQUEST_BODY {
                return Err(AppError::new("too_large", "The request body is too large."));
            }
            req = req.body(body);
        }
        let res = req.send().await.map_err(|e| {
            let msg = if e.is_connect() {
                format!("Nothing is listening on {} on the desktop. Start the dev server first.", url.origin().ascii_serialization())
            } else if e.is_timeout() {
                "The dev server did not answer in time.".to_string()
            } else {
                format!("Request failed: {e}")
            };
            AppError::new("unreachable", msg)
        })?;
        let status = res.status().as_u16();
        let headers: Vec<Value> = res
            .headers()
            .iter()
            .filter(|(k, _)| !HOP_HEADERS.contains(&k.as_str()))
            .filter_map(|(k, v)| v.to_str().ok().map(|v| json!([k.as_str(), v])))
            .collect();
        let mut body = Vec::new();
        let mut res = res;
        while let Some(chunk) = res.chunk().await.map_err(|e| AppError::new("unreachable", format!("Reading the response failed: {e}")))? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_BODY_BYTES {
                return Err(AppError::new("too_large", "The page or file is larger than 64 MiB."));
            }
        }
        let mut out = json!({ "status": status, "headers": headers, "size": body.len() });
        out["body"] = json!(self.first_chunk(body, &mut out));
        Ok(out)
    }

    /// The first chunk (base64); stores the rest and sets `stream_id`.
    fn first_chunk(&self, mut body: Vec<u8>, out: &mut Value) -> String {
        let rest = if body.len() > CHUNK_BYTES { body.split_off(CHUNK_BYTES) } else { Vec::new() };
        if !rest.is_empty() {
            let id = uuid::Uuid::new_v4().to_string();
            let mut streams = self.streams();
            streams.retain(|_, s| s.at.elapsed() < STREAM_TTL);
            streams.insert(id.clone(), Stream { rest, at: Instant::now() });
            out["stream_id"] = json!(id);
        }
        base64::engine::general_purpose::STANDARD.encode(body)
    }

    /// `http.read {stream_id}` → `{body: b64, done: bool}`: the next chunk.
    pub fn read(&self, params: &Value) -> Result<Value, AppError> {
        let id = params.get("stream_id").and_then(|v| v.as_str()).ok_or_else(|| bad("missing stream_id"))?;
        let mut streams = self.streams();
        let Some(s) = streams.get_mut(id) else {
            return Err(AppError::new("not_found", "The response expired. Reload the page."));
        };
        let n = s.rest.len().min(CHUNK_BYTES);
        let chunk: Vec<u8> = s.rest.drain(..n).collect();
        s.at = Instant::now();
        let done = s.rest.is_empty();
        if done {
            streams.remove(id);
        }
        Ok(json!({ "body": base64::engine::general_purpose::STANDARD.encode(chunk), "done": done }))
    }

    /// Drop every stored response (full access turned off).
    pub fn clear(&self) {
        self.streams().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_urls() {
        for ok in ["http://localhost:5173/", "http://127.0.0.1:8000/x?y=1", "http://[::1]:3000/", "http://app.localhost:4000/", "https://localhost/"] {
            assert!(loopback_url(ok).is_ok(), "{ok}");
        }
        for no in [
            "http://192.168.1.5:5173/",
            "http://example.com/",
            "http://localhost.evil.com/",
            "http://10.0.0.1/",
            "file:///etc/passwd",
            "ftp://localhost/",
            "http://user:pw@localhost/",
            "http://0.0.0.0:5173/",
        ] {
            assert!(loopback_url(no).is_err(), "{no}");
        }
    }

    #[test]
    fn large_bodies_come_in_chunks() {
        let p = HttpProxy::new();
        let body: Vec<u8> = (0..(CHUNK_BYTES * 2 + 10)).map(|i| (i % 251) as u8).collect();
        let mut out = json!({});
        let first = p.first_chunk(body.clone(), &mut out);
        let id = out["stream_id"].as_str().unwrap().to_string();
        let mut got = base64::engine::general_purpose::STANDARD.decode(first).unwrap();
        loop {
            let r = p.read(&json!({ "stream_id": id })).unwrap();
            got.extend(base64::engine::general_purpose::STANDARD.decode(r["body"].as_str().unwrap()).unwrap());
            if r["done"].as_bool().unwrap() {
                break;
            }
        }
        assert_eq!(got, body);
        assert!(p.read(&json!({ "stream_id": id })).is_err(), "stream is gone once read");

        // Small bodies are sent whole.
        let mut out = json!({});
        p.first_chunk(vec![1, 2, 3], &mut out);
        assert!(out.get("stream_id").is_none());
    }

    #[tokio::test]
    async fn fetches_a_local_server_and_passes_redirects_back() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            for _ in 0..2 {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let resp = if req.starts_with("GET /go ") {
                    "HTTP/1.1 302 Found\r\nLocation: /home\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                } else {
                    let enc = if req.to_ascii_lowercase().contains("accept-encoding: identity") { "identity" } else { "other" };
                    format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nX-Enc: {enc}\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello")
                };
                s.write_all(resp.as_bytes()).await.unwrap();
            }
        });
        let p = HttpProxy::new();
        let r = p.request(&json!({ "url": format!("http://127.0.0.1:{port}/"), "headers": [["Accept-Encoding", "gzip"]] })).await.unwrap();
        assert_eq!(r["status"], 200);
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(r["body"].as_str().unwrap()).unwrap(), b"hello");
        let hs = r["headers"].as_array().unwrap();
        assert!(hs.iter().any(|h| h[0] == "x-enc" && h[1] == "identity"));
        assert!(!hs.iter().any(|h| h[0] == "content-length"));
        let r = p.request(&json!({ "url": format!("http://127.0.0.1:{port}/go") })).await.unwrap();
        assert_eq!(r["status"], 302);
        assert!(r["headers"].as_array().unwrap().iter().any(|h| h[0] == "location" && h[1] == "/home"));

        // Nothing listening: a clear error.
        let e = p.request(&json!({ "url": "http://127.0.0.1:1/" })).await.unwrap_err();
        assert_eq!(e.code, "unreachable");
    }
}
