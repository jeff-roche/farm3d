use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

const SETTINGS_FILE_NAME: &str = "settings.json";

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub theme_mode: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme_mode: "system".to_string(),
        }
    }
}

fn settings_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join(SETTINGS_FILE_NAME)
}

fn write_settings_to(config_dir: &Path, settings: &Settings) -> Result<(), String> {
    fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(settings_file_path(config_dir), json).map_err(|e| e.to_string())
}

fn load_settings_from(config_dir: &Path) -> Result<Settings, String> {
    let path = settings_file_path(config_dir);
    if !path.exists() {
        let defaults = Settings::default();
        write_settings_to(config_dir, &defaults)?;
        return Ok(defaults);
    }
    match fs::read_to_string(&path) {
        Ok(contents) => Ok(serde_json::from_str(&contents).unwrap_or_else(|e| {
            eprintln!("settings.json is not valid JSON ({e}); using defaults");
            Settings::default()
        })),
        Err(e) => {
            eprintln!("Could not read settings.json ({e}); using defaults");
            Ok(Settings::default())
        }
    }
}

fn app_config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn load_settings(app: AppHandle) -> Result<Settings, String> {
    load_settings_from(&app_config_dir(&app)?)
}

#[tauri::command]
pub fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    write_settings_to(&app_config_dir(&app)?, &settings)
}

#[tauri::command]
pub fn open_settings_file(app: AppHandle) -> Result<(), String> {
    let config_dir = app_config_dir(&app)?;
    let path = settings_file_path(&config_dir);
    if !path.exists() {
        write_settings_to(&config_dir, &Settings::default())?;
    }
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_dir() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("farm3d-settings-test-{}-{}", std::process::id(), id))
    }

    #[test]
    fn load_creates_default_file_when_missing() {
        let dir = temp_dir();
        let settings = load_settings_from(&dir).unwrap();
        assert_eq!(settings, Settings::default());
        assert!(settings_file_path(&dir).exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir();
        let settings = Settings {
            theme_mode: "farm3d-dark".to_string(),
        };
        write_settings_to(&dir, &settings).unwrap();
        let loaded = load_settings_from(&dir).unwrap();
        assert_eq!(loaded, settings);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_falls_back_to_default_on_corrupt_file() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(settings_file_path(&dir), "not valid json").unwrap();
        let loaded = load_settings_from(&dir).unwrap();
        assert_eq!(loaded, Settings::default());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_fills_missing_fields_with_defaults() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(settings_file_path(&dir), "{}").unwrap();
        let loaded = load_settings_from(&dir).unwrap();
        assert_eq!(loaded, Settings::default());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn json_uses_camel_case_keys() {
        let settings = Settings {
            theme_mode: "farm3d-dark".to_string(),
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(json, r#"{"themeMode":"farm3d-dark"}"#);
    }
}
