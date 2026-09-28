//! P8: a Printer's alert defaults (spec "Backend model" module layout,
//! `printers/alerts.rs`). This file holds the value types the Attention
//! observer reads (`AlertDefaults`, `OfflineAlertMinutes`,
//! `NotificationMode`), the `printer_alert_defaults` repository ([`get`],
//! [`set`]), and the `get_printer_alert_defaults` /
//! `set_printer_alert_defaults` commands.
//!
//! [`get`] returns the schema's own defaults (`5`, `follow`, both
//! captures on) when a Printer has no row — D9 "Notes": "No
//! `printer_alert_defaults` row means the defaults". [`set`] upserts,
//! bumping `revision` on every call after the first (last-writer-wins, a
//! single-user app's own settings; see the design spec's "Commands":
//! `set_printer_alert_defaults` takes no `expectedRevision`).

use chrono::Duration;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ts_rs::TS;

use crate::bootstrap::BootstrapState;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::persistence::{RepositoryError, StorageError};
use crate::spools::operations::{self, Claim, OperationKind};
use crate::spools::{decode_enum, encode_enum};
use crate::RuntimeServices;

/// The offline grace a Printer's `printer.offline` Condition waits for:
/// 1, 5, or 15 minutes (`printer_alert_defaults.offline_after_minutes`'s
/// CHECK). "Off" is `None` on [`AlertDefaults::offline_after_minutes`].
/// Serialized as the bare number.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OfflineAlertMinutes {
    One,
    Five,
    Fifteen,
}

impl OfflineAlertMinutes {
    pub const ALL: [OfflineAlertMinutes; 3] = [
        OfflineAlertMinutes::One,
        OfflineAlertMinutes::Five,
        OfflineAlertMinutes::Fifteen,
    ];

    pub fn minutes(self) -> i64 {
        match self {
            OfflineAlertMinutes::One => 1,
            OfflineAlertMinutes::Five => 5,
            OfflineAlertMinutes::Fifteen => 15,
        }
    }

    pub fn from_minutes(minutes: i64) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.minutes() == minutes)
    }

    pub fn grace(self) -> Duration {
        Duration::minutes(self.minutes())
    }
}

impl Serialize for OfflineAlertMinutes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(self.minutes())
    }
}

impl<'de> Deserialize<'de> for OfflineAlertMinutes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let minutes = i64::deserialize(deserializer)?;
        OfflineAlertMinutes::from_minutes(minutes).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "offlineAfterMinutes must be 1, 5, or 15 (got {minutes})"
            ))
        })
    }
}

/// Whether a Printer's Events notify (`follow` the class settings) or
/// never do (`muted`).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/NotificationMode.ts")]
pub enum NotificationMode {
    #[default]
    Follow,
    Muted,
}

impl NotificationMode {
    pub const ALL: [NotificationMode; 2] = [NotificationMode::Follow, NotificationMode::Muted];
}

/// A Printer's offline grace, notification muting, and capture toggles.
/// No `printer_alert_defaults` row means [`AlertDefaults::default`]
/// (`5`, `follow`, true, true).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AlertDefaults.ts")]
pub struct AlertDefaults {
    /// `null` = offline alerts off.
    #[ts(type = "1 | 5 | 15 | null")]
    pub offline_after_minutes: Option<OfflineAlertMinutes>,
    pub notifications: NotificationMode,
    pub snapshot_on_incident: bool,
    pub snapshot_on_completion: bool,
}

impl Default for AlertDefaults {
    fn default() -> Self {
        Self {
            offline_after_minutes: Some(OfflineAlertMinutes::Five),
            notifications: NotificationMode::Follow,
            snapshot_on_incident: true,
            snapshot_on_completion: true,
        }
    }
}

/// `get_printer_alert_defaults`/`set_printer_alert_defaults`'s result. No
/// `printer_alert_defaults` row means [`AlertDefaults::default`]
/// (`revision`/`updatedAt` both `null`).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PrinterAlertDefaults.ts")]
pub struct PrinterAlertDefaults {
    pub printer_id: String,
    #[ts(type = "number | null")]
    pub revision: Option<i64>,
    pub alert_defaults: AlertDefaults,
    pub updated_at: Option<String>,
}

