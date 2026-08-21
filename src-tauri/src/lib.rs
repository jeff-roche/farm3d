pub mod catalog;
pub mod connections;
mod printers;
mod settings;

use catalog::commands::{catalog_info, list_catalog_models, list_catalog_variants, preview_profile};
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
