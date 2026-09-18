use std::sync::Arc;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::persistence::{Storage, StorageError};
use crate::printers::operational::HostActivity;

const PERIODIC_WRITE_INTERVAL: Duration = Duration::seconds(30);

/// The normalized data retained for Monitor after live adapter telemetry ends.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrinterTelemetry {
    pub host_activity: HostActivity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_activity_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nozzle_temp_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nozzle_target_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bed_temp_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bed_target_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub print_duration_s: Option<f64>,
}

/// A reconstructable last-known telemetry observation for one Printer.
#[derive(Clone, PartialEq, Debug)]
pub struct StoredTelemetrySnapshot {
    pub printer_id: String,
    pub telemetry: PrinterTelemetry,
    pub last_observed_at: String,
}

/// The reason a telemetry observation should bypass periodic coalescing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SnapshotWrite {
    Periodic,
    ActivityTransition,
}

/// A recoverable failure confined to the last-known telemetry cache.
#[derive(Debug)]
pub enum StatusCacheError {
    InvalidTelemetry,
    InvalidTimestamp,
    MalformedSnapshot,
    Storage(StorageError),
}

impl std::fmt::Display for StatusCacheError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidTelemetry => "telemetry cache received invalid Monitor telemetry",
            Self::InvalidTimestamp => "telemetry cache received an invalid observation timestamp",
            Self::MalformedSnapshot => "telemetry cache contains a malformed snapshot",
            Self::Storage(_) => "telemetry cache storage is unavailable",
        })
    }
}

impl std::error::Error for StatusCacheError {}

impl From<StorageError> for StatusCacheError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

pub struct StatusRepository {
    storage: Arc<Storage>,
    clock: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
}

impl StatusRepository {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self::with_clock(storage, Utc::now)
    }

    pub fn with_clock(
        storage: Arc<Storage>,
        clock: impl Fn() -> DateTime<Utc> + Send + Sync + 'static,
    ) -> Self {
        Self {
            storage,
            clock: Arc::new(clock),
        }
    }

    pub fn list(&self) -> Result<Vec<StoredTelemetrySnapshot>, StatusCacheError> {
        let rows: Vec<(String, String, String)> = self.storage.read(|connection| {
            let mut statement = connection.prepare(
                "SELECT printer_id, telemetry_json, last_observed_at
                 FROM printer_status_snapshots ORDER BY CAST(printer_id AS BLOB)",
            )?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })?;
        rows.into_iter().map(decode_snapshot).collect()
    }

    pub fn get(
        &self,
        printer_id: &str,
    ) -> Result<Option<StoredTelemetrySnapshot>, StatusCacheError> {
        let row = self.storage.read(|connection| {
            connection
                .query_row(
                    "SELECT printer_id, telemetry_json, last_observed_at
                     FROM printer_status_snapshots WHERE printer_id = ?1",
                    [printer_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
        })?;
        row.map(decode_snapshot).transpose()
    }

    pub fn save_if_due(
        &self,
        snapshot: &StoredTelemetrySnapshot,
        write: SnapshotWrite,
    ) -> Result<(), StatusCacheError> {
        validate_telemetry(&snapshot.telemetry)?;
        let last_observed_at = canonical_utc_timestamp(&snapshot.last_observed_at)
            .map_err(|_| StatusCacheError::InvalidTimestamp)?;
        let telemetry_json = serde_json::to_string(&snapshot.telemetry)
            .map_err(|_| StatusCacheError::InvalidTelemetry)?;
        let persisted_at = (self.clock)();

        self.storage
            .write(|transaction| {
                if write == SnapshotWrite::Periodic {
                    let existing = transaction
                        .query_row(
                            "SELECT persisted_at FROM printer_status_snapshots WHERE printer_id = ?1",
                            [&snapshot.printer_id],
                            |row| row.get::<_, String>(0),
                        )
                        .optional()?;
                    if let Some(existing) = existing {
                        let existing = match canonical_utc_timestamp(&existing) {
                            Ok(existing) => existing,
                            Err(()) => return Ok(Err(StatusCacheError::MalformedSnapshot)),
                        };
                        let existing = match DateTime::parse_from_rfc3339(&existing) {
                            Ok(existing) => existing.with_timezone(&Utc),
                            Err(_) => return Ok(Err(StatusCacheError::MalformedSnapshot)),
                        };
                        if persisted_at - existing < PERIODIC_WRITE_INTERVAL {
                            return Ok(Ok(()));
                        }
                    }
                }
                transaction.execute(
                    "INSERT INTO printer_status_snapshots(printer_id, telemetry_json, last_observed_at, persisted_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(printer_id) DO UPDATE SET
                        telemetry_json = excluded.telemetry_json,
                        last_observed_at = excluded.last_observed_at,
                        persisted_at = excluded.persisted_at",
                    (
                        &snapshot.printer_id,
                        telemetry_json,
                        last_observed_at,
                        persisted_at.to_rfc3339_opts(SecondsFormat::AutoSi, true),
                    ),
                )?;
                Ok(Ok(()))
            })??;
        Ok(())
    }

    pub fn delete(&self, printer_id: &str) -> Result<(), StatusCacheError> {
        self.storage.write(|transaction| {
            transaction.execute(
                "DELETE FROM printer_status_snapshots WHERE printer_id = ?1",
                [printer_id],
            )?;
            Ok(())
        })?;
        Ok(())
    }
}

fn decode_snapshot(
    (printer_id, telemetry_json, last_observed_at): (String, String, String),
) -> Result<StoredTelemetrySnapshot, StatusCacheError> {
    let telemetry =
        serde_json::from_str(&telemetry_json).map_err(|_| StatusCacheError::MalformedSnapshot)?;
    validate_telemetry(&telemetry).map_err(|_| StatusCacheError::MalformedSnapshot)?;
    let last_observed_at = canonical_utc_timestamp(&last_observed_at)
        .map_err(|_| StatusCacheError::MalformedSnapshot)?;
    Ok(StoredTelemetrySnapshot {
        printer_id,
        telemetry,
        last_observed_at,
    })
}

fn canonical_utc_timestamp(value: &str) -> Result<String, ()> {
    let timestamp = DateTime::parse_from_rfc3339(value).map_err(|_| ())?;
    if timestamp.offset().local_minus_utc() != 0 {
        return Err(());
    }
    Ok(timestamp
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::AutoSi, true))
}

