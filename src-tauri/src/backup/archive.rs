//! D2's archive layout and D3's reader rules: the entry path rules, the
//! limits, the zip writer the backup writer streams into, and the reader
//! that verifies an archive entry by entry with a streaming SHA-256.
//!
//! The reader walks the central directory itself (rules 1–4) before the
//! zip crate reads it, because the crate keys entries by name and would
//! silently fold a duplicate into one entry. Rules 5–8 then read and check
//! the manifest against the archive, and [`ArchiveReader::copy_entry`]
//! applies rule 10 to one entry. [`verify`] runs rules 1–8 and 10 over a
//! whole file (the safety-backup verifier, D9); restore staging
//! (`backup::staging`) adds rules 9 and 11.
//!
//! No error carries an entry's bytes or an unsafe name: a rule-3 failure
//! is reported as `entries[<central-directory index>]`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use chrono::{DateTime, Datelike, Timelike, Utc};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::manifest::{is_sha256_hex, Manifest, ManifestError};
use super::BackupInvalidReason;

pub const MANIFEST_PATH: &str = "manifest.json";
pub const DATABASE_PATH: &str = "database/farm3d.sqlite3";
/// Rule 2: the manifest plus 1,000,000 entries.
pub const MAX_ENTRIES: u64 = 1_000_001;
/// Rule 5: the largest manifest, uncompressed.
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
/// Rule 3: the longest entry name, in bytes.
pub const MAX_NAME_BYTES: usize = 512;
const TOP_LEVEL: [&str; 4] = ["manifest.json", "database", "content", "media"];

/// Why an archive was refused.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ArchiveError {
    /// `BACKUP_INVALID` with `details.reason` and `details.fieldPath`.
    Invalid {
        reason: BackupInvalidReason,
        field_path: String,
    },
    /// `UNSUPPORTED_BACKUP_FORMAT`.
    UnsupportedFormat { received: i64 },
    /// `UNSUPPORTED_SCHEMA_VERSION`.
    UnsupportedSchema { received: i64 },
    /// The sink [`ArchiveReader::copy_entry`] writes to failed (a staging
    /// disk problem, not the archive's).
    Sink(io::ErrorKind),
}

impl ArchiveError {
    fn invalid(reason: BackupInvalidReason, field_path: impl Into<String>) -> Self {
        ArchiveError::Invalid {
            reason,
            field_path: field_path.into(),
        }
    }
}

impl From<ManifestError> for ArchiveError {
    fn from(error: ManifestError) -> Self {
        match error {
            ManifestError::Invalid { field_path } => {
                ArchiveError::invalid(BackupInvalidReason::ManifestInvalid, field_path)
            }
            ManifestError::UnsupportedFormat { received } => {
                ArchiveError::UnsupportedFormat { received }
            }
            ManifestError::UnsupportedSchema { received } => {
                ArchiveError::UnsupportedSchema { received }
            }
        }
    }
}

// --- path rules --------------------------------------------------------------------

/// D3 rule 3 for one raw entry name.
pub fn is_safe_entry_name(raw: &[u8]) -> bool {
    let Ok(name) = std::str::from_utf8(raw) else {
        return false;
    };
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return false;
    }
    let bytes = name.as_bytes();
    let drive_letter = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if name.starts_with('/') || drive_letter || name.contains('\\') {
        return false;
    }
    if name
        .chars()
        .any(|character| character <= '\u{1f}' || character == '\u{7f}')
    {
        return false;
    }
    let mut segments = name.split('/');
    let first = segments.next().unwrap_or_default();
    if !TOP_LEVEL.contains(&first) {
        return false;
    }
    name.split('/')
        .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

/// A `camera_snapshots.rel_path`: `snapshots/<yyyy>/<mm>/<snp-id>.<jpg|png>`.
pub fn is_media_rel_path(rel_path: &str) -> bool {
    let segments: Vec<&str> = rel_path.split('/').collect();
    let [root, year, month, file] = segments.as_slice() else {
        return false;
    };
    let digits = |text: &str, length: usize| {
        text.len() == length && text.bytes().all(|byte| byte.is_ascii_digit())
    };
    let stem = file
        .strip_suffix(".jpg")
        .or_else(|| file.strip_suffix(".png"));
    *root == "snapshots"
        && digits(year, 4)
        && digits(month, 2)
        && stem.is_some_and(|stem| stem.len() > 4 && stem.starts_with("snp-"))
}

/// Whether `path` is `content/sha256/<hh>/<hex>`; returns the hex.
fn content_hex(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("content/sha256/")?;
    let (prefix, hex) = rest.split_once('/')?;
    (is_sha256_hex(hex) && prefix == &hex[..2]).then_some(hex)
}

