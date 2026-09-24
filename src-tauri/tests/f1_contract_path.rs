#[test]
fn command_inventory_is_exactly_the_f1_inventory_plus_p2_p3_additions() {
    assert_eq!(farm3d_lib::COMMAND_NAMES.len(), 41);
    assert_eq!(
        farm3d_lib::COMMAND_NAMES,
        [
            "load_settings",
            "save_settings",
            "export_settings",
            "import_settings",
            "list_printers",
            "create_printer",
            "set_material_slot_layout",
            "update_printer",
            "delete_printer",
            "set_printer_override",
            "rebind_printer",
            "resolve_profile_drift",
            "printer_lifecycle_eligibility",
            "archive_printer",
            "unarchive_printer",
            "export_printers",
            "import_printers",
            "list_catalog_models",
            "list_catalog_variants",
            "preview_profile",
            "catalog_info",
            "set_printer_connection",
            "clear_printer_connection",
            "test_printer_connection",
            "credential_store_info",
            "discover_printers",
            "printer_statuses",
            "probe_connection",
            "create_printers_batch",
            "cancel_printer_batch",
            "list_duplicate_host_archives",
            "list_spools",
            "spool_history",
            "create_spool",
            "update_spool",
            "record_spool_amount",
            "move_spool",
            "set_spool_lifecycle",
            "create_tare",
            "update_tare",
            "delete_tare",
        ]
    );
}

#[test]
fn generated_contracts_and_sqlite_never_contain_the_fixture_secret() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/generated/contracts");
    let mut stack = vec![root];
    while let Some(path) = stack.pop() {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.path().is_dir() {
                stack.push(entry.path());
            } else {
                let text = std::fs::read_to_string(entry.path()).unwrap();
                assert!(!text.contains("F0_FIXTURE_SENTINEL"));
            }
        }
    }
}

