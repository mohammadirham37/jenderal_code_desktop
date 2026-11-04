//! Relay client: HTTP API (§4), the pairing wait socket (§5.2) and the
//! long-lived message socket (§5.1) with reconnect.

use crate::protocol::{ClientFrame, PairingCreated, ServerFrame};
use anyhow::{anyhow, bail, Context, Result};
use futures::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// No frame at all (the relay pings every 30 s) for this long means the
/// connection is dead.
const READ_TIMEOUT: Duration = Duration::from_secs(75);

/// Check a relay base URL: `https://host[:port][/prefix]`, or `http://` for
/// loopback hosts only (local development). Returns it without a trailing
/// slash.
pub fn normalize_relay_url(raw: &str) -> Result<String> {
    let url = raw.trim().trim_end_matches('/');
    let (scheme, rest) = url.split_once("://").ok_or_else(|| anyhow!("relay URL must start with https://"))?;
    let host_port = rest.split('/').next().unwrap_or_default();
    if host_port.is_empty() || host_port.contains('@') || url.contains(['?', '#', ' ']) {
        bail!("invalid relay URL");
    }
    let host = if let Some(h) = host_port.strip_prefix('[') {
        h.split(']').next().unwrap_or_default()
    } else {
        host_port.split(':').next().unwrap_or_default()
    };
    match scheme {
        "https" => Ok(url.to_string()),
        "http" if matches!(host, "localhost" | "127.0.0.1" | "::1") => Ok(url.to_string()),
        "http" => bail!("plain http is only allowed for localhost; use https://"),
        _ => bail!("relay URL must start with https://"),
    }
}

/// Error body of a failed relay call (`{"error","message"}`).
#[derive(Debug, Clone)]
pub struct RelayError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for RelayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.message.is_empty() {
            write!(f, "relay error {} ({})", self.code, self.status)
        } else {
            write!(f, "{} ({})", self.message, self.code)
        }
    }
}

impl std::error::Error for RelayError {}

/// Outcome of waiting for a phone to approve a pairing.
#[derive(Debug, Clone)]
pub enum PairingOutcome {
    Approved {
        device_id: String,
        device_token: Option<String>,
        mobile_device_id: String,
        mobile_name: String,
        mobile_public_key: String,
        user_email: String,
    },
    Expired,
}

#[derive(Clone)]
pub struct RelayApi {
    base: String,
    http: reqwest::Client,
}