/// `content/sha256/<hh>/<hex>`.
pub fn content_path(sha256: &str) -> String {
    format!("content/sha256/{}/{sha256}", &sha256[..2])
}

/// `media/<rel_path>`.
pub fn media_path(rel_path: &str) -> String {
    format!("media/{rel_path}")
}

// --- streaming SHA-256 --------------------------------------------------------------

/// A reader that hashes and counts what passes through it.
pub struct Sha256Reader<R> {
    inner: R,
    hasher: Sha256,
    bytes: u64,
}

impl<R: Read> Sha256Reader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            bytes: 0,
        }
    }

    /// The byte count and the lowercase hex SHA-256 of everything read.
    pub fn finish(self) -> (u64, String) {
        (self.bytes, format!("{:x}", self.hasher.finalize()))
    }
}

impl<R: Read> Read for Sha256Reader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.hasher.update(&buffer[..read]);
        self.bytes += read as u64;
        Ok(read)
    }
}

// --- the writer ----------------------------------------------------------------------

/// How an entry is compressed (D2's table).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntryCompression {
    Stored,
    Deflated,
}

/// D2's zip writer: regular-file entries only, every modification time the
/// manifest's `createdAt` at DOS precision, no directory entries.
pub struct ArchiveWriter<W: Write + Seek> {
    zip: ZipWriter<W>,
    modified: zip::DateTime,
}

impl<W: Write + Seek> ArchiveWriter<W> {
    pub fn new(inner: W, created_at: DateTime<Utc>) -> io::Result<Self> {
        let modified = zip::DateTime::from_date_and_time(
            u16::try_from(created_at.year()).map_err(|_| io::Error::other("year out of range"))?,
            created_at.month() as u8,
            created_at.day() as u8,
            created_at.hour() as u8,
            created_at.minute() as u8,
            created_at.second() as u8,
        )
        .map_err(|_| io::Error::other("createdAt is outside the zip date range"))?;
        Ok(Self {
            zip: ZipWriter::new(inner),
            modified,
        })
    }

    fn options(&self, compression: EntryCompression, bytes: u64) -> SimpleFileOptions {
        SimpleFileOptions::default()
            .compression_method(match compression {
                EntryCompression::Stored => CompressionMethod::Stored,
                EntryCompression::Deflated => CompressionMethod::Deflated,
            })
            .last_modified_time(self.modified)
            .unix_permissions(0o644)
            .large_file(bytes >= u64::from(u32::MAX))
    }

    /// Writes `manifest.json` (deflate). Must be the first entry.
    pub fn write_manifest(&mut self, manifest: &Manifest) -> io::Result<()> {
        let json = manifest.to_json();
        self.zip
            .start_file(
                MANIFEST_PATH,
                self.options(EntryCompression::Deflated, json.len() as u64),
            )
            .map_err(io::Error::other)?;
        self.zip.write_all(&json)
    }

    /// Streams `reader` into entry `path`, hashing as it copies. `bytes` is
    /// the expected length (it chooses zip64 for a large entry). Returns
    /// the length and SHA-256 actually written.
    pub fn write_entry(
        &mut self,
        path: &str,
        reader: impl Read,
        compression: EntryCompression,
        bytes: u64,
    ) -> io::Result<(u64, String)> {
        self.zip
            .start_file(path, self.options(compression, bytes))
            .map_err(io::Error::other)?;
        let mut hashing = Sha256Reader::new(reader);
        io::copy(&mut hashing, &mut self.zip)?;
        Ok(hashing.finish())
    }

    pub fn finish(self) -> io::Result<W> {
        self.zip.finish().map_err(io::Error::other)
    }
}

// --- the reader ----------------------------------------------------------------------

/// An archive that passed D3 rules 1–8.
pub struct ArchiveReader {
    zip: ZipArchive<File>,
    manifest: Manifest,
    /// Manifest entry path -> its index in the zip.
    indices: BTreeMap<String, usize>,
}

/// One central-directory record, as rules 2–4 need it.
struct DirectoryEntry {
    name: Vec<u8>,
    flags: u16,
    method: u16,
    version_made_by: u16,
    external_attributes: u32,
}

fn not_a_zip() -> ArchiveError {
    ArchiveError::invalid(BackupInvalidReason::NotAZip, "archive")
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight bytes"))
}

const EOCD_SIGNATURE: u32 = 0x0605_4b50;
const ZIP64_LOCATOR_SIGNATURE: u32 = 0x0706_4b50;
const ZIP64_EOCD_SIGNATURE: u32 = 0x0606_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;

