pub mod catalog;
pub mod connections;
pub mod printers;
mod settings;

use catalog::commands::{catalog_info, list_catalog_models, list_catalog_variants, preview_profile};
use connections::commands::{
    clear_printer_connection, credential_store_info, discover_printers, printer_statuses,
    set_printer_connection, test_printer_connection,
};
use connections::supervisor::ConnectionManager;
use printers::{
    create_printer, delete_printer, list_printers, open_printers_file, rebind_printer,
    resolve_profile_drift, set_printer_override, update_printer,
};
use settings::{load_settings, open_settings_file, save_settings};
use std::sync::Arc;
use tauri::path::BaseDirectory;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let resource_path = app
                .path()
                .resolve("resources/printer-catalog.json", BaseDirectory::Resource)
                .expect("printer-catalog.json should be a bundled resource");
            let snapshot = catalog::load_snapshot(&resource_path)
                .expect("bundled printer-catalog.json should be valid — this is a packaging bug");
            app.manage(Arc::new(snapshot));

            let manager = Arc::new(ConnectionManager::new(app.handle().clone()));
            // Reconnect everything that was already configured, so a printer
            // is live on the dashboard without the user opening its tab.
            if let Ok(dir) = app.path().app_config_dir() {
                if let Ok(file) = printers::load_printers_from(&dir) {
                    let store = connections::credentials::CredentialStore::detect(dir);
                    for stored in &file.printers {
                        let Some(config) = stored.connection.clone() else { continue };
                        let api_key = match config.credential_ref.as_deref() {
                            None => None,
                            Some(key) => match store.get(key) {
                                Ok(found) => found,
                                // NOT the same as "no key configured": the
                                // store itself is unreadable (a corrupt
                                // credentials.json, a refused keychain).
                                // Connecting with `None` would loop forever
                                // reporting an AUTH failure, sending the user
                                // after their API key instead of the store.
                                Err(e) => {
                                    eprintln!(
                                        "farm3d: cannot read the stored credential for {}: {e}",
                                        stored.id
                                    );
                                    manager.report_error(
                                        &stored.id,
                                        format!("Could not read this printer's stored credential: {e}"),
                                    );
                                    continue;
                                }
                            },
                        };
                        manager.start(stored.id.clone(), config, api_key);
                    }
                }
            }
            app.manage(manager);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            open_settings_file,
            list_printers,
            create_printer,
            update_printer,
            delete_printer,
            set_printer_override,
            rebind_printer,
            resolve_profile_drift,
            open_printers_file,
            list_catalog_models,
            list_catalog_variants,
            preview_profile,
            catalog_info,
            set_printer_connection,
            clear_printer_connection,
            test_printer_connection,
            credential_store_info,
            discover_printers,
            printer_statuses,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
