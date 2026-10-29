//! Credential storage (PRD §10: "Simpan API key menggunakan OS keychain").
//!
//! Primary implementation is the OS credential store (macOS Keychain,
//! Windows Credential Manager, Linux Secret Service). A file-based fallback
//! exists for environments where the OS store is unavailable; it is clearly
//! flagged as insecure and never logged.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const KEYCHAIN_SERVICE: &str = "com.jenderalcode.desktop";

#[async_trait]
pub trait CredentialStore: Send + Sync {
    async fn set(&self, account: &str, secret: &str) -> Result<()>;
    async fn get(&self, account: &str) -> Result<Option<String>>;
    async fn delete(&self, account: &str) -> Result<()>;
    /// Short label for diagnostics.
    fn backend_name(&self) -> &'static str;
}

/// Round-trip a dummy entry through the OS keychain to check availability.
pub fn probe_keyring() -> bool {
    let entry = match keyring::Entry::new(KEYCHAIN_SERVICE, "__jenderalcode_probe__") {
        Ok(e) => e,
        Err(_) => return false,
    };
    use keyring::Error;
    match entry.set_password("probe") {
        Ok(()) => matches!(entry.delete_credential(), Ok(()) | Err(Error::NoEntry)),
        Err(_) => false,
    }
}

/// OS keychain-backed store.
pub struct KeyringCredentialStore;

#[async_trait]
impl CredentialStore for KeyringCredentialStore {
    async fn set(&self, account: &str, secret: &str) -> Result<()> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .context("creating keychain entry")?;
        entry
            .set_password(secret)
            .with_context(|| format!("storing credential {account} in the OS keychain"))?;
        Ok(())
    }

    async fn get(&self, account: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .context("creating keychain entry")?;
        match entry.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(keyring::Error::Ambiguous(_)) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("reading keychain credential {account}: {e}")),
        }
    }

    async fn delete(&self, account: &str) -> Result<()> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .context("creating keychain entry")?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(anyhow::anyhow!("deleting keychain credential {account}: {e}")),
        }
    }

    fn backend_name(&self) -> &'static str {
        "os-keychain"
    }
}

/// File-based fallback. Credentials live in a JSON file with 0600 perms.
/// Insecure by comparison to the OS keychain — used only when the keychain
/// is unavailable.
pub struct FileCredentialStore {
    path: PathBuf,
    state: Mutex<HashMap<String, String>>,
}

impl FileCredentialStore {
    /// Load the store from `path`.
    ///
    /// A file that cannot be parsed is never silently discarded (the next
    /// save would overwrite it and lose every key): it is moved aside to
    /// `<name>.corrupt-<unix-ms>` and the store starts empty. I/O errors
    /// other than "not found" are returned.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let state = match std::fs::read_to_string(&path) {
            Ok(raw) if raw.trim().is_empty() => HashMap::new(),
            Ok(raw) => match serde_json::from_str::<HashMap<String, String>>(&raw) {
                Ok(map) => map,
                Err(e) => {
                    let quarantined = quarantine_corrupt(&path)?;
                    // Never log the file contents (they may hold keys).
                    eprintln!(
                        "[jenderalcode] credentials file {} is corrupt ({e}); moved to {} and starting empty",
                        path.display(),
                        quarantined.display()
                    );
                    HashMap::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => {
                return Err(anyhow::Error::new(e)
                    .context(format!("reading credentials file {}", path.display())))
            }
        };
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    /// Atomically replace the credentials file: write a 0600 temp file in
    /// the same directory, fsync, then rename over the target. The file is
    /// never world-readable, not even briefly.
    fn persist(&self, map: &HashMap<String, String>) -> Result<()> {
        write_atomic_private(&self.path, serde_json::to_string_pretty(map)?.as_bytes())
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Remove `accounts` from the file. Deletes the file entirely when it
    /// ends up empty. Used after migrating keys to the OS keychain so no
    /// plaintext copy stays behind.
    pub fn purge(&self, accounts: &[String]) -> Result<()> {
        let mut map = self.lock_state();
        for a in accounts {
            map.remove(a);
        }
        if map.is_empty() {
            match std::fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            }
        } else {
            self.persist(&map)
        }
    }
}

fn quarantine_corrupt(path: &Path) -> Result<PathBuf> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "credentials.json".into());
    let target = path.with_file_name(format!("{name}.corrupt-{ts}"));
    std::fs::rename(path, &target)
        .with_context(|| format!("moving corrupt credentials file {}", path.display()))?;
    Ok(target)
}