/// Rules 1 and 2: finds the end of central directory (zip64 aware) and
/// returns `(entry count, central directory offset, its size)`.
fn end_of_central_directory(file: &mut File) -> Result<(u64, u64, u64), ArchiveError> {
    let length = file.metadata().map_err(|_| not_a_zip())?.len();
    if length < 22 {
        return Err(not_a_zip());
    }
    let tail_length = length.min(22 + 65_535);
    let tail_start = length - tail_length;
    let mut tail = vec![0_u8; tail_length as usize];
    file.seek(SeekFrom::Start(tail_start))
        .and_then(|_| file.read_exact(&mut tail))
        .map_err(|_| not_a_zip())?;
    // The last record whose comment runs exactly to the end of the file.
    let eocd = (0..=tail.len() - 22)
        .rev()
        .find(|&at| {
            u32_at(&tail, at) == EOCD_SIGNATURE
                && at + 22 + usize::from(u16_at(&tail, at + 20)) == tail.len()
        })
        .ok_or_else(not_a_zip)?;
    let mut count = u64::from(u16_at(&tail, eocd + 10));
    let mut size = u64::from(u32_at(&tail, eocd + 12));
    let mut offset = u64::from(u32_at(&tail, eocd + 16));
    let eocd_position = tail_start + eocd as u64;
    if count == 0xffff || size == 0xffff_ffff || offset == 0xffff_ffff {
        if eocd < 20 || u32_at(&tail, eocd - 20) != ZIP64_LOCATOR_SIGNATURE {
            return Err(not_a_zip());
        }
        let record_offset = u64_at(&tail, eocd - 20 + 8);
        let mut record = [0_u8; 56];
        file.seek(SeekFrom::Start(record_offset))
            .and_then(|_| file.read_exact(&mut record))
            .map_err(|_| not_a_zip())?;
        if u32_at(&record, 0) != ZIP64_EOCD_SIGNATURE {
            return Err(not_a_zip());
        }
        count = u64_at(&record, 32);
        size = u64_at(&record, 40);
        offset = u64_at(&record, 48);
    }
    if offset
        .checked_add(size)
        .is_none_or(|end| end > eocd_position)
    {
        return Err(not_a_zip());
    }
    if count > MAX_ENTRIES {
        return Err(ArchiveError::invalid(
            BackupInvalidReason::TooManyEntries,
            "archive",
        ));
    }
    Ok((count, offset, size))
}

/// Rule 1: reads every central-directory record (names included, even
/// duplicate ones the zip crate would fold together).
fn central_directory(file: &mut File) -> Result<Vec<DirectoryEntry>, ArchiveError> {
    let (count, offset, size) = end_of_central_directory(file)?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| not_a_zip())?;
    let mut reader = io::BufReader::new(Read::take(&mut *file, size));
    let mut entries = Vec::new();
    for index in 0..count {
        let mut fixed = [0_u8; 46];
        reader.read_exact(&mut fixed).map_err(|_| not_a_zip())?;
        if u32_at(&fixed, 0) != CENTRAL_SIGNATURE {
            return Err(not_a_zip());
        }
        let name_length = usize::from(u16_at(&fixed, 28));
        let extra_length = u64::from(u16_at(&fixed, 30));
        let comment_length = u64::from(u16_at(&fixed, 32));
        // Rule 3's length cap, before a buffer is sized from the record.
        if name_length > MAX_NAME_BYTES {
            return Err(ArchiveError::invalid(
                BackupInvalidReason::UnsafePath,
                format!("entries[{index}]"),
            ));
        }
        let mut name = vec![0_u8; name_length];
        reader.read_exact(&mut name).map_err(|_| not_a_zip())?;
        let skip = extra_length + comment_length;
        if io::copy(&mut Read::take(&mut reader, skip), &mut io::sink()).map_err(|_| not_a_zip())?
            != skip
        {
            return Err(not_a_zip());
        }
        entries.push(DirectoryEntry {
            name,
            flags: u16_at(&fixed, 8),
            method: u16_at(&fixed, 10),
            version_made_by: u16_at(&fixed, 4),
            external_attributes: u32_at(&fixed, 38),
        });
    }
    Ok(entries)
}

