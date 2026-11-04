//! End-to-end crypto between a desktop and a paired phone
//! (docs/REMOTE_PROTOCOL.md §3): X25519 static keys, a 6-digit SAS to
//! confirm the pairing, HKDF-SHA256 per-direction keys and
//! XChaCha20-Poly1305 sealed bodies. The relay never sees these keys.

use anyhow::{anyhow, bail, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

pub const NONCE_LEN: usize = 24;
/// Messages whose timestamp is further than this from now are dropped.
pub const MAX_SKEW_MS: i64 = 60_000;
/// How long a message id is remembered for replay detection.
pub const REPLAY_WINDOW_MS: i64 = 120_000;

pub fn b64u(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn from_b64u(s: &str) -> Result<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s.trim()).map_err(|e| anyhow!("invalid base64url: {e}"))
}

/// Decode a base64url X25519 public key.
pub fn public_key_from_b64u(s: &str) -> Result<[u8; 32]> {
    from_b64u(s)?
        .try_into()
        .map_err(|_| anyhow!("public key must be 32 bytes"))
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).expect("OS random number generator unavailable");
    buf
}

/// A device's static X25519 key pair.
pub struct KeyPair {
    secret: StaticSecret,
    public: [u8; 32],
}

impl KeyPair {
    pub fn generate() -> Self {
        Self::from_secret_bytes(random_bytes::<32>())
    }

    pub fn from_secret_bytes(bytes: [u8; 32]) -> Self {
        let secret = StaticSecret::from(bytes);
        let public = PublicKey::from(&secret).to_bytes();
        Self { secret, public }
    }

    /// Restore from [`KeyPair::secret_b64u`].
    pub fn from_secret_b64u(s: &str) -> Result<Self> {
        let bytes = Zeroizing::new(from_b64u(s)?);
        let arr: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("secret key must be 32 bytes"))?;
        Ok(Self::from_secret_bytes(arr))
    }

    /// The secret key for the credential store. Never send it anywhere.
    pub fn secret_b64u(&self) -> Zeroizing<String> {
        Zeroizing::new(b64u(self.secret.as_bytes()))
    }

    pub fn public(&self) -> [u8; 32] {
        self.public
    }

    pub fn public_b64u(&self) -> String {
        b64u(&self.public)
    }

    fn shared(&self, peer: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>> {
        let shared = self.secret.diffie_hellman(&PublicKey::from(*peer));
        if !shared.was_contributory() {
            bail!("peer public key is invalid (low-order point)");
        }
        Ok(Zeroizing::new(shared.to_bytes()))
    }
}

/// The 6-digit verification code both screens show while pairing.
pub fn sas(pk_desktop: &[u8; 32], pk_mobile: &[u8; 32]) -> String {
    let mut h = Sha256::new();
    h.update(b"jcode-sas-v1");
    h.update(pk_desktop);
    h.update(pk_mobile);
    let d = h.finalize();
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{n:06}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Desktop,
    Mobile,
}

fn join_ids(a: &str, b: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(a.len() + b.len() + 1);
    v.extend_from_slice(a.as_bytes());
    v.push(0);
    v.extend_from_slice(b.as_bytes());
    v
}

/// Sealing/opening state for one desktop ↔ phone pair.
pub struct Channel {
    self_id: String,
    peer_id: String,
    send: XChaCha20Poly1305,
    recv: XChaCha20Poly1305,
}

impl Channel {
    /// `desktop_id` / `mobile_id` are the relay device ids of the pair.
    pub fn new(own: &KeyPair, peer_public: &[u8; 32], side: Side, desktop_id: &str, mobile_id: &str) -> Result<Self> {
        let (d2m, m2d) = derive_keys(own, peer_public, desktop_id, mobile_id)?;
        let (send, recv, self_id, peer_id) = match side {
            Side::Desktop => (d2m, m2d, desktop_id, mobile_id),
            Side::Mobile => (m2d, d2m, mobile_id, desktop_id),
        };
        Ok(Self {
            self_id: self_id.to_string(),
            peer_id: peer_id.to_string(),
            send: XChaCha20Poly1305::new((&*send).into()),
            recv: XChaCha20Poly1305::new((&*recv).into()),
        })
    }

    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    /// Seal `plaintext` for the peer: `b64u(nonce ‖ ciphertext)`.
    pub fn seal(&self, plaintext: &[u8]) -> String {
        self.seal_with_nonce(plaintext, &random_bytes::<NONCE_LEN>())
    }

    pub fn seal_with_nonce(&self, plaintext: &[u8], nonce: &[u8; NONCE_LEN]) -> String {
        let aad = join_ids(&self.self_id, &self.peer_id);
        let ct = self
            .send
            .encrypt(XNonce::from_slice(nonce), Payload { msg: plaintext, aad: &aad })
            .expect("XChaCha20-Poly1305 encryption cannot fail for in-memory input");
        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(nonce);
        out.extend_from_slice(&ct);
        b64u(&out)
    }

