use std::sync::Arc;

use farm3d_lib::connections::{ConnectionConfig, DEFAULT_MOONRAKER_PORT, MOONRAKER_KIND};
use farm3d_lib::persistence::{MetadataRootLease, RepositoryError, Storage, StoragePaths};
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::{CatalogRef, PrinterProfileOverrides, StartSafety, StoredPrinter};
use farm3d_lib::settings::commands::{MonitorDensity, MonitorSection};
use farm3d_lib::settings::repository::SettingsRepository;

fn storage() -> (tempfile::TempDir, MetadataRootLease, Arc<Storage>) {
    let temp = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Arc::new(Storage::open(paths, &lease).unwrap());
    (temp, lease, storage)
}

fn printer(id: &str) -> StoredPrinter {
    StoredPrinter {
        id: id.to_string(),
        name: "Printer".to_string(),
        catalog_ref: CatalogRef {
            vendor: "Vendor".to_string(),
            model: "Model".to_string(),
            variant: "Variant".to_string(),
            model_id: "model-id".to_string(),
            printer_variant: "0.4".to_string(),
        },
        notes: String::new(),
        overrides: PrinterProfileOverrides::default(),
        ..Default::default()
    }
}

#[test]
fn repositories_persist_revisions_and_detect_stale_updates() {
    let (_temp, _lease, storage) = storage();
    let settings = SettingsRepository::new(Arc::clone(&storage));
    let printers = PrinterRepository::new(Arc::clone(&storage));

    let initial = settings.ensure_default().unwrap();
    assert_eq!(initial.revision, 1);
    let saved = settings
        .save(
            initial.revision,
            "farm3d-dark",
            MonitorSection::PrinterModel,
            MonitorDensity::Comfortable,
        )
        .unwrap();
    assert_eq!(saved.revision, 2);
    assert!(settings
        .save(
            1,
            "system",
            MonitorSection::PrinterModel,
            MonitorDensity::Comfortable,
        )
        .is_err());

    let created = printers.create(printer("prn-a")).unwrap();
    assert_eq!(created.revision, 1);
    let updated = printers
        .update(created.id.as_str(), 1, |value| {
            value.notes = "persisted".to_string()
        })
        .unwrap();
    assert_eq!((updated.revision, updated.notes.as_str()), (2, "persisted"));
    assert!(printers.update("prn-a", 1, |_| {}).is_err());
    drop(printers);
    let reopened = PrinterRepository::new(storage);
    assert_eq!(reopened.list().unwrap()[0].notes, "persisted");
}

#[test]
fn generated_printer_ids_are_uuid_v4_and_unique() {
    let ids: std::collections::HashSet<_> = (0..1000)
        .map(|_| PrinterRepository::generate_id())
        .collect();
    assert_eq!(ids.len(), 1000);
    assert!(ids
        .iter()
        .all(|id| id.starts_with("prn-") && id.len() == 40));
}