/// Rule 4's regular-file test: stored or deflated, unencrypted, not a
/// directory or a symlink (by the Unix mode or the DOS directory bit).
fn is_supported(entry: &DirectoryEntry) -> bool {
    const S_IFMT: u32 = 0o170_000;
    const S_IFREG: u32 = 0o100_000;
    let encrypted = entry.flags & 1 != 0;
    let method_ok = entry.method == 0 || entry.method == 8;
    let unix = entry.version_made_by >> 8 == 3;
    let file_type = (entry.external_attributes >> 16) & S_IFMT;
    let unix_ok = !unix || file_type == 0 || file_type == S_IFREG;
    let dos_directory = entry.external_attributes & 0x10 != 0;
    !encrypted && method_ok && unix_ok && !dos_directory
}

/// Opens `path` and applies D3 rules 1–8.
pub fn open(path: &Path) -> Result<ArchiveReader, ArchiveError> {
    open_from_schema(path, super::manifest::MIN_SCHEMA_VERSION)
}

/// [`open`] with the compatibility window starting at
/// `min_schema_version` ([`Manifest::parse_from_schema`]).
pub fn open_from_schema(
    path: &Path,
    min_schema_version: i64,
) -> Result<ArchiveReader, ArchiveError> {
    let mut file = File::open(path).map_err(|_| not_a_zip())?;
    let directory = central_directory(&mut file)?;

    // Rule 3, then rule 4.
    if let Some(index) = directory
        .iter()
        .position(|entry| !is_safe_entry_name(&entry.name))
    {
        return Err(ArchiveError::invalid(
            BackupInvalidReason::UnsafePath,
            format!("entries[{index}]"),
        ));
    }
    let mut seen = BTreeSet::new();
    for (index, entry) in directory.iter().enumerate() {
        if !is_supported(entry) {
            return Err(ArchiveError::invalid(
                BackupInvalidReason::UnsupportedEntry,
                format!("entries[{index}]"),
            ));
        }
        if !seen.insert(entry.name.as_slice()) {
            return Err(ArchiveError::invalid(
                BackupInvalidReason::DuplicatePath,
                format!("entries[{index}]"),
            ));
        }
    }

    file.seek(SeekFrom::Start(0)).map_err(|_| not_a_zip())?;
    let mut zip = ZipArchive::new(file).map_err(|_| not_a_zip())?;
    if zip.len() != directory.len() {
        return Err(not_a_zip());
    }

    // Rule 5 (and 6, inside `Manifest::parse`).
    let manifest_index = zip
        .index_for_name(MANIFEST_PATH)
        .ok_or_else(|| ArchiveError::invalid(BackupInvalidReason::ManifestMissing, "manifest"))?;
    let manifest = {
        let too_large = || ArchiveError::invalid(BackupInvalidReason::ManifestTooLarge, "manifest");
        let entry = zip.by_index(manifest_index).map_err(|_| not_a_zip())?;
        if entry.size() > MAX_MANIFEST_BYTES {
            return Err(too_large());
        }
        let mut bytes = Vec::new();
        Read::take(entry, MAX_MANIFEST_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ArchiveError::invalid(BackupInvalidReason::ManifestInvalid, "manifest"))?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(too_large());
        }
        Manifest::parse_from_schema(&bytes, min_schema_version)?
    };

    // Rule 7.
    let entry_path_invalid = |index: usize| {
        ArchiveError::invalid(
            BackupInvalidReason::ManifestInvalid,
            format!("manifest.entries[{index}].path"),
        )
    };
    let mut listed = BTreeSet::new();
    let mut databases = 0;
    for (index, entry) in manifest.entries.iter().enumerate() {
        let path = entry.path.as_str();
        if !listed.insert(path) || path == MANIFEST_PATH || !is_safe_entry_name(path.as_bytes()) {
            return Err(entry_path_invalid(index));
        }
        let shape_ok = if path == DATABASE_PATH {
            databases += 1;
            true
        } else if path.starts_with("content/") {
            content_hex(path).is_some()
        } else if let Some(rel_path) = path.strip_prefix("media/") {
            is_media_rel_path(rel_path)
        } else {
            false
        };
        if !shape_ok {
            return Err(entry_path_invalid(index));
        }
    }
    if databases != 1 {
        return Err(ArchiveError::invalid(
            BackupInvalidReason::ManifestInvalid,
            "manifest.entries",
        ));
    }

    // Rule 8.
    for (index, entry) in directory.iter().enumerate() {
        let name = std::str::from_utf8(&entry.name).expect("rule 3 checked UTF-8");
        if name != MANIFEST_PATH && !listed.contains(name) {
            return Err(ArchiveError::invalid(
                BackupInvalidReason::EntryUnlisted,
                format!("entries[{index}]"),
            ));
        }
    }
    let mut indices = BTreeMap::new();
    for (index, entry) in manifest.entries.iter().enumerate() {
        let zip_index = zip.index_for_name(&entry.path).ok_or_else(|| {
            ArchiveError::invalid(
                BackupInvalidReason::EntryMissing,
                format!("manifest.entries[{index}]"),
            )
        })?;
        indices.insert(entry.path.clone(), zip_index);
    }

    Ok(ArchiveReader {
        zip,
        manifest,
        indices,
    })
}

