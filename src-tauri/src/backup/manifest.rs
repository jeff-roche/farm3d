//! D2's manifest: a closed JSON object, `formatVersion` 1, and D4's
//! compatibility window.
//!
//! [`Manifest::to_json`] writes the fields in the spec's order.
//! [`Manifest::parse`] reads a manifest back under D3 rules 5 and 6: the
//! JSON must parse into the closed schema (an unknown field, a missing
//! field, or a value of the wrong shape is `manifestInvalid` with the JSON
//! path of the field), `format` must be `farm3d-backup`, and the versions
//! must be inside the window. A newer `formatVersion` is reported as newer
//! before the closed schema is applied, so a manifest from a later format
//! with fields this one doesn't know is "newer", not "malformed".

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};

use super::{BackupExcludedClass, BackupMediaChoice, BackupOrigin};
use crate::persistence::CURRENT_SCHEMA_VERSION;

/// The only `format` value.
pub const FORMAT: &str = "farm3d-backup";
/// The one `formatVersion` this binary writes and reads.
pub const FORMAT_VERSION: i64 = 1;
/// D4: the oldest schema a backup can carry (P9's own).
pub const MIN_SCHEMA_VERSION: i64 = 10;

#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ManifestMigration {
    pub version: i64,
    pub name: String,
    pub checksum: String,
}

#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ManifestPlatform {
    pub os: String,
    pub arch: String,
}

impl ManifestPlatform {
    /// `std::env::consts::OS` and `ARCH`; nothing else about the machine.
    pub fn current() -> Self {
        Self {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        }
    }
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ManifestContents {
    pub media: BackupMediaChoice,
}

/// The camera snapshot rows the writer handled (D5).
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ManifestMediaCounts {
    pub included: i64,
    pub not_in_backup: i64,
    pub missing_file: i64,
}

/// One archive entry other than `manifest.json`.
#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub path: String,
    /// The uncompressed length.
    pub bytes: u64,
    /// Lowercase hex SHA-256 of the uncompressed bytes.
    pub sha256: String,
}

/// D2's manifest, fields in the spec's order.
#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: String,
    pub format_version: i64,
    pub created_at: String,
    pub app_version: String,
    pub schema_version: i64,
    pub migrations: Vec<ManifestMigration>,
    pub platform: ManifestPlatform,
    pub origin: BackupOrigin,
    pub contents: ManifestContents,
    pub counts: BTreeMap<String, i64>,
    pub excluded: Vec<BackupExcludedClass>,
    pub credential_ref_count: i64,
    pub media: ManifestMediaCounts,
    pub entries: Vec<ManifestEntry>,
}

/// Why [`Manifest::parse`] refused a manifest.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ManifestError {
    /// `manifestInvalid` at `field_path` (for example
    /// `manifest.entries[3].sha256`).
    Invalid { field_path: String },
    /// `formatVersion` is newer than [`FORMAT_VERSION`]
    /// (`UNSUPPORTED_BACKUP_FORMAT`).
    UnsupportedFormat { received: i64 },
    /// `schemaVersion` is newer than this binary's
    /// (`UNSUPPORTED_SCHEMA_VERSION`).
    UnsupportedSchema { received: i64 },
}

