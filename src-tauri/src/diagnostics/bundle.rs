//! D13's diagnostics bundle: the wire types, the zip layout, and the
//! build, which collects, pseudonymizes, serializes, and egress-scans every
//! entry before a single byte is written.
//!
//! `farm3d-diagnostics-<yyyymmddThhmmssZ>.zip` holds `bundle.json`
//! (`{ format, formatVersion, createdAt, appVersion, sections }`) and one
//! entry per selected section, in canonical order. It is built in memory
//! (at most 32 MiB; larger is `INTERNAL`) and only then written, with
//! `document_io::atomic_write`, by the command.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::PathBuf;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use zeroize::Zeroizing;

use super::collect::{self, CollectContext, RenderedEntry};
use super::egress::{self, Corpus, EntryFormat, ScanEntry};
use super::pseudonym::BundlePseudonyms;
use super::EntryHook;
use crate::backup::DesktopRequiredReason;
use crate::connections::credentials::CredentialStore;
use crate::persistence::{RepositoryError, Storage, StorageError};

/// The bundle's `format`.
pub const BUNDLE_FORMAT: &str = "farm3d-diagnostics";
/// The bundle's `formatVersion`.
pub const BUNDLE_FORMAT_VERSION: u32 = 1;
/// The in-memory limit (D13); a larger bundle is `INTERNAL`.
pub const MAX_BUNDLE_BYTES: usize = 32 * 1024 * 1024;

/// D13's sections, in canonical (entry) order.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/DiagnosticsSection.ts")]
pub enum DiagnosticsSection {
    About,
    Health,
    Storage,
    Configuration,
    Logs,
    RecentProblems,
}

impl DiagnosticsSection {
    pub const ALL: [DiagnosticsSection; 6] = [
        Self::About,
        Self::Health,
        Self::Storage,
        Self::Configuration,
        Self::Logs,
        Self::RecentProblems,
    ];

    /// The wire name (`recentProblems`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::About => "about",
            Self::Health => "health",
            Self::Storage => "storage",
            Self::Configuration => "configuration",
            Self::Logs => "logs",
            Self::RecentProblems => "recentProblems",
        }
    }

    /// How a message names it (`recent problems`).
    pub fn label(self) -> &'static str {
        match self {
            Self::RecentProblems => "recent problems",
            other => other.as_str(),
        }
    }
}

/// One row of `diagnostics_preview`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/DiagnosticsSectionEstimate.ts"
)]
pub struct DiagnosticsSectionEstimate {
    pub section: DiagnosticsSection,
    #[ts(type = "number")]
    pub estimated_bytes: u64,
}

/// `diagnostics_preview`'s result: every section with its estimated
/// (uncompressed) size.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/DiagnosticsPreview.ts")]
pub struct DiagnosticsPreview {
    pub sections: Vec<DiagnosticsSectionEstimate>,
}

/// `export_diagnostics`'s result. `fileName` is the destination's basename.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/ExportDiagnosticsOutcome.ts"
)]
pub enum ExportDiagnosticsOutcome {
    Cancelled,
    Exported {
        exported_at: String,
        file_name: String,
        #[ts(type = "number")]
        bytes: u64,
        sections: Vec<DiagnosticsSection>,
    },
    Unsupported {
        reason: DesktopRequiredReason,
    },
}

/// Why a bundle wasn't built. Never carries a term, a path, or a name.
#[derive(Debug)]
pub enum BundleError {
    /// The egress scan found a corpus term in this section.
    RedactionFailed(DiagnosticsSection),
    Repository(RepositoryError),
    /// Over [`MAX_BUNDLE_BYTES`], or a serialization failure.
    Internal,
}

impl From<StorageError> for BundleError {
    fn from(error: StorageError) -> Self {
        Self::Repository(RepositoryError::Storage(error))
    }
}

impl From<RepositoryError> for BundleError {
    fn from(error: RepositoryError) -> Self {
        Self::Repository(error)
    }
}