impl ArchiveReader {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// D3 rule 10 for manifest entry `entry_index`: streams the entry once
    /// into `sink`, cut off at its declared `bytes` plus one, and checks
    /// its length, SHA-256, and (for a content entry) its name.
    pub fn copy_entry(
        &mut self,
        entry_index: usize,
        sink: &mut impl Write,
    ) -> Result<(), ArchiveError> {
        let entry = &self.manifest.entries[entry_index];
        let field_path = format!("manifest.entries[{entry_index}]");
        let damaged =
            || ArchiveError::invalid(BackupInvalidReason::ChecksumMismatch, field_path.clone());
        let zip_index = self.indices[&entry.path];
        let file = self.zip.by_index(zip_index).map_err(|_| damaged())?;
        // Cut off at the declared length plus one: a lying header or a zip
        // bomb stops there.
        let mut reader = Sha256Reader::new(Read::take(file, entry.bytes.saturating_add(1)));
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            // A decompression or CRC-32 failure surfaces as a read error.
            let read = reader.read(&mut buffer).map_err(|_| damaged())?;
            if read == 0 {
                break;
            }
            sink.write_all(&buffer[..read])
                .map_err(|error| ArchiveError::Sink(error.kind()))?;
        }
        let (bytes, sha256) = reader.finish();
        if bytes != entry.bytes {
            return Err(ArchiveError::invalid(
                BackupInvalidReason::SizeMismatch,
                field_path,
            ));
        }
        if sha256 != entry.sha256 {
            return Err(damaged());
        }
        if content_hex(&entry.path).is_some_and(|hex| hex != sha256) {
            return Err(ArchiveError::invalid(
                BackupInvalidReason::ContentNameMismatch,
                field_path,
            ));
        }
        Ok(())
    }
}