impl Manifest {
    /// The manifest as UTF-8 JSON, fields in the spec's order.
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("a manifest always serializes")
    }

    /// D3 rules 5 (the closed schema) and 6 (the compatibility window).
    pub fn parse(bytes: &[u8]) -> Result<Manifest, ManifestError> {
        Self::parse_from_schema(bytes, MIN_SCHEMA_VERSION)
    }

    /// [`parse`](Self::parse) with the window starting at
    /// `min_schema_version` instead of [`MIN_SCHEMA_VERSION`]. Only tests
    /// open it wider: this binary's window starts at its own schema, so an
    /// older-schema backup exists only once a later schema ships.
    pub fn parse_from_schema(
        bytes: &[u8],
        min_schema_version: i64,
    ) -> Result<Manifest, ManifestError> {
        let value: Value = serde_json::from_slice(bytes).map_err(|_| invalid("manifest"))?;
        let root = object(&value, "manifest")?;

        // Rule 5's `format`, then rule 6's `formatVersion`, before the
        // closed schema: a newer format is "newer", whatever it contains.
        let format = string(field(root, "manifest", "format")?, "manifest.format")?;
        if format != FORMAT {
            return Err(invalid("manifest.format"));
        }
        let format_version = integer(
            field(root, "manifest", "formatVersion")?,
            "manifest.formatVersion",
        )?;
        if format_version > FORMAT_VERSION {
            return Err(ManifestError::UnsupportedFormat {
                received: format_version,
            });
        }
        if format_version < FORMAT_VERSION {
            return Err(invalid("manifest.formatVersion"));
        }

        closed(
            root,
            "manifest",
            &[
                "format",
                "formatVersion",
                "createdAt",
                "appVersion",
                "schemaVersion",
                "migrations",
                "platform",
                "origin",
                "contents",
                "counts",
                "excluded",
                "credentialRefCount",
                "media",
                "entries",
            ],
        )?;
        let created_at = string(field(root, "manifest", "createdAt")?, "manifest.createdAt")?;
        if chrono::DateTime::parse_from_rfc3339(&created_at).is_err() {
            return Err(invalid("manifest.createdAt"));
        }
        let app_version = string(
            field(root, "manifest", "appVersion")?,
            "manifest.appVersion",
        )?;
        let schema_version = integer(
            field(root, "manifest", "schemaVersion")?,
            "manifest.schemaVersion",
        )?;
        let migrations = parse_migrations(field(root, "manifest", "migrations")?)?;
        let platform = {
            let path = "manifest.platform";
            let platform = object(field(root, "manifest", "platform")?, path)?;
            closed(platform, path, &["os", "arch"])?;
            ManifestPlatform {
                os: string(field(platform, path, "os")?, "manifest.platform.os")?,
                arch: string(field(platform, path, "arch")?, "manifest.platform.arch")?,
            }
        };
        let origin = enumeration(field(root, "manifest", "origin")?, "manifest.origin")?;
        let contents = {
            let path = "manifest.contents";
            let contents = object(field(root, "manifest", "contents")?, path)?;
            closed(contents, path, &["media"])?;
            ManifestContents {
                media: enumeration(field(contents, path, "media")?, "manifest.contents.media")?,
            }
        };
        let counts = {
            let counts = object(field(root, "manifest", "counts")?, "manifest.counts")?;
            let mut parsed = BTreeMap::new();
            for (table, rows) in counts {
                let path = format!("manifest.counts.{table}");
                parsed.insert(table.clone(), non_negative(rows, &path)?);
            }
            parsed
        };
        let excluded: Vec<BackupExcludedClass> =
            serde_json::from_value(field(root, "manifest", "excluded")?.clone())
                .map_err(|_| invalid("manifest.excluded"))?;
        if excluded != BackupExcludedClass::ALL {
            return Err(invalid("manifest.excluded"));
        }
        let credential_ref_count = non_negative(
            field(root, "manifest", "credentialRefCount")?,
            "manifest.credentialRefCount",
        )?;
        let media = {
            let path = "manifest.media";
            let media = object(field(root, "manifest", "media")?, path)?;
            closed(media, path, &["included", "notInBackup", "missingFile"])?;
            ManifestMediaCounts {
                included: non_negative(field(media, path, "included")?, "manifest.media.included")?,
                not_in_backup: non_negative(
                    field(media, path, "notInBackup")?,
                    "manifest.media.notInBackup",
                )?,
                missing_file: non_negative(
                    field(media, path, "missingFile")?,
                    "manifest.media.missingFile",
                )?,
            }
        };
        let entries = parse_entries(field(root, "manifest", "entries")?)?;

        // Rule 6: the schema window.
        if schema_version > CURRENT_SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedSchema {
                received: schema_version,
            });
        }
        if schema_version < min_schema_version {
            return Err(invalid("manifest.schemaVersion"));
        }

        Ok(Manifest {
            format,
            format_version,
            created_at,
            app_version,
            schema_version,
            migrations,
            platform,
            origin,
            contents,
            counts,
            excluded,
            credential_ref_count,
            media,
            entries,
        })
    }
}

fn parse_migrations(value: &Value) -> Result<Vec<ManifestMigration>, ManifestError> {
    let rows = value
        .as_array()
        .ok_or_else(|| invalid("manifest.migrations"))?;
    let mut migrations: Vec<ManifestMigration> = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let path = format!("manifest.migrations[{index}]");
        let row = object(row, &path)?;
        closed(row, &path, &["version", "name", "checksum"])?;
        let version_path = format!("{path}.version");
        let version = integer(field(row, &path, "version")?, &version_path)?;
        if version < 1
            || migrations
                .last()
                .is_some_and(|last| last.version >= version)
        {
            return Err(invalid(version_path));
        }
        let name = string(field(row, &path, "name")?, &format!("{path}.name"))?;
        let checksum_path = format!("{path}.checksum");
        let checksum = string(field(row, &path, "checksum")?, &checksum_path)?;
        if !is_sha256_hex(&checksum) {
            return Err(invalid(checksum_path));
        }
        migrations.push(ManifestMigration {
            version,
            name,
            checksum,
        });
    }
    Ok(migrations)
}