#[test]
fn every_registered_handler_returns_the_captured_nonretryable_bootstrap_error() {
    use farm3d_lib::bootstrap::BootstrapState;
    use farm3d_lib::contracts::command::CommandError;
    use farm3d_lib::RuntimeServices;
    use serde_json::{json, Value};
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{mock_builder, mock_context, noop_assets, INVOKE_KEY};
    use tauri::webview::InvokeRequest;
    use tauri::{Manager, WebviewWindowBuilder};

    fn invoke(
        webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
        command: &str,
        body: Value,
    ) -> Result<Value, Value> {
        tauri::test::get_ipc_response(
            webview,
            InvokeRequest {
                cmd: command.to_string(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: InvokeBody::Json(body),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|response| response.deserialize().unwrap())
    }

    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![
            farm3d_lib::settings::commands::load_settings,
            farm3d_lib::settings::commands::save_settings,
            farm3d_lib::settings::commands::export_settings,
            farm3d_lib::settings::commands::import_settings,
            farm3d_lib::printers::commands::list_printers,
            farm3d_lib::printers::commands::create_printer,
            farm3d_lib::printers::commands::set_material_slot_layout,
            farm3d_lib::printers::commands::update_printer,
            farm3d_lib::printers::commands::delete_printer,
            farm3d_lib::printers::commands::set_printer_override,
            farm3d_lib::printers::commands::rebind_printer,
            farm3d_lib::printers::commands::resolve_profile_drift,
            farm3d_lib::printers::commands::printer_lifecycle_eligibility,
            farm3d_lib::printers::commands::archive_printer,
            farm3d_lib::printers::commands::unarchive_printer,
            farm3d_lib::printers::commands::export_printers,
            farm3d_lib::printers::commands::import_printers,
            farm3d_lib::catalog::commands::list_catalog_models,
            farm3d_lib::catalog::commands::list_catalog_variants,
            farm3d_lib::catalog::commands::preview_profile,
            farm3d_lib::catalog::commands::catalog_info,
            farm3d_lib::connections::commands::set_printer_connection,
            farm3d_lib::connections::commands::clear_printer_connection,
            farm3d_lib::connections::commands::test_printer_connection,
            farm3d_lib::connections::commands::credential_store_info,
            farm3d_lib::connections::commands::discover_printers,
            farm3d_lib::connections::commands::printer_statuses,
            farm3d_lib::printers::create::probe_connection,
            farm3d_lib::printers::batch::create_printers_batch,
            farm3d_lib::printers::batch::cancel_printer_batch,
            farm3d_lib::printers::commands::list_duplicate_host_archives,
            farm3d_lib::spools::commands::list_spools,
            farm3d_lib::spools::commands::spool_history,
            farm3d_lib::spools::commands::create_spool,
            farm3d_lib::spools::commands::update_spool,
            farm3d_lib::spools::commands::record_spool_amount,
            farm3d_lib::spools::commands::move_spool,
            farm3d_lib::spools::commands::set_spool_lifecycle,
            farm3d_lib::spools::commands::create_tare,
            farm3d_lib::spools::commands::update_tare,
            farm3d_lib::spools::commands::delete_tare,
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let expected = CommandError::migration_failed();
    app.manage(
        BootstrapState::<RuntimeServices<tauri::test::MockRuntime>>::failed(
            expected.clone(),
            || Err(CommandError::internal()),
        ),
    );
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let catalog_ref =
        json!({"vendor":"v","model":"m","variant":"x","modelId":"i","printerVariant":"0.4"});
    let submission =
        json!({"kind":"moonraker","host":"printer.invalid","port":7125,"useTls":false});
    let cases = vec![
        ("load_settings", json!({})),
        (
            "save_settings",
            json!({"expectedRevision":1,"themeMode":"system","monitorSection":"printerModel","monitorDensity":"comfortable"}),
        ),
        ("export_settings", json!({})),
        ("import_settings", json!({"expectedRevision":1})),
        ("list_printers", json!({})),
        (
            "create_printer",
            json!({"name":"p","catalogRef":catalog_ref.clone()}),
        ),
        (
            "set_material_slot_layout",
            json!({"printerId":"p","expectedRevision":1,"slots":[]}),
        ),
        (
            "update_printer",
            json!({"id":"p","expectedRevision":1,"patch":{"name":"p"}}),
        ),
        ("delete_printer", json!({"id":"p","expectedRevision":1})),
        (
            "set_printer_override",
            json!({"id":"p","expectedRevision":1,"field":"printableHeightMm","value":null}),
        ),
        (
            "rebind_printer",
            json!({"id":"p","expectedRevision":1,"catalogRef":catalog_ref.clone()}),
        ),
        (
            "resolve_profile_drift",
            json!({"id":"p","expectedRevision":1,"action":"accept"}),
        ),
        ("printer_lifecycle_eligibility", json!({"id":"p"})),
        (
            "archive_printer",
            json!({"id":"p","expectedRevision":1,"operationId":"op","spoolDispositions":[]}),
        ),
        ("unarchive_printer", json!({"id":"p","expectedRevision":1})),
        ("export_printers", json!({})),
        ("import_printers", json!({"expectedRevisions":[]})),
        ("list_catalog_models", json!({})),
        ("list_catalog_variants", json!({"vendor":"v","model":"m"})),
        ("preview_profile", json!({"catalogRef":catalog_ref.clone()})),
        ("catalog_info", json!({})),
        (
            "set_printer_connection",
            json!({"id":"p","expectedRevision":1,"submission":submission.clone()}),
        ),
        (
            "clear_printer_connection",
            json!({"id":"p","expectedRevision":1}),
        ),
        (
            "test_printer_connection",
            json!({"id":"p","submission":submission.clone()}),
        ),
        ("credential_store_info", json!({})),
        ("discover_printers", json!({})),
        ("printer_statuses", json!({})),
        (
            "probe_connection",
            json!({"submission": submission.clone()}),
        ),
        (
            "create_printers_batch",
            json!({"input": {
                "batchId": "b",
                "shared": {"catalogRef": catalog_ref.clone(), "startSafety": "confirmBedClear"},
                "probe": false,
                "rows": [],
            }}),
        ),
        ("cancel_printer_batch", json!({"batchId": "b"})),
        ("list_duplicate_host_archives", json!({})),
        ("list_spools", json!({})),
        ("spool_history", json!({"spoolId": "s"})),
        (
            "create_spool",
            json!({
                "fields": {
                    "manufacturer": "m",
                    "materialFamily": "PLA",
                    "colorName": "c",
                    "diameter": "1.75",
                    "nominalMg": 1_000_000,
                    "lowThresholdMg": 0,
                },
                "initialAmount": {"kind": "net", "netMg": 1, "confidence": "estimated"},
            }),
        ),
        (
            "update_spool",
            json!({"id": "s", "expectedRevision": 1, "patch": {
                "manufacturer": "m",
                "materialFamily": "PLA",
                "colorName": "c",
                "diameter": "1.75",
                "nominalMg": 1_000_000,
                "lowThresholdMg": 0,
            }}),
        ),
        (
            "record_spool_amount",
            json!({"id": "s", "expectedRevision": 1, "entry": {"kind": "net", "netMg": 1, "confidence": "estimated"}}),
        ),
        (
            "move_spool",
            json!({"operationId": "op", "spoolId": "s", "expectedSpoolRevision": 1, "destination": {"kind": "storage"}}),
        ),
        (
            "set_spool_lifecycle",
            json!({"operationId": "op", "id": "s", "expectedRevision": 1, "action": "archive"}),
        ),
        ("create_tare", json!({"name": "t", "weightMg": 1})),
        (
            "update_tare",
            json!({"id": "t", "expectedRevision": 1, "name": "t", "weightMg": 1}),
        ),
        ("delete_tare", json!({"id": "t", "expectedRevision": 1})),
    ];
    assert_eq!(cases.len(), farm3d_lib::COMMAND_NAMES.len());
    for (command, mut body) in cases {
        body["contractVersion"] = json!(1);
        let error = invoke(&webview, command, body).unwrap_err();
        assert_eq!(error, serde_json::to_value(&expected).unwrap(), "{command}");
    }
}