    /// Open a body sealed by the peer.
    pub fn open(&self, body: &str) -> Result<Vec<u8>> {
        let raw = from_b64u(body)?;
        if raw.len() < NONCE_LEN + 16 {
            bail!("sealed body too short");
        }
        let (nonce, ct) = raw.split_at(NONCE_LEN);
        let aad = join_ids(&self.peer_id, &self.self_id);
        self.recv
            .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad: &aad })
            .map_err(|_| anyhow!("message could not be decrypted"))
    }

    /// Wrap an application message (§3.4) and seal it.
    pub fn seal_app(&self, m: &serde_json::Value, now_ms: i64) -> String {
        let plain = serde_json::json!({
            "v": 1,
            "ts": now_ms,
            "mid": hex::encode(random_bytes::<16>()),
            "m": m,
        });
        self.seal(plain.to_string().as_bytes())
    }

    /// Open, check version, clock skew and replays, and return `m`.
    pub fn open_app(&self, body: &str, now_ms: i64, guard: &mut ReplayGuard) -> Result<serde_json::Value> {
        let plain = self.open(body)?;
        let mut v: serde_json::Value = serde_json::from_slice(&plain)?;
        if v.get("v").and_then(|x| x.as_i64()) != Some(1) {
            bail!("unsupported message version");
        }
        let ts = v.get("ts").and_then(|x| x.as_i64()).ok_or_else(|| anyhow!("missing ts"))?;
        let mid = v.get("mid").and_then(|x| x.as_str()).ok_or_else(|| anyhow!("missing mid"))?;
        guard.check(mid, ts, now_ms)?;
        Ok(v.get_mut("m").map(serde_json::Value::take).unwrap_or(serde_json::Value::Null))
    }
}

fn derive_keys(
    own: &KeyPair,
    peer_public: &[u8; 32],
    desktop_id: &str,
    mobile_id: &str,
) -> Result<(Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>)> {
    let shared = own.shared(peer_public)?;
    let salt = join_ids(desktop_id, mobile_id);
    let hk = Hkdf::<Sha256>::new(Some(&salt), &*shared);
    let mut d2m = Zeroizing::new([0u8; 32]);
    let mut m2d = Zeroizing::new([0u8; 32]);
    hk.expand(b"jcode-remote-v1 d2m", &mut *d2m).expect("32 bytes is a valid HKDF length");
    hk.expand(b"jcode-remote-v1 m2d", &mut *m2d).expect("32 bytes is a valid HKDF length");
    Ok((d2m, m2d))
}

/// Remembers recently seen message ids (§3.4).
#[derive(Default)]
pub struct ReplayGuard {
    seen: HashMap<String, i64>,
}

impl ReplayGuard {
    pub fn check(&mut self, mid: &str, ts: i64, now_ms: i64) -> Result<()> {
        if (now_ms - ts).abs() > MAX_SKEW_MS {
            bail!("message timestamp outside the allowed window");
        }
        self.seen.retain(|_, at| now_ms - *at <= REPLAY_WINDOW_MS);
        if self.seen.contains_key(mid) {
            bail!("replayed message");
        }
        self.seen.insert(mid.to_string(), now_ms);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESKTOP_ID: &str = "0b7d6c55-1f43-4b8e-9d2a-6f1e2c3d4a5b";
    const MOBILE_ID: &str = "9a8b7c6d-5e4f-4a3b-8c2d-1e0f9a8b7c6d";
    const PLAINTEXT: &str = r#"{"v":1,"ts":1730000000000,"mid":"00112233445566778899aabbccddeeff","m":{"t":"req","id":"1","method":"hello","params":{"app_version":"0.1.0"}}}"#;

    fn seq<const N: usize>(start: u8) -> [u8; N] {
        let mut a = [0u8; N];
        for (i, b) in a.iter_mut().enumerate() {
            *b = start.wrapping_add(i as u8);
        }
        a
    }

    fn vectors() -> serde_json::Value {
        let sk_d = seq::<32>(0x01);
        let sk_m = seq::<32>(0x41);
        let nonce_d2m = seq::<24>(0x80);
        let nonce_m2d = seq::<24>(0xa0);
        let d = KeyPair::from_secret_bytes(sk_d);
        let m = KeyPair::from_secret_bytes(sk_m);
        let shared = d.shared(&m.public()).unwrap();
        let (d2m, m2d) = derive_keys(&d, &m.public(), DESKTOP_ID, MOBILE_ID).unwrap();
        let dc = Channel::new(&d, &m.public(), Side::Desktop, DESKTOP_ID, MOBILE_ID).unwrap();
        let mc = Channel::new(&m, &d.public(), Side::Mobile, DESKTOP_ID, MOBILE_ID).unwrap();
        serde_json::json!({
            "description": "docs/REMOTE_PROTOCOL.md §3 test vectors. Generated by `JCODE_WRITE_VECTORS=1 cargo test -p jcode-remote`.",
            "desktop_secret_hex": hex::encode(sk_d),
            "mobile_secret_hex": hex::encode(sk_m),
            "desktop_device_id": DESKTOP_ID,
            "mobile_device_id": MOBILE_ID,
            "desktop_public_b64u": d.public_b64u(),
            "mobile_public_b64u": m.public_b64u(),
            "shared_hex": hex::encode(*shared),
            "key_d2m_hex": hex::encode(*d2m),
            "key_m2d_hex": hex::encode(*m2d),
            "sas": sas(&d.public(), &m.public()),
            "plaintext": PLAINTEXT,
            "nonce_d2m_hex": hex::encode(nonce_d2m),
            "nonce_m2d_hex": hex::encode(nonce_m2d),
            "body_d2m": dc.seal_with_nonce(PLAINTEXT.as_bytes(), &nonce_d2m),
            "body_m2d": mc.seal_with_nonce(PLAINTEXT.as_bytes(), &nonce_m2d),
        })
    }

    #[test]
    fn test_vectors_match_the_committed_file() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/remote-test-vectors.json");
        let want = vectors();
        if std::env::var_os("JCODE_WRITE_VECTORS").is_some() {
            std::fs::write(path, serde_json::to_string_pretty(&want).unwrap() + "\n").unwrap();
        }
        let have: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(have, want, "crypto output changed; the Dart client must change with it");
    }

