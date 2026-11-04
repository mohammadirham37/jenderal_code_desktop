//! Remote control from a paired phone through the relay
//! (docs/REMOTE_PROTOCOL.md). The relay is untrusted: every request is
//! decrypted with the key of a phone the user confirmed on this desktop
//! (6-digit code), and anything else is dropped.
//!
//! `RemoteController` is both the request handler (phone → agent runtime)
//! and an `EventSink` (runtime events → subscribed phones), wired into the
//! runtime through `sink::FanoutSink`.

use crate::sink::{DeltaAction, DeltaCoalescer};
use crate::state::AppState;
use crate::terminal::{TerminalEvent, TerminalInfo, TerminalManager, TerminalOptions, TerminalOrigin};
use async_trait::async_trait;
use jcode_agent_runtime::{AgentRuntime, EventSink};
use jcode_common::*;
use jcode_provider_core::{CredentialStore, SwitchableCredentialStore};
use jcode_remote::crypto::{self, KeyPair};
use jcode_remote::protocol::{pairing_uri, AppError, AppMessage};
use jcode_remote::relay::{LinkEvent, PairingOutcome, RelayApi, Link};
use jcode_remote::PeerBook;
use jcode_storage::Db;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;

pub const EVENT_REMOTE_STATUS: &str = "remote://status";
pub const EVENT_REMOTE_PAIRING: &str = "remote://pairing";
/// A session was created from the phone: the session list should reload.
pub const EVENT_REMOTE_SESSIONS: &str = "remote://sessions";

/// The official relay (docs/REMOTE_PROTOCOL.md §0). Fixed: users cannot
/// point the app at another relay.
pub const RELAY_URL: &str = "https://backendcode.jenderalcorp.com";

/// Where the phone app is downloaded (GitHub releases of jenderalcode_mobile).
pub const MOBILE_DOWNLOAD_URL: &str = "https://github.com/mohammadirham37/jenderalcode_mobile/releases/latest";

const KEY_ACCOUNT: &str = "remote:key";
const DEVICE_ACCOUNT: &str = "remote:device";
/// Phones get coarser deltas than the webview (mobile networks).
const DELTA_INTERVAL: Duration = Duration::from_millis(100);
const MAX_CONTENT_CHARS: usize = 100_000;
/// Terminal output is batched for phones at this interval…
const TERM_INTERVAL: Duration = Duration::from_millis(50);
/// …and split into events of at most this size.
const TERM_EVENT_BYTES: usize = 64 * 1024;
const MAX_REMOTE_TERMINALS: usize = 6;