fn parse_entries(value: &Value) -> Result<Vec<ManifestEntry>, ManifestError> {
    let rows = value
        .as_array()
        .ok_or_else(|| invalid("manifest.entries"))?;
    let mut entries = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let path = format!("manifest.entries[{index}]");
        let row = object(row, &path)?;
        closed(row, &path, &["path", "bytes", "sha256"])?;
        let entry_path = string(field(row, &path, "path")?, &format!("{path}.path"))?;
        let bytes_path = format!("{path}.bytes");
        let bytes = field(row, &path, "bytes")?
            .as_u64()
            .ok_or_else(|| invalid(bytes_path))?;
        let sha_path = format!("{path}.sha256");
        let sha256 = string(field(row, &path, "sha256")?, &sha_path)?;
        if !is_sha256_hex(&sha256) {
            return Err(invalid(sha_path));
        }
        entries.push(ManifestEntry {
            path: entry_path,
            bytes,
            sha256,
        });
    }
    Ok(entries)
}

fn invalid(field_path: impl Into<String>) -> ManifestError {
    ManifestError::Invalid {
        field_path: field_path.into(),
    }
}

fn object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>, ManifestError> {
    value.as_object().ok_or_else(|| invalid(path))
}

/// An unknown key is `manifestInvalid` at its own path.
fn closed(map: &Map<String, Value>, path: &str, allowed: &[&str]) -> Result<(), ManifestError> {
    match map.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(unknown) => Err(invalid(format!("{path}.{unknown}"))),
        None => Ok(()),
    }
}

fn field<'a>(
    map: &'a Map<String, Value>,
    path: &str,
    key: &str,
) -> Result<&'a Value, ManifestError> {
    map.get(key).ok_or_else(|| invalid(format!("{path}.{key}")))
}

fn string(value: &Value, path: &str) -> Result<String, ManifestError> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| invalid(path))
}

fn integer(value: &Value, path: &str) -> Result<i64, ManifestError> {
    value.as_i64().ok_or_else(|| invalid(path))
}

fn non_negative(value: &Value, path: &str) -> Result<i64, ManifestError> {
    integer(value, path).and_then(|number| {
        if number >= 0 {
            Ok(number)
        } else {
            Err(invalid(path))
        }
    })
}

fn enumeration<T: serde::de::DeserializeOwned>(
    value: &Value,
    path: &str,
) -> Result<T, ManifestError> {
    serde_json::from_value(value.clone()).map_err(|_| invalid(path))
}

