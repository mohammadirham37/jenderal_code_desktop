//! Remote control of the desktop from a paired phone, over an untrusted
//! relay. Spec: docs/REMOTE_PROTOCOL.md.

pub mod crypto;
pub mod protocol;
pub mod relay;

use crypto::{Channel, KeyPair, ReplayGuard, Side};
use std::collections::HashMap;

/// The desktop's encrypted channels to its confirmed phones. A message from
/// any device not in the book is dropped, whatever the relay claims.
pub struct PeerBook {
    channels: HashMap<String, (Channel, ReplayGuard)>,
}

impl PeerBook {
    /// `peers`: (device id, base64url public key) of confirmed phones.
    /// Peers with an unusable key are skipped.
    pub fn new(own: &KeyPair, desktop_id: &str, peers: &[(String, String)]) -> Self {
        let mut channels = HashMap::new();
        for (id, pk) in peers {
            let Ok(pk) = crypto::public_key_from_b64u(pk) else { continue };
            if let Ok(ch) = Channel::new(own, &pk, Side::Desktop, desktop_id, id) {
                channels.insert(id.clone(), (ch, ReplayGuard::default()));
            }
        }
        Self { channels }
    }

    pub fn contains(&self, device_id: &str) -> bool {
        self.channels.contains_key(device_id)
    }

    pub fn ids(&self) -> impl Iterator<Item = &String> {
        self.channels.keys()
    }

    /// Decrypt and check a message from `from`. `None` for unknown senders,
    /// tampering, stale timestamps and replays.
    pub fn open(&mut self, from: &str, body: &str, now_ms: i64) -> Option<serde_json::Value> {
        let (ch, guard) = self.channels.get_mut(from)?;
        ch.open_app(body, now_ms, guard).ok()
    }

    pub fn seal(&self, to: &str, m: &serde_json::Value, now_ms: i64) -> Option<String> {
        self.channels.get(to).map(|(ch, _)| ch.seal_app(m, now_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto::{Channel, KeyPair, Side};

    #[test]
    fn peer_book_only_talks_to_confirmed_phones() {
        let desktop = KeyPair::generate();
        let phone = KeyPair::generate();
        let stranger = KeyPair::generate();
        let mut book = PeerBook::new(&desktop, "d", &[("m".into(), phone.public_b64u()), ("bad".into(), "nope".into())]);
        assert!(book.contains("m"));
        assert!(!book.contains("bad"));

        let phone_ch = Channel::new(&phone, &desktop.public(), Side::Mobile, "d", "m").unwrap();
        let now = 1_000_000;
        let body = phone_ch.seal_app(&serde_json::json!({"t":"req"}), now);
        assert_eq!(book.open("m", &body, now).unwrap()["t"], "req");
        assert!(book.open("m", &body, now).is_none(), "replay");

        // A stranger claiming to be "m", or an unknown sender id.
        let fake = Channel::new(&stranger, &desktop.public(), Side::Mobile, "d", "m").unwrap();
        assert!(book.open("m", &fake.seal_app(&serde_json::json!({}), now), now).is_none());
        assert!(book.open("x", &body, now).is_none());

        let sealed = book.seal("m", &serde_json::json!({"t":"res"}), now).unwrap();
        let mut guard = crypto::ReplayGuard::default();
        assert_eq!(phone_ch.open_app(&sealed, now, &mut guard).unwrap()["t"], "res");
        assert!(book.seal("x", &serde_json::json!({}), now).is_none());
    }
}