/// A Printer's alert defaults, or the schema's own defaults when it has no
/// `printer_alert_defaults` row.
pub fn get(conn: &Connection, printer_id: &str) -> Result<PrinterAlertDefaults, StorageError> {
    let row = conn
        .query_row(
            "SELECT revision, offline_after_minutes, notifications, snapshot_on_incident,
                    snapshot_on_completion, updated_at
             FROM printer_alert_defaults WHERE printer_id = ?1",
            [printer_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, bool>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;

    Ok(match row {
        Some((revision, offline_minutes, notifications_text, snapshot_on_incident, snapshot_on_completion, updated_at)) => {
            let offline_after_minutes = offline_minutes.map(|minutes| {
                OfflineAlertMinutes::from_minutes(minutes)
                    .expect("printer_alert_defaults.offline_after_minutes satisfies its own CHECK")
            });
            let notifications: NotificationMode = decode_enum(&notifications_text)
                .expect("printer_alert_defaults.notifications satisfies its own CHECK");
            PrinterAlertDefaults {
                printer_id: printer_id.to_string(),
                revision: Some(revision),
                alert_defaults: AlertDefaults {
                    offline_after_minutes,
                    notifications,
                    snapshot_on_incident,
                    snapshot_on_completion,
                },
                updated_at: Some(updated_at),
            }
        }
        None => PrinterAlertDefaults {
            printer_id: printer_id.to_string(),
            revision: None,
            alert_defaults: AlertDefaults::default(),
            updated_at: None,
        },
    })
}

/// Upserts `printer_id`'s alert defaults: `revision` starts at 1 on the
/// first call and bumps by one on every later one (last-writer-wins).
pub fn set(
    tx: &Transaction<'_>,
    printer_id: &str,
    defaults: &AlertDefaults,
    now: &str,
) -> Result<PrinterAlertDefaults, RepositoryError> {
    tx.execute(
        "INSERT INTO printer_alert_defaults(
             printer_id, revision, offline_after_minutes, notifications, snapshot_on_incident,
             snapshot_on_completion, updated_at
         ) VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(printer_id) DO UPDATE SET
             revision = printer_alert_defaults.revision + 1,
             offline_after_minutes = excluded.offline_after_minutes,
             notifications = excluded.notifications,
             snapshot_on_incident = excluded.snapshot_on_incident,
             snapshot_on_completion = excluded.snapshot_on_completion,
             updated_at = excluded.updated_at",
        params![
            printer_id,
            defaults.offline_after_minutes.map(OfflineAlertMinutes::minutes),
            encode_enum(defaults.notifications),
            defaults.snapshot_on_incident,
            defaults.snapshot_on_completion,
            now,
        ],
    )?;
    Ok(get(tx, printer_id)?)
}

fn printer_exists(conn: &Connection, printer_id: &str) -> Result<bool, StorageError> {
    Ok(conn
        .query_row("SELECT 1 FROM printers WHERE id = ?1", [printer_id], |_| Ok(()))
        .optional()?
        .is_some())
}

type Services<'a, R> = tauri::State<'a, BootstrapState<RuntimeServices<R>>>;

/// `get_printer_alert_defaults`: the Printer's alert defaults, or
/// decision 12's defaults (`revision: null`) when it has none. Archived
/// Printers are allowed; a missing one is `NOT_FOUND`.
#[tauri::command]
pub async fn get_printer_alert_defaults<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    printer_id: String,
) -> Result<CommandSuccess<PrinterAlertDefaults>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let found = services
        .storage
        .read(|connection| {
            Ok((|| -> Result<_, StorageError> {
                if !printer_exists(connection, &printer_id)? {
                    return Ok(None);
                }
                Ok(Some(get(connection, &printer_id)?))
            })())
        })
        .and_then(|found| found)
        .map_err(|error| CommandError::from_repository(RepositoryError::Storage(error)))?;
    found
        .map(CommandSuccess::new)
        .ok_or_else(|| CommandError::not_found(printer_id))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetDigest<'a> {
    printer_id: &'a str,
    alert_defaults: &'a AlertDefaults,
}

