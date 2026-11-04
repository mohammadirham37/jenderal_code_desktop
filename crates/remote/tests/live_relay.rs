//! Interop check against a running relay started with DEV_LOGIN:
//!
//!   DEV_LOGIN=1 go run ./cmd/jcrelay        (in jenderalcode_backend)
//!   cargo test -p jcode-remote --test live_relay -- --ignored
//!
//! Plays the desktop with this crate and the phone with plain HTTP/WS
//! calls, then exchanges sealed messages both ways.

use futures::{SinkExt, StreamExt};
use jcode_remote::crypto::{Channel, KeyPair, ReplayGuard, Side};
use jcode_remote::relay::{LinkEvent, PairingOutcome, RelayApi};
use jcode_remote::PeerBook;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

fn relay_url() -> String {
    std::env::var("JCODE_RELAY_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

async fn post(http: &reqwest::Client, path: &str, token: Option<&str>, body: Value) -> Value {
    let mut req = http.post(format!("{}{path}", relay_url())).json(&body);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let res = req.send().await.unwrap();
    let status = res.status();
    let v: Value = res.json().await.unwrap_or_default();
    assert!(status.is_success(), "{path} failed: {status} {v}");
    v
}

async fn next_event(rx: &mut mpsc::UnboundedReceiver<LinkEvent>, want: impl Fn(&LinkEvent) -> bool) -> LinkEvent {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let ev = rx.recv().await.expect("link closed");
            if want(&ev) {
                return ev;
            }
        }
    })
    .await
    .expect("timed out waiting for a link event")
}

#[tokio::test]
#[ignore]
async fn live_pairing_and_sealed_messages() {
    let http = reqwest::Client::new();
    let api = RelayApi::new(&relay_url()).unwrap();

    // Phone: dev login.
    let phone_key = KeyPair::generate();
    let login = post(
        &http,
        "/v1/auth/dev",
        None,
        json!({ "email": "live-test@example.com", "device_name": "Test phone", "public_key": phone_key.public_b64u() }),
    )
    .await;
    let phone_id = login["device_id"].as_str().unwrap().to_string();
    let phone_token = login["device_token"].as_str().unwrap().to_string();

    // Desktop: create a pairing and wait for approval.
    let desk_key = KeyPair::generate();
    let created = api.create_pairing(&desk_key.public_b64u(), "Test desktop", None).await.unwrap();
    let wait = {
        let api = api.clone();
        let (id, secret) = (created.pairing_id.clone(), created.wait_secret.clone());
        tokio::spawn(async move { api.wait_pairing(&id, &secret).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Phone: claim (checks the key from the "QR") and approve.
    let claim = post(&http, "/v1/pairings/claim", Some(&phone_token), json!({ "code": created.code })).await;
    assert_eq!(claim["desktop_public_key"], desk_key.public_b64u());
    let pairing_id = claim["pairing_id"].as_str().unwrap();
    let approve = post(&http, &format!("/v1/pairings/{pairing_id}/approve"), Some(&phone_token), json!({})).await;

    let outcome = wait.await.unwrap().unwrap();
    let PairingOutcome::Approved { device_id, device_token, mobile_device_id, mobile_public_key, .. } = outcome else {
        panic!("pairing expired");
    };
    assert_eq!(approve["desktop_device_id"], device_id.as_str());
    assert_eq!(mobile_device_id, phone_id);
    assert_eq!(mobile_public_key, phone_key.public_b64u());
    let desk_token = device_token.expect("a new desktop gets a token");

    // Desktop link.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let link = api.connect(desk_token.clone(), tx);
    next_event(&mut rx, |e| matches!(e, LinkEvent::Ready { .. })).await;
    let mut book = PeerBook::new(&desk_key, &device_id, &[(phone_id.clone(), phone_key.public_b64u())]);

    // Phone socket.
    let ws_url = relay_url().replacen("http", "ws", 1) + "/v1/ws";
    let (mut phone_ws, _) = tokio_tungstenite::connect_async(ws_url.as_str()).await.unwrap();
    phone_ws
        .send(Message::text(json!({ "t": "auth", "token": phone_token }).to_string()))
        .await
        .unwrap();
    let phone_ch = Channel::new(&phone_key, &desk_key.public(), Side::Mobile, &device_id, &phone_id).unwrap();
    let mut phone_guard = ReplayGuard::default();

    let presence = next_event(&mut rx, |e| matches!(e, LinkEvent::Presence { online: true, .. })).await;
    assert_eq!(presence, LinkEvent::Presence { device_id: phone_id.clone(), online: true });

    // Phone → desktop request.
    let req = json!({ "t": "req", "id": "1", "method": "hello", "params": {} });
    phone_ws
        .send(Message::text(
            json!({ "t": "msg", "to": device_id, "id": "m1", "body": phone_ch.seal_app(&req, now()) }).to_string(),
        ))
        .await
        .unwrap();
    let LinkEvent::Msg { from, body } = next_event(&mut rx, |e| matches!(e, LinkEvent::Msg { .. })).await else {
        unreachable!()
    };
    assert_eq!(from, phone_id);
    assert_eq!(book.open(&from, &body, now()).unwrap(), req);

    // Desktop → phone response.
    let res = json!({ "t": "res", "id": "1", "ok": true, "result": { "desktop_name": "Test desktop" } });
    link.send(&phone_id, book.seal(&phone_id, &res, now()).unwrap());
    let got = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let Message::Text(t) = phone_ws.next().await.unwrap().unwrap() else { continue };
            let f: Value = serde_json::from_str(&t).unwrap();
            if f["t"] == "msg" {
                assert_eq!(f["from"], device_id.as_str());
                return phone_ch.open_app(f["body"].as_str().unwrap(), now(), &mut phone_guard).unwrap();
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(got, res);

    // Clean up: the desktop unregisters itself.
    drop(link);
    api.delete_device(&desk_token, &device_id).await.unwrap();
}