    #[test]
    fn both_sides_open_each_others_messages() {
        let d = KeyPair::generate();
        let m = KeyPair::generate();
        let dc = Channel::new(&d, &m.public(), Side::Desktop, DESKTOP_ID, MOBILE_ID).unwrap();
        let mc = Channel::new(&m, &d.public(), Side::Mobile, DESKTOP_ID, MOBILE_ID).unwrap();
        assert_eq!(mc.open(&dc.seal(b"hi phone")).unwrap(), b"hi phone");
        assert_eq!(dc.open(&mc.seal(b"hi desk")).unwrap(), b"hi desk");
        // A sender cannot open its own messages (keys differ per direction),
        // and a reflected message fails because the AAD names the direction.
        assert!(dc.open(&dc.seal(b"x")).is_err());
    }

    #[test]
    fn tampering_and_wrong_keys_are_rejected() {
        let d = KeyPair::generate();
        let m = KeyPair::generate();
        let other = KeyPair::generate();
        let dc = Channel::new(&d, &m.public(), Side::Desktop, DESKTOP_ID, MOBILE_ID).unwrap();
        let mc = Channel::new(&m, &d.public(), Side::Mobile, DESKTOP_ID, MOBILE_ID).unwrap();
        let body = mc.seal(b"approve");
        let mut raw = from_b64u(&body).unwrap();
        *raw.last_mut().unwrap() ^= 1;
        assert!(dc.open(&b64u(&raw)).is_err());
        let evil = Channel::new(&other, &d.public(), Side::Mobile, DESKTOP_ID, MOBILE_ID).unwrap();
        assert!(dc.open(&evil.seal(b"approve")).is_err());
        // Same keys but the relay swapped device ids: AAD/salt mismatch.
        let swapped = Channel::new(&m, &d.public(), Side::Mobile, DESKTOP_ID, "other-mobile").unwrap();
        assert!(dc.open(&swapped.seal(b"approve")).is_err());
        assert!(dc.open("short").is_err());
    }

    #[test]
    fn app_messages_reject_replays_and_stale_timestamps() {
        let d = KeyPair::generate();
        let m = KeyPair::generate();
        let dc = Channel::new(&d, &m.public(), Side::Desktop, DESKTOP_ID, MOBILE_ID).unwrap();
        let mc = Channel::new(&m, &d.public(), Side::Mobile, DESKTOP_ID, MOBILE_ID).unwrap();
        let now = 1_730_000_000_000;
        let mut guard = ReplayGuard::default();
        let body = mc.seal_app(&serde_json::json!({"t":"req","id":"1"}), now);
        assert_eq!(dc.open_app(&body, now + 10, &mut guard).unwrap()["id"], "1");
        assert!(dc.open_app(&body, now + 20, &mut guard).is_err(), "replay");
        let stale = mc.seal_app(&serde_json::json!({}), now - MAX_SKEW_MS - 1);
        assert!(dc.open_app(&stale, now, &mut guard).is_err(), "stale");
        // Ids expire from the guard after the window.
        guard.check("a", now, now).unwrap();
        guard.check("b", now + REPLAY_WINDOW_MS + 1, now + REPLAY_WINDOW_MS + 1).unwrap();
        assert!(!guard.seen.contains_key("a"));
    }

    #[test]
    fn sas_is_six_digits_and_order_sensitive() {
        let a = KeyPair::generate().public();
        let b = KeyPair::generate().public();
        let s = sas(&a, &b);
        assert_eq!(s.len(), 6);
        assert!(s.chars().all(|c| c.is_ascii_digit()));
        assert_ne!(sas(&a, &b), sas(&b, &a));
    }

    #[test]
    fn secret_round_trips_through_storage_form() {
        let k = KeyPair::generate();
        let restored = KeyPair::from_secret_b64u(&k.secret_b64u()).unwrap();
        assert_eq!(restored.public(), k.public());
        assert!(KeyPair::from_secret_b64u("AAAA").is_err());
    }
}
