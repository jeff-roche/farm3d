//! One derivation of Printer setup facts (spec D4) and one `supervise`
//! helper that every command call site uses instead of hand-building
//! `PrinterSetupFacts` and choosing between `start`/`reconcile_printer`
//! itself.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::catalog::resolve::{resolve_catalog_ref, CatalogStatus};
use crate::catalog::Catalog;
use crate::connections::credentials::CredentialBackend;
use crate::connections::supervisor::{ConnectionManager, PrinterSetupFacts};
use crate::connections::MOONRAKER_KIND;

use super::StoredPrinter;

/// The ordered, possibly-empty reasons a Printer's setup is incomplete. See
/// spec D4.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SetupGap.ts")]
pub enum SetupGap {
    MissingConnection,
    UnsupportedAdapter,
    UnresolvedProfile,
}

/// Derives `PrinterSetupFacts` and its ordered `SetupGap`s straight from
/// persisted columns. Never stored — recomputed on every read, every
/// supervisor start/reconcile, and at restart (spec D4).
///
/// A missing credential is deliberately NOT considered here: it is a
/// runtime condition surfaced by `supervise_printer`, not a `SetupGap`
/// (plan clarification 1).
pub fn derive_setup_facts(
    printer: &StoredPrinter,
    catalog: &Catalog,
) -> (PrinterSetupFacts, Vec<SetupGap>) {
    let mut gaps = Vec::new();
    let has_usable_connection = match &printer.connection {
        None => {
            gaps.push(SetupGap::MissingConnection);
            false
        }
        Some(connection) if connection.kind != MOONRAKER_KIND => {
            gaps.push(SetupGap::UnsupportedAdapter);
            false
        }
        Some(_) => true,
    };
    let (_, status) = resolve_catalog_ref(catalog, &printer.catalog_ref);
    let profile_resolved = matches!(status, CatalogStatus::Ok | CatalogStatus::Rematched);
    if !profile_resolved {
        gaps.push(SetupGap::UnresolvedProfile);
    }
    (
        PrinterSetupFacts {
            has_usable_connection,
            profile_resolved,
        },
        gaps,
    )
}

/// What `supervise_printer` did for one Printer, for a caller that needs to
/// react — e.g. surface `OperationWarning::credential_required`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupervisionOutcome {
    Started,
    Reconciled,
    CredentialRequired,
    /// Carries whether the stop was graceful, so a caller (e.g.
    /// `archive_printer`) can surface `OperationWarning::supervisor` when it
    /// wasn't.
    Archived(bool),
}