/// This desktop's registration on one relay. Lives in the credential store
/// (the token is a secret); tied to the relay URL it was issued by.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeviceIdentity {
    relay_url: String,
    device_id: String,
    device_token: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteDevice {
    pub device_id: String,
    pub name: String,
    pub user_email: String,
    pub created_at: i64,
    pub last_seen: Option<i64>,
    pub online: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteStatus {
    pub enabled: bool,
    pub relay_url: String,
    /// `off` | `no_relay` | `unpaired` | `connecting` | `online` | `offline` | `stopped`
    pub state: &'static str,
    pub error: Option<String>,
    pub desktop_name: String,
    pub devices: Vec<RemoteDevice>,
    pub allow_full_access: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairingStarted {
    pub pairing_id: String,
    pub qr_svg: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairingEvent {
    pub pairing_id: String,
    /// `approved` (confirm with the SAS) | `expired` | `error`
    pub state: &'static str,
    pub sas: Option<String>,
    pub mobile_name: Option<String>,
    pub user_email: Option<String>,
    pub error: Option<String>,
}

struct PendingPairing {
    pairing_id: String,
    desktop_id: String,
    /// Set when this pairing registered the desktop on the relay.
    new_token: Option<String>,
    peer: RemotePeer,
}

#[derive(Default)]
struct Inner {
    link: Option<Link>,
    /// Bumped on every (re)connect so events of an old link are ignored.
    link_gen: u64,
    state: &'static str,
    error: Option<String>,
    book: Option<PeerBook>,
    online: HashSet<String>,
    /// phone device id → subscribed session ids
    subs: HashMap<String, HashSet<String>>,
    /// phone device id → attached terminal ids
    term_attach: HashMap<String, HashSet<String>>,
    pairing_task: Option<tauri::async_runtime::JoinHandle<()>>,
    pending: Option<PendingPairing>,
}

pub struct RemoteController {
    me: Weak<RemoteController>,
    app: AppHandle,
    db: Arc<Db>,
    creds: Arc<SwitchableCredentialStore>,
    runtime: OnceLock<Arc<AgentRuntime>>,
    inner: Mutex<Inner>,
    terminals: OnceLock<TerminalManager>,
    /// Terminal output waiting to be sent (coalesced per terminal).
    term_out: Mutex<HashMap<String, String>>,
    deltas: DeltaCoalescer,
    /// Serializes connect/disconnect so settings changes cannot race.
    reconnect_lock: tokio::sync::Mutex<()>,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn bad(msg: impl Into<String>) -> AppError {
    AppError::new("bad_request", msg)
}

fn internal(e: impl std::fmt::Display) -> AppError {
    AppError::new("internal", format!("{e:#}"))
}

/// The computer's name for the QR code and the phone's list.
pub fn desktop_name() -> String {
    let from_env = ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()));
    let name = from_env
        .or_else(|| {
            if cfg!(target_os = "macos") {
                std::process::Command::new("scutil")
                    .args(["--get", "ComputerName"])
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
            } else {
                std::fs::read_to_string("/etc/hostname").ok()
            }
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "JenderalCode Desktop".to_string());
    name.chars().take(64).collect()
}

fn qr_svg(data: &str) -> anyhow::Result<String> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::new(data.as_bytes())?;
    Ok(code
        .render::<svg::Color>()
        .min_dimensions(240, 240)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .quiet_zone(true)
        .build())
}

impl RemoteController {
    pub fn new(app: AppHandle, db: Arc<Db>, creds: Arc<SwitchableCredentialStore>) -> Arc<Self> {
        Arc::new_cyclic(|me: &Weak<Self>| {
            let w = me.clone();
            Self {
                me: me.clone(),
                app,
                db,
                creds,
                runtime: OnceLock::new(),
                terminals: OnceLock::new(),
                term_out: Mutex::new(HashMap::new()),
                inner: Mutex::new(Inner {
                    state: "off",
                    ..Default::default()
                }),
                deltas: DeltaCoalescer::new(move |ev| {
                    if let Some(c) = w.upgrade() {
                        let sid = ev.session_id.clone();
                        c.send_event(&sid, "delta", json!(ev));
                    }
                }),
                reconnect_lock: tokio::sync::Mutex::new(()),
            }
        })
    }

    pub fn set_runtime(&self, runtime: Arc<AgentRuntime>) {
        let _ = self.runtime.set(runtime);
    }

    /// Share the terminal manager and receive its output for attached phones.
    pub fn set_terminals(&self, terminals: TerminalManager) {
        let me = self.me.clone();
        terminals.set_observer(move |ev| {
            if let Some(c) = me.upgrade() {
                c.on_terminal_event(ev);
            }
        });
        let _ = self.terminals.set(terminals);
    }

    fn rt(&self) -> Result<&Arc<AgentRuntime>, AppError> {
        self.runtime.get().ok_or_else(|| internal("runtime not ready"))
    }

    fn arc(&self) -> Arc<Self> {
        self.me.upgrade().expect("controller alive")
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ------------------------------------------------------------------
    // Identity and keys
    // ------------------------------------------------------------------

    async fn keypair(&self) -> anyhow::Result<KeyPair> {
        if let Some(s) = self.creds.get(KEY_ACCOUNT).await? {
            if let Ok(k) = KeyPair::from_secret_b64u(&s) {
                return Ok(k);
            }
        }
        let k = KeyPair::generate();
        self.creds.set(KEY_ACCOUNT, &k.secret_b64u()).await?;
        Ok(k)
    }

    /// The identity for `relay_url`, if this desktop is registered there.
    async fn identity(&self, relay_url: &str) -> Option<DeviceIdentity> {
        let raw = self.creds.get(DEVICE_ACCOUNT).await.ok()??;
        let id: DeviceIdentity = serde_json::from_str(&raw).ok()?;
        (id.relay_url == relay_url).then_some(id)
    }

    async fn forget_identity(&self) {
        let _ = self.creds.delete(DEVICE_ACCOUNT).await;
        let _ = self.db.clear_remote_peers();
    }

    fn settings(&self) -> AppSettings {
        self.db.get_settings().unwrap_or_default()
    }

    fn relay(&self) -> Result<RelayApi, String> {
        RelayApi::new(RELAY_URL).map_err(|e| format!("{e:#}"))
    }

    // ------------------------------------------------------------------
    // Status
    // ------------------------------------------------------------------

    pub fn snapshot(&self) -> RemoteStatus {
        let s = self.settings();
        let peers = self.db.list_remote_peers().unwrap_or_default();
        let inner = self.lock();
        RemoteStatus {
            enabled: s.remote_enabled,
            relay_url: RELAY_URL.to_string(),
            state: inner.state,
            error: inner.error.clone(),
            desktop_name: desktop_name(),
            allow_full_access: s.remote_allow_full_access,
            devices: peers
                .into_iter()
                .map(|p| RemoteDevice {
                    online: inner.online.contains(&p.device_id),
                    device_id: p.device_id,
                    name: p.name,
                    user_email: p.user_email,
                    created_at: p.created_at,
                    last_seen: p.last_seen,
                })
                .collect(),
        }
    }

    fn emit_status(&self) {
        let _ = self.app.emit(EVENT_REMOTE_STATUS, self.snapshot());
    }

    fn set_state(&self, state: &'static str, error: Option<String>) {
        {
            let mut inner = self.lock();
            inner.state = state;
            inner.error = error;
        }
        self.emit_status();
    }

    // ------------------------------------------------------------------
    // Connection
    // ------------------------------------------------------------------

    /// Bring the connection in line with the settings (call after saving
    /// them). A changed relay URL forgets the old registration and phones.
    pub async fn apply_settings(&self) {
        let _guard = self.reconnect_lock.lock().await;
        let s = self.settings();
        let url = jcode_remote::relay::normalize_relay_url(RELAY_URL).ok();
        // Registered on another relay: those phones cannot reach us here.
        if let Ok(Some(raw)) = self.creds.get(DEVICE_ACCOUNT).await {
            let stale = serde_json::from_str::<DeviceIdentity>(&raw)
                .map(|id| Some(id.relay_url) != url)
                .unwrap_or(true);
            if stale {
                self.forget_identity().await;
            }
        }
        self.disconnect();
        if !s.remote_enabled {
            self.cancel_pairing_inner();
            return self.set_state("off", None);
        }
        let Some(url) = url else {
            return self.set_state("no_relay", Some("invalid relay URL".to_string()));
        };
        let Some(identity) = self.identity(&url).await else {
            return self.set_state("unpaired", None);
        };
        let peers = self.db.list_remote_peers().unwrap_or_default();
        if peers.is_empty() {
            return self.set_state("unpaired", None);
        }
        let kp = match self.keypair().await {
            Ok(k) => k,
            Err(e) => return self.set_state("stopped", Some(format!("cannot load the remote key: {e:#}"))),
        };
        let api = match RelayApi::new(&url) {
            Ok(a) => a,
            Err(e) => return self.set_state("no_relay", Some(format!("{e:#}"))),
        };
        let pairs: Vec<(String, String)> = peers.into_iter().map(|p| (p.device_id, p.public_key)).collect();
        let book = PeerBook::new(&kp, &identity.device_id, &pairs);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let link = api.connect(identity.device_token.clone(), tx);
        let gen = {
            let mut inner = self.lock();
            inner.link_gen += 1;
            inner.link = Some(link);
            inner.book = Some(book);
            inner.state = "connecting";
            inner.error = None;
            inner.link_gen
        };
        self.emit_status();
        let me = self.arc();
        tauri::async_runtime::spawn(async move {
            while let Some(ev) = rx.recv().await {
                if me.lock().link_gen != gen {
                    break;
                }
                me.on_link_event(ev).await;
            }
        });
    }

    fn disconnect(&self) {
        let mut inner = self.lock();
        inner.link_gen += 1;
        inner.link = None;
        inner.book = None;
        inner.online.clear();
        inner.subs.clear();
        inner.term_attach.clear();
    }

    /// Rebuild the peer book from the DB (after adding or removing a phone)
    /// without dropping the connection.
    async fn reload_peers(&self) {
        let Ok(url) = jcode_remote::relay::normalize_relay_url(RELAY_URL) else { return };
        let (Some(identity), Ok(kp)) = (self.identity(&url).await, self.keypair().await) else { return };
        let pairs: Vec<(String, String)> = self
            .db
            .list_remote_peers()
            .unwrap_or_default()
            .into_iter()
            .map(|p| (p.device_id, p.public_key))
            .collect();
        let book = PeerBook::new(&kp, &identity.device_id, &pairs);
        let mut inner = self.lock();
        let keep: HashSet<String> = book.ids().cloned().collect();
        inner.subs.retain(|id, _| keep.contains(id));
        inner.term_attach.retain(|id, _| keep.contains(id));
        inner.online.retain(|id| keep.contains(id));
        inner.book = Some(book);
    }

    async fn on_link_event(&self, ev: LinkEvent) {
        match ev {
            LinkEvent::Connecting => {
                let first = self.lock().state != "offline";
                if first {
                    self.set_state("connecting", None);
                }
            }
            LinkEvent::Ready { peers, .. } => {
                let now = now_ms();
                {
                    let mut inner = self.lock();
                    let known: HashSet<String> = inner.book.as_ref().map(|b| b.ids().cloned().collect()).unwrap_or_default();
                    inner.online = peers
                        .into_iter()
                        .filter(|(id, online)| *online && known.contains(id))
                        .map(|(id, _)| id)
                        .collect();
                    for id in &inner.online {
                        let _ = self.db.touch_remote_peer(id, now);
                    }
                }
                self.set_state("online", None);
            }
            LinkEvent::Presence { device_id, online } => {
                {
                    let mut inner = self.lock();
                    if !inner.book.as_ref().is_some_and(|b| b.contains(&device_id)) {
                        return;
                    }
                    if online {
                        inner.online.insert(device_id.clone());
                    } else {
                        inner.online.remove(&device_id);
                        inner.subs.remove(&device_id);
                        inner.term_attach.remove(&device_id);
                    }
                }
                let _ = self.db.touch_remote_peer(&device_id, now_ms());
                self.emit_status();
            }
            LinkEvent::Msg { from, body } => self.on_message(from, body),
            LinkEvent::Unpaired { device_id } => {
                // The phone unpaired itself (or was revoked on the relay).
                let _ = self.db.delete_remote_peer(&device_id);
                self.reload_peers().await;
                self.emit_status();
            }
            LinkEvent::Undeliverable { to, .. } => {
                let mut inner = self.lock();
                inner.online.remove(&to);
                inner.subs.remove(&to);
                inner.term_attach.remove(&to);
            }
            LinkEvent::Disconnected { error } => {
                {
                    let mut inner = self.lock();
                    inner.online.clear();
                    inner.subs.clear();
                    inner.term_attach.clear();
                }
                self.set_state("offline", Some(error));
            }
            LinkEvent::Stopped { revoked, error } => {
                if revoked {
                    self.forget_identity().await;
                }
                self.disconnect();
                self.set_state("stopped", Some(error));
            }
        }
    }

    fn on_message(&self, from: String, body: String) {
        let m = {
            let mut inner = self.lock();
            let Some(book) = inner.book.as_mut() else { return };
            match book.open(&from, &body, now_ms()) {
                Some(m) => m,
                None => return, // unknown phone, tampering or replay: drop silently
            }
        };
        let Ok(AppMessage::Req { id, method, params }) = serde_json::from_value::<AppMessage>(m) else {
            return;
        };
        self.lock().online.insert(from.clone());
        let me = self.arc();
        tauri::async_runtime::spawn(async move {
            let res = match me.handle_request(&from, &method, params).await {
                Ok(v) => AppMessage::ok(id, v),
                Err(e) => AppMessage::err(id, e),
            };
            me.send_to(&from, &res);
        });
    }

    fn send_to(&self, peer: &str, m: &AppMessage) {
        let Ok(v) = serde_json::to_value(m) else { return };
        let inner = self.lock();
        let (Some(book), Some(link)) = (inner.book.as_ref(), inner.link.as_ref()) else { return };
        if let Some(body) = book.seal(peer, &v, now_ms()) {
            link.send(peer, body);
        }
    }

    // ------------------------------------------------------------------
    // Requests from the phone (§6.2)
    // ------------------------------------------------------------------

    async fn db_call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Db) -> anyhow::Result<T> + Send + 'static,
    ) -> Result<T, AppError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || f(&db))
            .await
            .map_err(internal)?
            .map_err(internal)
    }

    fn str_param<'a>(params: &'a Value, key: &str) -> Result<&'a str, AppError> {
        params
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty() && s.len() <= 200)
            .ok_or_else(|| bad(format!("missing {key}")))
    }

    async fn session(&self, id: &str) -> Result<SessionSummary, AppError> {
        let id = id.to_string();
        self.db_call(move |db| db.get_session(&id))
            .await?
            .ok_or_else(|| AppError::new("not_found", "session not found"))
    }

    /// The permission mode a turn in this session runs with.
    async fn effective_policy(&self, s: &SessionSummary) -> Result<PolicyMode, AppError> {
        if let Some(m) = self.rt()?.policy_mode(&s.id).or(s.policy_mode) {
            return Ok(m);
        }
        let pid = s.agent_profile_id.clone();
        let profile_mode = self
            .db_call(move |db| {
                Ok(pid.and_then(|pid| {
                    db.list_agent_profiles()
                        .ok()
                        .and_then(|l| l.into_iter().find(|p| p.id == pid))
                        .map(|p| p.policy_mode)
                }))
            })
            .await?;
        Ok(profile_mode.unwrap_or_default())
    }

    /// The desktop setting that unlocks full access, terminals and writes.
    fn allow_full_access(&self) -> bool {
        self.settings().remote_allow_full_access
    }

    fn capabilities(&self) -> Value {
        let on = self.allow_full_access();
        json!({ "full_access": on, "terminal": on, "edit": on })
    }

    /// The setting changed: tell every online phone, and when it was turned
    /// off, detach their terminals and close the ones they opened.
    pub fn capabilities_changed(&self) {
        if !self.allow_full_access() {
            self.lock().term_attach.clear();
            if let Some(t) = self.terminals.get() {
                for info in t.infos().into_iter().filter(|i| i.origin == TerminalOrigin::Remote) {
                    let _ = t.kill(&info.id);
                }
            }
        }
        let peers: Vec<String> = self.lock().online.iter().cloned().collect();
        self.send_event_to(&peers, "capabilities", self.capabilities());
        self.emit_status();
    }

    async fn check_not_yolo(&self, s: &SessionSummary) -> Result<(), AppError> {
        if !self.allow_full_access() && self.effective_policy(s).await? == PolicyMode::Yolo {
            return Err(AppError::new(
                "yolo_blocked",
                "This session uses Full access. Change its mode, or allow Full access from the phone on the desktop (Settings → Remote control).",
            ));
        }
        Ok(())
    }

    fn attachments_param(params: &Value) -> Result<Vec<String>, AppError> {
        let Some(list) = params.get("attachments").and_then(|v| v.as_array()) else {
            return Ok(vec![]);
        };
        if list.len() > 20 {
            return Err(bad("at most 20 attached files"));
        }
        Ok(list.iter().filter_map(|v| v.as_str()).filter(|p| !p.is_empty() && p.len() <= 1000).map(str::to_string).collect())
    }

    /// Images from the phone (§6.6). The runtime checks vision support and
    /// per-image limits; this caps count and total size.
    fn images_param(params: &Value) -> Result<Vec<ImageAttachment>, AppError> {
        let Some(list) = params.get("images").and_then(|v| v.as_array()) else {
            return Ok(vec![]);
        };
        if list.len() > 4 {
            return Err(bad("at most 4 images"));
        }
        let images: Vec<ImageAttachment> = list
            .iter()
            .map(|v| serde_json::from_value(v.clone()).map_err(|_| bad("invalid image")))
            .collect::<Result<_, _>>()?;
        let total: usize = images.iter().map(|i| i.data.len()).sum();
        if total > 8 * 1024 * 1024 {
            return Err(bad("images are too large"));
        }
        for i in &images {
            if !matches!(i.mime.as_str(), "image/png" | "image/jpeg" | "image/gif" | "image/webp") {
                return Err(bad("unsupported image type"));
            }
        }
        Ok(images)
    }

    fn content_param(params: &Value) -> Result<String, AppError> {
        let c = params.get("content").and_then(|v| v.as_str()).unwrap_or_default().trim();
        if c.is_empty() {
            return Err(bad("empty message"));
        }
        if c.chars().count() > MAX_CONTENT_CHARS {
            return Err(bad("message too long"));
        }
        Ok(c.to_string())
    }

    async fn handle_request(&self, peer: &str, method: &str, params: Value) -> Result<Value, AppError> {
        if !self.settings().remote_enabled {
            return Err(AppError::new("remote_disabled", "remote control is turned off on the desktop"));
        }
        match method {
            "hello" => Ok(json!({
                "desktop_version": env!("CARGO_PKG_VERSION"),
                "desktop_name": desktop_name(),
                "capabilities": self.capabilities(),
            })),
            "session.set_policy" => {
                let sid = Self::str_param(&params, "session_id")?;
                let mode = match Self::str_param(&params, "mode")? {
                    "safe" => PolicyMode::Safe,
                    "balanced" => PolicyMode::Balanced,
                    "yolo" => PolicyMode::Yolo,
                    _ => return Err(bad("mode must be safe, balanced or yolo")),
                };
                if mode == PolicyMode::Yolo && !self.allow_full_access() {
                    return Err(crate::remote_ops::not_allowed());
                }
                let session = self.session(sid).await?;
                let rt = self.rt()?;
                if let Some(ws) = self.db.get_workspace(&session.workspace_id).ok().flatten() {
                    rt.ensure_session_policy(sid, &ws.root_path, mode);
                }
                rt.set_policy_mode(sid, mode).await.map_err(internal)?;
                let _ = self.app.emit(EVENT_REMOTE_SESSIONS, json!({ "workspace_id": session.workspace_id }));
                Ok(json!({ "session": self.session(sid).await? }))
            }
            "checkpoints.list" => {
                let sid = Self::str_param(&params, "session_id")?.to_string();
                self.session(&sid).await?;
                let rt = self.rt()?.clone();
                let list = tokio::task::spawn_blocking(move || rt.list_checkpoints(&sid)).await.map_err(internal)?;
                Ok(json!({ "checkpoints": list }))
            }
            "checkpoints.restore" => {
                if !self.allow_full_access() {
                    return Err(crate::remote_ops::not_allowed());
                }
                let sid = Self::str_param(&params, "session_id")?.to_string();
                let session = self.session(&sid).await?;
                let root = crate::remote_ops::workspace_root(&self.db, &session.workspace_id)?;
                let rt = self.rt()?.clone();
                let n = tokio::task::spawn_blocking(move || rt.restore_checkpoints(&sid, &root))
                    .await
                    .map_err(internal)?
                    .map_err(|e| bad(format!("{e:#}")))?;
                Ok(json!({ "restored": n }))
            }
            m if m.starts_with("terminal.") => self.handle_terminal(peer, m, &params).await,
            m if m.starts_with("agent_clis.") => self.handle_agent_clis(m, &params).await,
            "workspaces.list" => {
                let ws = self.db_call(|db| db.list_workspaces()).await?;
                Ok(json!({ "workspaces": ws }))
            }
            "sessions.list" => {
                let wid = Self::str_param(&params, "workspace_id")?.to_string();
                let sessions = self.db_call(move |db| db.list_sessions(Some(&wid))).await?;
                Ok(json!({ "sessions": sessions }))
            }
            "session.new" => self.new_session(Self::str_param(&params, "workspace_id")?).await,
            "session.history" => {
                let sid = Self::str_param(&params, "session_id")?.to_string();
                let session = self.session(&sid).await?;
                let busy = self.rt()?.is_busy(&sid);
                let (messages, tool_runs, approvals) = self
                    .db_call(move |db| {
                        let mut messages = db.list_messages(&sid)?;
                        for m in &mut messages {
                            m.images.clear();
                        }
                        Ok((messages, db.list_tool_runs(&sid)?, db.list_approvals(&sid)?))
                    })
                    .await?;
                Ok(json!({
                    "session": session,
                    "messages": messages,
                    "tool_runs": tool_runs,
                    "approvals": approvals,
                    "busy": busy,
                }))
            }
            "session.subscribe" => {
                let sid = Self::str_param(&params, "session_id")?;
                self.session(sid).await?;
                self.lock().subs.entry(peer.to_string()).or_default().insert(sid.to_string());
                Ok(json!({}))
            }
            "session.unsubscribe" => {
                let sid = Self::str_param(&params, "session_id")?;
                if let Some(set) = self.lock().subs.get_mut(peer) {
                    set.remove(sid);
                }
                Ok(json!({}))
            }
            "chat.send" => {
                let sid = Self::str_param(&params, "session_id")?;
                let content = Self::content_param(&params)?;
                let session = self.session(sid).await?;
                self.check_not_yolo(&session).await?;
                let attachments = Self::attachments_param(&params)?;
                let images = Self::images_param(&params)?;
                let rt = self.rt()?;
                if rt.is_busy(sid) {
                    return Err(AppError::new("busy", "a turn is already running in this session"));
                }
                rt.send_message(sid, &content, &attachments, images)
                    .await
                    .map_err(|e| bad(format!("{e:#}")))?;
                Ok(json!({}))
            }
            "chat.steer" => {
                let sid = Self::str_param(&params, "session_id")?;
                let content = Self::content_param(&params)?;
                let session = self.session(sid).await?;
                self.check_not_yolo(&session).await?;
                Ok(json!({ "message_id": self.rt()?.steer(sid, &content) }))
            }
            "chat.cancel" => {
                let sid = Self::str_param(&params, "session_id")?;
                self.rt()?.cancel(sid).await.map_err(internal)?;
                Ok(json!({}))
            }
            "approval.decide" => {
                let aid = Self::str_param(&params, "approval_id")?;
                let approved = params
                    .get("approved")
                    .and_then(|v| v.as_bool())
                    .ok_or_else(|| bad("missing approved"))?;
                // Never a grant: a remote approval covers this one action only.
                self.rt()?
                    .decide_approval(aid, approved, None)
                    .await
                    .map_err(|e| AppError::new("not_found", format!("{e:#}")))?;
                Ok(json!({}))
            }
            _ => {
                let (m, p, edit) = (method.to_string(), params, self.allow_full_access());
                let db = self.db.clone();
                let r = tokio::task::spawn_blocking(move || crate::remote_ops::dispatch(&db, &m, &p, edit))
                    .await
                    .map_err(internal)?;
                if matches!(method, "session.rename" | "session.delete" | "session.set_agent" | "session.set_model") {
                    if let Some(Ok(_)) = &r {
                        let _ = self.app.emit(EVENT_REMOTE_SESSIONS, json!({ "workspace_id": null }));
                    }
                }
                r.unwrap_or_else(|| Err(bad(format!("unknown method {method}"))))
            }
        }
    }

    /// A new session in `workspace_id`, using the agent and model of the
    /// project's most recent session (a session without a model cannot
    /// run). Never starts in full access.
    async fn new_session(&self, workspace_id: &str) -> Result<Value, AppError> {
        let wid = workspace_id.to_string();
        let (session, root, mode) = self
            .db_call(move |db| {
                let ws = db.get_workspace(&wid)?.ok_or_else(|| anyhow::anyhow!("workspace not found"))?;
                let base = db.list_sessions(Some(&wid))?.into_iter().next();
                let mut s = db.create_session(
                    &wid,
                    "New session",
                    base.as_ref().and_then(|b| b.agent_profile_id.as_deref()),
                    base.as_ref().and_then(|b| b.provider_id.as_deref()),
                    base.as_ref().and_then(|b| b.model_name.as_deref()),
                )?;
                let mut mode = db.get_settings().unwrap_or_default().default_policy_mode;
                if mode == PolicyMode::Yolo {
                    mode = PolicyMode::Safe;
                }
                db.set_session_policy_mode(&s.id, mode.as_str())?;
                s.policy_mode = Some(mode);
                Ok((s, ws.root_path, mode))
            })
            .await?;
        self.rt()?.ensure_session_policy(&session.id, &root, mode);
        let _ = self.app.emit(EVENT_REMOTE_SESSIONS, json!({ "workspace_id": session.workspace_id }));
        Ok(json!({ "session": session }))
    }

    // ------------------------------------------------------------------
    // Pairing (§4, §5.2)
    // ------------------------------------------------------------------

    pub async fn start_pairing(&self) -> Result<PairingStarted, String> {
        let s = self.settings();
        if !s.remote_enabled {
            return Err("turn on remote control first".into());
        }
        let api = self.relay()?;
        self.cancel_pairing_inner();
        let kp = self.keypair().await.map_err(|e| format!("{e:#}"))?;
        let identity = self.identity(api.base()).await;
        let name = desktop_name();
        let created = api
            .create_pairing(&kp.public_b64u(), &name, identity.as_ref().map(|i| i.device_token.as_str()))
            .await
            .map_err(|e| format!("{e:#}"))?;
        let uri = pairing_uri(&created.code, &kp.public_b64u(), &name);
        let svg = qr_svg(&uri).map_err(|e| format!("{e:#}"))?;

        let me = self.arc();
        let pairing_id = created.pairing_id.clone();
        let desktop_pk = kp.public();
        let task = tauri::async_runtime::spawn(async move {
            let outcome = api.wait_pairing(&created.pairing_id, &created.wait_secret).await;
            let ev = match outcome {
                Ok(PairingOutcome::Approved {
                    device_id,
                    device_token,
                    mobile_device_id,
                    mobile_name,
                    mobile_public_key,
                    user_email,
                }) => match crypto::public_key_from_b64u(&mobile_public_key) {
                    Ok(mobile_pk) => {
                        let sas = crypto::sas(&desktop_pk, &mobile_pk);
                        me.lock().pending = Some(PendingPairing {
                            pairing_id: created.pairing_id.clone(),
                            desktop_id: device_id,
                            new_token: device_token,
                            peer: RemotePeer {
                                device_id: mobile_device_id,
                                name: mobile_name.chars().take(64).collect(),
                                public_key: mobile_public_key,
                                user_email: user_email.clone(),
                                created_at: now_ms(),
                                last_seen: None,
                            },
                        });
                        PairingEvent {
                            pairing_id: created.pairing_id.clone(),
                            state: "approved",
                            sas: Some(sas),
                            mobile_name: Some(mobile_name),
                            user_email: Some(user_email),
                            error: None,
                        }
                    }
                    Err(e) => PairingEvent::error(&created.pairing_id, format!("{e:#}")),
                },
                Ok(PairingOutcome::Expired) => PairingEvent {
                    state: "expired",
                    ..PairingEvent::error(&created.pairing_id, String::new())
                },
                Err(e) => PairingEvent::error(&created.pairing_id, format!("{e:#}")),
            };
            let _ = me.app.emit(EVENT_REMOTE_PAIRING, ev);
        });
        self.lock().pairing_task = Some(task);
        Ok(PairingStarted {
            pairing_id,
            qr_svg: svg,
            expires_at: created.expires_at,
        })
    }

    /// After the user compared the 6-digit codes: keep or undo the pairing.
    pub async fn confirm_pairing(&self, pairing_id: &str, accept: bool) -> Result<RemoteStatus, String> {
        let pending = {
            let mut inner = self.lock();
            match inner.pending.take() {
                Some(p) if p.pairing_id == pairing_id => p,
                other => {
                    inner.pending = other;
                    return Err("this pairing is no longer pending".into());
                }
            }
        };
        let api = self.relay()?;
        let existing = self.identity(api.base()).await;
        let token = pending
            .new_token
            .clone()
            .or_else(|| existing.as_ref().map(|i| i.device_token.clone()))
            .ok_or("this desktop has no relay token")?;
        if accept {
            if pending.new_token.is_some() {
                let id = DeviceIdentity {
                    relay_url: api.base().to_string(),
                    device_id: pending.desktop_id.clone(),
                    device_token: token,
                };
                let raw = serde_json::to_string(&id).map_err(|e| e.to_string())?;
                self.creds.set(DEVICE_ACCOUNT, &raw).await.map_err(|e| format!("{e:#}"))?;
            }
            self.db.upsert_remote_peer(&pending.peer).map_err(|e| format!("{e:#}"))?;
            let connected = self.lock().link.is_some();
            if connected {
                self.reload_peers().await;
                self.emit_status();
            } else {
                self.apply_settings().await;
            }
        } else {
            // Undo on the relay too. A desktop registered by this pairing
            // removes itself again.
            let _ = api.delete_peer(&token, &pending.peer.device_id).await;
            if pending.new_token.is_some() {
                let _ = api.delete_device(&token, &pending.desktop_id).await;
            }
        }
        Ok(self.snapshot())
    }

    fn cancel_pairing_inner(&self) -> Option<PendingPairing> {
        let mut inner = self.lock();
        if let Some(t) = inner.pairing_task.take() {
            t.abort();
        }
        inner.pending.take()
    }

    pub async fn cancel_pairing(&self) {
        if let Some(p) = self.cancel_pairing_inner() {
            let id = p.pairing_id.clone();
            self.lock().pending = Some(p);
            let _ = self.confirm_pairing(&id, false).await;
        }
    }

    /// Remove a phone: effective at once locally, then on the relay.
    pub async fn revoke_device(&self, device_id: &str) -> Result<RemoteStatus, String> {
        self.db.delete_remote_peer(device_id).map_err(|e| format!("{e:#}"))?;
        self.reload_peers().await;
        if let Ok(api) = self.relay() {
            if let Some(id) = self.identity(api.base()).await {
                let _ = api.delete_peer(&id.device_token, device_id).await;
            }
        }
        if self.db.list_remote_peers().map(|p| p.is_empty()).unwrap_or(false) {
            self.apply_settings().await;
        }
        let status = self.snapshot();
        let _ = self.app.emit(EVENT_REMOTE_STATUS, status.clone());
        Ok(status)
    }

    /// Unregister this desktop from the relay and forget every phone.
    pub async fn forget_desktop(&self) -> Result<RemoteStatus, String> {
        self.cancel_pairing_inner();
        if let Ok(api) = self.relay() {
            if let Some(id) = self.identity(api.base()).await {
                let _ = api.delete_device(&id.device_token, &id.device_id).await;
            }
        }
        self.forget_identity().await;
        self.apply_settings().await;
        Ok(self.snapshot())
    }

    // ------------------------------------------------------------------
    // Terminals (§6.5)
    // ------------------------------------------------------------------

    fn terms(&self) -> Result<&TerminalManager, AppError> {
        self.terminals.get().ok_or_else(|| internal("terminals not ready"))
    }

    fn terminal_json(&self, info: &TerminalInfo, workspaces: &[WorkspaceInfo]) -> Value {
        let cwd = std::fs::canonicalize(&info.cwd).unwrap_or_else(|_| info.cwd.clone());
        let workspace_id = workspaces
            .iter()
            .find(|w| std::fs::canonicalize(&w.root_path).map(|p| p == cwd).unwrap_or(false))
            .map(|w| w.id.clone());
        json!({
            "id": info.id,
            "title": info.title,
            "kind": if info.agent { "agent" } else { "shell" },
            "origin": info.origin.as_str(),
            "workspace_id": workspace_id,
            "cols": info.cols,
            "rows": info.rows,
            "bypass": info.bypass,
        })
    }

    async fn handle_terminal(&self, peer: &str, method: &str, params: &Value) -> Result<Value, AppError> {
        if !self.allow_full_access() {
            return Err(crate::remote_ops::not_allowed());
        }
        let terms = self.terms()?.clone();
        let workspaces = self.db.list_workspaces().unwrap_or_default();
        let tid = || Self::str_param(params, "terminal_id");
        let info_of = |id: &str| terms.info(id).ok_or_else(|| AppError::new("not_found", "terminal not found"));
        let size = |key: &str, lo: u64, hi: u64, def: u64| -> u16 {
            params.get(key).and_then(|v| v.as_u64()).unwrap_or(def).clamp(lo, hi) as u16
        };
        match method {
            "terminal.list" => {
                let list: Vec<Value> = terms.infos().iter().map(|i| self.terminal_json(i, &workspaces)).collect();
                Ok(json!({ "terminals": list }))
            }
            "terminal.open" => {
                let shells = terms.infos().iter().filter(|i| i.origin == TerminalOrigin::Remote && !i.agent).count();
                let title = format!("phone shell {}", shells + 1);
                let (cols, rows) = (size("cols", 20, 400, 80), size("rows", 5, 200, 24));
                let info = self.open_remote_terminal(params, title, None, false, cols, rows).await?;
                Ok(json!({ "terminal": self.terminal_json(&info, &workspaces) }))
            }
            "terminal.attach" => {
                let id = tid()?;
                let info = info_of(id)?;
                // Attach first, then snapshot: output in between is in the
                // scrollback (events the phone gets before this response are
                // dropped by it, see §6.5), so nothing is lost.
                self.lock().term_attach.entry(peer.to_string()).or_default().insert(id.to_string());
                let scrollback = terms.scrollback(id).unwrap_or_default();
                Ok(json!({ "terminal": self.terminal_json(&info, &workspaces), "scrollback": scrollback }))
            }
            "terminal.detach" => {
                let id = tid()?;
                if let Some(set) = self.lock().term_attach.get_mut(peer) {
                    set.remove(id);
                }
                Ok(json!({}))
            }
            "terminal.input" => {
                let id = tid()?.to_string();
                let data = params.get("data").and_then(|v| v.as_str()).ok_or_else(|| bad("missing data"))?.to_string();
                if data.len() > TERM_EVENT_BYTES {
                    return Err(bad("input too long"));
                }
                info_of(&id)?;
                tokio::task::spawn_blocking(move || terms.write(&id, &data))
                    .await
                    .map_err(internal)?
                    .map_err(internal)?;
                Ok(json!({}))
            }
            "terminal.resize" | "terminal.close" => {
                let id = tid()?;
                if info_of(id)?.origin != TerminalOrigin::Remote {
                    return Err(AppError::new("not_allowed", "only terminals opened from the phone can be resized or closed"));
                }
                if method == "terminal.resize" {
                    terms.resize(id, size("cols", 20, 400, 80), size("rows", 5, 200, 24)).map_err(internal)?;
                } else {
                    let (t, id) = (terms.clone(), id.to_string());
                    tokio::task::spawn_blocking(move || t.kill(&id)).await.map_err(internal)?.map_err(internal)?;
                }
                Ok(json!({}))
            }
            _ => Err(bad(format!("unknown method {method}"))),
        }
    }

    /// Start a terminal for the phone in the project named by
    /// `params.workspace_id` (a shell, or an agent CLI when `command` is set).
    async fn open_remote_terminal(
        &self,
        params: &Value,
        title: String,
        command: Option<String>,
        bypass: bool,
        cols: u16,
        rows: u16,
    ) -> Result<TerminalInfo, AppError> {
        let terms = self.terms()?.clone();
        let root = crate::remote_ops::workspace_root(&self.db, Self::str_param(params, "workspace_id")?)?;
        let remote = terms.infos().iter().filter(|i| i.origin == TerminalOrigin::Remote).count();
        if remote >= MAX_REMOTE_TERMINALS {
            return Err(bad(format!("at most {MAX_REMOTE_TERMINALS} terminals can be opened from the phone")));
        }
        let id = format!("remote-{}", uuid::Uuid::new_v4());
        let (app, t, id2) = (self.app.clone(), terms.clone(), id.clone());
        tokio::task::spawn_blocking(move || {
            let opts = TerminalOptions { command: command.as_deref(), title: Some(&title), origin: TerminalOrigin::Remote, bypass };
            t.create_with(&app, &id2, std::path::Path::new(&root), cols, rows, opts)
        })
        .await
        .map_err(internal)?
        .map_err(internal)?;
        terms.info(&id).ok_or_else(|| AppError::new("not_found", "the terminal exited at once"))
    }

    /// §6.9: agent CLI presets, launching one, and their usage.
    async fn handle_agent_clis(&self, method: &str, params: &Value) -> Result<Value, AppError> {
        let clis: Vec<AgentCli> = self.settings().agent_clis.into_iter().filter(|a| a.enabled).collect();
        let flag = |a: &Option<String>| a.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        match method {
            "agent_clis.list" => {
                let commands: Vec<String> = clis.iter().map(|a| a.command.clone()).collect();
                let installed = tokio::task::spawn_blocking(move || crate::agent_detect::detect(&commands))
                    .await
                    .unwrap_or_default();
                let list: Vec<Value> = clis
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        json!({
                            "id": a.id,
                            "name": a.name,
                            "command": a.command,
                            "installed": installed.get(i).copied().unwrap_or(false),
                            "has_bypass": flag(&a.bypass_args).is_some(),
                            "has_resume": flag(&a.resume_args).is_some(),
                        })
                    })
                    .collect();
                Ok(json!({ "clis": list }))
            }
            "agent_clis.open" => {
                if !self.allow_full_access() {
                    return Err(crate::remote_ops::not_allowed());
                }
                let id = Self::str_param(params, "cli_id")?;
                let cli = clis.iter().find(|a| a.id == id).ok_or_else(|| AppError::new("not_found", "agent CLI not found"))?;
                let want = |k: &str| params.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
                let resume = if want("resume") { flag(&cli.resume_args) } else { None };
                let bypass = if want("bypass") { flag(&cli.bypass_args) } else { None };
                // Only the user's own preset strings are run, never phone text.
                let is_bypass = bypass.is_some();
                let command = [Some(cli.command.trim().to_string()), resume, bypass]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                let size = |key: &str, lo: u64, hi: u64, def: u64| -> u16 {
                    params.get(key).and_then(|v| v.as_u64()).unwrap_or(def).clamp(lo, hi) as u16
                };
                let info = self
                    .open_remote_terminal(params, cli.name.clone(), Some(command), is_bypass, size("cols", 20, 400, 80), size("rows", 5, 200, 24))
                    .await?;
                let workspaces = self.db.list_workspaces().unwrap_or_default();
                Ok(json!({ "terminal": self.terminal_json(&info, &workspaces) }))
            }
            "agent_clis.usage" => Ok(json!({ "reports": crate::agent_usage::collect().await })),
            _ => Err(bad(format!("unknown method {method}"))),
        }
    }

    fn attached_peers(&self, terminal_id: &str) -> Vec<String> {
        let inner = self.lock();
        inner
            .term_attach
            .iter()
            .filter(|(peer, ids)| ids.contains(terminal_id) && inner.online.contains(*peer))
            .map(|(peer, _)| peer.clone())
            .collect()
    }

    /// Called from the PTY reader threads.
    fn on_terminal_event(&self, ev: &TerminalEvent) {
        match ev {
            TerminalEvent::Data { id, data } => {
                if self.attached_peers(id).is_empty() {
                    return;
                }
                let (schedule, flush_now) = {
                    let mut out = self.term_out.lock().unwrap_or_else(|e| e.into_inner());
                    let buf = out.entry(id.clone()).or_default();
                    let first = buf.is_empty();
                    buf.push_str(data);
                    (first, buf.len() >= TERM_EVENT_BYTES)
                };
                if flush_now {
                    self.flush_terminal(id);
                } else if schedule {
                    let me = self.arc();
                    let id = id.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(TERM_INTERVAL).await;
                        me.flush_terminal(&id);
                    });
                }
            }
            TerminalEvent::Exit { id, code } => {
                self.flush_terminal(id);
                let peers = self.attached_peers(id);
                self.send_event_to(&peers, "terminal.exit", json!({ "terminal_id": id, "code": code }));
                for set in self.lock().term_attach.values_mut() {
                    set.remove(id);
                }
            }
            TerminalEvent::Resized { id, cols, rows } => {
                let peers = self.attached_peers(id);
                self.send_event_to(&peers, "terminal.resized", json!({ "terminal_id": id, "cols": cols, "rows": rows }));
            }
        }
    }

    fn flush_terminal(&self, id: &str) {
        let Some(text) = self.term_out.lock().unwrap_or_else(|e| e.into_inner()).remove(id) else { return };
        let peers = self.attached_peers(id);
        let mut rest = text.as_str();
        while !rest.is_empty() {
            let mut cut = rest.len().min(TERM_EVENT_BYTES);
            while !rest.is_char_boundary(cut) {
                cut -= 1;
            }
            let (chunk, tail) = rest.split_at(cut);
            self.send_event_to(&peers, "terminal.data", json!({ "terminal_id": id, "data": chunk }));
            rest = tail;
        }
    }

    // ------------------------------------------------------------------
    // Events to phones (§6.3)
    // ------------------------------------------------------------------

    /// Send an event to specific online phones (not tied to a session).
    fn send_event_to(&self, peers: &[String], name: &str, data: Value) {
        if peers.is_empty() {
            return;
        }
        let inner = self.lock();
        let (Some(book), Some(link)) = (inner.book.as_ref(), inner.link.as_ref()) else { return };
        let msg = json!({ "t": "event", "name": name, "data": data });
        let now = now_ms();
        for peer in peers {
            if inner.online.contains(peer) {
                if let Some(body) = book.seal(peer, &msg, now) {
                    link.send(peer, body);
                }
            }
        }
    }

    fn has_subscribers(&self, session_id: &str) -> bool {
        self.lock().subs.values().any(|s| s.contains(session_id))
    }

    fn send_event(&self, session_id: &str, name: &str, data: Value) {
        let inner = self.lock();
        let (Some(book), Some(link)) = (inner.book.as_ref(), inner.link.as_ref()) else { return };
        let msg = json!({ "t": "event", "name": name, "data": data });
        let now = now_ms();
        for (peer, sessions) in &inner.subs {
            if sessions.contains(session_id) && inner.online.contains(peer) {
                if let Some(body) = book.seal(peer, &msg, now) {
                    link.send(peer, body);
                }
            }
        }
    }

    /// Flush buffered deltas for the session, then send `name`.
    fn send_ordered(&self, session_id: &str, name: &str, data: Value) {
        if !self.has_subscribers(session_id) {
            return;
        }
        self.deltas.flush_then(session_id, || self.send_event(session_id, name, data));
    }

    fn push_delta(&self, session_id: &str, message_id: &str, delta: &str, reasoning: bool) {
        if !self.has_subscribers(session_id) {
            return;
        }
        if let DeltaAction::ScheduleFlush = self.deltas.push(session_id, message_id, delta, reasoning) {
            let me = self.arc();
            let sid = session_id.to_string();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(DELTA_INTERVAL).await;
                me.deltas.flush(&sid);
            });
        }
    }
}