fn write_atomic_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "credentials.json".into());
    let tmp = dir.join(format!(".{name}.tmp-{}-{}", std::process::id(), uuid::Uuid::new_v4().simple()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut f = opts
            .open(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
            .with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[async_trait]
impl CredentialStore for FileCredentialStore {
    async fn set(&self, account: &str, secret: &str) -> Result<()> {
        let mut map = self.lock_state();
        map.insert(account.to_string(), secret.to_string());
        self.persist(&map)
            .with_context(|| format!("persisting credentials file {}", self.path.display()))?;
        Ok(())
    }

    async fn get(&self, account: &str) -> Result<Option<String>> {
        Ok(self.lock_state().get(account).cloned())
    }

    async fn delete(&self, account: &str) -> Result<()> {
        let mut map = self.lock_state();
        map.remove(account);
        self.persist(&map)?;
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "file-insecure"
    }
}

/// A credential store that can switch its backing implementation at runtime
/// (file ↔ OS keychain) and memoizes reads in memory.
///
/// The memory cache matters on macOS: unsigned builds trigger a Security
/// Agent password prompt on every keychain read, and providers read the key
/// on every request. With the cache, the backend is hit at most once per key
/// per app launch.
pub struct SwitchableCredentialStore {
    backend: tokio::sync::RwLock<Box<dyn CredentialStore>>,
    cache: Mutex<HashMap<String, String>>,
    file_path: PathBuf,
}

impl SwitchableCredentialStore {
    /// `preferred` is "file" or "keychain"; anything else falls back to file.
    pub fn new(preferred: &str, file_path: impl Into<PathBuf>) -> Self {
        let file_path = file_path.into();
        let backend: Box<dyn CredentialStore> = match preferred {
            "keychain" => Box::new(KeyringCredentialStore),
            _ => Box::new(
                FileCredentialStore::new(&file_path).expect("credential fallback file"),
            ),
        };
        Self {
            backend: tokio::sync::RwLock::new(backend),
            cache: Mutex::new(HashMap::new()),
            file_path,
        }
    }

    pub async fn backend_name(&self) -> &'static str {
        self.backend.read().await.backend_name()
    }

    /// Switch the backing store, carrying `refs` (credential references)
    /// over so keys keep working without re-entry. Returns how many
    /// credentials were migrated.
    pub async fn switch_to(&self, target: &str, refs: &[String]) -> Result<usize> {
        let target_name = if target == "keychain" { "os-keychain" } else { "file-insecure" };
        if self.backend.read().await.backend_name() == target_name {
            return Ok(0); // already there
        }

        // Read everything first (fills the memory cache, may prompt once for
        // the keychain on macOS).
        let mut values: Vec<(String, String)> = Vec::new();
        for r in refs {
            if let Some(v) = self.get(r).await? {
                values.push((r.clone(), v));
            }
        }

        let new_backend: Box<dyn CredentialStore> = if target == "keychain" {
            Box::new(KeyringCredentialStore)
        } else {
            Box::new(FileCredentialStore::new(&self.file_path)?)
        };
        *self.backend.write().await = new_backend;

        let mut migrated = 0;
        let mut moved: Vec<String> = Vec::new();
        for (r, v) in values {
            self.set(&r, &v).await?;
            moved.push(r);
            migrated += 1;
        }

        // Moving to the keychain: drop the plaintext copies from the file.
        if target == "keychain" && !moved.is_empty() {
            match FileCredentialStore::new(&self.file_path) {
                Ok(file) => file
                    .purge(&moved)
                    .context("removing migrated keys from the credentials file")?,
                Err(e) => eprintln!("[jenderalcode] could not purge credentials file: {e:#}"),
            }
        }
        Ok(migrated)
    }
}

#[async_trait]
impl CredentialStore for SwitchableCredentialStore {
    async fn set(&self, account: &str, secret: &str) -> Result<()> {
        self.backend.read().await.set(account, secret).await?;
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(account.to_string(), secret.to_string());
        Ok(())
    }

    async fn get(&self, account: &str) -> Result<Option<String>> {
        if let Some(v) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(account) {
            return Ok(Some(v.clone()));
        }
        let v = self.backend.read().await.get(account).await?;
        if let Some(v) = &v {
            self.cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(account.to_string(), v.clone());
        }
        Ok(v)
    }

    async fn delete(&self, account: &str) -> Result<()> {
        self.backend.read().await.delete(account).await?;
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).remove(account);
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        // Name is dynamic; report via the async accessor in commands. The
        // sync trait method returns the file name as a static placeholder.
        "switchable"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("jcode-creds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("creds.json");
        let _ = std::fs::remove_file(&path);
        let store = FileCredentialStore::new(&path).unwrap();
        store.set("prov1", "sk-secret").await.unwrap();
        assert_eq!(store.get("prov1").await.unwrap().as_deref(), Some("sk-secret"));
        assert_eq!(store.get("missing").await.unwrap(), None);

        // Persists across instances.
        let store2 = FileCredentialStore::new(&path).unwrap();
        assert_eq!(store2.get("prov1").await.unwrap().as_deref(), Some("sk-secret"));

        store.delete("prov1").await.unwrap();
        assert_eq!(store.get("prov1").await.unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn corrupt_file_is_quarantined_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(&path, "{ not json sk-live-abc").unwrap();
        let store = FileCredentialStore::new(&path).unwrap();
        assert_eq!(store.get("x").await.unwrap(), None);
        let quarantined: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("credentials.json.corrupt-"))
            .collect();
        assert_eq!(quarantined.len(), 1);
        assert_eq!(
            std::fs::read_to_string(quarantined[0].path()).unwrap(),
            "{ not json sk-live-abc"
        );
        // New saves go to a fresh file; the quarantined copy is untouched.
        store.set("p", "v").await.unwrap();
        let store2 = FileCredentialStore::new(&path).unwrap();
        assert_eq!(store2.get("p").await.unwrap().as_deref(), Some("v"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn file_is_private_and_no_temp_left() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let store = FileCredentialStore::new(&path).unwrap();
        store.set("a", "b").await.unwrap();
        store.set("c", "d").await.unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["credentials.json".to_string()]);
    }

    #[tokio::test]
    async fn purge_removes_keys_and_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let store = FileCredentialStore::new(&path).unwrap();
        store.set("a", "1").await.unwrap();
        store.set("b", "2").await.unwrap();
        store.purge(&["a".to_string()]).unwrap();
        let reloaded = FileCredentialStore::new(&path).unwrap();
        assert_eq!(reloaded.get("a").await.unwrap(), None);
        assert_eq!(reloaded.get("b").await.unwrap().as_deref(), Some("2"));
        store.purge(&["b".to_string()]).unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn switchable_store_caches_reads() {
        let dir = std::env::temp_dir().join(format!("jcode-switch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.json");
        let _ = std::fs::remove_file(&path);

        let store = SwitchableCredentialStore::new("file", &path);
        store.set("k1", "v1").await.unwrap();

        // Remove the backing file entirely: the memory cache must still
        // serve the value (the OS keychain would not be hit again either).
        std::fs::remove_file(&path).unwrap();
        assert_eq!(store.get("k1").await.unwrap().as_deref(), Some("v1"));

        // Deleting goes through and clears the cache.
        store.delete("k1").await.unwrap();
        assert_eq!(store.get("k1").await.unwrap(), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn switchable_store_set_get_delete_roundtrip() {
        let dir = std::env::temp_dir().join(format!("jcode-switch2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.json");
        let _ = std::fs::remove_file(&path);
        let store = SwitchableCredentialStore::new("file", &path);
        store.set("p", "secret").await.unwrap();
        assert_eq!(store.get("p").await.unwrap().as_deref(), Some("secret"));
        store.delete("p").await.unwrap();
        assert_eq!(store.get("p").await.unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
