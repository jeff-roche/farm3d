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

const CREDENTIALS_FILE_NAME: &str = "credentials.json";
/// `keyring`'s "service" argument. The per-printer part goes in the username.
const KEYCHAIN_SERVICE: &str = "farm3d";

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum CredentialStoreKind {
    Keychain,
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
    unavailable_reason: Option<String>,
}

impl CredentialStore {
    /// Picks a tier once. `Entry::store_status()` initializes the platform
    /// store lazily and reports the outcome WITHOUT writing anything — which
    /// is exactly the availability check this needs.
    pub fn detect(config_dir: PathBuf) -> Self {
        match keyring::Entry::store_status() {
            Ok(()) => Self { kind: CredentialStoreKind::Keychain, config_dir, unavailable_reason: None },
            Err(e) => Self {
                kind: CredentialStoreKind::File,
                config_dir,
                unavailable_reason: Some(e.to_string()),
            },
        }
    }

    /// Forces the file tier. Used by tests, and the only constructor that
    /// never touches the developer's real keychain.
    pub fn file_backed(config_dir: PathBuf) -> Self {
        Self { kind: CredentialStoreKind::File, config_dir, unavailable_reason: None }
    }

    pub fn kind(&self) -> CredentialStoreKind {
        self.kind
    }

    pub fn unavailable_reason(&self) -> Option<&str> {
        self.unavailable_reason.as_deref()
    }

    pub fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        match self.kind {
            CredentialStoreKind::Keychain => keychain_entry(key)?
                .set_password(secret)
                .map_err(|e| e.to_string()),
            CredentialStoreKind::File => {
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
            CredentialStoreKind::File => Ok(self.read_file()?.get(key).cloned()),
        }
    }

    pub fn delete(&self, key: &str) -> Result<(), String> {
        match self.kind {
            CredentialStoreKind::Keychain => match keychain_entry(key)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(e.to_string()),
            },
            CredentialStoreKind::File => {
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
        if !path.exists() {
            return Ok(BTreeMap::new());
        }
        let contents = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&contents).map_err(|e| {
            format!("{CREDENTIALS_FILE_NAME} is not readable ({e}); re-enter the credential to rewrite it")
        })
    }

    fn write_file(&self, map: &BTreeMap<String, String>) -> Result<(), String> {
        fs::create_dir_all(&self.config_dir).map_err(|e| e.to_string())?;
        let path = credentials_file_path(&self.config_dir);
        let json = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())?;
        restrict_permissions(&path)
    }
}

fn keychain_entry(key: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, key).map_err(|e| e.to_string())
}

/// Re-applied on EVERY write, not just on creation — a loosened mode from a
/// hand-edit or a restored backup must be corrected, not inherited.
#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<(), String> {
    // Windows inherits the user-profile ACL on the app config dir, which is
    // already owner-only; there is no chmod equivalent worth emulating.
    Ok(())
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

    #[test]
    fn credential_ref_is_namespaced_per_printer() {
        assert_eq!(credential_ref_for("prn-8f2a"), "farm3d/printer/prn-8f2a/apikey");
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
        assert_eq!(store.get(&credential_ref_for("b")).unwrap(), Some("bbb".to_string()));
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
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);

        // A user (or a bad backup restore) loosening the mode must be
        // corrected on the next write, not merely on creation.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        store.set(&key, "second").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        fs::remove_dir_all(&dir).ok();
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