impl PairingEvent {
    fn error(pairing_id: &str, error: String) -> Self {
        Self {
            pairing_id: pairing_id.to_string(),
            state: "error",
            sas: None,
            mobile_name: None,
            user_email: None,
            error: (!error.is_empty()).then_some(error),
        }
    }
}

#[async_trait]
impl EventSink for RemoteController {
    async fn status(&self, session_id: &str, status: ChatStatus, detail: Option<String>) {
        self.send_ordered(session_id, "status", json!({ "session_id": session_id, "status": status, "detail": detail }));
    }

    async fn delta(&self, session_id: &str, message_id: &str, delta: &str) {
        self.push_delta(session_id, message_id, delta, false);
    }

    async fn reasoning_delta(&self, session_id: &str, message_id: &str, delta: &str) {
        self.push_delta(session_id, message_id, delta, true);
    }

    async fn message(&self, session_id: &str, message: &MessageRecord) {
        if !self.has_subscribers(session_id) {
            return;
        }
        let mut m = message.clone();
        m.images.clear();
        self.send_ordered(session_id, "message", json!({ "session_id": session_id, "message": m }));
    }

    async fn tool_run(&self, session_id: &str, run: &ToolRunRecord) {
        self.send_ordered(session_id, "tool_run", json!({ "session_id": session_id, "run": run }));
    }

