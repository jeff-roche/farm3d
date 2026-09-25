//! Two-tier secret storage for Connection credentials.
//!
//! Tier 1 is the platform secret store (Keychain Services / Credential
//! Manager / Secret Service) via `keyring`. Tier 2 is a `credentials.json`
//! beside `printers.json`, owner-readable only.
//!
//! The fallback exists because `keyring` needs a running Secret Service on
//! *nix, and a headless or minimal box may have none. It is deliberately
//! specified rather than discovered: farm3d reports which tier is live
//! (see `CredentialStoreKind`) rather than silently downgrading where a
//! user's API key lives.
//!
//! Under NO tier does a secret enter `printers.json`, which stores only the
//! `credentialRef` naming it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::file_links::file_has_multiple_links;

const CREDENTIALS_FILE_NAME: &str = "credentials.json";
/// `keyring`'s "service" argument. The per-printer part goes in the username.
const KEYCHAIN_SERVICE: &str = "farm3d";

fn open_credential_file(path: &Path) -> Result<Option<fs::File>, String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("credential store unavailable".to_string()),
    };
    let metadata = file
        .metadata()
        .map_err(|_| "credential store unavailable".to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || file_has_multiple_links(&file) {
        return Err("credential store unavailable".to_string());
    }
    Ok(Some(file))
}

fn fallback_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/CredentialStoreKind.ts")]
pub enum CredentialStoreKind {
    Keychain,
    #[serde(rename = "fallbackFile")]
    File,
}

/// The stable name for one printer's secret. Also the exact string written
/// into `printers.json` as `credentialRef`, so it must stay stable across
/// releases — changing this format orphans every stored credential.
pub fn credential_ref_for(printer_id: &str) -> String {
    format!("farm3d/printer/{printer_id}/apikey")
}

pub fn credentials_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join(CREDENTIALS_FILE_NAME)
}

pub struct CredentialStore {
    kind: CredentialStoreKind,
    config_dir: PathBuf,
    /// Why the keychain was unavailable, for the Connection tab to explain.
    unavailable_reason_code: Option<&'static str>,
}

/// Injectable credential boundary. Errors are deliberately opaque so callers
/// cannot accidentally copy platform/keychain text (which may include paths or
/// secret material) into command errors, events, or logs.
pub trait CredentialBackend: Send + Sync {
    fn set(&self, key: &str, secret: &str) -> Result<(), ()>;
    fn get(&self, key: &str) -> Result<Option<String>, ()>;
    fn delete(&self, key: &str) -> Result<(), ()>;
}

impl CredentialBackend for CredentialStore {
    fn set(&self, key: &str, secret: &str) -> Result<(), ()> {
        CredentialStore::set(self, key, secret).map_err(|_| ())
    }

    fn get(&self, key: &str) -> Result<Option<String>, ()> {
        CredentialStore::get(self, key).map_err(|_| ())
    }

    fn delete(&self, key: &str) -> Result<(), ()> {
        CredentialStore::delete(self, key).map_err(|_| ())
    }
}

impl CredentialStore {
    /// Picks a tier once. `Entry::store_status()` initializes the platform
    /// store lazily and reports the outcome WITHOUT writing anything — which
    /// is exactly the availability check this needs.
    pub fn detect(config_dir: PathBuf) -> Self {
        let store = match keyring::Entry::store_status() {
            Ok(()) => Self {
                kind: CredentialStoreKind::Keychain,
                config_dir,
                unavailable_reason_code: None,
            },
            Err(_) => Self {
                kind: CredentialStoreKind::File,
                config_dir,
                unavailable_reason_code: Some("KEYCHAIN_UNAVAILABLE"),
            },
        };
        if store.kind == CredentialStoreKind::File {
            store.cleanup_interrupted_replacements();
        }
        store
    }

    /// Forces the file tier. Used by tests, and the only constructor that
    /// never touches the developer's real keychain.
    pub fn file_backed(config_dir: PathBuf) -> Self {
        let store = Self {
            kind: CredentialStoreKind::File,
            config_dir,
            unavailable_reason_code: None,
        };
        store.cleanup_interrupted_replacements();
        store
    }

    pub fn kind(&self) -> CredentialStoreKind {
        self.kind
    }

    pub fn unavailable_reason_code(&self) -> Option<&str> {
        self.unavailable_reason_code
    }

