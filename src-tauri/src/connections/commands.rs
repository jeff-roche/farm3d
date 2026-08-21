//! The `#[tauri::command]` surface for connections. Thin by design — every
//! decision worth testing lives in a sibling module.

use super::credentials::{credential_ref_for, CredentialStore, CredentialStoreKind};
use super::discovery::{discover, DiscoveredPrinter};
use super::moonraker::MoonrakerConnection;
use super::supervisor::ConnectionManager;
use super::{ConnectionConfig, PrinterConnection, PrinterStatus, ProbeResult, MOONRAKER_KIND};
use crate::catalog::resolve::{resolve_printer, ResolvedPrinter};
use crate::catalog::Catalog;
use crate::printers::{load_printers_from, write_printers_to};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Manager};

const DISCOVERY_WINDOW: Duration = Duration::from_secs(3);

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSubmission {
    pub kind: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub use_tls: bool,
    /// `None` = leave the stored secret untouched (the UI never echoes it
    /// back). `Some("")` = clear it. These must stay distinct.
    #[serde(default)]
    pub api_key: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStoreInfo {
    pub kind: CredentialStoreKind,
    /// Why the keychain was unavailable, so the Connection tab can explain
    /// the fallback rather than silently downgrading.
    pub reason: Option<String>,
}

/// Splits a submission into what gets persisted and what goes to the
/// credential store. The whole point is that the secret exits here and is
/// never part of the returned config.
pub fn split_submission(
    submission: ConnectionSubmission,
    printer_id: &str,
) -> (ConnectionConfig, Option<String>) {
    let key_ref = credential_ref_for(printer_id);
    let (credential_ref, secret) = match submission.api_key.as_deref() {
        None => (Some(key_ref), None),
        Some(key) if key.trim().is_empty() => (None, None),
        Some(key) => (Some(key_ref), Some(key.trim().to_string())),
    };
    (
        ConnectionConfig {
            kind: submission.kind,
            host: submission.host,
            port: submission.port,
            use_tls: submission.use_tls,
            credential_ref,
        },
        secret,
    )
}

fn config_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path().app_config_dir().map_err(|e| e.to_string())
}

fn store(app: &AppHandle) -> Result<CredentialStore, String> {
    Ok(CredentialStore::detect(config_dir(app)?))
}

fn api_key_for(app: &AppHandle, config: &ConnectionConfig) -> Result<Option<String>, String> {
    match &config.credential_ref {
        None => Ok(None),
        Some(key) => store(app)?.get(key),
    }
}

fn adapter(
    config: &ConnectionConfig,
    api_key: Option<String>,
) -> Result<Box<dyn PrinterConnection>, String> {
    match config.kind.as_str() {
        MOONRAKER_KIND => Ok(Box::new(MoonrakerConnection::new(config.clone(), api_key))),
        other => Err(format!("This build cannot speak `{other}` connections")),
    }
}

/// The OLD credential (if any) that must be deleted from the store because
/// the newly split config no longer references one — e.g. the user
/// explicitly cleared their API key (`split_submission` turns `Some("")`
/// into `credential_ref: None`). If the new config still carries a
/// `credential_ref`, nothing needs clearing, even if it happens to name the
/// same key: the "omitted" case round-trips the same ref on purpose.
///
/// Pure over the two refs so this is testable without a Tauri `AppHandle`.
fn credential_to_clear(previous_ref: Option<&str>, new_config: &ConnectionConfig) -> Option<String> {
    if new_config.credential_ref.is_some() {
        return None;
    }
    previous_ref.map(str::to_string)
}