/// The one path every call site uses to bring a Printer's live supervision
/// state in line with its persisted configuration.
///
/// A Connection whose referenced credential is missing from the credential
/// store reconciles to Setup incomplete here, at runtime — that check is
/// deliberately not part of `derive_setup_facts` (plan clarification 1).
pub async fn supervise_printer<R: tauri::Runtime>(
    manager: &ConnectionManager<R>,
    credentials: &dyn CredentialBackend,
    catalog: &Catalog,
    printer: &StoredPrinter,
) -> SupervisionOutcome {
    if printer.archived_at.is_some() {
        let graceful = manager.stop(&printer.id).await; // stop also publishes removed
        return SupervisionOutcome::Archived(graceful);
    }
    let (facts, _) = derive_setup_facts(printer, catalog);
    let Some(config) = printer
        .connection
        .clone()
        .filter(|_| facts.has_usable_connection)
    else {
        manager.reconcile_printer(&printer.id, facts);
        return SupervisionOutcome::Reconciled;
    };
    let secret = match config.credential_ref.as_deref() {
        None => None,
        Some(reference) => match credentials.get(reference) {
            Ok(Some(value)) => Some(zeroize::Zeroizing::new(value)),
            _ => {
                manager.reconcile_printer(
                    &printer.id,
                    PrinterSetupFacts {
                        has_usable_connection: false,
                        ..facts
                    },
                );
                return SupervisionOutcome::CredentialRequired;
            }
        },
    };
    manager.start(printer.id.clone(), config, secret, facts).await;
    SupervisionOutcome::Started
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{BedShape, CatalogModel, CatalogVariant};
    use crate::connections::credentials::CredentialStore;
    use crate::connections::status_repository::StatusRepository;
    use crate::connections::ConnectionConfig;
    use crate::printers::operational::OperationalState;
    use crate::printers::CatalogRef;
    use chrono::{TimeZone, Utc};
    use std::sync::Arc;

    fn variant(name: &str, printer_variant: &str) -> CatalogVariant {
        CatalogVariant {
            variant: name.to_string(),
            printer_variant: printer_variant.to_string(),
            bed_shape: BedShape::Rectangular {
                width_mm: 256.0,
                depth_mm: 256.0,
                origin_x_mm: 0.0,
                origin_y_mm: 0.0,
            },
            printable_height_mm: 256.0,
            bed_exclude_areas: vec![],
            default_bed_type: "4".to_string(),
            nozzle_diameter_mm: vec![0.4],
            nozzle_type: "hardened_steel".to_string(),
            gcode_flavor: "klipper".to_string(),
            has_auxiliary_fan: true,
            supports_air_filtration: true,
            supports_multi_filament: true,
            suggested_host_type: Some("elegoolink".to_string()),
        }
    }

    fn a_catalog() -> Catalog {
        Catalog {
            generated_at: "2026-08-20T00:00:00Z".to_string(),
            source_tag: "v2.4.2".to_string(),
            notice: "test".to_string(),
            models: vec![CatalogModel {
                model_id: "Elegoo-CC".to_string(),
                vendor: "Elegoo".to_string(),
                model: "Elegoo Centauri Carbon".to_string(),
                variants: vec![variant("Elegoo Centauri Carbon 0.4 nozzle", "0.4")],
            }],
        }
    }

    fn a_ref() -> CatalogRef {
        CatalogRef {
            vendor: "Elegoo".to_string(),
            model: "Elegoo Centauri Carbon".to_string(),
            variant: "Elegoo Centauri Carbon 0.4 nozzle".to_string(),
            model_id: "Elegoo-CC".to_string(),
            printer_variant: "0.4".to_string(),
        }
    }

    fn a_printer() -> StoredPrinter {
        StoredPrinter {
            id: "prn-1".to_string(),
            name: "Test Printer".to_string(),
            catalog_ref: a_ref(),
            ..Default::default()
        }
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
    fn no_connection_is_missing_connection_with_a_resolved_profile() {
        let catalog = a_catalog();
        let printer = a_printer();

        let (facts, gaps) = derive_setup_facts(&printer, &catalog);

        assert!(!facts.has_usable_connection);
        assert!(facts.profile_resolved);
        assert_eq!(gaps, vec![SetupGap::MissingConnection]);
    }

    #[test]
    fn an_unsupported_adapter_kind_is_an_unsupported_adapter_gap() {
        let catalog = a_catalog();
        let mut printer = a_printer();
        printer.connection = Some(ConnectionConfig {
            kind: "octoprint".to_string(),
            host: "printer.local".to_string(),
            port: 80,
            use_tls: false,
            credential_ref: None,
        });

        let (facts, gaps) = derive_setup_facts(&printer, &catalog);

        assert!(!facts.has_usable_connection);
        assert_eq!(gaps, vec![SetupGap::UnsupportedAdapter]);
    }

    #[test]
    fn an_unresolvable_ref_with_no_connection_orders_missing_connection_before_unresolved_profile()
    {
        let catalog = a_catalog();
        let mut printer = a_printer();
        printer.catalog_ref.vendor = "NoSuchVendor".to_string();

        let (facts, gaps) = derive_setup_facts(&printer, &catalog);

        assert!(!facts.has_usable_connection);
        assert!(!facts.profile_resolved);
        assert_eq!(
            gaps,
            vec![SetupGap::MissingConnection, SetupGap::UnresolvedProfile]
        );
    }

    #[test]
    fn a_moonraker_connection_with_a_resolved_profile_has_no_gaps_and_is_setup_complete() {
        let catalog = a_catalog();
        let mut printer = a_printer();
        printer.connection = Some(moonraker_config(None));

        let (facts, gaps) = derive_setup_facts(&printer, &catalog);

        assert!(gaps.is_empty());
        assert!(facts.has_usable_connection && facts.profile_resolved);
    }

    fn manager_with_factory(
        storage: Arc<crate::persistence::Storage>,
        app: &tauri::App<tauri::test::MockRuntime>,
        factory: impl Fn(
                &ConnectionConfig,
                Option<zeroize::Zeroizing<String>>,
            ) -> Option<Box<dyn crate::connections::PrinterConnection>>
            + Send
            + Sync
            + 'static,
    ) -> ConnectionManager<tauri::test::MockRuntime> {
        ConnectionManager::with_clock_and_factory(
            app.handle().clone(),
            Arc::new(StatusRepository::new(storage)),
            || Utc.with_ymd_and_hms(2026, 9, 22, 12, 0, 0).unwrap(),
            factory,
        )
    }

    #[tokio::test]
    async fn an_archived_printer_is_stopped_and_never_supervised() {
        let (_root, _lease, storage) = crate::test_storage();
        let app = tauri::test::mock_app();
        let manager = manager_with_factory(storage, &app, |_config, _key| None);
        let credentials = CredentialStore::file_backed(
            tempfile::tempdir().expect("tempdir").path().to_path_buf(),
        );
        let catalog = a_catalog();
        let mut printer = a_printer();
        printer.connection = Some(moonraker_config(None));
        printer.archived_at = Some("2026-09-20T00:00:00Z".to_string());

        let outcome = supervise_printer(&manager, &credentials, &catalog, &printer).await;

        assert_eq!(outcome, SupervisionOutcome::Archived(true));
        assert!(manager.statuses().is_empty());
    }

    #[tokio::test]
    async fn no_connection_reconciles_to_setup_incomplete() {
        let (_root, _lease, storage) = crate::test_storage();
        let app = tauri::test::mock_app();
        let manager = manager_with_factory(storage, &app, |_config, _key| None);
        let credentials = CredentialStore::file_backed(
            tempfile::tempdir().expect("tempdir").path().to_path_buf(),
        );
        let catalog = a_catalog();
        let printer = a_printer();

        let outcome = supervise_printer(&manager, &credentials, &catalog, &printer).await;

        assert_eq!(outcome, SupervisionOutcome::Reconciled);
        assert_eq!(
            manager.statuses()["prn-1"].operational_state,
            OperationalState::SetupIncomplete
        );
    }

    #[tokio::test]
    async fn a_missing_stored_credential_reconciles_to_setup_incomplete() {
        let (_root, _lease, storage) = crate::test_storage();
        let app = tauri::test::mock_app();
        let manager = manager_with_factory(storage, &app, |_config, _key| None);
        let credentials = CredentialStore::file_backed(
            tempfile::tempdir().expect("tempdir").path().to_path_buf(),
        );
        let catalog = a_catalog();
        let mut printer = a_printer();
        printer.connection = Some(moonraker_config(Some("farm3d/credential/missing")));

        let outcome = supervise_printer(&manager, &credentials, &catalog, &printer).await;

        assert_eq!(outcome, SupervisionOutcome::CredentialRequired);
        assert_eq!(
            manager.statuses()["prn-1"].operational_state,
            OperationalState::SetupIncomplete
        );
    }

    #[tokio::test]
    async fn a_connection_with_its_secret_present_starts_supervision() {
        let (_root, _lease, storage) = crate::test_storage();
        let app = tauri::test::mock_app();
        // The factory runs inside the supervisor's spawned task, which may
        // land on a different runtime thread than this test — a channel
        // (awaited with a timeout) is deterministic where a bare
        // `yield_now` is not.
        let (factory_called_tx, mut factory_called_rx) = tokio::sync::mpsc::channel::<()>(1);
        let manager = manager_with_factory(storage, &app, move |_config, _key| {
            let _ = factory_called_tx.try_send(());
            None
        });
        let dir = tempfile::tempdir().expect("tempdir");
        let credentials = CredentialStore::file_backed(dir.path().to_path_buf());
        credentials
            .set("farm3d/credential/present", "s3cret")
            .expect("credential store write");
        let catalog = a_catalog();
        let mut printer = a_printer();
        printer.connection = Some(moonraker_config(Some("farm3d/credential/present")));

        let outcome = supervise_printer(&manager, &credentials, &catalog, &printer).await;
        let called = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            factory_called_rx.recv(),
        )
        .await
        .expect("the connection factory should be invoked promptly");

        assert_eq!(outcome, SupervisionOutcome::Started);
        assert!(called.is_some());
    }
}
