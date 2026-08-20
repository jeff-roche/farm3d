pub mod catalog;
mod settings;

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
            open_settings_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
