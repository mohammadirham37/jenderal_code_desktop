//! Desktop half of the three-way interop check with the Flutter app's
//! code and the Go relay; see the phone half in jenderalcode_mobile
//! `test/live/live_relay_test.dart` for how to run both.

use jcode_remote::crypto::{self, KeyPair};
use jcode_remote::protocol::{pairing_uri, AppError, AppMessage};
use jcode_remote::relay::{LinkEvent, PairingOutcome, RelayApi};
use jcode_remote::PeerBook;
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}

#[tokio::test]
#[ignore]
async fn live_e2e_desktop_side() {
    let relay = std::env::var("JCODE_RELAY_URL").unwrap_or_else(|_| "http://127.0.0.1:18080".into());
    let dir = std::path::Path::new("/tmp/jc-e2e");
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();

    let api = RelayApi::new(&relay).unwrap();
    let key = KeyPair::generate();
    let created = api.create_pairing(&key.public_b64u(), "E2E desktop", None).await.unwrap();
    std::fs::write(dir.join("qr.txt"), pairing_uri(&created.code, &key.public_b64u(), "E2E desktop")).unwrap();

    let outcome = tokio::time::timeout(Duration::from_secs(60), api.wait_pairing(&created.pairing_id, &created.wait_secret))
        .await
        .expect("phone did not approve in time")
        .unwrap();
    let PairingOutcome::Approved { device_id, device_token, mobile_device_id, mobile_public_key, .. } = outcome else {
        panic!("expired");
    };
    // The 6-digit codes on both screens must match.
    let sas = crypto::sas(&key.public(), &crypto::public_key_from_b64u(&mobile_public_key).unwrap());
    let mut phone_sas = String::new();
    for _ in 0..50 {
        phone_sas = std::fs::read_to_string(dir.join("sas-mobile.txt")).unwrap_or_default();
        if !phone_sas.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(sas, phone_sas.trim(), "SAS mismatch between desktop and phone");

    let token = device_token.unwrap();
    let mut book = PeerBook::new(&key, &device_id, &[(mobile_device_id.clone(), mobile_public_key)]);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let link = api.connect(token.clone(), tx);

    let served = tokio::time::timeout(Duration::from_secs(60), async {
        let mut methods = Vec::new();
        while let Some(ev) = rx.recv().await {
            let LinkEvent::Msg { from, body } = ev else { continue };
            let Some(m) = book.open(&from, &body, now()) else { panic!("could not open a phone message") };
            let AppMessage::Req { id, method, params } = serde_json::from_value(m).unwrap() else { continue };
            methods.push(method.clone());
            let res = match method.as_str() {
                "hello" => AppMessage::ok(id, json!({ "desktop_version": "e2e", "desktop_name": "E2E desktop" })),
                "workspaces.list" => AppMessage::ok(
                    id,
                    json!({ "workspaces": [{ "id": "w1", "name": "demo", "root_path": "/tmp/demo", "last_opened_at": 1, "is_git_repo": false }] }),
                ),
                "session.subscribe" => {
                    let sid = params["session_id"].as_str().unwrap().to_string();
                    let ok = AppMessage::ok(id, json!({}));
                    link.send(&from, book.seal(&from, &serde_json::to_value(&ok).unwrap(), now()).unwrap());
                    let ev = json!({ "t": "event", "name": "status", "data": { "session_id": sid, "status": "streaming", "detail": null } });
                    link.send(&from, book.seal(&from, &ev, now()).unwrap());
                    continue;
                }
                "chat.send" => AppMessage::err(id, AppError::new("yolo_blocked", "full access")),
                "bye" => {
                    let ok = AppMessage::ok(id, json!({}));
                    link.send(&from, book.seal(&from, &serde_json::to_value(&ok).unwrap(), now()).unwrap());
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    return methods;
                }
                _ => AppMessage::err(id, AppError::new("bad_request", "unknown")),
            };
            link.send(&from, book.seal(&from, &serde_json::to_value(&res).unwrap(), now()).unwrap());
        }
        methods
    })
    .await
    .expect("phone did not finish in time");
    assert_eq!(served, ["hello", "workspaces.list", "session.subscribe", "chat.send", "bye"]);
    drop(link);
    api.delete_device(&token, &device_id).await.unwrap();
}