impl RelayApi {
    pub fn new(base_url: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("JenderalCode-Desktop/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            base: normalize_relay_url(base_url)?,
            http,
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn ws_url(&self, path: &str) -> String {
        let base = if let Some(rest) = self.base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else {
            format!("ws://{}", self.base.trim_start_matches("http://"))
        };
        format!("{base}{path}")
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        token: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        let mut req = self.http.request(method, format!("{}{path}", self.base));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.context("relay unreachable")?;
        let status = res.status();
        let bytes = res.bytes().await.context("reading relay response")?;
        if !status.is_success() {
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
            return Err(RelayError {
                status: status.as_u16(),
                code: v["error"].as_str().unwrap_or("http_error").to_string(),
                message: v["message"].as_str().unwrap_or_default().to_string(),
            }
            .into());
        }
        let bytes = if bytes.is_empty() { &b"{}"[..] } else { &bytes[..] };
        serde_json::from_slice(bytes).context("unexpected relay response")
    }

    /// `POST /v1/pairings`. With `token` the phone is added to this
    /// already-registered desktop.
    pub async fn create_pairing(&self, public_key: &str, name: &str, token: Option<&str>) -> Result<PairingCreated> {
        self.call(
            reqwest::Method::POST,
            "/v1/pairings",
            token,
            Some(json!({ "public_key": public_key, "name": name })),
        )
        .await
    }

    /// Wait on `/v1/pairings/{id}/wait` until the phone approves or the
    /// pairing expires.
    pub async fn wait_pairing(&self, pairing_id: &str, wait_secret: &str) -> Result<PairingOutcome> {
        let url = self.ws_url(&format!("/v1/pairings/{pairing_id}/wait"));
        let (mut ws, _) = tokio_tungstenite::connect_async(url.as_str())
            .await
            .context("connecting to the relay")?;
        let auth = ClientFrame::Auth {
            token: None,
            secret: Some(wait_secret.to_string()),
        };
        ws.send(Message::text(serde_json::to_string(&auth)?)).await?;
        while let Some(msg) = ws.next().await {
            let text = match msg? {
                Message::Text(t) => t.to_string(),
                Message::Close(_) => break,
                _ => continue,
            };
            match serde_json::from_str::<ServerFrame>(&text) {
                Ok(ServerFrame::Approved {
                    device_id,
                    device_token,
                    mobile_device_id,
                    mobile_name,
                    mobile_public_key,
                    user_email,
                }) => {
                    let _ = ws.close(None).await;
                    return Ok(PairingOutcome::Approved {
                        device_id,
                        device_token,
                        mobile_device_id,
                        mobile_name,
                        mobile_public_key,
                        user_email,
                    });
                }
                Ok(ServerFrame::Expired) => return Ok(PairingOutcome::Expired),
                Ok(ServerFrame::Error { error }) => bail!("relay refused the pairing: {error}"),
                _ => continue,
            }
        }
        bail!("the relay closed the pairing connection")
    }

    /// `DELETE /v1/peers/{id}`: unpair this device from `device_id`.
    pub async fn delete_peer(&self, token: &str, device_id: &str) -> Result<()> {
        let _: serde_json::Value = self
            .call(reqwest::Method::DELETE, &format!("/v1/peers/{device_id}"), Some(token), None)
            .await?;
        Ok(())
    }

    /// `DELETE /v1/devices/{id}`: revoke a device (here: this desktop).
    pub async fn delete_device(&self, token: &str, device_id: &str) -> Result<()> {
        let _: serde_json::Value = self
            .call(reqwest::Method::DELETE, &format!("/v1/devices/{device_id}"), Some(token), None)
            .await?;
        Ok(())
    }

    /// Keep a `/v1/ws` connection open (reconnecting with backoff) and
    /// report what happens. Frames on `outbound` are sent while connected
    /// and dropped otherwise. Ends after `Revoked` or an auth error, or when
    /// the returned handle is dropped.
    pub fn connect(&self, token: String, events: mpsc::UnboundedSender<LinkEvent>) -> Link {
        let (out_tx, out_rx) = mpsc::unbounded_channel::<ClientFrame>();
        let url = self.ws_url("/v1/ws");
        let task = tokio::spawn(run_link(url, token, out_rx, events));
        Link { out: out_tx, task }
    }
}

/// What the message socket reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkEvent {
    Connecting,
    Ready { device_id: String, peers: Vec<(String, bool)> },
    Msg { from: String, body: String },
    Presence { device_id: String, online: bool },
    Unpaired { device_id: String },
    Undeliverable { to: String, reason: String },
    /// Lost the connection; it will retry.
    Disconnected { error: String },
    /// The relay revoked this device or refused its token; no more retries.
    Stopped { revoked: bool, error: String },
}

/// Handle of a running message socket; dropping it closes the connection.
pub struct Link {
    out: mpsc::UnboundedSender<ClientFrame>,
    task: tokio::task::JoinHandle<()>,
}

impl Link {
    pub fn send(&self, to: &str, body: String) {
        let _ = self.out.send(ClientFrame::Msg {
            to: to.to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            body,
        });
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.task.abort();
    }
}

enum SessionEnd {
    Retry(String),
    Stop { revoked: bool, error: String },
}

async fn run_link(
    url: String,
    token: String,
    mut outbound: mpsc::UnboundedReceiver<ClientFrame>,
    events: mpsc::UnboundedSender<LinkEvent>,
) {
    let mut backoff = BACKOFF_MIN;
    loop {
        let _ = events.send(LinkEvent::Connecting);
        let (end, was_ready) = run_session(&url, &token, &mut outbound, &events).await;
        match end {
            SessionEnd::Stop { revoked, error } => {
                let _ = events.send(LinkEvent::Stopped { revoked, error });
                return;
            }
            SessionEnd::Retry(error) => {
                let _ = events.send(LinkEvent::Disconnected { error });
            }
        }
        if was_ready {
            backoff = BACKOFF_MIN;
        }
        // Drop what queued up while offline: requests are re-sent by the phone.
        while outbound.try_recv().is_ok() {}
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

async fn run_session(
    url: &str,
    token: &str,
    outbound: &mut mpsc::UnboundedReceiver<ClientFrame>,
    events: &mpsc::UnboundedSender<LinkEvent>,
) -> (SessionEnd, bool) {
    let mut ready = false;
    let (mut ws, _) = match tokio_tungstenite::connect_async(url).await {
        Ok(c) => c,
        Err(e) => return (SessionEnd::Retry(format!("cannot reach the relay: {e}")), ready),
    };
    let auth = ClientFrame::Auth {
        token: Some(token.to_string()),
        secret: None,
    };
    if let Err(e) = ws
        .send(Message::text(serde_json::to_string(&auth).unwrap_or_default()))
        .await
    {
        return (SessionEnd::Retry(e.to_string()), ready);
    }
    loop {
        tokio::select! {
            frame = outbound.recv() => {
                let Some(frame) = frame else { return (SessionEnd::Stop { revoked: false, error: String::new() }, ready) };
                if !ready {
                    continue;
                }
                let text = serde_json::to_string(&frame).unwrap_or_default();
                if let Err(e) = ws.send(Message::text(text)).await {
                    return (SessionEnd::Retry(e.to_string()), ready);
                }
            }
            msg = tokio::time::timeout(READ_TIMEOUT, ws.next()) => {
                let msg = match msg {
                    Err(_) => return (SessionEnd::Retry("relay stopped responding".into()), ready),
                    Ok(None) => return (SessionEnd::Retry("connection closed".into()), ready),
                    Ok(Some(Err(e))) => return (SessionEnd::Retry(e.to_string()), ready),
                    Ok(Some(Ok(m))) => m,
                };
                let text = match msg {
                    Message::Text(t) => t.to_string(),
                    Message::Close(frame) => {
                        // 4000: replaced by a newer connection of this device.
                        let replaced = frame.as_ref().is_some_and(|f| u16::from(f.code) == 4000);
                        if replaced {
                            return (SessionEnd::Stop { revoked: false, error: "another connection of this desktop took over".into() }, ready);
                        }
                        return (SessionEnd::Retry("connection closed".into()), ready);
                    }
                    _ => continue, // ping/pong are answered by tungstenite
                };
                let Ok(frame) = serde_json::from_str::<ServerFrame>(&text) else { continue };
                let ev = match frame {
                    ServerFrame::Ready { device_id, peers } => {
                        ready = true;
                        LinkEvent::Ready { device_id, peers: peers.into_iter().map(|p| (p.device_id, p.online)).collect() }
                    }
                    ServerFrame::Msg { from, body, .. } => LinkEvent::Msg { from, body },
                    ServerFrame::Presence { device_id, online } => LinkEvent::Presence { device_id, online },
                    ServerFrame::Unpaired { device_id } => LinkEvent::Unpaired { device_id },
                    ServerFrame::Undeliverable { to, reason, .. } => LinkEvent::Undeliverable { to, reason },
                    ServerFrame::Revoked => return (SessionEnd::Stop { revoked: true, error: "this desktop was removed on the relay".into() }, ready),
                    ServerFrame::Error { error } => {
                        let auth_failed = matches!(error.as_str(), "unauthorized" | "forbidden");
                        return if auth_failed {
                            (SessionEnd::Stop { revoked: true, error: format!("the relay refused this desktop ({error})") }, ready)
                        } else {
                            (SessionEnd::Retry(error), ready)
                        };
                    }
                    _ => continue,
                };
                let _ = events.send(ev);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_urls_require_https_except_loopback() {
        assert_eq!(normalize_relay_url("https://relay.example.com/").unwrap(), "https://relay.example.com");
        assert_eq!(normalize_relay_url(" https://r.example.com/jc ").unwrap(), "https://r.example.com/jc");
        assert!(normalize_relay_url("http://127.0.0.1:8080").is_ok());
        assert!(normalize_relay_url("http://localhost:8080").is_ok());
        assert!(normalize_relay_url("http://[::1]:8080").is_ok());
        assert!(normalize_relay_url("http://relay.example.com").is_err());
        assert!(normalize_relay_url("http://127.0.0.1.evil.com").is_err());
        assert!(normalize_relay_url("ftp://x").is_err());
        assert!(normalize_relay_url("https://user@x.com").is_err());
        assert!(normalize_relay_url("relay.example.com").is_err());
        assert!(normalize_relay_url("https://x.com?a=b").is_err());
    }

    #[test]
    fn ws_urls_follow_the_scheme() {
        let a = RelayApi::new("https://r.example.com/jc").unwrap();
        assert_eq!(a.ws_url("/v1/ws"), "wss://r.example.com/jc/v1/ws");
        let b = RelayApi::new("http://127.0.0.1:8080").unwrap();
        assert_eq!(b.ws_url("/v1/ws"), "ws://127.0.0.1:8080/v1/ws");
    }
}