    async fn approval(&self, session_id: &str, approval: &ApprovalRecord) {
        self.send_ordered(session_id, "approval", json!({ "session_id": session_id, "approval": approval }));
    }

    async fn usage(&self, session_id: &str, usage: TokenUsage, cost_estimate: Option<f64>) {
        self.send_ordered(
            session_id,
            "usage",
            json!({
                "session_id": session_id,
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
                "cost_estimate": cost_estimate,
            }),
        );
    }

    async fn error(&self, session_id: &str, error: &str) {
        self.send_ordered(session_id, "error", json!({ "session_id": session_id, "error": error }));
    }
}

// ----------------------------------------------------------------------
// Tauri commands
// ----------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct MobileDownload {
    pub url: String,
    /// QR code of `url` (SVG), for scanning with the phone's camera.
    pub qr_svg: String,
}

#[tauri::command]
pub fn remote_mobile_download() -> Result<MobileDownload, String> {
    Ok(MobileDownload {
        url: MOBILE_DOWNLOAD_URL.to_string(),
        qr_svg: qr_svg(MOBILE_DOWNLOAD_URL).map_err(|e| format!("{e:#}"))?,
    })
}

/// Open the phone app's download page in the system browser. Takes no URL
/// from the webview: only this fixed address can be opened.
#[tauri::command]
pub async fn remote_open_mobile_download() -> Result<(), String> {
    let url = MOBILE_DOWNLOAD_URL;
    tokio::task::spawn_blocking(move || {
        let mut cmd = if cfg!(target_os = "macos") {
            let mut c = std::process::Command::new("open");
            c.arg(url);
            c
        } else if cfg!(windows) {
            let mut c = std::process::Command::new("rundll32");
            c.args(["url.dll,FileProtocolHandler", url]);
            c
        } else {
            let mut c = std::process::Command::new("xdg-open");
            c.arg(url);
            c
        };
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("could not open the browser: {e}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn remote_status(state: State<'_, AppState>) -> RemoteStatus {
    state.remote.snapshot()
}

/// Re-read the settings (enabled flag, relay URL) and reconnect.
#[tauri::command]
pub async fn remote_apply_settings(state: State<'_, AppState>) -> Result<RemoteStatus, String> {
    state.remote.apply_settings().await;
    Ok(state.remote.snapshot())
}

#[tauri::command]
pub async fn remote_start_pairing(state: State<'_, AppState>) -> Result<PairingStarted, String> {
    state.remote.start_pairing().await
}

#[tauri::command]
pub async fn remote_confirm_pairing(
    state: State<'_, AppState>,
    pairing_id: String,
    accept: bool,
) -> Result<RemoteStatus, String> {
    state.remote.confirm_pairing(&pairing_id, accept).await
}

#[tauri::command]
pub async fn remote_cancel_pairing(state: State<'_, AppState>) -> Result<(), String> {
    state.remote.cancel_pairing().await;
    Ok(())
}

#[tauri::command]
pub async fn remote_revoke_device(state: State<'_, AppState>, device_id: String) -> Result<RemoteStatus, String> {
    state.remote.revoke_device(&device_id).await
}

#[tauri::command]
pub async fn remote_forget_desktop(state: State<'_, AppState>) -> Result<RemoteStatus, String> {
    state.remote.forget_desktop().await
}