#[tauri::command]
pub fn set_printer_connection(
    app: AppHandle,
    catalog: tauri::State<Arc<Catalog>>,
    manager: tauri::State<Arc<ConnectionManager>>,
    id: String,
    submission: ConnectionSubmission,
) -> Result<ResolvedPrinter, String> {
    let dir = config_dir(&app)?;
    let mut file = load_printers_from(&dir)?;
    let (config, secret) = split_submission(submission, &id);

    let previous_credential_ref = file
        .printers
        .iter()
        .find(|p| p.id == id)
        .and_then(|p| p.connection.as_ref())
        .and_then(|c| c.credential_ref.clone());

    // Write the secret BEFORE persisting the reference: a config pointing at
    // a credential that was never stored is worse than a stored credential
    // nothing points at yet.
    if let (Some(secret), Some(key)) = (&secret, config.credential_ref.as_deref()) {
        store(&app)?.set(key, secret)?;
    }

    // An explicit clear (submission.api_key was `Some("")`) means the new
    // config carries no credential_ref at all. That must also delete the
    // OLD stored secret — otherwise "clear credential" only forgets the
    // pointer while the secret itself sits orphaned in the store forever,
    // which is exactly the failure mode the None/empty asymmetry exists to
    // prevent.
    if let Some(old_key) = credential_to_clear(previous_credential_ref.as_deref(), &config) {
        store(&app)?.delete(&old_key)?;
    }

    {
        let stored = file
            .printers
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("no printer with id {id:?}"))?;
        stored.connection = Some(config.clone());
    }
    write_printers_to(&dir, &file)?;

    let api_key = api_key_for(&app, &config)?;
    manager.start(id.clone(), config, api_key);

    Ok(resolve_printer(&catalog, file.printers.iter().find(|p| p.id == id).unwrap()))
}

#[tauri::command]
pub fn clear_printer_connection(
    app: AppHandle,
    catalog: tauri::State<Arc<Catalog>>,
    manager: tauri::State<Arc<ConnectionManager>>,
    id: String,
) -> Result<ResolvedPrinter, String> {
    let dir = config_dir(&app)?;
    let mut file = load_printers_from(&dir)?;
    manager.stop(&id);

    {
        let stored = file
            .printers
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("no printer with id {id:?}"))?;
        stored.connection = None;
    }
    write_printers_to(&dir, &file)?;
    // Removing the secret last: a failure here leaves an orphaned credential,
    // which is harmless, rather than an unusable config.
    store(&app)?.delete(&credential_ref_for(&id))?;

    Ok(resolve_printer(&catalog, file.printers.iter().find(|p| p.id == id).unwrap()))
}