fn validate_telemetry(telemetry: &PrinterTelemetry) -> Result<(), StatusCacheError> {
    if telemetry
        .progress
        .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
        || [
            telemetry.nozzle_temp_c,
            telemetry.nozzle_target_c,
            telemetry.bed_temp_c,
            telemetry.bed_target_c,
            telemetry.print_duration_s,
        ]
        .into_iter()
        .flatten()
        .any(|value| !value.is_finite())
        || telemetry.print_duration_s.is_some_and(|value| value < 0.0)
        || [
            telemetry.host_activity_name.as_deref(),
            telemetry.job_name.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|value| value.is_empty() || value.len() > 512 || value.chars().any(char::is_control))
    {
        Err(StatusCacheError::InvalidTelemetry)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::{DateTime, SecondsFormat, TimeZone, Utc};

    use crate::persistence::Storage;
    use crate::printers::operational::HostActivity;
    use crate::printers::repository::PrinterRepository;
    use crate::printers::StoredPrinter;

    use super::{
        PrinterTelemetry, SnapshotWrite, StatusCacheError, StatusRepository,
        StoredTelemetrySnapshot,
    };

    fn storage() -> (
        tempfile::TempDir,
        crate::persistence::MetadataRootLease,
        Arc<Storage>,
    ) {
        let (temporary_root, lease, storage) = crate::test_storage();
        PrinterRepository::new(Arc::clone(&storage))
            .create(StoredPrinter {
                id: "prn-1".to_string(),
                name: "Snapshot test printer".to_string(),
                ..Default::default()
            })
            .expect("test printer");
        (temporary_root, lease, storage)
    }

    fn at(second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 18, 0, 0, second)
            .single()
            .expect("valid test timestamp")
    }

    fn snapshot_at(second: u32) -> StoredTelemetrySnapshot {
        StoredTelemetrySnapshot {
            printer_id: "prn-1".to_string(),
            telemetry: PrinterTelemetry {
                host_activity: HostActivity::Printing,
                host_activity_name: Some("printing".to_string()),
                job_name: Some("calibration-cube.gcode".to_string()),
                progress: Some(0.25),
                nozzle_temp_c: Some(210.0),
                nozzle_target_c: Some(215.0),
                bed_temp_c: Some(60.0),
                bed_target_c: Some(60.0),
                print_duration_s: Some(120.0),
            },
            last_observed_at: at(second).to_rfc3339_opts(SecondsFormat::Secs, true),
        }
    }

    fn repository_at(storage: Arc<Storage>, now: Arc<Mutex<DateTime<Utc>>>) -> StatusRepository {
        StatusRepository::with_clock(storage, move || now.lock().expect("test clock").clone())
    }

    #[test]
    fn periodic_writes_coalesce_until_thirty_seconds_have_elapsed() {
        let (_temporary_root, _lease, storage) = storage();
        let now = Arc::new(Mutex::new(at(0)));
        let repository = repository_at(storage, Arc::clone(&now));

        repository
            .save_if_due(&snapshot_at(0), SnapshotWrite::Periodic)
            .expect("initial snapshot");
        *now.lock().expect("test clock") = at(10);
        repository
            .save_if_due(&snapshot_at(10), SnapshotWrite::Periodic)
            .expect("coalesced snapshot");

        assert_eq!(
            repository
                .get("prn-1")
                .expect("cached snapshot")
                .expect("snapshot exists")
                .last_observed_at,
            "2026-09-18T00:00:00Z"
        );
    }

    #[test]
    fn activity_transitions_bypass_periodic_write_coalescing() {
        let (_temporary_root, _lease, storage) = storage();
        let now = Arc::new(Mutex::new(at(0)));
        let repository = repository_at(storage, Arc::clone(&now));
        repository
            .save_if_due(&snapshot_at(0), SnapshotWrite::Periodic)
            .expect("initial snapshot");
        *now.lock().expect("test clock") = at(10);

        repository
            .save_if_due(&snapshot_at(10), SnapshotWrite::ActivityTransition)
            .expect("transition snapshot");

        assert_eq!(
            repository
                .get("prn-1")
                .expect("cached snapshot")
                .expect("snapshot exists")
                .last_observed_at,
            "2026-09-18T00:00:10Z"
        );
    }

    #[test]
    fn periodic_writes_persist_at_the_thirty_second_boundary() {
        let (_temporary_root, _lease, storage) = storage();
        let now = Arc::new(Mutex::new(at(0)));
        let repository = repository_at(storage, Arc::clone(&now));
        repository
            .save_if_due(&snapshot_at(0), SnapshotWrite::Periodic)
            .expect("initial snapshot");
        *now.lock().expect("test clock") = at(30);

        repository
            .save_if_due(&snapshot_at(30), SnapshotWrite::Periodic)
            .expect("due snapshot");

        assert_eq!(
            repository
                .get("prn-1")
                .expect("cached snapshot")
                .expect("snapshot exists")
                .last_observed_at,
            "2026-09-18T00:00:30Z"
        );
    }

    #[test]
    fn list_returns_cached_snapshots_in_printer_id_order() {
        let (_temporary_root, _lease, storage) = storage();
        PrinterRepository::new(Arc::clone(&storage))
            .create(StoredPrinter {
                id: "prn-2".to_string(),
                name: "Second snapshot test printer".to_string(),
                ..Default::default()
            })
            .expect("second test printer");
        let now = Arc::new(Mutex::new(at(0)));
        let repository = repository_at(storage, Arc::clone(&now));
        repository
            .save_if_due(&snapshot_at(0), SnapshotWrite::Periodic)
            .expect("first snapshot");
        *now.lock().expect("test clock") = at(30);
        let mut second_snapshot = snapshot_at(30);
        second_snapshot.printer_id = "prn-2".to_string();
        repository
            .save_if_due(&second_snapshot, SnapshotWrite::Periodic)
            .expect("second snapshot");

        assert_eq!(
            repository
                .list()
                .expect("cached snapshots")
                .into_iter()
                .map(|snapshot| snapshot.printer_id)
                .collect::<Vec<_>>(),
            vec!["prn-1", "prn-2"]
        );
    }

    #[test]
    fn delete_removes_a_cached_snapshot_without_deleting_its_printer() {
        let (_temporary_root, _lease, storage) = storage();
        let now = Arc::new(Mutex::new(at(0)));
        let repository = repository_at(Arc::clone(&storage), now);
        repository
            .save_if_due(&snapshot_at(0), SnapshotWrite::Periodic)
            .expect("initial snapshot");

        repository.delete("prn-1").expect("delete snapshot");

        assert_eq!(
            (
                repository.get("prn-1").expect("cached snapshot").is_none(),
                PrinterRepository::new(storage)
                    .get("prn-1")
                    .expect("printer lookup")
                    .is_some(),
            ),
            (true, true)
        );
    }

    #[test]
    fn deleting_a_printer_cascades_its_cached_snapshot() {
        let (_temporary_root, _lease, storage) = storage();
        let now = Arc::new(Mutex::new(at(0)));
        let repository = repository_at(Arc::clone(&storage), now);
        repository
            .save_if_due(&snapshot_at(0), SnapshotWrite::Periodic)
            .expect("initial snapshot");
        let printer = PrinterRepository::new(Arc::clone(&storage))
            .get("prn-1")
            .expect("printer lookup")
            .expect("test printer");

        PrinterRepository::new(storage)
            .delete("prn-1", printer.revision)
            .expect("delete printer");

        assert_eq!(repository.get("prn-1").expect("cached snapshot"), None);
    }

    #[test]
    fn malformed_stored_json_returns_a_cache_specific_error() {
        let (_temporary_root, _lease, storage) = storage();
        let now = Arc::new(Mutex::new(at(0)));
        let repository = repository_at(Arc::clone(&storage), now);
        repository
            .save_if_due(&snapshot_at(0), SnapshotWrite::Periodic)
            .expect("initial snapshot");
        storage
            .write(|transaction| {
                transaction.execute_batch("PRAGMA ignore_check_constraints = ON")?;
                let update = transaction.execute(
                    "UPDATE printer_status_snapshots
                     SET telemetry_json = '{not json'
                     WHERE printer_id = 'prn-1'",
                    [],
                );
                transaction.execute_batch("PRAGMA ignore_check_constraints = OFF")?;
                update?;
                Ok(())
            })
            .expect("corrupt cache fixture");

        assert!(matches!(
            repository.get("prn-1"),
            Err(StatusCacheError::MalformedSnapshot)
        ));
    }

    #[test]
    fn cache_writes_do_not_increment_printer_revisions() {
        let (_temporary_root, _lease, storage) = storage();
        let now = Arc::new(Mutex::new(at(0)));
        let repository = repository_at(Arc::clone(&storage), now);
        let before = PrinterRepository::new(Arc::clone(&storage))
            .get("prn-1")
            .expect("printer lookup")
            .expect("test printer")
            .revision;

        repository
            .save_if_due(&snapshot_at(0), SnapshotWrite::Periodic)
            .expect("initial snapshot");

        assert_eq!(
            PrinterRepository::new(storage)
                .get("prn-1")
                .expect("printer lookup")
                .expect("test printer")
                .revision,
            before
        );
    }

    #[test]
    fn non_utc_observation_timestamps_are_rejected() {
        let (_temporary_root, _lease, storage) = storage();
        let now = Arc::new(Mutex::new(at(0)));
        let repository = repository_at(storage, now);
        let mut snapshot = snapshot_at(0);
        snapshot.last_observed_at = "2026-09-18T01:00:00+01:00".to_string();

        assert!(matches!(
            repository.save_if_due(&snapshot, SnapshotWrite::Periodic),
            Err(StatusCacheError::InvalidTimestamp)
        ));
    }

    #[test]
    fn non_finite_monitor_telemetry_is_rejected() {
        let (_temporary_root, _lease, storage) = storage();
        let now = Arc::new(Mutex::new(at(0)));
        let repository = repository_at(storage, now);
        let mut snapshot = snapshot_at(0);
        snapshot.telemetry.nozzle_temp_c = Some(f64::NAN);

        assert!(matches!(
            repository.save_if_due(&snapshot, SnapshotWrite::Periodic),
            Err(StatusCacheError::InvalidTelemetry)
        ));
    }
}