    pub fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        match self.kind {
            CredentialStoreKind::Keychain => keychain_entry(key)?
                .set_password(secret)
                .map_err(|e| e.to_string()),
            CredentialStoreKind::File => {
                let _guard = fallback_lock()
                    .lock()
                    .map_err(|_| "credential store unavailable")?;
                let mut map = self.read_file()?;
                map.insert(key.to_string(), secret.to_string());
                self.write_file(&map)
            }
        }
    }

    pub fn get(&self, key: &str) -> Result<Option<String>, String> {
        match self.kind {
            CredentialStoreKind::Keychain => match keychain_entry(key)?.get_password() {
                Ok(secret) => Ok(Some(secret)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(e.to_string()),
            },
            CredentialStoreKind::File => {
                let _guard = fallback_lock()
                    .lock()
                    .map_err(|_| "credential store unavailable")?;
                Ok(self.read_file()?.get(key).cloned())
            }
        }
    }

    pub fn delete(&self, key: &str) -> Result<(), String> {
        match self.kind {
            CredentialStoreKind::Keychain => match keychain_entry(key)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(e.to_string()),
            },
            CredentialStoreKind::File => {
                let _guard = fallback_lock()
                    .lock()
                    .map_err(|_| "credential store unavailable")?;
                let mut map = self.read_file()?;
                // Deleting a key that was never set is a success, not an
                // error — clearing a connection that had no credential is a
                // normal action.
                if map.remove(key).is_none() {
                    return Ok(());
                }
                self.write_file(&map)
            }
        }
    }

    fn read_file(&self) -> Result<BTreeMap<String, String>, String> {
        let path = credentials_file_path(&self.config_dir);
        let Some(mut file) = open_credential_file(&path)? else {
            return Ok(BTreeMap::new());
        };
        let mut contents = String::new();
        std::io::Read::read_to_string(&mut file, &mut contents).map_err(|e| e.to_string())?;
        serde_json::from_str(&contents).map_err(|e| {
            format!("{CREDENTIALS_FILE_NAME} is not readable ({e}); re-enter the credential to rewrite it")
        })
    }

    fn cleanup_interrupted_replacements(&self) {
        let Ok(_guard) = fallback_lock().lock() else {
            return;
        };
        let Ok(entries) = fs::read_dir(&self.config_dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let interrupted = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(".credentials.json.") && name.ends_with(".tmp")
                });
            if interrupted {
                let _ = fs::remove_file(path);
            }
        }
    }

    fn write_file(&self, map: &BTreeMap<String, String>) -> Result<(), String> {
        ensure_private_directory(&self.config_dir)?;
        let path = credentials_file_path(&self.config_dir);
        let json = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
        let temporary = self
            .config_dir
            .join(format!(".credentials.json.{}.tmp", uuid::Uuid::new_v4()));
        write_owner_only(&temporary, &json)?;
        atomic_replace(&temporary, &path)?;
        sync_directory(&self.config_dir)
    }
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err("credential store unavailable".to_string());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|_| "credential store unavailable".to_string())?;
        }
        Err(_) => return Err("credential store unavailable".to_string()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::fs::PermissionsExt;
        let directory = fs::OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(path)
            .map_err(|_| "credential store unavailable".to_string())?;
        if !directory
            .metadata()
            .map_err(|_| "credential store unavailable".to_string())?
            .is_dir()
        {
            return Err("credential store unavailable".to_string());
        }
        directory
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|_| "credential store unavailable".to_string())?;
    }
    Ok(())
}

fn atomic_replace(source: &Path, destination: &Path) -> Result<(), String> {
    crate::document_io::atomic_replace(source, destination).map_err(|error| error.to_string())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), String> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Creates the file at 0600 rather than at the default 0644-then-chmod: this