/// Everything a build reads beside the database.
pub struct BundleInputs<'a> {
    pub storage: &'a Storage,
    pub credentials: &'a CredentialStore,
    pub context: CollectContext<'a>,
    /// The home directory (and any override), matched as paths.
    pub home_dirs: Vec<PathBuf>,
    /// Other absolute paths farm3d knows at runtime: the Slicer profile
    /// cache and the resolved engine and preset paths.
    pub runtime_paths: Vec<PathBuf>,
    pub app_version: &'a str,
    pub created_at: DateTime<Utc>,
    /// Test-only: runs on each serialized entry before the scan.
    pub entry_hook: Option<&'a EntryHook>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BundleManifest<'a> {
    format: &'static str,
    format_version: u32,
    created_at: String,
    app_version: &'a str,
    sections: &'a [DiagnosticsSection],
}

/// `bundle.json`'s closed paths.
const MANIFEST_CLOSED: &[&str] = &["format", "createdAt", "sections[]"];

/// The collected sections and the corpus, read in one read transaction.
fn read(
    inputs: &BundleInputs<'_>,
    sections: &BTreeSet<DiagnosticsSection>,
) -> Result<(collect::Collected, Corpus, Vec<String>), BundleError> {
    let (collected, (corpus, refs)) = inputs.storage.read_transaction(|tx| {
        let corpus = egress::read_corpus(tx)?;
        Ok((collect::collect(tx, sections, &inputs.context), corpus))
    })?;
    Ok((collected?, corpus, refs))
}

/// Collects, pseudonymizes, and serializes `sections`, without the scan:
/// what [`build`] and the preview share.
fn render(
    inputs: &BundleInputs<'_>,
    sections: &BTreeSet<DiagnosticsSection>,
) -> Result<(collect::Collected, Vec<RenderedEntry>, Corpus, Vec<String>), BundleError> {
    let (collected, corpus, refs) = read(inputs, sections)?;
    let mut pseudonyms = BundlePseudonyms::new();
    collected.note_ids(&mut pseudonyms);
    let entries = collected
        .render(&pseudonyms.assign())
        .map_err(|_| BundleError::Internal)?;
    Ok((collected, entries, corpus, refs))
}

/// `diagnostics_preview`: each section's uncompressed size (the log files'
/// size on disk for `logs`).
pub fn preview(inputs: &BundleInputs<'_>) -> Result<Vec<DiagnosticsSectionEstimate>, BundleError> {
    let all: BTreeSet<DiagnosticsSection> = DiagnosticsSection::ALL.into_iter().collect();
    let (collected, entries, _, _) = render(inputs, &all)?;
    Ok(DiagnosticsSection::ALL
        .into_iter()
        .map(|section| {
            let estimated_bytes = if section == DiagnosticsSection::Logs {
                collected.log_file_bytes().unwrap_or(0)
            } else {
                entries
                    .iter()
                    .filter(|entry| entry.section == section)
                    .map(|entry| entry.bytes.len() as u64)
                    .sum()
            };
            DiagnosticsSectionEstimate {
                section,
                estimated_bytes,
            }
        })
        .collect())
}

