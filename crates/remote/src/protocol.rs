//! Wire types (docs/REMOTE_PROTOCOL.md §4–§7). Relay frames are visible to
//! the relay; [`AppMessage`] only ever travels inside a sealed body.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Frames the client sends on `/v1/ws` and `/v1/pairings/{id}/wait`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientFrame {
    Auth {
        #[serde(skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        secret: Option<String>,
    },
    Msg { to: String, id: String, body: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PeerPresence {
    pub device_id: String,
    #[serde(default)]
    pub online: bool,
}

/// Frames the relay sends. Unknown frame types are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerFrame {
    Ready {
        device_id: String,
        #[serde(default)]
        peers: Vec<PeerPresence>,
    },
    Msg { from: String, id: String, body: String },
    Undeliverable {
        id: String,
        to: String,
        #[serde(default)]
        reason: String,
    },
    Presence { device_id: String, online: bool },
    Unpaired { device_id: String },
    Revoked,
    Error {
        #[serde(default)]
        error: String,
    },
    Approved {
        device_id: String,
        #[serde(default)]
        device_token: Option<String>,
        mobile_device_id: String,
        #[serde(default)]
        mobile_name: String,
        mobile_public_key: String,
        #[serde(default)]
        user_email: String,
    },
    Expired,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PairingCreated {
    pub pairing_id: String,
    pub wait_secret: String,
    pub code: String,
    pub expires_at: i64,
}

/// Application messages inside the encryption (§6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum AppMessage {
    Req {
        id: String,
        method: String,
        #[serde(default)]
        params: Value,
    },
    Res {
        id: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<AppError>,
    },
    Event { name: String, data: Value },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppError {
    pub code: String,
    pub message: String,
}

impl AppError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

impl AppMessage {
    pub fn ok(id: String, result: Value) -> Self {
        AppMessage::Res {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: String, error: AppError) -> Self {
        AppMessage::Res {
            id,
            ok: false,
            result: None,
            error: Some(error),
        }
    }
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The QR payload (§7).
pub fn pairing_uri(code: &str, desktop_public_b64u: &str, desktop_name: &str) -> String {
    format!(
        "jcode://pair?v=1&c={}&k={}&n={}",
        percent_encode(code),
        percent_encode(desktop_public_b64u),
        percent_encode(desktop_name)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_frames_parse_and_unknown_types_are_tolerated() {
        let f: ServerFrame = serde_json::from_str(r#"{"t":"ready","device_id":"d","peers":[{"device_id":"m","online":true}]}"#).unwrap();
        assert_eq!(
            f,
            ServerFrame::Ready {
                device_id: "d".into(),
                peers: vec![PeerPresence { device_id: "m".into(), online: true }]
            }
        );
        let f: ServerFrame = serde_json::from_str(r#"{"t":"approved","device_id":"d","device_token":null,"mobile_device_id":"m","mobile_name":"P","mobile_public_key":"k","user_email":"e"}"#).unwrap();
        assert!(matches!(f, ServerFrame::Approved { device_token: None, .. }));
        let f: ServerFrame = serde_json::from_str(r#"{"t":"something_new","x":1}"#).unwrap();
        assert_eq!(f, ServerFrame::Unknown);
    }

    #[test]
    fn client_frames_serialize_per_spec() {
        let auth = ClientFrame::Auth { token: Some("tok".into()), secret: None };
        assert_eq!(serde_json::to_string(&auth).unwrap(), r#"{"t":"auth","token":"tok"}"#);
        let msg = ClientFrame::Msg { to: "m".into(), id: "1".into(), body: "b".into() };
        assert_eq!(serde_json::to_string(&msg).unwrap(), r#"{"t":"msg","to":"m","id":"1","body":"b"}"#);
    }

    #[test]
    fn app_messages_round_trip() {
        let req: AppMessage = serde_json::from_str(r#"{"t":"req","id":"1","method":"hello","params":{"app_version":"1"}}"#).unwrap();
        assert!(matches!(req, AppMessage::Req { ref method, .. } if method == "hello"));
        let res = serde_json::to_value(AppMessage::err("1".into(), AppError::new("yolo_blocked", "x"))).unwrap();
        assert_eq!(res["ok"], false);
        assert_eq!(res["error"]["code"], "yolo_blocked");
        assert!(res.get("result").is_none());
    }

    #[test]
    fn pairing_uri_escapes_the_name() {
        assert_eq!(pairing_uri("ab-_", "K", "Irham's Mac"), "jcode://pair?v=1&c=ab-_&k=K&n=Irham%27s%20Mac");
    }
}
