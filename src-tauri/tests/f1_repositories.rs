use std::sync::Arc;

use farm3d_lib::persistence::{MetadataRootLease, Storage, StoragePaths};
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::{CatalogRef, PrinterProfileOverrides, StoredPrinter};
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
    let saved = settings.save(initial.revision, "farm3d-dark").unwrap();
    assert_eq!(saved.revision, 2);
    assert!(settings.save(1, "system").is_err());

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