/// Builds the bundle in memory. Every entry is serialized and scanned
/// against the live corpus before this returns; a hit is
/// [`BundleError::RedactionFailed`] and nothing is produced.
pub fn build(
    inputs: &BundleInputs<'_>,
    sections: &BTreeSet<DiagnosticsSection>,
) -> Result<Vec<u8>, BundleError> {
    // Lines queued before the export reach the files first.
    super::log::flush();
    let (_, mut entries, mut corpus, refs) = render(inputs, sections)?;

    let listed: Vec<DiagnosticsSection> = sections.iter().copied().collect();
    let manifest = BundleManifest {
        format: BUNDLE_FORMAT,
        format_version: BUNDLE_FORMAT_VERSION,
        created_at: inputs
            .created_at
            .to_rfc3339_opts(SecondsFormat::Millis, true),
        app_version: inputs.app_version,
        sections: &listed,
    };
    entries.insert(
        0,
        RenderedEntry {
            // `bundle.json` carries only the app version beside closed
            // values; a hit there is reported as `about`.
            section: DiagnosticsSection::About,
            name: "bundle.json".to_string(),
            bytes: serde_json::to_vec_pretty(&manifest).map_err(|_| BundleError::Internal)?,
            format: EntryFormat::Json,
            closed_paths: MANIFEST_CLOSED,
        },
    );
    if let Some(hook) = inputs.entry_hook {
        for entry in &mut entries {
            hook(entry.section, &entry.name, &mut entry.bytes);
        }
    }

    // The corpus: the database's terms, every credential value behind every
    // ref (read into `Zeroizing`, compared, dropped), and the paths.
    for reference in &refs {
        if let Ok(Some(value)) = inputs.credentials.get(reference) {
            corpus.add_credential(Zeroizing::new(value));
        }
    }
    let paths = inputs.storage.paths();
    for root in [
        paths.metadata_root(),
        paths.database(),
        paths.snapshot_root(),
        paths.legacy_root(),
        paths.content_root(),
        paths.media_root(),
        paths.log_root(),
        paths.backup_root(),
        paths.app_data_root(),
    ] {
        corpus.add_path(root);
    }
    for path in inputs.home_dirs.iter().chain(&inputs.runtime_paths) {
        corpus.add_path(path);
    }
    let compiled = corpus.compile();
    let scanned: Vec<ScanEntry<'_>> = entries
        .iter()
        .map(|entry| ScanEntry {
            section: entry.section,
            bytes: &entry.bytes,
            format: entry.format,
            closed_paths: entry.closed_paths,
        })
        .collect();
    egress::scan(&compiled, &scanned).map_err(BundleError::RedactionFailed)?;
    drop(compiled);

    zip(&entries)
}

fn zip(entries: &[RenderedEntry]) -> Result<Vec<u8>, BundleError> {
    let total: usize = entries.iter().map(|entry| entry.bytes.len()).sum();
    if total > MAX_BUNDLE_BYTES {
        return Err(BundleError::Internal);
    }
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for entry in entries {
        writer
            .start_file(entry.name.as_str(), options)
            .map_err(|_| BundleError::Internal)?;
        writer
            .write_all(&entry.bytes)
            .map_err(|_| BundleError::Internal)?;
    }
    let bytes = writer
        .finish()
        .map_err(|_| BundleError::Internal)?
        .into_inner();
    check_size(bytes.len())?;
    Ok(bytes)
}

/// D13: a bundle over 32 MiB is `INTERNAL`.
pub fn check_size(bytes: usize) -> Result<(), BundleError> {
    if bytes > MAX_BUNDLE_BYTES {
        Err(BundleError::Internal)
    } else {
        Ok(())
    }
}

/// The suggested file name, `farm3d-diagnostics-<yyyymmddThhmmssZ>.zip`.
pub fn suggested_file_name(now: DateTime<Utc>) -> String {
    format!("farm3d-diagnostics-{}.zip", now.format("%Y%m%dT%H%M%SZ"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_limit_is_32_mib_and_the_name_is_timestamped() {
        assert!(check_size(MAX_BUNDLE_BYTES).is_ok());
        assert!(matches!(
            check_size(MAX_BUNDLE_BYTES + 1),
            Err(BundleError::Internal)
        ));
        let now = "2026-09-29T12:34:56Z".parse().unwrap();
        assert_eq!(
            suggested_file_name(now),
            "farm3d-diagnostics-20260929T123456Z.zip"
        );
    }

    #[test]
    fn sections_serialize_by_their_wire_names_in_canonical_order() {
        let names: Vec<String> = DiagnosticsSection::ALL
            .iter()
            .map(|section| {
                serde_json::to_value(section)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        let wire: Vec<&str> = DiagnosticsSection::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(names, wire);
        let mut sorted = DiagnosticsSection::ALL.to_vec();
        sorted.sort();
        assert_eq!(sorted, DiagnosticsSection::ALL);
    }
}