/// is the one file whose whole purpose is holding a plaintext secret, and
/// `fs::write` would leave it world-readable for the window between the first
/// byte landing and `restrict_permissions` correcting it.
#[cfg(unix)]
fn write_owner_only(path: &Path, contents: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(contents.as_bytes())
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn write_owner_only(path: &Path, contents: &str) -> Result<(), String> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(contents.as_bytes())
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

fn keychain_entry(key: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, key).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_dir() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("farm3d-cred-test-{}-{}", std::process::id(), id))
    }

    fn file_store(dir: &Path) -> CredentialStore {
        CredentialStore::file_backed(dir.to_path_buf())
    }

    fn persistence_fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/persistence/v1")
            .join(name)
    }

    #[test]
    fn baseline_file_backed_credentials_fixture_loads_through_the_storage_seam() {
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut tempdir = tempfile::Builder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tempdir.permissions(fs::Permissions::from_mode(0o700));
        }
        let dir = tempdir.tempdir().unwrap();
        fs::copy(
            persistence_fixture("credentials.json"),
            credentials_file_path(dir.path()),
        )
        .unwrap();
        let store = file_store(dir.path());

        assert_eq!(
            store
                .get("farm3d/printer/prn-f0-connected/apikey")
                .unwrap()
                .as_deref(),
            Some("F0_FIXTURE_SENTINEL")
        );
    }

    #[test]
    fn credential_ref_is_namespaced_per_printer() {
        assert_eq!(
            credential_ref_for("prn-8f2a"),
            "farm3d/printer/prn-8f2a/apikey"
        );
    }

    #[test]
    fn file_tier_round_trips_a_secret() {
        let dir = temp_dir();
        let store = file_store(&dir);
        let key = credential_ref_for("prn-1");
        store.set(&key, "s3cret").unwrap();
        assert_eq!(store.get(&key).unwrap(), Some("s3cret".to_string()));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn concurrent_file_writes_preserve_every_credential() {
        let dir = temp_dir();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(9));
        let handles: Vec<_> = (0..8)
            .map(|index| {
                let dir = dir.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    file_store(&dir)
                        .set(&format!("credential-{index}"), &format!("value-{index}"))
                        .unwrap();
                })
            })
            .collect();
        barrier.wait();
        for handle in handles {
            handle.join().unwrap();
        }
        let store = file_store(&dir);
        for index in 0..8 {
            assert_eq!(
                store.get(&format!("credential-{index}")).unwrap(),
                Some(format!("value-{index}"))
            );
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn leftover_atomic_replacement_file_is_ignored() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(".credentials.json.interrupted.tmp"),
            "{not complete",
        )
        .unwrap();
        let store = file_store(&dir);
        store.set("credential", "value").unwrap();
        assert_eq!(store.get("credential").unwrap().as_deref(), Some("value"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_tier_returns_none_for_an_unknown_key() {
        let dir = temp_dir();
        let store = file_store(&dir);
        assert_eq!(store.get("farm3d/printer/nope/apikey").unwrap(), None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_tier_deletes_without_disturbing_siblings() {
        let dir = temp_dir();
        let store = file_store(&dir);
        store.set(&credential_ref_for("a"), "aaa").unwrap();
        store.set(&credential_ref_for("b"), "bbb").unwrap();
        store.delete(&credential_ref_for("a")).unwrap();
        assert_eq!(store.get(&credential_ref_for("a")).unwrap(), None);
        assert_eq!(
            store.get(&credential_ref_for("b")).unwrap(),
            Some("bbb".to_string())
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn deleting_a_missing_key_is_not_an_error() {
        // Clearing a connection that never had a credential must not fail.
        let dir = temp_dir();
        let store = file_store(&dir);
        assert!(store.delete(&credential_ref_for("ghost")).is_ok());
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn file_tier_is_owner_only_and_stays_that_way() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir();
        let store = file_store(&dir);
        let key = credential_ref_for("prn-1");
        store.set(&key, "first").unwrap();

        let path = credentials_file_path(&dir);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        // A user (or a bad backup restore) loosening the mode must be
        // corrected on the next write, not merely on creation.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        store.set(&key, "second").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_brand_new_credentials_file_is_owner_only_from_its_first_byte() {
        // Deliberately exercises the creation path ALONE, with no
        // `restrict_permissions` after it: a default-0644 create followed by
        // a chmod would leave the secret world-readable in between.
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let path = credentials_file_path(&dir);
        write_owner_only(&path, "{}").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn fallback_store_rejects_a_symlink_without_reading_or_replacing_its_target() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let outside = dir.with_extension("outside");
        fs::write(&outside, r#"{"sentinel":"unchanged"}"#).unwrap();
        symlink(&outside, credentials_file_path(&dir)).unwrap();
        let store = file_store(&dir);

        assert!(store.get("sentinel").is_err());
        assert!(store.set("sentinel", "replacement").is_err());
        assert_eq!(
            fs::read_to_string(&outside).unwrap(),
            r#"{"sentinel":"unchanged"}"#
        );
        fs::remove_dir_all(&dir).ok();
        fs::remove_file(&outside).ok();
    }

    #[cfg(unix)]
    #[test]
    fn fallback_store_rejects_a_multiply_linked_credentials_file() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let outside = dir.with_extension("outside-hardlink");
        fs::write(&outside, r#"{"sentinel":"unchanged"}"#).unwrap();
        fs::hard_link(&outside, credentials_file_path(&dir)).unwrap();
        let store = file_store(&dir);

        assert!(store.get("sentinel").is_err());
        assert!(store.set("sentinel", "replacement").is_err());
        assert_eq!(
            fs::read_to_string(&outside).unwrap(),
            r#"{"sentinel":"unchanged"}"#
        );
        fs::remove_dir_all(&dir).ok();
        fs::remove_file(&outside).ok();
    }

    #[test]
    fn a_corrupt_credentials_file_does_not_take_down_the_app() {
        // Unlike printers.json, a lost API key is re-enterable — so this
        // reports an error rather than quarantining, and the caller surfaces
        // it in the Connection tab.
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(credentials_file_path(&dir), "not json").unwrap();
        assert!(file_store(&dir).get(&credential_ref_for("prn-1")).is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn detect_reports_which_tier_is_live() {
        // Whichever tier this machine lands on, `kind()` must name it — the
        // spec's requirement is that farm3d never downgrades silently.
        let dir = temp_dir();
        let store = CredentialStore::detect(dir.clone());
        assert!(matches!(
            store.kind(),
            CredentialStoreKind::Keychain | CredentialStoreKind::File
        ));
        fs::remove_dir_all(&dir).ok();
    }
}