/// Whether `text` is 64 lowercase hex digits.
pub fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) fn sample() -> Manifest {
        Manifest {
            format: FORMAT.to_string(),
            format_version: 1,
            created_at: "2026-09-29T12:00:00.000Z".to_string(),
            app_version: "0.1.0".to_string(),
            schema_version: CURRENT_SCHEMA_VERSION,
            migrations: vec![ManifestMigration {
                version: 1,
                name: "0001_foundation".to_string(),
                checksum: "a".repeat(64),
            }],
            platform: ManifestPlatform {
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
            origin: BackupOrigin::Operator,
            contents: ManifestContents {
                media: BackupMediaChoice::All,
            },
            counts: BTreeMap::from([
                ("attention_events".to_string(), 4),
                ("camera_snapshots".to_string(), 3),
            ]),
            excluded: BackupExcludedClass::ALL.to_vec(),
            credential_ref_count: 2,
            media: ManifestMediaCounts {
                included: 2,
                not_in_backup: 1,
                missing_file: 0,
            },
            entries: vec![
                ManifestEntry {
                    path: "database/farm3d.sqlite3".to_string(),
                    bytes: 245_760,
                    sha256: "b".repeat(64),
                },
                ManifestEntry {
                    path: format!("content/sha256/cc/{}", "c".repeat(64)),
                    bytes: 200,
                    sha256: "c".repeat(64),
                },
            ],
        }
    }

    fn value() -> Value {
        serde_json::from_slice(&sample().to_json()).unwrap()
    }

    fn parse_value(value: &Value) -> Result<Manifest, ManifestError> {
        Manifest::parse(&serde_json::to_vec(value).unwrap())
    }

    fn invalid_at(path: &str) -> Result<Manifest, ManifestError> {
        Err(ManifestError::Invalid {
            field_path: path.to_string(),
        })
    }

    #[test]
    fn the_codec_round_trips() {
        let manifest = sample();
        assert_eq!(Manifest::parse(&manifest.to_json()), Ok(manifest));
    }

    #[test]
    fn fields_are_written_in_the_spec_order() {
        let json = String::from_utf8(sample().to_json()).unwrap();
        // Top-level keys only (two-space indent): `contents` holds its own
        // `media` key.
        let order = [
            "format",
            "formatVersion",
            "createdAt",
            "appVersion",
            "schemaVersion",
            "migrations",
            "platform",
            "origin",
            "contents",
            "counts",
            "excluded",
            "credentialRefCount",
            "media",
            "entries",
        ];
        let positions: Vec<usize> = order
            .iter()
            .map(|key| json.find(&format!("\n  \"{key}\":")).unwrap())
            .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{json}");
        assert!(json.contains(
            "\"excluded\": [\n    \"credentials\",\n    \"slicerRuntimePaths\",\n    \"printerStatusCache\",\n    \"pendingCredentialCleanup\",\n    \"pendingBlobCleanup\",\n    \"logs\"\n  ]"
        ));
    }

    #[test]
    fn an_unknown_field_is_invalid_at_its_path() {
        let mut manifest = value();
        manifest["extra"] = json!(1);
        assert_eq!(parse_value(&manifest), invalid_at("manifest.extra"));
        let mut manifest = value();
        manifest["entries"][1]["mode"] = json!("0644");
        assert_eq!(
            parse_value(&manifest),
            invalid_at("manifest.entries[1].mode")
        );
    }

    #[test]
    fn a_missing_or_malformed_field_is_invalid_at_its_path() {
        let mut manifest = value();
        manifest.as_object_mut().unwrap().remove("createdAt");
        assert_eq!(parse_value(&manifest), invalid_at("manifest.createdAt"));
        let mut manifest = value();
        manifest["entries"][1]["sha256"] = json!("C".repeat(64));
        assert_eq!(
            parse_value(&manifest),
            invalid_at("manifest.entries[1].sha256")
        );
        let mut manifest = value();
        manifest["counts"]["camera_snapshots"] = json!(-1);
        assert_eq!(
            parse_value(&manifest),
            invalid_at("manifest.counts.camera_snapshots")
        );
        let mut manifest = value();
        manifest["origin"] = json!("someoneElse");
        assert_eq!(parse_value(&manifest), invalid_at("manifest.origin"));
        let mut manifest = value();
        manifest["excluded"] = json!(["credentials"]);
        assert_eq!(parse_value(&manifest), invalid_at("manifest.excluded"));
        let mut manifest = value();
        manifest["createdAt"] = json!("yesterday");
        assert_eq!(parse_value(&manifest), invalid_at("manifest.createdAt"));
        assert_eq!(Manifest::parse(b"{not json"), invalid_at("manifest"));
        assert_eq!(Manifest::parse(b"[]"), invalid_at("manifest"));
    }

    #[test]
    fn the_format_must_be_farm3d_backup() {
        let mut manifest = value();
        manifest["format"] = json!("other");
        assert_eq!(parse_value(&manifest), invalid_at("manifest.format"));
    }

    #[test]
    fn the_compatibility_window_is_format_1_and_schema_10_through_current() {
        let mut manifest = value();
        manifest["formatVersion"] = json!(2);
        // A newer format may add fields this one doesn't know: it is still
        // "newer", not "malformed".
        manifest["newField"] = json!(true);
        assert_eq!(
            parse_value(&manifest),
            Err(ManifestError::UnsupportedFormat { received: 2 })
        );
        for older in [json!(0), json!(-1)] {
            let mut manifest = value();
            manifest["formatVersion"] = older;
            assert_eq!(parse_value(&manifest), invalid_at("manifest.formatVersion"));
        }
        let mut manifest = value();
        manifest.as_object_mut().unwrap().remove("formatVersion");
        assert_eq!(parse_value(&manifest), invalid_at("manifest.formatVersion"));

        let mut manifest = value();
        manifest["schemaVersion"] = json!(CURRENT_SCHEMA_VERSION + 1);
        assert_eq!(
            parse_value(&manifest),
            Err(ManifestError::UnsupportedSchema {
                received: CURRENT_SCHEMA_VERSION + 1
            })
        );
        let mut manifest = value();
        manifest["schemaVersion"] = json!(9);
        assert_eq!(parse_value(&manifest), invalid_at("manifest.schemaVersion"));
    }
}