#[test]
fn credential_cleanup_precedence_and_ties_are_deterministic_in_every_enqueue_order() {
    let (_temp, _lease, storage) = storage();
    let repository = PrinterRepository::new(Arc::clone(&storage));
    let reasons = [
        "import_orphan",
        "provisional",
        "replaced",
        "cleared",
        "printer_deleted",
    ];

    for (left_index, left) in reasons.iter().enumerate() {
        for (right_index, right) in reasons.iter().enumerate() {
            let reference = format!("matrix-{left_index}-{right_index}");
            repository
                .enqueue_credential_cleanup(&reference, Some("z-printer"), left)
                .unwrap();
            repository
                .enqueue_credential_cleanup(&reference, Some("a-printer"), right)
                .unwrap();
            let retained: (String, Option<String>) = storage
                .read(|db| {
                    db.query_row(
                "SELECT reason, printer_id FROM pending_credential_cleanup WHERE credential_ref=?1",
                [&reference],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
                })
                .unwrap();
            let expected_reason = reasons[left_index.max(right_index)];
            let expected_printer = if left_index > right_index {
                "z-printer"
            } else {
                "a-printer"
            };
            assert_eq!(
                retained,
                (
                    expected_reason.to_string(),
                    Some(expected_printer.to_string())
                )
            );
        }
    }
}

#[test]
fn import_orphan_never_demotes_an_automatic_cleanup_reason() {
    let (_temp, _lease, storage) = storage();
    let repository = PrinterRepository::new(Arc::clone(&storage));
    repository
        .enqueue_credential_cleanup("shared", Some("prn-z"), "cleared")
        .unwrap();
    repository
        .enqueue_credential_cleanup("shared", None, "import_orphan")
        .unwrap();

    let retained: (String, Option<String>) = storage
        .read(|db| {
            db.query_row(
        "SELECT reason, printer_id FROM pending_credential_cleanup WHERE credential_ref='shared'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
        })
        .unwrap();
    assert_eq!(retained, ("cleared".to_string(), Some("prn-z".to_string())));
}

fn connection_at(host: &str) -> ConnectionConfig {
    ConnectionConfig {
        kind: MOONRAKER_KIND.to_string(),
        host: host.to_string(),
        port: DEFAULT_MOONRAKER_PORT,
        use_tls: false,
        credential_ref: None,
    }
}

/// D3: two non-archived Printers may never share a host identity — the
/// repository's precheck must reject the second `create`.
#[test]
fn creating_a_second_active_printer_with_the_same_host_is_a_duplicate_host_error() {
    let (_temp, _lease, storage) = storage();
    let repository = PrinterRepository::new(Arc::clone(&storage));
    let mut first = printer("prn-a");
    first.connection = Some(connection_at("voron.local"));
    repository.create(first).unwrap();

    let mut second = printer("prn-b");
    second.connection = Some(connection_at("voron.local"));
    let error = repository.create(second).unwrap_err();

    assert!(matches!(
        error,
        RepositoryError::DuplicateHost { conflicting_printer_id } if conflicting_printer_id == "prn-a"
    ));
}

/// An archived Printer does not reserve its host identity (D3) — creating a
/// new active Printer on the same host must succeed.
#[test]
fn the_same_host_is_available_once_the_first_printer_is_archived() {
    let (_temp, _lease, storage) = storage();
    let repository = PrinterRepository::new(Arc::clone(&storage));
    let mut first = printer("prn-a");
    first.connection = Some(connection_at("voron.local"));
    let first = repository.create(first).unwrap();
    repository
        .update(&first.id, first.revision, |value| {
            value.archived_at = Some("2026-09-22T00:00:00.000Z".to_string());
        })
        .unwrap();

    let mut second = printer("prn-b");
    second.connection = Some(connection_at("voron.local"));
    let created = repository.create(second).unwrap();

    assert_eq!(created.id, "prn-b");
}

/// `location`, `startSafety`, and `archivedAt` round-trip through the
/// repository like every other stored field.
#[test]
fn location_start_safety_and_archived_at_round_trip() {
    let (_temp, _lease, storage) = storage();
    let repository = PrinterRepository::new(Arc::clone(&storage));
    let mut fresh = printer("prn-a");
    fresh.location = Some("Bay 3".to_string());
    fresh.start_safety = StartSafety::Unattended;
    let created = repository.create(fresh).unwrap();

    assert_eq!(created.location.as_deref(), Some("Bay 3"));
    assert_eq!(created.start_safety, StartSafety::Unattended);
    assert_eq!(created.archived_at, None);

    let archived = repository
        .update(&created.id, created.revision, |value| {
            value.archived_at = Some("2026-09-22T00:00:00.000Z".to_string());
        })
        .unwrap();

    assert_eq!(
        archived.archived_at.as_deref(),
        Some("2026-09-22T00:00:00.000Z")
    );
    let reloaded = repository.get(&archived.id).unwrap().unwrap();
    assert_eq!(reloaded.location.as_deref(), Some("Bay 3"));
    assert_eq!(reloaded.start_safety, StartSafety::Unattended);
    assert_eq!(
        reloaded.archived_at.as_deref(),
        Some("2026-09-22T00:00:00.000Z")
    );
}