/// `set_printer_alert_defaults`: last writer wins (no
/// `expectedRevision`); the `operationId` makes a retry safe. The same
/// values again are a no-op. After a change it pokes the Attention
/// projector, so a new offline grace takes effect at once.
#[tauri::command]
pub async fn set_printer_alert_defaults<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    printer_id: String,
    alert_defaults: AlertDefaults,
) -> Result<CommandSuccess<PrinterAlertDefaults>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let digest = operations::digest(&SetDigest {
        printer_id: &printer_id,
        alert_defaults: &alert_defaults,
    });
    let now = crate::printers::now_rfc3339();
    let (defaults, changed) = services
        .storage
        .write_repo(|tx| {
            if operations::claim(
                tx,
                &operation_id,
                OperationKind::SetPrinterAlertDefaults,
                &digest,
            )? == Claim::Replay
            {
                return Ok((get(tx, &printer_id)?, false));
            }
            if !printer_exists(tx, &printer_id)? {
                return Err(RepositoryError::NotFound {
                    entity_id: printer_id.clone(),
                });
            }
            let current = get(tx, &printer_id)?;
            if current.revision.is_some() && current.alert_defaults == alert_defaults {
                return Ok((current, false));
            }
            Ok((set(tx, &printer_id, &alert_defaults, &now)?, true))
        })
        .map_err(CommandError::from_repository)?;
    if changed {
        services.attention.poke();
    }
    Ok(CommandSuccess::new(defaults))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{MetadataRootLease, Storage, StoragePaths};

    const NOW: &str = "2026-09-28T12:00:00Z";

    fn storage() -> (tempfile::TempDir, Storage) {
        let temp = tempfile::tempdir().unwrap();
        let paths =
            StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
        let lease = MetadataRootLease::acquire(&paths).unwrap();
        let storage = Storage::open(paths, &lease).unwrap();
        (temp, storage)
    }

    fn seed_printer(tx: &Transaction<'_>, id: &str) {
        tx.execute_batch(&format!(
            "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model,
               catalog_variant, catalog_model_id, catalog_printer_variant, notes,
               overrides_json, created_at, updated_at)
             VALUES ('{id}', 1, 'Printer', '', '', '', '', '', '', '{{}}', '{NOW}', '{NOW}');"
        ))
        .unwrap();
    }

    #[test]
    fn get_returns_the_defaults_when_there_is_no_row() {
        let (_temp, storage) = storage();
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                seed_printer(tx, "prn-a");
                let defaults = get(tx, "prn-a")?;
                assert_eq!(defaults.printer_id, "prn-a");
                assert_eq!(defaults.revision, None);
                assert_eq!(defaults.updated_at, None);
                assert_eq!(defaults.alert_defaults, AlertDefaults::default());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn set_upserts_and_bumps_revision_on_every_later_call() {
        let (_temp, storage) = storage();
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                seed_printer(tx, "prn-a");
                let muted = AlertDefaults {
                    offline_after_minutes: None,
                    notifications: NotificationMode::Muted,
                    snapshot_on_incident: false,
                    snapshot_on_completion: false,
                };
                let first = set(tx, "prn-a", &muted, NOW)?;
                assert_eq!(first.revision, Some(1));
                assert_eq!(first.alert_defaults, muted);
                assert_eq!(first.updated_at.as_deref(), Some(NOW));

                let follow_fifteen = AlertDefaults {
                    offline_after_minutes: Some(OfflineAlertMinutes::Fifteen),
                    notifications: NotificationMode::Follow,
                    snapshot_on_incident: true,
                    snapshot_on_completion: true,
                };
                let second = set(tx, "prn-a", &follow_fifteen, "2026-09-28T12:05:00Z")?;
                assert_eq!(second.revision, Some(2));
                assert_eq!(second.alert_defaults, follow_fifteen);

                let fetched = get(tx, "prn-a")?;
                assert_eq!(fetched, second);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn defaults_are_five_minutes_follow_and_both_captures() {
        assert_eq!(
            serde_json::to_value(AlertDefaults::default()).unwrap(),
            serde_json::json!({
                "offlineAfterMinutes": 5,
                "notifications": "follow",
                "snapshotOnIncident": true,
                "snapshotOnCompletion": true,
            })
        );
    }

    #[test]
    fn offline_minutes_round_trip_as_numbers_and_reject_other_values() {
        for minutes in OfflineAlertMinutes::ALL {
            let json = serde_json::to_string(&minutes).unwrap();
            assert_eq!(json, minutes.minutes().to_string());
            assert_eq!(
                serde_json::from_str::<OfflineAlertMinutes>(&json).unwrap(),
                minutes
            );
        }
        for bad in ["0", "2", "10", "60", "-5", "\"5\""] {
            assert!(
                serde_json::from_str::<OfflineAlertMinutes>(bad).is_err(),
                "{bad}"
            );
        }
        let off: AlertDefaults = serde_json::from_value(serde_json::json!({
            "offlineAfterMinutes": null,
            "notifications": "muted",
            "snapshotOnIncident": false,
            "snapshotOnCompletion": false,
        }))
        .unwrap();
        assert_eq!(off.offline_after_minutes, None);
        assert_eq!(off.notifications, NotificationMode::Muted);
    }

    #[test]
    fn grace_is_the_minutes_as_a_duration() {
        assert_eq!(OfflineAlertMinutes::Fifteen.grace(), Duration::minutes(15));
    }
}