/// D3 rules 1–8 and 10 over the whole file: every entry streamed and
/// hashed, nothing extracted (D9's safety-backup verifier).
pub fn verify(path: &Path) -> Result<Manifest, ArchiveError> {
    let mut reader = open(path)?;
    for index in 0..reader.manifest.entries.len() {
        reader.copy_entry(index, &mut io::sink())?;
    }
    Ok(reader.manifest)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::backup::manifest::{
        ManifestContents, ManifestEntry, ManifestMediaCounts, ManifestMigration, ManifestPlatform,
    };
    use crate::backup::{BackupExcludedClass, BackupMediaChoice, BackupOrigin};
    use crate::persistence::CURRENT_SCHEMA_VERSION;
    use std::path::PathBuf;

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn created_at() -> DateTime<Utc> {
        "2026-09-29T12:00:00.000Z".parse().unwrap()
    }

    fn manifest_for(entries: &[(&str, &[u8])]) -> Manifest {
        Manifest {
            format: "farm3d-backup".to_string(),
            format_version: 1,
            created_at: "2026-09-29T12:00:00.000Z".to_string(),
            app_version: "0.1.0".to_string(),
            schema_version: CURRENT_SCHEMA_VERSION,
            migrations: vec![ManifestMigration {
                version: 1,
                name: "0001_foundation".to_string(),
                checksum: "a".repeat(64),
            }],
            platform: ManifestPlatform::current(),
            origin: BackupOrigin::Operator,
            contents: ManifestContents {
                media: BackupMediaChoice::All,
            },
            counts: BTreeMap::new(),
            excluded: BackupExcludedClass::ALL.to_vec(),
            credential_ref_count: 0,
            media: ManifestMediaCounts::default(),
            entries: entries
                .iter()
                .map(|(path, bytes)| ManifestEntry {
                    path: path.to_string(),
                    bytes: bytes.len() as u64,
                    sha256: sha256_hex(bytes),
                })
                .collect(),
        }
    }

    const DB: &[u8] = b"not really a database";
    const BLOB: &[u8] = b"blob bytes";

    fn blob_path() -> String {
        content_path(&sha256_hex(BLOB))
    }

    /// Writes `manifest` then each `(path, bytes)` with [`ArchiveWriter`].
    pub(crate) fn write_archive(
        dir: &Path,
        manifest: Option<&Manifest>,
        entries: &[(&str, &[u8])],
    ) -> PathBuf {
        let path = dir.join(format!("{}.farm3d-backup", uuid::Uuid::new_v4()));
        let mut writer = ArchiveWriter::new(File::create(&path).unwrap(), created_at()).unwrap();
        if let Some(manifest) = manifest {
            writer.write_manifest(manifest).unwrap();
        }
        for (name, bytes) in entries {
            writer
                .write_entry(name, *bytes, EntryCompression::Deflated, bytes.len() as u64)
                .unwrap();
        }
        writer.finish().unwrap();
        path
    }

    fn good(dir: &Path) -> PathBuf {
        let blob = blob_path();
        let entries: [(&str, &[u8]); 2] = [(DATABASE_PATH, DB), (&blob, BLOB)];
        write_archive(dir, Some(&manifest_for(&entries)), &entries)
    }

    fn invalid(reason: BackupInvalidReason, field_path: &str) -> ArchiveError {
        ArchiveError::Invalid {
            reason,
            field_path: field_path.to_string(),
        }
    }

    fn verify_err(path: &Path) -> ArchiveError {
        verify(path).map(|_| ()).unwrap_err()
    }

    #[test]
    fn each_path_rule_rejects_its_case() {
        for ok in [
            "manifest.json",
            "database/farm3d.sqlite3",
            "content/sha256/ab/abcdef",
            "media/snapshots/2026/01/snp-a.jpg",
        ] {
            assert!(is_safe_entry_name(ok.as_bytes()), "{ok}");
        }
        let long = format!("media/{}", "a".repeat(MAX_NAME_BYTES));
        let rejected: Vec<Vec<u8>> = vec![
            vec![0xff, 0xfe],                // not UTF-8
            b"".to_vec(),                    // empty
            long.into_bytes(),               // over 512 bytes
            b"/media/x".to_vec(),            // absolute
            b"C:/media/x".to_vec(),          // drive letter
            b"c:media".to_vec(),             // drive-relative
            b"media\\x".to_vec(),            // backslash
            b"media/../database/x".to_vec(), // `..`
            b"media/./x".to_vec(),           // `.`
            b"media//x".to_vec(),            // empty segment
            b"media/".to_vec(),              // trailing empty segment
            b"media/x\ny".to_vec(),          // control character
            b"media/x\x7fy".to_vec(),        // DEL
            b"media/x\0".to_vec(),           // NUL
            b"logs/farm3d.log".to_vec(),     // unknown top level
            b"Manifest.json".to_vec(),       // case matters
        ];
        for name in rejected {
            assert!(
                !is_safe_entry_name(&name),
                "{:?} should be rejected",
                String::from_utf8_lossy(&name)
            );
        }
    }

    #[test]
    fn streaming_sha256_matches_the_one_shot_digest() {
        let data: Vec<u8> = (0..300_000_u32).map(|value| (value % 251) as u8).collect();
        let mut reader = Sha256Reader::new(&data[..]);
        let mut out = Vec::new();
        // Small reads, so the hash spans many updates.
        let mut buffer = [0_u8; 7];
        loop {
            let read = reader.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            out.extend_from_slice(&buffer[..read]);
        }
        assert_eq!(out, data);
        assert_eq!(reader.finish(), (data.len() as u64, sha256_hex(&data)));
        assert_eq!(
            Sha256Reader::new(&b"abc"[..]).finish(),
            (0, sha256_hex(b"")),
            "nothing read yet"
        );
        let mut abc = Sha256Reader::new(&b"abc"[..]);
        io::copy(&mut abc, &mut io::sink()).unwrap();
        assert_eq!(
            abc.finish().1,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_well_formed_archive_verifies() {
        let temp = tempfile::tempdir().unwrap();
        let path = good(temp.path());
        let manifest = verify(&path).expect("verifies");
        assert_eq!(manifest.entries.len(), 2);
    }

    #[test]
    fn the_writer_writes_regular_files_in_order_with_the_created_at_time() {
        let temp = tempfile::tempdir().unwrap();
        let path = good(temp.path());
        let mut zip = ZipArchive::new(File::open(&path).unwrap()).unwrap();
        let names: Vec<String> = zip.file_names().map(str::to_string).collect();
        assert_eq!(
            names,
            vec![
                MANIFEST_PATH.to_string(),
                DATABASE_PATH.to_string(),
                blob_path()
            ]
        );
        for index in 0..zip.len() {
            let entry = zip.by_index_raw(index).unwrap();
            assert!(entry.is_file() && !entry.encrypted());
            let modified = entry.last_modified().unwrap();
            assert_eq!(
                (
                    modified.year(),
                    modified.month(),
                    modified.day(),
                    modified.hour()
                ),
                (2026, 9, 29, 12)
            );
        }
    }

    #[test]
    fn a_content_entry_whose_hash_does_not_match_its_name_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        // Named for another hash; the manifest lists the true hash of the
        // bytes, so size and checksum pass and only the name is wrong.
        let wrong = content_path(&"d".repeat(64));
        let entries: [(&str, &[u8]); 2] = [(DATABASE_PATH, DB), (&wrong, BLOB)];
        let path = write_archive(temp.path(), Some(&manifest_for(&entries)), &entries);
        assert_eq!(
            verify_err(&path),
            invalid(
                BackupInvalidReason::ContentNameMismatch,
                "manifest.entries[1]"
            )
        );
    }

    #[test]
    fn a_truncated_file_is_not_a_zip() {
        let temp = tempfile::tempdir().unwrap();
        let path = good(temp.path());
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 30]).unwrap();
        assert_eq!(
            verify_err(&path),
            invalid(BackupInvalidReason::NotAZip, "archive")
        );
        std::fs::write(&path, b"plain text").unwrap();
        assert_eq!(
            verify_err(&path),
            invalid(BackupInvalidReason::NotAZip, "archive")
        );
    }

    #[test]
    fn an_unsafe_entry_name_is_reported_by_index_only() {
        let temp = tempfile::tempdir().unwrap();
        let entries: [(&str, &[u8]); 2] = [(DATABASE_PATH, DB), ("media/../escape", BLOB)];
        let path = write_archive(temp.path(), Some(&manifest_for(&entries[..1])), &entries);
        assert_eq!(
            verify_err(&path),
            invalid(BackupInvalidReason::UnsafePath, "entries[2]")
        );
    }

    /// A name over rule 3's cap is refused as its record is read, before a
    /// buffer is sized from it or a later record is parsed.
    #[test]
    fn an_overlong_name_is_refused_before_the_rest_of_the_directory() {
        let temp = tempfile::tempdir().unwrap();
        let name_length = MAX_NAME_BYTES + 88;
        let mut bytes = Vec::new();
        let mut record = [0_u8; 46];
        record[0..4].copy_from_slice(&CENTRAL_SIGNATURE.to_le_bytes());
        record[28..30].copy_from_slice(&(name_length as u16).to_le_bytes());
        bytes.extend_from_slice(&record);
        bytes.extend(std::iter::repeat_n(b'a', name_length));
        // A second record that isn't one.
        bytes.extend_from_slice(&[0_u8; 46]);
        let size = bytes.len() as u32;
        let mut eocd = [0_u8; 22];
        eocd[0..4].copy_from_slice(&EOCD_SIGNATURE.to_le_bytes());
        eocd[8..10].copy_from_slice(&2_u16.to_le_bytes());
        eocd[10..12].copy_from_slice(&2_u16.to_le_bytes());
        eocd[12..16].copy_from_slice(&size.to_le_bytes());
        bytes.extend_from_slice(&eocd);
        let path = temp.path().join("overlong.zip");
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(
            verify_err(&path),
            invalid(BackupInvalidReason::UnsafePath, "entries[0]")
        );
    }

    #[test]
    fn a_directory_entry_is_unsupported() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("dir.farm3d-backup");
        let mut zip = ZipWriter::new(File::create(&path).unwrap());
        zip.start_file(MANIFEST_PATH, SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&manifest_for(&[(DATABASE_PATH, DB)]).to_json())
            .unwrap();
        zip.start_file(DATABASE_PATH, SimpleFileOptions::default())
            .unwrap();
        zip.write_all(DB).unwrap();
        zip.add_symlink("media/link", "/etc/passwd", SimpleFileOptions::default())
            .unwrap();
        zip.finish().unwrap();
        assert_eq!(
            verify_err(&path),
            invalid(BackupInvalidReason::UnsupportedEntry, "entries[2]")
        );
    }

    #[test]
    fn a_duplicate_name_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let entries: [(&str, &[u8]); 3] = [
            (DATABASE_PATH, DB),
            ("media/snapshots/2026/01/snp-a.jpg", BLOB),
            ("media/snapshots/2026/01/snp-b.jpg", BLOB),
        ];
        let path = write_archive(temp.path(), Some(&manifest_for(&entries)), &entries);
        let bytes = std::fs::read(&path).unwrap();
        let patched = replace_all(&bytes, b"snp-b.jpg", b"snp-a.jpg");
        std::fs::write(&path, patched).unwrap();
        assert_eq!(
            verify_err(&path),
            invalid(BackupInvalidReason::DuplicatePath, "entries[3]")
        );
    }

    #[test]
    fn the_manifest_must_exist_and_fit() {
        let temp = tempfile::tempdir().unwrap();
        let path = write_archive(temp.path(), None, &[(DATABASE_PATH, DB)]);
        assert_eq!(
            verify_err(&path),
            invalid(BackupInvalidReason::ManifestMissing, "manifest")
        );
    }

    #[test]
    fn archive_and_manifest_entries_are_the_same_set() {
        let temp = tempfile::tempdir().unwrap();
        let blob = blob_path();
        let listed: [(&str, &[u8]); 1] = [(DATABASE_PATH, DB)];
        let written: [(&str, &[u8]); 2] = [(DATABASE_PATH, DB), (&blob, BLOB)];
        let path = write_archive(temp.path(), Some(&manifest_for(&listed)), &written);
        assert_eq!(
            verify_err(&path),
            invalid(BackupInvalidReason::EntryUnlisted, "entries[2]")
        );
        let path = write_archive(temp.path(), Some(&manifest_for(&written)), &listed);
        assert_eq!(
            verify_err(&path),
            invalid(BackupInvalidReason::EntryMissing, "manifest.entries[1]")
        );
    }

    #[test]
    fn manifest_entry_paths_are_checked() {
        let temp = tempfile::tempdir().unwrap();
        // No database entry.
        let blob = blob_path();
        let entries: [(&str, &[u8]); 1] = [(&blob, BLOB)];
        let path = write_archive(temp.path(), Some(&manifest_for(&entries)), &entries);
        assert_eq!(
            verify_err(&path),
            invalid(BackupInvalidReason::ManifestInvalid, "manifest.entries")
        );
        // A content path that isn't `content/sha256/<hh>/<hex>`.
        let entries: [(&str, &[u8]); 2] = [(DATABASE_PATH, DB), ("content/sha256/zz/bad", BLOB)];
        let path = write_archive(temp.path(), Some(&manifest_for(&entries)), &entries);
        assert_eq!(
            verify_err(&path),
            invalid(
                BackupInvalidReason::ManifestInvalid,
                "manifest.entries[1].path"
            )
        );
        // A media path that isn't a snapshot `rel_path`.
        let entries: [(&str, &[u8]); 2] = [(DATABASE_PATH, DB), ("media/other/x.jpg", BLOB)];
        let path = write_archive(temp.path(), Some(&manifest_for(&entries)), &entries);
        assert_eq!(
            verify_err(&path),
            invalid(
                BackupInvalidReason::ManifestInvalid,
                "manifest.entries[1].path"
            )
        );
    }

    #[test]
    fn a_size_or_checksum_mismatch_is_caught_while_streaming() {
        let temp = tempfile::tempdir().unwrap();
        let entries: [(&str, &[u8]); 1] = [(DATABASE_PATH, DB)];
        let mut manifest = manifest_for(&entries);
        manifest.entries[0].bytes -= 1;
        let path = write_archive(temp.path(), Some(&manifest), &entries);
        assert_eq!(
            verify_err(&path),
            invalid(BackupInvalidReason::SizeMismatch, "manifest.entries[0]")
        );
        let mut manifest = manifest_for(&entries);
        manifest.entries[0].bytes += 1;
        let path = write_archive(temp.path(), Some(&manifest), &entries);
        assert_eq!(
            verify_err(&path),
            invalid(BackupInvalidReason::SizeMismatch, "manifest.entries[0]")
        );
        let mut manifest = manifest_for(&entries);
        manifest.entries[0].sha256 = "e".repeat(64);
        let path = write_archive(temp.path(), Some(&manifest), &entries);
        assert_eq!(
            verify_err(&path),
            invalid(BackupInvalidReason::ChecksumMismatch, "manifest.entries[0]")
        );
    }

    #[test]
    fn a_newer_format_is_reported_as_newer() {
        let temp = tempfile::tempdir().unwrap();
        let entries: [(&str, &[u8]); 1] = [(DATABASE_PATH, DB)];
        let mut manifest = manifest_for(&entries);
        manifest.format_version = 2;
        let path = write_archive(temp.path(), Some(&manifest), &entries);
        assert_eq!(
            verify_err(&path),
            ArchiveError::UnsupportedFormat { received: 2 }
        );
    }

    fn replace_all(haystack: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
        assert_eq!(from.len(), to.len());
        let mut out = haystack.to_vec();
        let mut index = 0;
        while index + from.len() <= out.len() {
            if &out[index..index + from.len()] == from {
                out[index..index + from.len()].copy_from_slice(to);
                index += from.len();
            } else {
                index += 1;
            }
        }
        out
    }
}