/// Probes the SUBMITTED config, not the stored one, so "Test connection"
/// validates what the user typed before they commit it.
#[tauri::command]
pub async fn test_printer_connection(
    app: AppHandle,
    id: String,
    submission: ConnectionSubmission,
) -> Result<ProbeResult, String> {
    let (config, secret) = split_submission(submission, &id);
    let api_key = match secret {
        Some(secret) => Some(secret),
        None => api_key_for(&app, &config)?,
    };
    adapter(&config, api_key)?.probe().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn credential_store_info(app: AppHandle) -> Result<CredentialStoreInfo, String> {
    let store = store(&app)?;
    Ok(CredentialStoreInfo {
        kind: store.kind(),
        reason: store.unavailable_reason().map(str::to_string),
    })
}

#[tauri::command]
pub async fn discover_printers() -> Vec<DiscoveredPrinter> {
    // mdns-sd's receiver is blocking, so this belongs on the blocking pool
    // rather than parked on an async worker for three seconds.
    tauri::async_runtime::spawn_blocking(|| discover(DISCOVERY_WINDOW))
        .await
        .unwrap_or_default()
}

#[tauri::command]
pub fn printer_statuses(
    manager: tauri::State<Arc<ConnectionManager>>,
) -> HashMap<String, PrinterStatus> {
    manager.statuses()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_submitted_api_key_never_lands_in_the_persisted_config() {
        let (config, secret) = split_submission(ConnectionSubmission {
            kind: MOONRAKER_KIND.to_string(),
            host: "voron.local".to_string(),
            port: 7125,
            use_tls: false,
            api_key: Some("s3cret".to_string()),
        }, "prn-1");

        assert_eq!(secret.as_deref(), Some("s3cret"));
        assert_eq!(config.credential_ref.as_deref(), Some("farm3d/printer/prn-1/apikey"));
        let json = serde_json::to_string(&config).unwrap();
        assert!(!json.contains("s3cret"), "the secret leaked into the persisted config");
    }

    #[test]
    fn an_empty_api_key_stores_no_reference_at_all() {
        // Trusted-LAN Moonraker instances need no key; a dangling
        // credentialRef pointing at nothing would make the Connection tab
        // claim a credential exists when none does.
        let (config, secret) = split_submission(ConnectionSubmission {
            kind: MOONRAKER_KIND.to_string(),
            host: "voron.local".to_string(),
            port: 7125,
            use_tls: false,
            api_key: Some("   ".to_string()),
        }, "prn-1");
        assert_eq!(secret, None);
        assert_eq!(config.credential_ref, None);
    }

    #[test]
    fn omitting_the_api_key_field_preserves_the_existing_reference() {
        // The UI never echoes a stored secret back, so "no api_key in this
        // submission" must mean "leave the stored one alone", not "clear it".
        let (config, secret) = split_submission(ConnectionSubmission {
            kind: MOONRAKER_KIND.to_string(),
            host: "voron.local".to_string(),
            port: 7125,
            use_tls: false,
            api_key: None,
        }, "prn-1");
        assert_eq!(secret, None);
        assert_eq!(config.credential_ref.as_deref(), Some("farm3d/printer/prn-1/apikey"));
    }

    #[test]
    fn a_submitted_api_key_is_trimmed_before_it_is_stored() {
        // A pasted key with accidental leading/trailing whitespace must not
        // be stored verbatim — it would fail as an `X-Api-Key` header value
        // against the real printer with no indication why.
        let (config, secret) = split_submission(ConnectionSubmission {
            kind: MOONRAKER_KIND.to_string(),
            host: "voron.local".to_string(),
            port: 7125,
            use_tls: false,
            api_key: Some("  s3cret  \n".to_string()),
        }, "prn-1");
        assert_eq!(secret.as_deref(), Some("s3cret"));
        assert_eq!(config.credential_ref.as_deref(), Some("farm3d/printer/prn-1/apikey"));
    }

    fn moonraker_config(credential_ref: Option<&str>) -> ConnectionConfig {
        ConnectionConfig {
            kind: MOONRAKER_KIND.to_string(),
            host: "voron.local".to_string(),
            port: 7125,
            use_tls: false,
            credential_ref: credential_ref.map(str::to_string),
        }
    }

    #[test]
    fn clearing_the_credential_ref_forgets_the_old_one() {
        // The scenario from the orphaned-secret bug: the new config has no
        // credential_ref (an explicit clear), so the OLD ref must be
        // returned for deletion — otherwise the secret it named survives in
        // the store forever with nothing left pointing at it.
        let new_config = moonraker_config(None);
        assert_eq!(
            credential_to_clear(Some("farm3d/printer/prn-1/apikey"), &new_config),
            Some("farm3d/printer/prn-1/apikey".to_string())
        );
    }

    #[test]
    fn keeping_a_credential_ref_forgets_nothing() {
        // Whether the ref is unchanged (the "omitted api_key" case) or is a
        // fresh key for the same printer, the new config still names a
        // credential — there is nothing to delete.
        let new_config = moonraker_config(Some("farm3d/printer/prn-1/apikey"));
        assert_eq!(credential_to_clear(Some("farm3d/printer/prn-1/apikey"), &new_config), None);
    }

    #[test]
    fn there_was_never_a_credential_to_forget() {
        let new_config = moonraker_config(None);
        assert_eq!(credential_to_clear(None, &new_config), None);
    }
}
