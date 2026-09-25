use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::file_links::file_has_multiple_links;
use crate::printers::{PrintersFile, StoredPrinter};

use super::database::create_private_directory;
use super::{Storage, StorageError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacyMigrationOutcome {
    pub settings_rows: usize,
    pub printer_rows: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacySettings {
    theme_mode: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StagedSource {
    present: bool,
    sha256: Option<String>,
    file_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StagingManifest {
    created_at: String,
    settings: StagedSource,
    printers: StagedSource,
}

struct StagedImport {
    directory: PathBuf,
    manifest: StagingManifest,
    settings: Option<Vec<u8>>,
    printers: Option<Vec<u8>>,
}

pub fn migrate_legacy(storage: &Storage) -> Result<LegacyMigrationOutcome, StorageError> {
    let completed: i64 = storage.read(|connection| {
        connection.query_row("SELECT count(*) FROM legacy_imports", [], |row| row.get(0))
    })?;
    if completed == 2 {
        resume_archival(storage);
        return completed_outcome(storage);
    }
    if completed != 0 {
        return Err(StorageError::OperationFailed);
    }

    let staged = stage_inputs(storage)?;
    let settings_hash = staged.manifest.settings.sha256.clone();
    let printers_hash = staged.manifest.printers.sha256.clone();
    let (theme_mode, settings_warning) = match staged.settings.as_deref() {
        Some(bytes) => {
            if serde_json::from_slice::<serde_json::Value>(bytes)
                .ok()
                .and_then(|value| {
                    value
                        .get("schemaVersion")
                        .and_then(serde_json::Value::as_i64)
                })
                .is_some_and(|version| version > 0)
            {
                return Err(StorageError::UnsupportedSchemaVersion);
            }
            match serde_json::from_slice::<LegacySettings>(bytes) {
                Ok(settings) if !settings.theme_mode.is_empty() => (settings.theme_mode, false),
                _ => ("system".to_string(), true),
            }
        }
        None => ("system".to_string(), false),
    };
    let printers = match staged.printers.as_deref() {
        Some(bytes) => {
            if super::validation::validate_import_json(bytes).is_err() {
                record_warning(
                    storage,
                    "LEGACY_PRINTERS_BLOCKED",
                    Some("printers.json"),
                    printers_hash.as_deref(),
                    "Legacy Printers data could not be imported.",
                );
                return Err(corrupt_printers(printers_hash));
            }
            let parsed: PrintersFile = serde_json::from_slice(bytes).map_err(|_| {
                record_warning(
                    storage,
                    "LEGACY_PRINTERS_BLOCKED",
                    Some("printers.json"),
                    printers_hash.as_deref(),
                    "Legacy Printers data could not be imported.",
                );
                corrupt_printers(printers_hash.clone())
            })?;
            if parsed.schema_version > 1 {
                return Err(StorageError::UnsupportedSchemaVersion);
            }
            if parsed.schema_version != 1 || validate_printers(&parsed.printers).is_err() {
                record_warning(
                    storage,
                    "LEGACY_PRINTERS_BLOCKED",
                    Some("printers.json"),
                    printers_hash.as_deref(),
                    "Legacy Printers data could not be imported.",
                );
                return Err(corrupt_printers(printers_hash));
            }
            parsed.printers
        }
        None => Vec::new(),
    };
    let now = crate::printers::now_rfc3339();

    storage.write(|transaction| {
        let existing: i64 = transaction.query_row(
            "SELECT count(*) FROM legacy_imports",
            [],
            |row| row.get(0),
        )?;
        if existing != 0 {
            return Err(StorageError::OperationFailed);
        }
        transaction.execute(
            "INSERT INTO settings(singleton_id, revision, theme_mode, updated_at) VALUES (1, 1, ?1, ?2)",
            params![theme_mode, now],
        )?;
        for printer in &printers {
            insert_printer(transaction, printer, &now)?;
        }
        for (name, source, version, rows) in [
            ("settings.json", &staged.manifest.settings, None, 1_i64),
            (
                "printers.json",
                &staged.manifest.printers,
                staged.manifest.printers.present.then_some(1_i64),
                printers.len() as i64,
            ),
        ] {
            transaction.execute(
                "INSERT INTO legacy_imports(source_name, source_present, source_sha256, source_schema_version, imported_rows, completed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![name, source.present, source.sha256, version, rows, now],
            )?;
        }
        if settings_warning {
            insert_warning(
                transaction,
                "LEGACY_SETTINGS_DEFAULTED",
                Some("settings.json"),
                settings_hash.as_deref(),
                "Legacy settings were invalid; defaults were used.",
                "{}",
                &now,
            )?;
        }
        let ignored_groups = staged
            .printers
            .as_deref()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok())
            .and_then(|value| value.get("printers").and_then(serde_json::Value::as_array).cloned())
            .map_or(0, |rows| rows.iter().filter(|row| row.get("group").is_some()).count());
        if ignored_groups > 0 {
            insert_warning(
                transaction,
                "LEGACY_MONITOR_GROUP_IGNORED",
                Some("printers.json"),
                printers_hash.as_deref(),
                "Legacy Monitor Section grouping was not imported.",
                &format!(r#"{{"printerCount":{ignored_groups}}}"#),
                &now,
            )?;
        }
        Ok(())
    })?;

    finalize_staged_import(storage, &staged);
    Ok(LegacyMigrationOutcome {
        settings_rows: 1,
        printer_rows: printers.len(),
    })
}

fn corrupt_printers(source_sha256: Option<String>) -> StorageError {
    StorageError::CorruptData {
        source_name: "printers.json",
        source_sha256,
    }
}

fn completed_outcome(storage: &Storage) -> Result<LegacyMigrationOutcome, StorageError> {
    storage.read(|connection| {
        let printers = connection.query_row(
            "SELECT imported_rows FROM legacy_imports WHERE source_name = 'printers.json'",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(LegacyMigrationOutcome {
            settings_rows: 1,
            printer_rows: printers as usize,
        })
    })
}

fn stage_inputs(storage: &Storage) -> Result<StagedImport, StorageError> {
    let settings = read_source(&storage.paths.metadata_root().join("settings.json"))?;
    let printers = read_source(&storage.paths.metadata_root().join("printers.json"))?;
    let settings_hash = settings.as_deref().map(hash);
    let printers_hash = printers.as_deref().map(hash);
    let staging_root = storage.paths.legacy_root().join(".staging");
    create_private_directory(&staging_root)?;

    if let Some(staged) = reusable_staging(
        &staging_root,
        settings_hash.as_deref(),
        printers_hash.as_deref(),
    ) {
        return Ok(staged);
    }
    cleanup_invalid_staging(&staging_root);

    let directory = staging_root.join(uuid::Uuid::new_v4().to_string());
    create_private_directory(&directory)?;
    let settings_source = stage_source(&directory, "settings.json", settings.as_deref())?;
    let printers_source = stage_source(&directory, "printers.json", printers.as_deref())?;
    let manifest = StagingManifest {
        created_at: crate::printers::now_rfc3339(),
        settings: settings_source,
        printers: printers_source,
    };
    atomic_write(
        &directory.join("manifest.json"),
        &serde_json::to_vec(&manifest).map_err(|_| StorageError::OperationFailed)?,
    )?;
    sync_directory(&directory)?;
    Ok(StagedImport {
        directory,
        manifest,
        settings,
        printers,
    })
}

fn read_source(path: &Path) -> Result<Option<Vec<u8>>, StorageError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(StorageError::PathCollision)
        }
        Ok(_) => {
            let mut options = fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.custom_flags(
                    windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
                );
            }
            let mut file = options.open(path)?;
            let metadata = file.metadata()?;
            if !metadata.is_file() || file_has_multiple_links(&file) {
                return Err(StorageError::PathCollision);
            }
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn stage_source(
    directory: &Path,
    name: &str,
    bytes: Option<&[u8]>,
) -> Result<StagedSource, StorageError> {
    let Some(bytes) = bytes else {
        return Ok(StagedSource {
            present: false,
            sha256: None,
            file_name: None,
        });
    };
    atomic_write(&directory.join(name), bytes)?;
    Ok(StagedSource {
        present: true,
        sha256: Some(hash(bytes)),
        file_name: Some(name.to_string()),
    })
}

fn reusable_staging(
    root: &Path,
    settings_hash: Option<&str>,
    printers_hash: Option<&str>,
) -> Option<StagedImport> {
    let mut entries = fs::read_dir(root).ok()?.flatten().collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let directory = entry.path();
        if fs::symlink_metadata(&directory)
            .ok()
            .is_none_or(|metadata| !metadata.is_dir() || metadata.file_type().is_symlink())
        {
            continue;
        }
        let manifest: StagingManifest =
            serde_json::from_slice(&fs::read(directory.join("manifest.json")).ok()?).ok()?;
        if manifest.settings.sha256.as_deref() != settings_hash
            || manifest.printers.sha256.as_deref() != printers_hash
        {
            continue;
        }
        let settings = read_staged(&directory, &manifest.settings)?;
        let printers = read_staged(&directory, &manifest.printers)?;
        return Some(StagedImport {
            directory,
            manifest,
            settings,
            printers,
        });
    }
    None
}

fn read_staged(directory: &Path, source: &StagedSource) -> Option<Option<Vec<u8>>> {
    if !source.present {
        return Some(None);
    }
    let bytes = fs::read(directory.join(source.file_name.as_deref()?)).ok()?;
    (source.sha256.as_deref() == Some(hash(&bytes).as_str())).then_some(Some(bytes))
}

fn cleanup_invalid_staging(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let _ = fs::remove_dir_all(entry.path());
    }
}

fn resume_archival(storage: &Storage) {
    let hashes = storage.read(|connection| {
        let mut statement = connection.prepare("SELECT source_name, source_present, source_sha256 FROM legacy_imports ORDER BY source_name")?;
        let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?, row.get::<_, Option<String>>(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>();
        rows
    });
    let Ok(hashes) = hashes else { return };
    let settings_hash = hashes
        .iter()
        .find(|row| row.0 == "settings.json")
        .and_then(|row| row.2.as_deref());
    let printers_hash = hashes
        .iter()
        .find(|row| row.0 == "printers.json")
        .and_then(|row| row.2.as_deref());
    let root = storage.paths.legacy_root().join(".staging");
    if let Some(staged) = reusable_staging(&root, settings_hash, printers_hash) {
        finalize_staged_import(storage, &staged);
        return;
    }
    for (name, present, expected_hash) in hashes {
        let source = storage.paths.metadata_root().join(&name);
        let current = read_source(&source).ok().flatten();
        if let Some(bytes) = current {
            let stem = name.trim_end_matches(".json");
            let kind = if present && expected_hash.as_deref() == Some(hash(&bytes).as_str()) {
                "migrated"
            } else if present {
                "changed-after-import"
            } else {
                "ignored-after-migration"
            };
            if archive_bytes_or_source(storage, Some(&source), &bytes, stem, kind).is_err() {
                record_warning(
                    storage,
                    "LEGACY_ARCHIVE_INCOMPLETE",
                    Some(&name),
                    expected_hash.as_deref(),
                    "Legacy archival will be retried.",
                );
            }
        } else if present {
            record_warning(
                storage,
                "LEGACY_ARCHIVE_INCOMPLETE",
                Some(&name),
                expected_hash.as_deref(),
                "The staged legacy source was unavailable during archival.",
            );
        }
    }
}

fn finalize_staged_import(storage: &Storage, staged: &StagedImport) {
    for (name, source, staged_bytes) in [
        (
            "settings.json",
            &staged.manifest.settings,
            staged.settings.as_deref(),
        ),
        (
            "printers.json",
            &staged.manifest.printers,
            staged.printers.as_deref(),
        ),
    ] {
        let canonical = storage.paths.metadata_root().join(name);
        let current = read_source(&canonical).ok().flatten();
        let stem = name.trim_end_matches(".json");
        let result = match (source.present, staged_bytes, current.as_deref()) {
            (true, Some(_imported), Some(current))
                if hash(current) == source.sha256.as_deref().unwrap_or_default() =>
            {
                archive_bytes_or_source(storage, Some(&canonical), current, stem, "migrated")
            }
            (true, Some(imported), Some(current)) => {
                archive_bytes_or_source(storage, None, imported, stem, "migrated")
                    .and_then(|_| {
                        archive_bytes_or_source(
                            storage,
                            Some(&canonical),
                            current,
                            stem,
                            "changed-after-import",
                        )
                    })
                    .map(|_| {
                        record_warning(
                            storage,
                            "LEGACY_SOURCE_CHANGED",
                            Some(name),
                            source.sha256.as_deref(),
                            "Legacy data changed after import and was not imported again.",
                        )
                    })
            }
            (true, Some(imported), None) => {
                archive_bytes_or_source(storage, None, imported, stem, "migrated")
            }
            (false, _, Some(current)) => archive_bytes_or_source(
                storage,
                Some(&canonical),
                current,
                stem,
                "ignored-after-migration",
            )
            .map(|_| {
                record_warning(
                    storage,
                    "LEGACY_SOURCE_IGNORED",
                    Some(name),
                    Some(&hash(current)),
                    "Legacy data appeared after migration and was not imported.",
                )
            }),
            _ => Ok(()),
        };
        if result.is_err() {
            record_warning(
                storage,
                "LEGACY_ARCHIVE_INCOMPLETE",
                Some(name),
                source.sha256.as_deref(),
                "Legacy archival will be retried.",
            );
            return;
        }
    }
    let _ = fs::remove_dir_all(&staged.directory);
}

fn archive_bytes_or_source(
    storage: &Storage,
    source: Option<&Path>,
    bytes: &[u8],
    stem: &str,
    kind: &str,
) -> Result<(), StorageError> {
    let digest = hash(bytes);
    let marker = format!("-{}.json", &digest[..12]);
    if let Ok(entries) = fs::read_dir(storage.paths.legacy_root()) {
        let prefix = format!("{stem}.{kind}-");
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(&prefix)
                && (name.ends_with(&marker)
                    || name
                        .strip_suffix(".json")
                        .is_some_and(|name| name.contains(&format!("-{}-", &digest[..12]))))
                && read_source(&entry.path())?
                    .as_deref()
                    .is_some_and(|found| hash(found) == digest)
            {
                remove_matching_source(source, bytes)?;
                return Ok(());
            }
        }
    }
    let timestamp = crate::printers::now_rfc3339().replace(['-', ':'], "");
    let base = format!("{stem}.{kind}-{timestamp}-{}.json", &digest[..12]);
    let mut suffix = 1;
    loop {
        let file_name = if suffix == 1 {
            base.clone()
        } else {
            format!("{}-{suffix}.json", base.trim_end_matches(".json"))
        };
        let destination = storage.paths.legacy_root().join(file_name);
        if atomic_write_new(&destination, bytes)? {
            break;
        }
        suffix += 1;
    }
    remove_matching_source(source, bytes)?;
    sync_directory(storage.paths.legacy_root())
}

fn remove_matching_source(source: Option<&Path>, archived: &[u8]) -> Result<(), StorageError> {
    let Some(source) = source else { return Ok(()) };
    if read_source(source)?.as_deref() == Some(archived) {
        fs::remove_file(source)?;
    }
    Ok(())
}

fn atomic_write_new(path: &Path, bytes: &[u8]) -> Result<bool, StorageError> {
    let parent = path.parent().ok_or(StorageError::Filesystem)?;
    let temporary = parent.join(format!(".legacy-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        super::database::set_private_handle_permissions(&file)?;
        match fs::hard_link(&temporary, path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                fs::remove_file(&temporary)?;
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        }
        sync_directory(parent)?;
        fs::remove_file(&temporary)?;
        Ok(true)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
    let parent = path.parent().ok_or(StorageError::Filesystem)?;
    let temporary = parent.join(format!(".legacy-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        super::database::set_private_handle_permissions(&file)?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), StorageError> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_: &Path) -> Result<(), StorageError> {
    Ok(())
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_printers(printers: &[StoredPrinter]) -> Result<(), StorageError> {
    let mut ids = std::collections::HashSet::new();
    for printer in printers {
        if printer.id.is_empty()
            || printer.id.len() > 512
            || printer.id.chars().any(char::is_control)
            || !ids.insert(&printer.id)
        {
            return Err(StorageError::OperationFailed);
        }
        if let Some(reference) = printer
            .connection
            .as_ref()
            .and_then(|connection| connection.credential_ref.as_deref())
        {
            crate::printers::commands::validate_credential_reference(reference)
                .map_err(|_| StorageError::OperationFailed)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::atomic_write_new;
    use std::fs;

    #[test]
    fn archive_creation_never_overwrites_an_existing_name() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("archive.json");

        assert!(atomic_write_new(&destination, b"first").unwrap());
        assert!(!atomic_write_new(&destination, b"second").unwrap());
        assert_eq!(fs::read(destination).unwrap(), b"first");
    }
}

fn insert_printer(
    transaction: &rusqlite::Transaction<'_>,
    printer: &StoredPrinter,
    now: &str,
) -> Result<(), StorageError> {
    let overrides =
        serde_json::to_string(&printer.overrides).map_err(|_| StorageError::OperationFailed)?;
    let last_known_good = printer
        .last_known_good
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| StorageError::OperationFailed)?;
    let connection = printer
        .connection
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| StorageError::OperationFailed)?;
    transaction.execute(
        "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model, catalog_variant, catalog_model_id, catalog_printer_variant, notes, overrides_json, last_known_good_json, connection_json, created_at, updated_at) VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
        params![printer.id, printer.name, printer.catalog_ref.vendor, printer.catalog_ref.model, printer.catalog_ref.variant, printer.catalog_ref.model_id, printer.catalog_ref.printer_variant, printer.notes, overrides, last_known_good, connection, now],
    )?;
    Ok(())
}

fn record_warning(
    storage: &Storage,
    code: &str,
    source_name: Option<&str>,
    source_hash: Option<&str>,
    message: &str,
) {
    let now = crate::printers::now_rfc3339();
    let _ = storage.write(|transaction| {
        insert_warning(
            transaction,
            code,
            source_name,
            source_hash,
            message,
            "{}",
            &now,
        )
    });
}

fn insert_warning(
    transaction: &rusqlite::Transaction<'_>,
    code: &str,
    source_name: Option<&str>,
    source_hash: Option<&str>,
    message: &str,
    details: &str,
    now: &str,
) -> Result<(), StorageError> {
    transaction.execute(
        "INSERT OR IGNORE INTO migration_warnings(id, code, source_name, source_sha256, message, details_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![uuid::Uuid::new_v4().to_string(), code, source_name, source_hash, message, details, now],
    )?;
    Ok(())
}
