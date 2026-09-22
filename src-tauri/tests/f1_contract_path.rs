#[test]
fn command_inventory_is_exactly_the_f1_inventory_plus_p2_lifecycle_additions() {
    assert_eq!(farm3d_lib::COMMAND_NAMES.len(), 27);
    assert_eq!(
        farm3d_lib::COMMAND_NAMES,
        [
            "load_settings",
            "save_settings",
            "export_settings",
            "import_settings",
            "list_printers",
            "create_printer",
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
fn all_twenty_seven_handlers_return_the_captured_nonretryable_bootstrap_error() {
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
        ("archive_printer", json!({"id":"p","expectedRevision":1})),
        ("unarchive_printer", json!({"id":"p","expectedRevision":1})),
        ("export_printers", json!({})),
        ("import_printers", json!({"expectedRevisions":[]})),
        ("list_catalog_models", json!({})),
        ("list_catalog_variants", json!({"vendor":"v","model":"m"})),
        ("preview_profile", json!({"catalogRef":catalog_ref})),
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
    ];
    assert_eq!(cases.len(), 27);
    for (command, mut body) in cases {
        body["contractVersion"] = json!(1);
        let error = invoke(&webview, command, body).unwrap_err();
        assert_eq!(error, serde_json::to_value(&expected).unwrap(), "{command}");
    }
}
