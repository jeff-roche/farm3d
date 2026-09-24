//! D10: the 3MF reader, `zip` + streaming `quick-xml`.
//!
//! Every ZIP entry is read through [`LimitedRead`], which enforces D10's
//! size and ratio limits and the cancel flag. XML is streamed (no DOM), and
//! `quick-xml` never expands external or custom entities. Meshes are reduced
//! to per-object bounds and a triangle count as they stream past; vertex
//! arrays are never kept.

use std::cell::Cell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek};
use std::path::Path;
use std::rc::Rc;

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use zip::read::ZipFile;
use zip::result::ZipError;
use zip::ZipArchive;

use super::png::{self, MAX_THUMBNAIL_BYTES};
use super::{
    include_point, BoundsMm, InspectError, Plate, ThreeMfInspection, ThumbnailBytes,
    ThumbnailImageFormat, ThumbnailInfo, UnsupportedCode, UnsupportedEntry,
};
use crate::library::content::CancelFlag;
use crate::library::ImportWarning;

const CONTENT_TYPES: &str = "[Content_Types].xml";
const ROOT_RELS: &str = "_rels/.rels";
const MODEL_SETTINGS: &str = "Metadata/model_settings.config";
const MODEL_REL: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
const PRODUCTION_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/production/2015/06";
const MATERIALS_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/material/2015/02";
/// The most text kept from one `Title`, `Application`, or
/// `Thumbnail_Middle` metadata element.
pub(crate) const MAX_METADATA_BYTES: usize = 4096;

const NO_OBJECTS: &str = "This 3MF contains no objects.";
/// Component nesting deeper than this is treated like a cycle.
const MAX_COMPONENT_DEPTH: usize = 64;
/// Elements that hold material and colour groups (core and `m`).
const MATERIAL_ELEMENTS: [&str; 5] = [
    "basematerials",
    "colorgroup",
    "texture2dgroup",
    "compositematerials",
    "multiproperties",
];
/// Triangle attributes that carry paint or MMU segmentation.
const PAINT_ATTRIBUTES: [&str; 6] = [
    "paint_color",
    "paint_supports",
    "paint_seam",
    "mmu_segmentation",
    "custom_supports",
    "custom_seam",
];
/// D12's 3MF thumbnail order, after `Thumbnail_Middle`.
const THUMBNAIL_PARTS: [&str; 2] = ["Metadata/thumbnail.png", "Metadata/plate_1.png"];

/// D10's safety limits. Tests pass smaller ones.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ZipLimits {
    pub max_entries: usize,
    pub max_total_bytes: u64,
    pub max_entry_bytes: u64,
    pub max_ratio: u64,
    pub max_event_bytes: u64,
    /// Build items plus components across every model part. Past this the
    /// inspection is refused: dropping some would misreport bounds.
    pub max_placements: usize,
    /// `<object>` elements across every model part, likewise refused.
    pub max_objects: usize,
    /// Plates, object ids per plate, per-object setting keys, and paint
    /// attributes per part are only listed, so past this they're truncated.
    pub max_listed: usize,
}

impl ZipLimits {
    pub(crate) const SPEC: Self = Self {
        max_entries: 10_000,
        max_total_bytes: 4 << 30,
        max_entry_bytes: 2 << 30,
        max_ratio: 1_000,
        max_event_bytes: 16 << 20,
        max_placements: 1_000_000,
        max_objects: 1_000_000,
        max_listed: super::gcode::MAX_LISTED,
    };
}

type Inspected = (
    ThreeMfInspection,
    Option<ThumbnailBytes>,
    Vec<ImportWarning>,
);
/// Every listed thumbnail, the chosen one, and the skip warnings.
type Thumbnails = (
    Vec<ThumbnailInfo>,
    Option<ThumbnailBytes>,
    Vec<ImportWarning>,
);

/// D8: a ZIP that holds `[Content_Types].xml`. A ZIP that can't be read
/// isn't one.
pub fn is_3mf_package(file: File) -> Result<bool, InspectError> {
    Ok(open_archive(file, &ZipLimits::SPEC)?
        .is_some_and(|archive| archive.index_for_name(CONTENT_TYPES).is_some()))
}

pub fn inspect(path: &Path, cancel: &CancelFlag) -> Result<Inspected, InspectError> {
    inspect_reader(File::open(path)?, cancel, &ZipLimits::SPEC)
}

pub(crate) fn inspect_reader<R: Read + Seek>(
    reader: R,
    cancel: &CancelFlag,
    limits: &ZipLimits,
) -> Result<Inspected, InspectError> {
    if cancel.is_cancelled() {
        return Err(InspectError::Cancelled);
    }
    let archive = open_archive(reader, limits)?
        .ok_or_else(|| InspectError::invalid("This 3MF isn't a readable ZIP package."))?;
    if archive.len() > limits.max_entries {
        return Err(limit_exceeded(format!(
            "it has more than {} ZIP entries",
            limits.max_entries
        )));
    }
    let entry_names: Vec<String> = archive.file_names().map(str::to_string).collect();
    let mut package = Package {
        archive,
        cancel,
        limits,
        total_read: 0,
        counts: Counts::default(),
    };

    if !package.has(CONTENT_TYPES) {
        return Err(InspectError::invalid(
            "This 3MF has no [Content_Types].xml.",
        ));
    }
    package.parse(CONTENT_TYPES, |_| Ok(()))?;
    let start_part = package.start_part()?;
    let start_rels = rels_part_for(&start_part);
    if package.has(&start_rels) {
        package.model_relationship(&start_rels, parent_dir(&start_part))?;
    }

    // The start part, then every part a component or build item names.
    let mut parts: HashMap<String, ModelPart> = HashMap::new();
    let mut pending = vec![start_part.clone()];
    while let Some(name) = pending.pop() {
        if parts.contains_key(&name) {
            continue;
        }
        if !package.has(&name) {
            return Err(InspectError::invalid(format!(
                "This 3MF references {name}, which is missing."
            )));
        }
        let part = package.model_part(&name, name == start_part)?;
        pending.extend(part.referenced_parts());
        parts.insert(name, part);
    }
    let root = &parts[&start_part];

    let scale = unit_scale(&root.unit)?;
    let mut resolver = Resolver {
        parts: &parts,
        placed: HashMap::new(),
        visiting: HashSet::new(),
    };
    let mut bounds: Option<BoundsMm> = None;
    let mut triangle_count = 0u64;
    for item in &root.build {
        let (item_bounds, item_triangles) = resolver.place(item, 0)?;
        if let Some(item_bounds) = item_bounds {
            include_bounds(&mut bounds, &item_bounds);
        }
        triangle_count = triangle_count.saturating_add(item_triangles);
    }
    let bounds_mm = match bounds {
        Some(bounds) if root.object_count > 0 && triangle_count > 0 => bounds.scaled(scale),
        _ => return Err(InspectError::invalid(NO_OBJECTS)),
    };

    let (plates, per_object_keys) = if package.has(MODEL_SETTINGS) {
        package.model_settings()?
    } else {
        (Vec::new(), BTreeSet::new())
    };
    let unsupported = unsupported_entries(&entry_names, &parts, &per_object_keys);
    let (thumbnails, thumbnail, warnings) = package.thumbnails(root.thumbnail_middle.as_deref())?;

    let mut required_extensions = root.required.clone();
    let mut other_parts: Vec<_> = parts
        .iter()
        .filter(|(name, _)| **name != start_part)
        .collect();
    other_parts.sort_by_key(|(name, _)| *name);
    for (_, part) in other_parts {
        for prefix in &part.required {
            if !required_extensions.contains(prefix) {
                required_extensions.push(prefix.clone());
            }
        }
    }

    Ok((
        ThreeMfInspection {
            unit: root.unit.clone(),
            producer: root.application.clone(),
            title: root.title.clone(),
            object_count: root.object_count,
            build_item_count: root.build.len() as u32,
            triangle_count,
            bounds_mm,
            plates,
            required_extensions,
            unsupported,
            thumbnails,
        },
        thumbnail,
        warnings,
    ))
}

/// Resolves an OPC part reference against the directory of the part that
/// holds it (`""` for the package root) into a ZIP entry name. A `..` that
/// climbs above the package root is `INVALID_CONTENT`.
pub(crate) fn normalize_part_path(base_dir: &str, target: &str) -> Result<String, InspectError> {
    let escapes = || InspectError::invalid(format!("A part path escapes the package: {target}"));
    let mut segments: Vec<&str> = Vec::new();
    let (start, relative) = match target.strip_prefix('/') {
        Some(absolute) => ("", absolute),
        None => (base_dir, target),
    };
    for segment in start.split('/').chain(relative.split('/')) {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop().ok_or_else(escapes)?;
            }
            name => segments.push(name),
        }
    }
    if segments.is_empty() {
        return Err(escapes());
    }
    Ok(segments.join("/"))
}

fn parent_dir(part: &str) -> &str {
    part.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// `3D/3dmodel.model` → `3D/_rels/3dmodel.model.rels`.
fn rels_part_for(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

fn unit_scale(unit: &str) -> Result<f64, InspectError> {
    Ok(match unit {
        "micron" => 0.001,
        "millimeter" => 1.0,
        "centimeter" => 10.0,
        "inch" => 25.4,
        "foot" => 304.8,
        "meter" => 1000.0,
        other => {
            return Err(InspectError::invalid(format!(
                "This 3MF uses an unknown unit: {other}."
            )))
        }
    })
}

fn include_bounds(bounds: &mut Option<BoundsMm>, other: &BoundsMm) {
    match bounds {
        Some(existing) => existing.union(other),
        None => *bounds = Some(*other),
    }
}

// --- Opening the archive ----------------------------------------------------
//
// zip 8.6's `ZipArchive::new` reserves a `Vec` for the declared entry count
// and parses every central-directory record before `len()` can be checked.
// When the first end record fails to parse, it also scans backwards for an
// earlier one and tries again. A hostile ZIP64 file under the import size
// cap could therefore make it allocate for millions of entries.
//
// `open_archive` bounds that in two steps:
//
// 1. `guard_directory` reads the end record (and the ZIP64 end record) the
//    same way zip picks its first candidate: the highest `PK\x05\x06` in the
//    tail whose comment fits. It rejects a declared count over the entry
//    limit, a directory larger than `max_entries` records of 46 bytes plus
//    1 KiB, and any other end-record signature between the directory start
//    and the end record.
// 2. `ZipArchive::new` then reads through `GuardedReader`, which shows zip
//    only that checked region, serves zeros for every other byte, and stops
//    after a read budget. The guard is lifted once the archive is open.
//
// So the only end records zip can see are the checked ones. A retry after a
// failed parse finds no other signature (the rest of the file reads as
// zeros), and it runs out of budget if it keeps scanning.

/// The most bytes a central-directory record may average: the fixed 46
/// bytes plus 1 KiB of name, extra field, and comment.
const MAX_DIRECTORY_RECORD: u64 = 46 + 1024;
/// The longest end record plus comment.
const MAX_END_RECORD: u64 = 22 + u16::MAX as u64;
/// The ZIP64 end record plus its locator.
const ZIP64_END_RECORDS: u64 = 56 + 20;
/// Reads allowed during `ZipArchive::new` beyond the checked region.
const GUARD_SLACK: u64 = 1 << 20;

const END_SIGNATURE: &[u8; 4] = b"PK\x05\x06";
const ZIP64_END_SIGNATURE: &[u8; 4] = b"PK\x06\x06";
const ZIP64_LOCATOR_SIGNATURE: &[u8; 4] = b"PK\x06\x07";
const DIRECTORY_SIGNATURE: &[u8; 4] = b"PK\x01\x02";

/// Opens the ZIP with its central directory bounded (see above). `None`
/// means it isn't a ZIP farm3d can read; a limit breach is an error.
fn open_archive<R: Read + Seek>(
    reader: R,
    limits: &ZipLimits,
) -> Result<Option<ZipArchive<GuardedReader<R>>>, InspectError> {
    let guard = Rc::new(Cell::new(None));
    let mut view = GuardedReader::new(reader, guard.clone())?;
    let Some(region) = guard_directory(&mut view, limits)? else {
        return Ok(None);
    };
    guard.set(Some(DirectoryGuard {
        start: region.start,
        end: region.end,
        budget: (region.end - region.start).saturating_add(GUARD_SLACK),
    }));
    let opened = ZipArchive::new(view);
    guard.set(None);
    match opened {
        Ok(archive) => Ok(Some(archive)),
        Err(ZipError::Io(error)) if is_stop(&error) => Err(from_io(&error)),
        Err(ZipError::Io(error))
            if !matches!(
                error.kind(),
                io::ErrorKind::UnexpectedEof | io::ErrorKind::InvalidData
            ) =>
        {
            Err(error.into())
        }
        Err(_) => Ok(None),
    }
}

fn is_stop(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|inner| inner.is::<Stop>())
}

fn le_u16(bytes: &[u8], at: usize) -> u64 {
    u64::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]))
}

fn le_u32(bytes: &[u8], at: usize) -> u64 {
    u64::from(u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()))
}

fn le_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn read_at<R: Read + Seek>(reader: &mut R, at: u64, len: usize) -> io::Result<Vec<u8>> {
    reader.seek(io::SeekFrom::Start(at))?;
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// Checks the end records and returns the byte range zip may read while
/// opening: from the directory start to the end of the file. `None` means
/// there is no usable end record.
fn guard_directory<R: Read + Seek>(
    reader: &mut R,
    limits: &ZipLimits,
) -> Result<Option<std::ops::Range<u64>>, InspectError> {
    let file_len = reader.seek(io::SeekFrom::End(0))?;
    let tail_start = file_len.saturating_sub(MAX_END_RECORD);
    let tail = read_at(reader, tail_start, (file_len - tail_start) as usize)?;

    // zip's first candidate: the highest signature whose comment fits.
    let Some(end_at) = (0..tail.len().saturating_sub(21)).rev().find(|&at| {
        &tail[at..at + 4] == END_SIGNATURE
            && at + 22 + le_u16(&tail, at + 20) as usize <= tail.len()
    }) else {
        return Ok(None);
    };
    let end = &tail[end_at..end_at + 22];
    let end_offset = tail_start + end_at as u64;
    let mut declared = le_u16(end, 8).max(le_u16(end, 10));
    let mut directory_start = le_u32(end, 16);

    // A ZIP64 locator right before the end record names the ZIP64 end
    // record, which must be exactly where it says (no prepended data).
    let mut zip64_offset = None;
    if end_offset >= 20 {
        let locator = read_at(reader, end_offset - 20, 20)?;
        if &locator[..4] == ZIP64_LOCATOR_SIGNATURE {
            let offset = le_u64(&locator, 8);
            if offset.saturating_add(56) > end_offset - 20 {
                return Ok(None);
            }
            let record = read_at(reader, offset, 56)?;
            if &record[..4] != ZIP64_END_SIGNATURE {
                return Ok(None);
            }
            // 0xFFFF in the classic record defers to the ZIP64 counts.
            if declared == u64::from(u16::MAX) {
                declared = 0;
            }
            declared = declared.max(le_u64(&record, 24)).max(le_u64(&record, 32));
            directory_start = le_u64(&record, 48);
            zip64_offset = Some(offset);
        }
    }

    if declared > limits.max_entries as u64 {
        return Err(limit_exceeded(format!(
            "it has more than {} ZIP entries",
            limits.max_entries
        )));
    }
    let region_start = zip64_offset.map_or(directory_start, |offset| offset.min(directory_start));
    if region_start > end_offset {
        return Ok(None);
    }
    let directory_bytes = end_offset - region_start;
    let allowed = (limits.max_entries as u64)
        .saturating_mul(MAX_DIRECTORY_RECORD)
        .saturating_add(ZIP64_END_RECORDS);
    if directory_bytes > allowed {
        return Err(limit_exceeded(format!(
            "its ZIP central directory is larger than {allowed} bytes"
        )));
    }
    if declared > 0 && read_at(reader, directory_start, 4)? != DIRECTORY_SIGNATURE {
        return Ok(None);
    }

    // No second end record may hide in the directory for zip to retry.
    let directory = read_at(reader, region_start, directory_bytes as usize)?;
    let hidden = directory.windows(4).enumerate().any(|(at, window)| {
        window == END_SIGNATURE
            || (window == ZIP64_END_SIGNATURE && Some(region_start + at as u64) != zip64_offset)
    });
    if hidden {
        return Err(InspectError::invalid(
            "This 3MF has a malformed ZIP directory.",
        ));
    }
    Ok(Some(region_start..file_len))
}

/// While a guard is set, what [`ZipArchive::new`] may read.
#[derive(Clone, Copy, Debug)]
struct DirectoryGuard {
    start: u64,
    end: u64,
    budget: u64,
}

/// A `Read + Seek` view that, while its guard is set, serves real bytes only
/// inside the guarded range, zeros elsewhere, and fails once the read budget
/// is spent. Unguarded, it passes through. Seeks are applied lazily.
struct GuardedReader<R> {
    inner: R,
    guard: Rc<Cell<Option<DirectoryGuard>>>,
    len: u64,
    position: u64,
    inner_position: Option<u64>,
}

impl<R: Read + Seek> GuardedReader<R> {
    fn new(mut inner: R, guard: Rc<Cell<Option<DirectoryGuard>>>) -> io::Result<Self> {
        let len = inner.seek(io::SeekFrom::End(0))?;
        Ok(Self {
            inner,
            guard,
            len,
            position: 0,
            inner_position: None,
        })
    }

    fn read_inner(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.inner_position != Some(self.position) {
            self.inner.seek(io::SeekFrom::Start(self.position))?;
        }
        let count = self.inner.read(buf)?;
        self.position += count as u64;
        self.inner_position = Some(self.position);
        Ok(count)
    }
}

impl<R: Read + Seek> Read for GuardedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let Some(mut guard) = self.guard.get() else {
            return self.read_inner(buf);
        };
        if buf.is_empty() || self.position >= self.len {
            return Ok(0);
        }
        if guard.budget == 0 {
            return Err(io::Error::other(Stop::Limit(
                "its ZIP central directory can't be read within bounds".to_string(),
            )));
        }
        let inside = (guard.start..guard.end).contains(&self.position);
        let boundary = if inside {
            guard.end
        } else if self.position < guard.start {
            guard.start
        } else {
            self.len
        };
        let want = (buf.len() as u64)
            .min(boundary - self.position)
            .min(guard.budget) as usize;
        let count = if inside {
            self.read_inner(&mut buf[..want])?
        } else {
            buf[..want].fill(0);
            self.position += want as u64;
            want
        };
        guard.budget -= count as u64;
        self.guard.set(Some(guard));
        Ok(count)
    }
}

impl<R: Read + Seek> Seek for GuardedReader<R> {
    fn seek(&mut self, target: io::SeekFrom) -> io::Result<u64> {
        let position = match target {
            io::SeekFrom::Start(offset) => Some(offset),
            io::SeekFrom::End(offset) => self.len.checked_add_signed(offset),
            io::SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek out of range"))?;
        self.position = position;
        Ok(position)
    }
}

// --- Limits and errors ------------------------------------------------------

/// Why a [`LimitedRead`] stopped, carried through `io::Error` (and
/// `quick_xml::Error::Io`) so it can be told apart from a real I/O error.
#[derive(Debug)]
enum Stop {
    Cancelled,
    Limit(String),
}

impl fmt::Display for Stop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::Limit(detail) => f.write_str(detail),
        }
    }
}

impl std::error::Error for Stop {}

fn limit_exceeded(detail: String) -> InspectError {
    InspectError::invalid(format!("This 3MF exceeds a safety limit: {detail}."))
}

fn from_io(error: &io::Error) -> InspectError {
    match error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<Stop>())
    {
        Some(Stop::Cancelled) => InspectError::Cancelled,
        Some(Stop::Limit(detail)) => limit_exceeded(detail.clone()),
        None if error.kind() == io::ErrorKind::InvalidData => {
            InspectError::invalid("This 3MF has a corrupt ZIP entry.")
        }
        None => InspectError::Io(error.kind()),
    }
}

fn from_zip(error: ZipError) -> InspectError {
    match error {
        ZipError::Io(error) => from_io(&error),
        _ => InspectError::invalid("This 3MF isn't a readable ZIP package."),
    }
}

fn from_xml(error: quick_xml::Error, part: &str) -> InspectError {
    match error {
        quick_xml::Error::Io(error) => from_io(&error),
        _ => InspectError::invalid(format!("{part} isn't well-formed XML.")),
    }
}

/// Reads one ZIP entry while enforcing D10's per-entry size, per-entry
/// ratio, and package-wide total limits, and the cancel flag. `since_event`
/// counts bytes since the XML reader last produced an event, so one start
/// tag or text run can't grow its buffer past `max_event_bytes`.
struct LimitedRead<'a, R> {
    inner: R,
    cancel: &'a CancelFlag,
    limits: &'a ZipLimits,
    ratio_cap: u64,
    read: u64,
    since_event: u64,
    total: &'a mut u64,
}

impl<R: Read> Read for LimitedRead<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::other(Stop::Cancelled));
        }
        let count = self.inner.read(buf)?;
        self.read += count as u64;
        self.since_event += count as u64;
        *self.total += count as u64;
        let limit = if self.since_event > self.limits.max_event_bytes {
            Some(format!(
                "one XML element or text run is larger than {} bytes",
                self.limits.max_event_bytes
            ))
        } else if self.read > self.limits.max_entry_bytes {
            Some(format!(
                "a ZIP entry is larger than {} bytes",
                self.limits.max_entry_bytes
            ))
        } else if self.read > self.ratio_cap {
            Some(format!(
                "a ZIP entry expands more than {}:1, the compression ratio limit",
                self.limits.max_ratio
            ))
        } else if *self.total > self.limits.max_total_bytes {
            Some(format!(
                "more than {} bytes decompressed in total",
                self.limits.max_total_bytes
            ))
        } else {
            None
        };
        match limit {
            Some(detail) => Err(io::Error::other(Stop::Limit(detail))),
            None => Ok(count),
        }
    }
}

// --- The package ------------------------------------------------------------

struct Package<'a, R> {
    archive: ZipArchive<R>,
    cancel: &'a CancelFlag,
    limits: &'a ZipLimits,
    total_read: u64,
    counts: Counts,
}

impl<R: Read + Seek> Package<'_, R> {
    fn has(&self, name: &str) -> bool {
        self.archive.index_for_name(name).is_some()
    }

    fn open(&mut self, name: &str) -> Result<LimitedRead<'_, ZipFile<'_, R>>, InspectError> {
        let entry = self.archive.by_name(name).map_err(from_zip)?;
        let ratio_cap = entry
            .compressed_size()
            .max(1)
            .saturating_mul(self.limits.max_ratio);
        Ok(LimitedRead {
            inner: entry,
            cancel: self.cancel,
            limits: self.limits,
            ratio_cap,
            read: 0,
            since_event: 0,
            total: &mut self.total_read,
        })
    }

    /// Streams `name` as XML, handing each event to `on_event`.
    fn parse(
        &mut self,
        name: &str,
        mut on_event: impl FnMut(Event<'_>) -> Result<(), InspectError>,
    ) -> Result<(), InspectError> {
        let mut reader = Reader::from_reader(BufReader::new(self.open(name)?));
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(|error| from_xml(error, name))?
            {
                Event::Eof => return Ok(()),
                event => on_event(event)?,
            }
            buffer.clear();
            reader.get_mut().get_mut().since_event = 0;
        }
    }

    /// The first `3dmodel` relationship target in a relationships part.
    /// Every target is normalised against `base_dir`, so one that escapes
    /// the package is refused, but none is kept: a part can hold any number.
    fn model_relationship(
        &mut self,
        name: &str,
        base_dir: &str,
    ) -> Result<Option<String>, InspectError> {
        let mut found = None;
        self.parse(name, |event| {
            if let Event::Start(element) | Event::Empty(element) = event {
                if element.local_name().as_ref() == "Relationship" {
                    let target = attribute(&element, "Target", name)?.unwrap_or_default();
                    let target = normalize_part_path(base_dir, &target)?;
                    if found.is_none()
                        && attribute(&element, "Type", name)?.as_deref() == Some(MODEL_REL)
                    {
                        found = Some(target);
                    }
                }
            }
            Ok(())
        })?;
        Ok(found)
    }

    /// The start part named by the `3dmodel` relationship in `_rels/.rels`.
    fn start_part(&mut self) -> Result<String, InspectError> {
        let no_model = || InspectError::invalid("This 3MF has no 3D model part.");
        if !self.has(ROOT_RELS) {
            return Err(no_model());
        }
        let start = self
            .model_relationship(ROOT_RELS, "")?
            .ok_or_else(no_model)?;
        if self.has(&start) {
            Ok(start)
        } else {
            Err(no_model())
        }
    }

    fn model_part(&mut self, name: &str, is_start: bool) -> Result<ModelPart, InspectError> {
        let mut parser = ModelParser::new(name, is_start, self.limits, self.counts);
        self.parse(name, |event| parser.event(event))?;
        self.counts = parser.counts;
        Ok(parser.part)
    }

    /// Plates and per-object setting keys from `model_settings.config`.
    fn model_settings(&mut self) -> Result<(Vec<Plate>, BTreeSet<String>), InspectError> {
        let max_listed = self.limits.max_listed;
        let mut plates = Vec::new();
        let mut per_object_keys = BTreeSet::new();
        let mut object_depth = 0usize;
        let mut plate: Option<Plate> = None;
        let mut plate_id: Option<u32> = None;
        let mut in_instance = false;
        self.parse(MODEL_SETTINGS, |event| {
            let (element, is_start) = match &event {
                Event::Start(element) => (element, true),
                Event::Empty(element) => (element, false),
                Event::End(element) => {
                    match element.local_name().as_ref() {
                        "object" => object_depth = object_depth.saturating_sub(1),
                        "model_instance" => in_instance = false,
                        "plate" => {
                            if let Some(mut finished) = plate.take() {
                                finished.index = plate_id.unwrap_or(plates.len() as u32 + 1);
                                plates.push(finished);
                            }
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                _ => return Ok(()),
            };
            match element.local_name().as_ref() {
                "object" if is_start => object_depth += 1,
                // Plates past the listing cap are read but not kept.
                "plate" if is_start => {
                    plate = (plates.len() < max_listed).then(|| Plate {
                        index: 0,
                        name: None,
                        object_ids: Vec::new(),
                    });
                    plate_id = None;
                }
                "model_instance" if is_start => in_instance = true,
                "metadata" => {
                    let key = attribute(element, "key", MODEL_SETTINGS)?.unwrap_or_default();
                    let value = attribute(element, "value", MODEL_SETTINGS)?.unwrap_or_default();
                    let value = value.trim();
                    if object_depth > 0 {
                        if key != "name" && per_object_keys.len() < max_listed {
                            per_object_keys.insert(key);
                        }
                    } else if let Some(plate) = &mut plate {
                        match (in_instance, key.as_str()) {
                            (true, "object_id") if plate.object_ids.len() < max_listed => {
                                if let Ok(id) = value.parse() {
                                    plate.object_ids.push(id);
                                }
                            }
                            (false, "plater_id") => plate_id = value.parse().ok(),
                            (false, "plater_name") if !value.is_empty() => {
                                plate.name = Some(value.to_string());
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            Ok(())
        })?;
        Ok((plates, per_object_keys))
    }

    /// D12: `Thumbnail_Middle`, then `Metadata/thumbnail.png`, then
    /// `Metadata/plate_1.png`. Candidates that are absent (including a
    /// dangling thumbnail relationship) are ignored. Every readable PNG is
    /// listed; the first acceptable one is kept.
    fn thumbnails(&mut self, thumbnail_middle: Option<&str>) -> Result<Thumbnails, InspectError> {
        let mut candidates: Vec<String> = thumbnail_middle
            .and_then(|value| normalize_part_path("", value.trim()).ok())
            .into_iter()
            .collect();
        for part in THUMBNAIL_PARTS {
            if !candidates.iter().any(|candidate| candidate == part) {
                candidates.push(part.to_string());
            }
        }

        let mut listed = Vec::new();
        let mut chosen = None;
        let mut warnings = Vec::new();
        for part in candidates {
            if !self.has(&part) {
                continue;
            }
            let mut bytes = Vec::new();
            self.open(&part)?
                .take(MAX_THUMBNAIL_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| from_io(&error))?;
            if let Ok((width, height)) = png::dimensions(&bytes) {
                listed.push(ThumbnailInfo {
                    format: ThumbnailImageFormat::Png,
                    width,
                    height,
                    part: Some(part.clone()),
                    line: None,
                });
            }
            if chosen.is_none() {
                match png::accept_thumbnail(bytes, &part) {
                    Ok(thumbnail) => chosen = Some(thumbnail),
                    Err(warning) => warnings.push(warning),
                }
            }
        }
        Ok((listed, chosen, warnings))
    }
}

/// The unescaped value of the attribute whose local name is `local`.
fn attribute(
    element: &BytesStart<'_>,
    local: &str,
    part: &str,
) -> Result<Option<String>, InspectError> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|_| malformed(part))?;
        if attribute.key.local_name().as_ref() == local {
            let value = attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|_| malformed(part))?;
            return Ok(Some(value.into_owned()));
        }
    }
    Ok(None)
}

fn malformed(part: &str) -> InspectError {
    InspectError::invalid(format!("{part} isn't well-formed XML."))
}

// --- Model parts ------------------------------------------------------------

type Transform = [f64; 12];

const IDENTITY: Transform = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];

/// An object reference placed with a transform: a build item or a
/// component.
struct Placement {
    part: String,
    object_id: u32,
    transform: Transform,
}

enum Shape {
    Mesh {
        bounds: Option<BoundsMm>,
        triangles: u64,
    },
    Components(Vec<Placement>),
}

/// What one model part contributes. Only the start part's metadata, unit,
/// and build are used.
struct ModelPart {
    unit: String,
    title: Option<String>,
    application: Option<String>,
    thumbnail_middle: Option<String>,
    object_count: u32,
    objects: HashMap<u32, Shape>,
    build: Vec<Placement>,
    required: Vec<String>,
    materials: bool,
    paint: BTreeSet<String>,
}

impl ModelPart {
    /// Each part a component or build item names, once.
    fn referenced_parts(&self) -> BTreeSet<String> {
        let components = self.objects.values().flat_map(|shape| match shape {
            Shape::Components(list) => list.as_slice(),
            Shape::Mesh { .. } => &[],
        });
        let names: BTreeSet<&str> = components
            .chain(&self.build)
            .map(|placement| placement.part.as_str())
            .collect();
        names.into_iter().map(str::to_string).collect()
    }
}

/// Build items plus components, and objects, seen so far in the package.
#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    placements: usize,
    objects: usize,
}

struct ObjectInProgress {
    id: u32,
    bounds: Option<BoundsMm>,
    triangles: u64,
    components: Option<Vec<Placement>>,
}

struct ModelParser {
    name: String,
    limits: ZipLimits,
    counts: Counts,
    base_dir: String,
    is_start: bool,
    depth: usize,
    in_build: bool,
    object: Option<ObjectInProgress>,
    /// A `Title`/`Application`/`Thumbnail_Middle` element being read: its
    /// name and resolved text, capped at [`MAX_METADATA_BYTES`].
    metadata: Option<(String, String)>,
    part: ModelPart,
}

impl ModelParser {
    fn new(name: &str, is_start: bool, limits: &ZipLimits, counts: Counts) -> Self {
        Self {
            name: name.to_string(),
            limits: *limits,
            counts,
            base_dir: parent_dir(name).to_string(),
            is_start,
            depth: 0,
            in_build: false,
            object: None,
            metadata: None,
            part: ModelPart {
                unit: "millimeter".to_string(),
                title: None,
                application: None,
                thumbnail_middle: None,
                object_count: 0,
                objects: HashMap::new(),
                build: Vec::new(),
                required: Vec::new(),
                materials: false,
                paint: BTreeSet::new(),
            },
        }
    }

    fn event(&mut self, event: Event<'_>) -> Result<(), InspectError> {
        match event {
            Event::Start(element) => {
                self.element(&element, true)?;
                self.depth += 1;
            }
            Event::Empty(element) => self.element(&element, false)?,
            Event::End(element) => {
                self.depth = self.depth.saturating_sub(1);
                match element.local_name().as_ref() {
                    "object" => self.finish_object(),
                    "build" => self.in_build = false,
                    "metadata" => self.finish_metadata()?,
                    _ => {}
                }
            }
            // quick-xml 0.42 reports entity and character references as
            // separate events, so text runs hold no escapes.
            Event::Text(text) => {
                if let Some((_, value)) = &mut self.metadata {
                    push_capped(value, &text.into_inner());
                }
            }
            Event::GeneralRef(reference) => {
                if let Some((_, value)) = &mut self.metadata {
                    let mut utf8 = [0; 4];
                    let resolved = match reference.resolve_char_ref() {
                        Ok(Some(character)) => character.encode_utf8(&mut utf8),
                        Ok(None) => quick_xml::escape::resolve_predefined_entity(&reference)
                            .ok_or_else(|| malformed(&self.name))?,
                        Err(_) => return Err(malformed(&self.name)),
                    };
                    push_capped(value, resolved);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn element(&mut self, element: &BytesStart<'_>, has_body: bool) -> Result<(), InspectError> {
        let name = element.local_name();
        let local = name.as_ref();
        if MATERIAL_ELEMENTS.contains(&local) {
            self.part.materials = true;
        }
        match local {
            "model" if self.depth == 0 => self.model_attributes(element)?,
            "metadata" if self.depth == 1 && self.is_start && has_body => {
                let key = attribute(element, "name", &self.name)?.unwrap_or_default();
                if matches!(key.as_str(), "Title" | "Application" | "Thumbnail_Middle") {
                    self.metadata = Some((key, String::new()));
                }
            }
            "object" => {
                if self.object.is_some() {
                    return Err(malformed(&self.name));
                }
                self.counts.objects += 1;
                if self.counts.objects > self.limits.max_objects {
                    return Err(limit_exceeded(format!(
                        "it has more than {} objects",
                        self.limits.max_objects
                    )));
                }
                self.part.object_count += 1;
                self.object = Some(ObjectInProgress {
                    id: self.id_attribute(element, "id")?,
                    bounds: None,
                    triangles: 0,
                    components: None,
                });
                if !has_body {
                    self.finish_object();
                }
            }
            "vertex" => {
                let point = self.vertex(element)?;
                if let Some(object) = &mut self.object {
                    include_point(&mut object.bounds, point);
                }
            }
            "triangle" => {
                if let Some(object) = &mut self.object {
                    object.triangles += 1;
                }
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|_| malformed(&self.name))?;
                    if PAINT_ATTRIBUTES.contains(&attribute.key.local_name().as_ref()) {
                        let qualified: &str = attribute.key.as_ref();
                        if !self.part.paint.contains(qualified)
                            && self.part.paint.len() < self.limits.max_listed
                        {
                            self.part.paint.insert(qualified.to_string());
                        }
                    }
                }
            }
            "components" => {
                if let Some(object) = &mut self.object {
                    object.components.get_or_insert_with(Vec::new);
                }
            }
            "component" => {
                self.count_placement()?;
                let placement = self.placement(element)?;
                if let Some(object) = &mut self.object {
                    object
                        .components
                        .get_or_insert_with(Vec::new)
                        .push(placement);
                }
            }
            "build" => self.in_build = has_body,
            "item" if self.in_build => {
                self.count_placement()?;
                let placement = self.placement(element)?;
                self.part.build.push(placement);
            }
            _ => {}
        }
        Ok(())
    }

    fn count_placement(&mut self) -> Result<(), InspectError> {
        self.counts.placements += 1;
        if self.counts.placements > self.limits.max_placements {
            return Err(limit_exceeded(format!(
                "it has more than {} build items and components",
                self.limits.max_placements
            )));
        }
        Ok(())
    }

    /// `unit`, namespace declarations, and `requiredextensions` (D10).
    fn model_attributes(&mut self, element: &BytesStart<'_>) -> Result<(), InspectError> {
        let mut namespaces = HashMap::new();
        let mut required = Vec::new();
        for attribute in element.attributes() {
            let attribute = attribute.map_err(|_| malformed(&self.name))?;
            let key: &str = attribute.key.as_ref();
            let value = attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|_| malformed(&self.name))?;
            if let Some(prefix) = key.strip_prefix("xmlns:") {
                namespaces.insert(prefix.to_string(), value.into_owned());
            } else if key == "unit" {
                self.part.unit = value.trim().to_string();
            } else if key == "requiredextensions" {
                required = value.split_whitespace().map(str::to_string).collect();
            }
        }
        let mut unsupported = Vec::new();
        for prefix in &required {
            match namespaces.get(prefix).map(String::as_str) {
                Some(PRODUCTION_NS) => {}
                Some(MATERIALS_NS) => self.part.materials = true,
                _ => unsupported.push(prefix.clone()),
            }
        }
        if !unsupported.is_empty() {
            return Err(InspectError::UnsupportedFormat {
                reason: format!(
                    "This 3MF needs a required extension farm3d doesn't support: {}.",
                    unsupported.join(", ")
                ),
                extensions: unsupported,
            });
        }
        self.part.required = required;
        Ok(())
    }

    fn vertex(&self, element: &BytesStart<'_>) -> Result<[f64; 3], InspectError> {
        let mut point = [f64::NAN; 3];
        for attribute in element.attributes() {
            let attribute = attribute.map_err(|_| malformed(&self.name))?;
            let axis = match attribute.key.local_name().as_ref() {
                "x" => 0,
                "y" => 1,
                "z" => 2,
                _ => continue,
            };
            point[axis] = attribute.value.trim().parse().unwrap_or(f64::NAN);
        }
        if point.iter().all(|value| value.is_finite()) {
            Ok(point)
        } else {
            Err(InspectError::invalid(format!(
                "{} has a vertex with a missing or non-finite coordinate.",
                self.name
            )))
        }
    }

    fn id_attribute(&self, element: &BytesStart<'_>, local: &str) -> Result<u32, InspectError> {
        attribute(element, local, &self.name)?
            .and_then(|value| value.trim().parse().ok())
            .ok_or_else(|| {
                InspectError::invalid(format!(
                    "{} has an element without a valid {local}.",
                    self.name
                ))
            })
    }

    /// A component or build item: `objectid`, optional `p:path`, and an
    /// optional 3×4 `transform`.
    fn placement(&self, element: &BytesStart<'_>) -> Result<Placement, InspectError> {
        let object_id = self.id_attribute(element, "objectid")?;
        let part = match attribute(element, "path", &self.name)? {
            Some(path) => normalize_part_path(&self.base_dir, path.trim())?,
            None => self.name.clone(),
        };
        let transform = match attribute(element, "transform", &self.name)? {
            Some(text) => parse_transform(&text).ok_or_else(|| {
                InspectError::invalid(format!("{} has an invalid transform.", self.name))
            })?,
            None => IDENTITY,
        };
        Ok(Placement {
            part,
            object_id,
            transform,
        })
    }

    fn finish_object(&mut self) {
        if let Some(object) = self.object.take() {
            let shape = match object.components {
                Some(components) if object.triangles == 0 => Shape::Components(components),
                _ => Shape::Mesh {
                    bounds: object.bounds,
                    triangles: object.triangles,
                },
            };
            self.part.objects.insert(object.id, shape);
        }
    }

    fn finish_metadata(&mut self) -> Result<(), InspectError> {
        let Some((key, text)) = self.metadata.take() else {
            return Ok(());
        };
        let value = text.trim().to_string();
        let slot = match key.as_str() {
            "Title" => &mut self.part.title,
            "Application" => &mut self.part.application,
            _ => &mut self.part.thumbnail_middle,
        };
        if !value.is_empty() {
            *slot = Some(value);
        }
        Ok(())
    }
}

/// Appends `text` to `value` without growing it past
/// [`MAX_METADATA_BYTES`], cutting on a char boundary.
fn push_capped(value: &mut String, text: &str) {
    let room = MAX_METADATA_BYTES.saturating_sub(value.len());
    let mut end = text.len().min(room);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    value.push_str(&text[..end]);
}

fn parse_transform(text: &str) -> Option<Transform> {
    let mut transform = [0.0; 12];
    let mut values = text.split_whitespace();
    for slot in &mut transform {
        *slot = values
            .next()?
            .parse()
            .ok()
            .filter(|value: &f64| value.is_finite())?;
    }
    values.next().is_none().then_some(transform)
}

/// The corner method (D10): transforms the 8 corners of `bounds` with the
/// 3MF row-vector convention and returns their axis-aligned box.
fn transform_bounds(bounds: &BoundsMm, m: &Transform) -> BoundsMm {
    let mut result: Option<BoundsMm> = None;
    for corner in 0..8 {
        let x = if corner & 1 == 0 {
            bounds.min[0]
        } else {
            bounds.max[0]
        };
        let y = if corner & 2 == 0 {
            bounds.min[1]
        } else {
            bounds.max[1]
        };
        let z = if corner & 4 == 0 {
            bounds.min[2]
        } else {
            bounds.max[2]
        };
        include_point(
            &mut result,
            [
                x * m[0] + y * m[3] + z * m[6] + m[9],
                x * m[1] + y * m[4] + z * m[7] + m[10],
                x * m[2] + y * m[5] + z * m[8] + m[11],
            ],
        );
    }
    result.expect("eight corners were included")
}

/// Resolves placements to bounds and as-placed triangle counts, memoised
/// per object so shared components are computed once.
struct Resolver<'p> {
    parts: &'p HashMap<String, ModelPart>,
    placed: HashMap<(String, u32), (Option<BoundsMm>, u64)>,
    visiting: HashSet<(String, u32)>,
}

impl Resolver<'_> {
    fn place(
        &mut self,
        placement: &Placement,
        depth: usize,
    ) -> Result<(Option<BoundsMm>, u64), InspectError> {
        let (bounds, triangles) = self.object(&placement.part, placement.object_id, depth)?;
        Ok((
            bounds.map(|bounds| transform_bounds(&bounds, &placement.transform)),
            triangles,
        ))
    }

    fn object(
        &mut self,
        part: &str,
        id: u32,
        depth: usize,
    ) -> Result<(Option<BoundsMm>, u64), InspectError> {
        let key = (part.to_string(), id);
        if let Some(result) = self.placed.get(&key) {
            return Ok(*result);
        }
        if depth > MAX_COMPONENT_DEPTH || !self.visiting.insert(key.clone()) {
            return Err(InspectError::invalid(
                "This 3MF's components form a cycle or nest too deeply.",
            ));
        }
        let parts = self.parts;
        let shape = parts
            .get(part)
            .and_then(|model| model.objects.get(&id))
            .ok_or_else(|| {
                InspectError::invalid(format!(
                    "{part} references object {id}, which doesn't exist."
                ))
            })?;
        let result = match shape {
            Shape::Mesh { bounds, triangles } => (*bounds, *triangles),
            Shape::Components(components) => {
                let mut bounds = None;
                let mut triangles = 0u64;
                for component in components {
                    let (child_bounds, child_triangles) = self.place(component, depth + 1)?;
                    if let Some(child_bounds) = child_bounds {
                        include_bounds(&mut bounds, &child_bounds);
                    }
                    triangles = triangles.saturating_add(child_triangles);
                }
                (bounds, triangles)
            }
        };
        self.visiting.remove(&key);
        self.placed.insert(key, result);
        Ok(result)
    }
}

// --- Unsupported entries ----------------------------------------------------

/// D10's rich-3MF list: slicer metadata parts, per-object settings, paint
/// attributes, and material groups, sorted by part.
fn unsupported_entries(
    entry_names: &[String],
    parts: &HashMap<String, ModelPart>,
    per_object_keys: &BTreeSet<String>,
) -> Vec<UnsupportedEntry> {
    let mut entries: Vec<UnsupportedEntry> = entry_names
        .iter()
        .filter_map(|name| {
            let (code, detail) = classify_metadata_part(name)?;
            Some(UnsupportedEntry {
                part: name.clone(),
                code,
                detail: detail.to_string(),
            })
        })
        .collect();
    if !per_object_keys.is_empty() {
        let keys: Vec<&str> = per_object_keys.iter().map(String::as_str).collect();
        entries.push(UnsupportedEntry {
            part: MODEL_SETTINGS.to_string(),
            code: UnsupportedCode::PerObjectSettings,
            detail: format!("Per-object settings: {}", keys.join(", ")),
        });
    }
    for (name, part) in parts {
        if part.materials {
            entries.push(UnsupportedEntry {
                part: name.clone(),
                code: UnsupportedCode::Materials,
                detail: "Material and colour assignments".to_string(),
            });
        }
        for attribute in &part.paint {
            entries.push(UnsupportedEntry {
                part: name.clone(),
                code: UnsupportedCode::Paint,
                detail: format!("Paint attribute: {attribute}"),
            });
        }
    }
    entries.sort();
    entries
}

/// `Metadata/*.config|.gcode|.txt|.xml|.json`, other than
/// `model_settings.config` (whose plates are supported).
fn classify_metadata_part(name: &str) -> Option<(UnsupportedCode, &'static str)> {
    let file = name.strip_prefix("Metadata/")?;
    if name == MODEL_SETTINGS {
        return None;
    }
    let extension = file.rsplit_once('.')?.1.to_ascii_lowercase();
    if !["config", "gcode", "txt", "xml", "json"].contains(&extension.as_str()) {
        return None;
    }
    Some(if extension == "gcode" {
        (UnsupportedCode::EmbeddedGcode, "Embedded sliced G-code")
    } else if file.to_ascii_lowercase().contains("layer_height") {
        (UnsupportedCode::LayerHeightProfile, "Layer-height profile")
    } else if file == "project_settings.config"
        || (file.starts_with("Slic3r_PE") && extension == "config")
    {
        (UnsupportedCode::SlicerSettings, "Slicer settings")
    } else {
        (UnsupportedCode::SlicerMetadata, "Slicer metadata")
    })
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use std::cell::Cell;
    use std::io::SeekFrom;
    use std::rc::Rc;

    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    use super::*;
    use crate::library::formats::png::tests::png_header;
    use crate::library::formats::{
        BoundsMm, Plate, ThumbnailImageFormat, ThumbnailInfo, UnsupportedCode, UnsupportedEntry,
    };

    const CORE_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
    const PRODUCTION_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/production/2015/06";
    const MATERIALS_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/material/2015/02";
    const BEAM_LATTICE_NS: &str =
        "http://schemas.microsoft.com/3dmanufacturing/beamlattice/2017/02";
    const MODEL_REL: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
    const THUMBNAIL_REL: &str =
        "http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail";

    const CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
        <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
        <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
        <Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/>\
        </Types>";

    fn rels(entries: &[(&str, &str)]) -> String {
        let mut out = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
             <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
        );
        for (index, (target, kind)) in entries.iter().enumerate() {
            out.push_str(&format!(
                "<Relationship Id=\"rel{index}\" Target=\"{target}\" Type=\"{kind}\"/>"
            ));
        }
        out.push_str("</Relationships>");
        out
    }

    /// The 10 mm cube as a 3MF mesh: 8 vertices, 12 triangles. `triangle_extra`
    /// is added to the first triangle's attributes.
    fn cube_mesh(triangle_extra: &str) -> String {
        let vertices = [
            (0, 0, 0),
            (10, 0, 0),
            (10, 10, 0),
            (0, 10, 0),
            (0, 0, 10),
            (10, 0, 10),
            (10, 10, 10),
            (0, 10, 10),
        ];
        let triangles = [
            (0, 2, 1),
            (0, 3, 2),
            (4, 5, 6),
            (4, 6, 7),
            (0, 1, 5),
            (0, 5, 4),
            (3, 7, 6),
            (3, 6, 2),
            (0, 4, 7),
            (0, 7, 3),
            (1, 2, 6),
            (1, 6, 5),
        ];
        let mut out = String::from("<mesh><vertices>");
        for (x, y, z) in vertices {
            out.push_str(&format!("<vertex x=\"{x}\" y=\"{y}\" z=\"{z}\"/>"));
        }
        out.push_str("</vertices><triangles>");
        for (index, (a, b, c)) in triangles.iter().enumerate() {
            let extra = if index == 0 { triangle_extra } else { "" };
            out.push_str(&format!(
                "<triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\"{extra}/>"
            ));
        }
        out.push_str("</triangles></mesh>");
        out
    }

    fn model(attributes: &str, body: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
             <model unit=\"millimeter\" xmlns=\"{CORE_NS}\"{attributes}>{body}</model>"
        )
    }

    fn single_cube_model(attributes: &str, extra_resources: &str) -> String {
        model(
            attributes,
            &format!(
                "<resources>{extra_resources}<object id=\"1\" type=\"model\">{}</object></resources>\
                 <build><item objectid=\"1\"/></build>",
                cube_mesh("")
            ),
        )
    }

    fn package(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn core_package(model_xml: String, extra: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
        let mut entries = vec![
            ("[Content_Types].xml", CONTENT_TYPES.as_bytes().to_vec()),
            (
                "_rels/.rels",
                rels(&[("/3D/3dmodel.model", MODEL_REL)]).into_bytes(),
            ),
            ("3D/3dmodel.model", model_xml.into_bytes()),
        ];
        entries.extend(extra);
        package(&entries)
    }

    fn inspect_bytes(bytes: Vec<u8>) -> Result<Inspected, InspectError> {
        inspect_with(bytes, &ZipLimits::SPEC)
    }

    fn inspect_with(bytes: Vec<u8>, limits: &ZipLimits) -> Result<Inspected, InspectError> {
        inspect_reader(Cursor::new(bytes), &CancelFlag::never(), limits)
    }

    fn invalid_reason(result: Result<Inspected, InspectError>) -> String {
        match result {
            Err(InspectError::InvalidContent(reason)) => reason,
            other => panic!("expected INVALID_CONTENT, got {other:?}"),
        }
    }

    #[test]
    fn part_paths_normalise_within_the_package() {
        assert_eq!(
            normalize_part_path("", "/3D/3dmodel.model").unwrap(),
            "3D/3dmodel.model"
        );
        assert_eq!(
            normalize_part_path("3D", "Objects/object_1.model").unwrap(),
            "3D/Objects/object_1.model"
        );
        assert_eq!(
            normalize_part_path("3D", "./../Metadata/./plate_1.png").unwrap(),
            "Metadata/plate_1.png"
        );
        assert_eq!(
            normalize_part_path("3D/Objects", "/3D//a.model").unwrap(),
            "3D/a.model"
        );
    }

    #[test]
    fn part_paths_that_escape_the_package_are_rejected() {
        for (base, target) in [
            ("", "/../../etc/passwd"),
            ("", "../x.model"),
            ("3D", "../../x.model"),
            ("3D", "/3D/../../x.model"),
            ("", "/"),
        ] {
            let error = normalize_part_path(base, target).unwrap_err();
            assert_eq!(error.code(), "INVALID_CONTENT", "{base} + {target}");
        }
        let InspectError::InvalidContent(reason) =
            normalize_part_path("", "/../../etc/passwd").unwrap_err()
        else {
            unreachable!()
        };
        assert!(reason.contains("escapes the package"), "{reason}");
    }

    #[test]
    fn reads_a_core_model_with_transforms_and_units() {
        // Object 1 rotated 90° about Z, then moved by (5, 0, 0), in centimetres.
        let body = format!(
            "<metadata name=\"Title\">Rotated &amp; scaled</metadata>\
             <metadata name=\"Application\">Test Suite 1.0</metadata>\
             <resources><object id=\"1\" type=\"model\">{}</object></resources>\
             <build><item objectid=\"1\" transform=\"0 1 0 -1 0 0 0 0 1 5 0 0\"/></build>",
            cube_mesh("")
        );
        let xml = model("", &body).replace("millimeter", "centimeter");
        let (inspection, thumbnail, warnings) = inspect_bytes(core_package(xml, vec![])).unwrap();
        assert_eq!(inspection.unit, "centimeter");
        assert_eq!(inspection.title.as_deref(), Some("Rotated & scaled"));
        assert_eq!(inspection.producer.as_deref(), Some("Test Suite 1.0"));
        assert_eq!(inspection.object_count, 1);
        assert_eq!(inspection.build_item_count, 1);
        assert_eq!(inspection.triangle_count, 12);
        // Row vectors: (x, y, z) -> (-y + 5, x, z), so x spans -5..5 cm.
        assert_eq!(
            inspection.bounds_mm,
            BoundsMm {
                min: [-50.0, 0.0, 0.0],
                max: [50.0, 100.0, 100.0]
            }
        );
        assert_eq!(thumbnail, None);
        assert!(warnings.is_empty());
    }

    #[test]
    fn required_materials_reads_geometry_and_reports_materials() {
        let xml = single_cube_model(
            &format!(" xmlns:m=\"{MATERIALS_NS}\" requiredextensions=\"m\""),
            "<m:colorgroup id=\"5\"><m:color color=\"#FF0000\"/></m:colorgroup>",
        );
        let (inspection, _, _) = inspect_bytes(core_package(xml, vec![])).unwrap();
        assert_eq!(inspection.triangle_count, 12);
        assert_eq!(inspection.required_extensions, vec!["m".to_string()]);
        assert_eq!(
            inspection.unsupported,
            vec![UnsupportedEntry {
                part: "3D/3dmodel.model".into(),
                code: UnsupportedCode::Materials,
                detail: "Material and colour assignments".into(),
            }]
        );
    }

    #[test]
    fn required_beam_lattice_is_unsupported_format() {
        let xml = single_cube_model(
            &format!(" xmlns:b=\"{BEAM_LATTICE_NS}\" requiredextensions=\"b\""),
            "",
        );
        match inspect_bytes(core_package(xml, vec![])) {
            Err(InspectError::UnsupportedFormat { reason, extensions }) => {
                assert_eq!(extensions, vec!["b".to_string()]);
                assert!(reason.contains("required extension"), "{reason}");
            }
            other => panic!("expected UNSUPPORTED_FORMAT, got {other:?}"),
        }
    }

    /// OrcaSlicer's layout: the start part requires `p`, each object's mesh
    /// lives in its own part named through `3D/_rels/3dmodel.model.rels`,
    /// and `Metadata/model_settings.config` groups objects into plates.
    fn orca_layout() -> Vec<u8> {
        let root = model(
            &format!(" xmlns:p=\"{PRODUCTION_NS}\" requiredextensions=\"p\""),
            "<metadata name=\"Application\">OrcaSlicer-2.3.0</metadata>\
             <metadata name=\"Thumbnail_Middle\">/Metadata/plate_1.png</metadata>\
             <resources>\
               <object id=\"2\" p:UUID=\"00000001-0000-0000-0000-000000000000\" type=\"model\">\
                 <components><component p:path=\"/3D/Objects/object_1.model\" objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 0 0 0\"/></components>\
               </object>\
               <object id=\"4\" type=\"model\">\
                 <components><component p:path=\"/3D/Objects/object_2.model\" objectid=\"3\"/></components>\
               </object>\
             </resources>\
             <build>\
               <item objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 100 100 0\" printable=\"1\"/>\
               <item objectid=\"4\" transform=\"1 0 0 0 1 0 0 0 1 50 50 0\" printable=\"1\"/>\
             </build>",
        );
        let object_1 = model(
            &format!(" xmlns:p=\"{PRODUCTION_NS}\""),
            &format!(
                "<resources><object id=\"1\" type=\"model\">{}</object></resources><build/>",
                cube_mesh(" paint_color=\"4\"")
            ),
        );
        let object_2 = model(
            "",
            &format!(
                "<resources><object id=\"3\" type=\"model\">{}</object></resources><build/>",
                cube_mesh("")
            ),
        );
        let settings = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><config>\
            <object id=\"2\"><metadata key=\"name\" value=\"Cube\"/><metadata key=\"extruder\" value=\"1\"/>\
              <part id=\"1\" subtype=\"normal_part\"><metadata key=\"name\" value=\"Cube\"/></part></object>\
            <object id=\"4\"><metadata key=\"name\" value=\"Cube\"/></object>\
            <plate><metadata key=\"plater_id\" value=\"1\"/><metadata key=\"plater_name\" value=\"\"/>\
              <model_instance><metadata key=\"object_id\" value=\"2\"/><metadata key=\"instance_id\" value=\"0\"/></model_instance></plate>\
            <plate><metadata key=\"plater_id\" value=\"2\"/><metadata key=\"plater_name\" value=\"Second\"/>\
              <model_instance><metadata key=\"object_id\" value=\"4\"/><metadata key=\"instance_id\" value=\"0\"/></model_instance></plate>\
            </config>";
        package(&[
            ("[Content_Types].xml", CONTENT_TYPES.as_bytes().to_vec()),
            (
                "_rels/.rels",
                rels(&[("/3D/3dmodel.model", MODEL_REL)]).into_bytes(),
            ),
            ("3D/3dmodel.model", root.into_bytes()),
            (
                "3D/_rels/3dmodel.model.rels",
                rels(&[
                    ("/3D/Objects/object_1.model", MODEL_REL),
                    ("/3D/Objects/object_2.model", MODEL_REL),
                ])
                .into_bytes(),
            ),
            ("3D/Objects/object_1.model", object_1.into_bytes()),
            ("3D/Objects/object_2.model", object_2.into_bytes()),
            (
                "Metadata/model_settings.config",
                settings.as_bytes().to_vec(),
            ),
            ("Metadata/project_settings.config", b"{}".to_vec()),
            ("Metadata/plate_1.gcode", b"G28\n".to_vec()),
            ("Metadata/plate_1.png", png_header(2, 2)),
        ])
    }

    #[test]
    fn reads_orca_production_parts_plates_and_unsupported_settings() {
        let (inspection, thumbnail, warnings) = inspect_bytes(orca_layout()).unwrap();
        assert_eq!(inspection.producer.as_deref(), Some("OrcaSlicer-2.3.0"));
        assert_eq!(inspection.object_count, 2);
        assert_eq!(inspection.build_item_count, 2);
        assert_eq!(inspection.triangle_count, 24);
        assert_eq!(
            inspection.bounds_mm,
            BoundsMm {
                min: [50.0, 50.0, 0.0],
                max: [110.0, 110.0, 10.0]
            }
        );
        assert_eq!(inspection.required_extensions, vec!["p".to_string()]);
        assert_eq!(
            inspection.plates,
            vec![
                Plate {
                    index: 1,
                    name: None,
                    object_ids: vec![2]
                },
                Plate {
                    index: 2,
                    name: Some("Second".into()),
                    object_ids: vec![4]
                },
            ]
        );
        let unsupported: Vec<_> = inspection
            .unsupported
            .iter()
            .map(|entry| (entry.part.as_str(), entry.code, entry.detail.as_str()))
            .collect();
        assert_eq!(
            unsupported,
            vec![
                (
                    "3D/Objects/object_1.model",
                    UnsupportedCode::Paint,
                    "Paint attribute: paint_color"
                ),
                (
                    "Metadata/model_settings.config",
                    UnsupportedCode::PerObjectSettings,
                    "Per-object settings: extruder"
                ),
                (
                    "Metadata/plate_1.gcode",
                    UnsupportedCode::EmbeddedGcode,
                    "Embedded sliced G-code"
                ),
                (
                    "Metadata/project_settings.config",
                    UnsupportedCode::SlicerSettings,
                    "Slicer settings"
                ),
            ]
        );
        assert_eq!(
            inspection.thumbnails,
            vec![ThumbnailInfo {
                format: ThumbnailImageFormat::Png,
                width: 2,
                height: 2,
                part: Some("Metadata/plate_1.png".into()),
                line: None,
            }]
        );
        let thumbnail = thumbnail.unwrap();
        assert_eq!(thumbnail.origin_part, "Metadata/plate_1.png");
        assert_eq!(thumbnail.bytes, png_header(2, 2));
        assert!(warnings.is_empty());
    }

    #[test]
    fn thumbnail_order_prefers_thumbnail_middle_then_thumbnail_png() {
        let xml = single_cube_model("", "").replace(
            "<resources>",
            "<metadata name=\"Thumbnail_Middle\">/Metadata/middle.png</metadata><resources>",
        );
        let (inspection, thumbnail, _) = inspect_bytes(core_package(
            xml,
            vec![
                ("Metadata/plate_1.png", png_header(3, 3)),
                ("Metadata/thumbnail.png", png_header(2, 2)),
                ("Metadata/middle.png", png_header(4, 4)),
            ],
        ))
        .unwrap();
        assert_eq!(thumbnail.unwrap().origin_part, "Metadata/middle.png");
        let parts: Vec<_> = inspection
            .thumbnails
            .iter()
            .map(|info| info.part.clone().unwrap())
            .collect();
        assert_eq!(
            parts,
            vec![
                "Metadata/middle.png",
                "Metadata/thumbnail.png",
                "Metadata/plate_1.png"
            ]
        );
    }

    #[test]
    fn an_oversized_or_broken_thumbnail_falls_through_with_a_warning() {
        let (_, thumbnail, warnings) = inspect_bytes(core_package(
            single_cube_model("", ""),
            vec![
                ("Metadata/thumbnail.png", png_header(2000, 10)),
                ("Metadata/plate_1.png", png_header(8, 8)),
            ],
        ))
        .unwrap();
        assert_eq!(thumbnail.unwrap().origin_part, "Metadata/plate_1.png");
        assert_eq!(warnings.len(), 1);
        assert_eq!(
            warnings[0].code,
            crate::library::ImportWarningCode::ThumbnailSkipped
        );
    }

    #[test]
    fn a_dangling_thumbnail_relationship_is_no_thumbnail() {
        let bytes = package(&[
            ("[Content_Types].xml", CONTENT_TYPES.as_bytes().to_vec()),
            (
                "_rels/.rels",
                rels(&[
                    ("/3D/3dmodel.model", MODEL_REL),
                    ("/Metadata/thumbnail.png", THUMBNAIL_REL),
                ])
                .into_bytes(),
            ),
            ("3D/3dmodel.model", single_cube_model("", "").into_bytes()),
        ]);
        let (inspection, thumbnail, warnings) = inspect_bytes(bytes).unwrap();
        assert!(inspection.thumbnails.is_empty());
        assert_eq!(thumbnail, None);
        assert!(warnings.is_empty());
    }

    #[test]
    fn a_missing_start_part_or_object_is_invalid_content() {
        let no_model = package(&[
            ("[Content_Types].xml", CONTENT_TYPES.as_bytes().to_vec()),
            ("_rels/.rels", rels(&[]).into_bytes()),
        ]);
        invalid_reason(inspect_bytes(no_model));

        let dangling = model(
            "",
            &format!(
                "<resources><object id=\"1\" type=\"model\">{}</object></resources>\
                 <build><item objectid=\"9\"/></build>",
                cube_mesh("")
            ),
        );
        let reason = invalid_reason(inspect_bytes(core_package(dangling, vec![])));
        assert!(reason.contains('9'), "{reason}");
    }

    #[test]
    fn a_component_cycle_is_invalid_content() {
        let cycle = model(
            "",
            "<resources>\
               <object id=\"1\"><components><component objectid=\"2\"/></components></object>\
               <object id=\"2\"><components><component objectid=\"1\"/></components></object>\
             </resources><build><item objectid=\"1\"/></build>",
        );
        invalid_reason(inspect_bytes(core_package(cycle, vec![])));
    }

    #[test]
    fn a_non_finite_vertex_is_invalid_content() {
        let xml = single_cube_model("", "").replacen("x=\"10\"", "x=\"NaN\"", 1);
        invalid_reason(inspect_bytes(core_package(xml, vec![])));
    }

    #[test]
    fn the_entry_count_limit_is_enforced() {
        let limits = ZipLimits {
            max_entries: 3,
            ..ZipLimits::SPEC
        };
        let bytes = core_package(
            single_cube_model("", ""),
            vec![("Metadata/extra.txt", b"x".to_vec())],
        );
        let reason = invalid_reason(inspect_with(bytes, &limits));
        assert!(reason.contains("entries"), "{reason}");
    }

    #[test]
    fn the_compression_ratio_limit_is_enforced() {
        let limits = ZipLimits {
            max_ratio: 20,
            ..ZipLimits::SPEC
        };
        let padded = single_cube_model("", "").replace(
            "<resources>",
            &format!("{}<resources>", " ".repeat(200_000)),
        );
        let reason = invalid_reason(inspect_with(core_package(padded, vec![]), &limits));
        assert!(reason.contains("ratio"), "{reason}");
    }

    #[test]
    fn the_total_and_per_entry_size_limits_are_enforced() {
        let bytes = core_package(single_cube_model("", ""), vec![]);
        let total = ZipLimits {
            max_total_bytes: 600,
            ..ZipLimits::SPEC
        };
        let reason = invalid_reason(inspect_with(bytes.clone(), &total));
        assert!(reason.contains("in total"), "{reason}");
        let per_entry = ZipLimits {
            max_entry_bytes: 600,
            ..ZipLimits::SPEC
        };
        let reason = invalid_reason(inspect_with(bytes, &per_entry));
        assert!(reason.contains("entry"), "{reason}");
    }

    #[test]
    fn cancellation_stops_the_read() {
        let (sender, receiver) = tokio::sync::watch::channel(false);
        sender.send(true).unwrap();
        let result = inspect_reader(
            Cursor::new(orca_layout()),
            &CancelFlag::new(receiver),
            &ZipLimits::SPEC,
        );
        assert_eq!(result.unwrap_err(), InspectError::Cancelled);
    }

    // --- Central-directory guard (review I1) --------------------------------

    /// Overwrites the EOCD's two entry counts (it is the last 22 bytes when
    /// there is no comment).
    fn with_declared_entries(mut bytes: Vec<u8>, count: u16) -> Vec<u8> {
        let eocd = bytes.len() - 22;
        assert_eq!(&bytes[eocd..eocd + 4], b"PK\x05\x06");
        bytes[eocd + 8..eocd + 10].copy_from_slice(&count.to_le_bytes());
        bytes[eocd + 10..eocd + 12].copy_from_slice(&count.to_le_bytes());
        bytes
    }

    #[test]
    fn a_declared_entry_count_over_the_limit_is_rejected_before_parsing() {
        let bytes = with_declared_entries(core_package(single_cube_model("", ""), vec![]), 60_000);
        let reason = invalid_reason(inspect_bytes(bytes));
        assert!(reason.contains("entries"), "{reason}");
    }

    /// A ZIP64 end record claiming 20 million entries, as a sub-1 GiB file
    /// could. zip 8.6 would reserve a `Vec` for all of them before reading
    /// a single record; the guard must refuse it first.
    #[test]
    fn a_zip64_directory_claiming_millions_of_entries_is_rejected() {
        let mut bytes = b"PK\x03\x04".to_vec();
        bytes.resize(4096, 0);
        let eocd64_offset = bytes.len() as u64;
        bytes.extend_from_slice(b"PK\x06\x06");
        bytes.extend_from_slice(&44u64.to_le_bytes());
        bytes.extend_from_slice(&[45, 0, 45, 0]);
        bytes.extend_from_slice(&[0; 8]);
        bytes.extend_from_slice(&20_000_000u64.to_le_bytes());
        bytes.extend_from_slice(&20_000_000u64.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(b"PK\x06\x07");
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&eocd64_offset.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(b"PK\x05\x06");
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&[0xff; 4]);
        bytes.extend_from_slice(&[0xff; 8]);
        bytes.extend_from_slice(&[0; 2]);
        let started = std::time::Instant::now();
        let reason = invalid_reason(inspect_bytes(bytes));
        assert!(reason.contains("entries"), "{reason}");
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn a_central_directory_larger_than_the_limit_is_rejected() {
        let limits = ZipLimits {
            max_entries: 4,
            ..ZipLimits::SPEC
        };
        let long_name = format!("Metadata/{}.txt", "n".repeat(5000));
        let bytes = core_package(
            single_cube_model("", ""),
            vec![(long_name.as_str(), b"x".to_vec())],
        );
        let reason = invalid_reason(inspect_with(bytes, &limits));
        assert!(reason.contains("directory"), "{reason}");
    }

    /// A second end record hidden in the directory is what zip's retry
    /// would find after the real one fails, so it is refused up front.
    #[test]
    fn an_end_record_signature_inside_the_directory_is_rejected() {
        let bytes = core_package(
            single_cube_model("", ""),
            vec![("Metadata/PK\u{5}\u{6}.txt", b"x".to_vec())],
        );
        invalid_reason(inspect_bytes(bytes));
    }

    #[test]
    fn the_guarded_view_zeroes_bytes_outside_the_directory_and_caps_reads() {
        let data: Vec<u8> = (1..=100).collect();
        let guard = Rc::new(Cell::new(Some(DirectoryGuard {
            start: 40,
            end: 60,
            budget: 50,
        })));
        let mut view = GuardedReader::new(Cursor::new(data), guard.clone()).unwrap();
        let mut buffer = [0xaau8; 30];
        view.seek(SeekFrom::Start(20)).unwrap();
        view.read_exact(&mut buffer[..20]).unwrap();
        assert_eq!(
            &buffer[..20],
            &[0u8; 20],
            "before the directory reads as zeros"
        );
        let mut inside = [0u8; 20];
        view.read_exact(&mut inside).unwrap();
        assert_eq!(inside.to_vec(), (41..=60).collect::<Vec<u8>>());
        let error = view.read_exact(&mut buffer).unwrap_err();
        assert!(error.get_ref().is_some_and(|inner| inner.is::<Stop>()));

        guard.set(None);
        view.seek(SeekFrom::Start(0)).unwrap();
        let mut all = Vec::new();
        view.read_to_end(&mut all).unwrap();
        assert_eq!(all, (1..=100).collect::<Vec<u8>>());
    }

    // --- Per-event XML bound (review I2) -------------------------------------

    fn titled_model(title: &str) -> String {
        single_cube_model("", "").replace(
            "<resources>",
            &format!("<metadata name=\"Title\">{title}</metadata><resources>"),
        )
    }

    #[test]
    fn an_oversized_xml_event_is_rejected() {
        let limits = ZipLimits {
            max_event_bytes: 16 * 1024,
            ..ZipLimits::SPEC
        };
        let bytes = core_package(titled_model(&"t".repeat(100_000)), vec![]);
        let reason = invalid_reason(inspect_with(bytes, &limits));
        assert!(reason.contains("XML"), "{reason}");
    }

    #[test]
    fn retained_metadata_text_is_capped() {
        let title = format!("{}&amp;{}", "t".repeat(3000), "é".repeat(3000));
        let (inspection, _, _) = inspect_bytes(core_package(titled_model(&title), vec![])).unwrap();
        let kept = inspection.title.unwrap();
        // 3,000 + 1 ASCII bytes, then as many 2-byte `é` as fit whole.
        assert_eq!(kept.len(), MAX_METADATA_BYTES - 1);
        assert!(kept.starts_with(&format!("{}&é", "t".repeat(3000))));
    }

    // --- Per-element collection bounds (final review I1) ----------------------

    fn listing_limits(max_listed: usize) -> ZipLimits {
        ZipLimits {
            max_listed,
            ..ZipLimits::SPEC
        }
    }

    #[test]
    fn the_build_item_and_component_count_limit_is_enforced() {
        let limits = ZipLimits {
            max_placements: 3,
            ..ZipLimits::SPEC
        };
        let items = "<item objectid=\"1\"/>".repeat(4);
        let xml = single_cube_model("", "").replace(
            "<build><item objectid=\"1\"/></build>",
            &format!("<build>{items}</build>"),
        );
        let reason = invalid_reason(inspect_with(core_package(xml, vec![]), &limits));
        assert!(reason.contains("safety limit"), "{reason}");
        assert!(reason.contains("build items and components"), "{reason}");

        // Components count toward the same cap: 2 components + 2 items.
        let components = "<component objectid=\"1\"/>".repeat(2);
        let xml = single_cube_model(
            "",
            "",
        )
        .replace(
            "</resources>",
            &format!(
                "<object id=\"2\" type=\"model\"><components>{components}</components></object></resources>"
            ),
        )
        .replace(
            "<build><item objectid=\"1\"/></build>",
            "<build><item objectid=\"2\"/><item objectid=\"2\"/></build>",
        );
        let reason = invalid_reason(inspect_with(core_package(xml, vec![]), &limits));
        assert!(reason.contains("build items and components"), "{reason}");
    }

    #[test]
    fn the_object_count_limit_is_enforced() {
        let limits = ZipLimits {
            max_objects: 2,
            ..ZipLimits::SPEC
        };
        let extra = "<object id=\"7\" type=\"model\"/><object id=\"8\" type=\"model\"/>";
        let xml = single_cube_model("", extra);
        let reason = invalid_reason(inspect_with(core_package(xml, vec![]), &limits));
        assert!(reason.contains("safety limit"), "{reason}");
        assert!(reason.contains("objects"), "{reason}");
    }

    fn plate_settings(plates: usize, ids_per_plate: usize, object_keys: &[&str]) -> Vec<u8> {
        let mut xml =
            String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><config><object id=\"1\">");
        for key in object_keys {
            xml.push_str(&format!("<metadata key=\"{key}\" value=\"1\"/>"));
        }
        xml.push_str("</object>");
        for plate in 1..=plates {
            xml.push_str(&format!(
                "<plate><metadata key=\"plater_id\" value=\"{plate}\"/>"
            ));
            for id in 1..=ids_per_plate {
                xml.push_str(&format!(
                    "<model_instance><metadata key=\"object_id\" value=\"{id}\"/></model_instance>"
                ));
            }
            xml.push_str("</plate>");
        }
        xml.push_str("</config>");
        xml.into_bytes()
    }

    #[test]
    fn plates_and_their_object_ids_are_listed_up_to_the_cap() {
        let bytes = core_package(
            single_cube_model("", ""),
            vec![("Metadata/model_settings.config", plate_settings(3, 3, &[]))],
        );
        let (inspection, _, _) = inspect_with(bytes, &listing_limits(2)).unwrap();
        assert_eq!(
            inspection.plates,
            vec![
                Plate {
                    index: 1,
                    name: None,
                    object_ids: vec![1, 2]
                },
                Plate {
                    index: 2,
                    name: None,
                    object_ids: vec![1, 2]
                },
            ]
        );
    }

    #[test]
    fn per_object_setting_keys_are_listed_up_to_the_cap() {
        let bytes = core_package(
            single_cube_model("", ""),
            vec![(
                "Metadata/model_settings.config",
                plate_settings(0, 0, &["a_key", "b_key", "c_key"]),
            )],
        );
        let (inspection, _, _) = inspect_with(bytes, &listing_limits(2)).unwrap();
        let details: Vec<_> = inspection
            .unsupported
            .iter()
            .filter(|entry| entry.code == UnsupportedCode::PerObjectSettings)
            .map(|entry| entry.detail.as_str())
            .collect();
        assert_eq!(details, vec!["Per-object settings: a_key, b_key"]);
    }

    #[test]
    fn paint_attributes_are_listed_up_to_the_cap() {
        let xml = model(
            "",
            &format!(
                "<resources><object id=\"1\" type=\"model\">{}</object></resources>\
                 <build><item objectid=\"1\"/></build>",
                cube_mesh(" paint_color=\"4\" paint_seam=\"1\" paint_supports=\"2\"")
            ),
        );
        let (inspection, _, _) =
            inspect_with(core_package(xml, vec![]), &listing_limits(2)).unwrap();
        let paint = inspection
            .unsupported
            .iter()
            .filter(|entry| entry.code == UnsupportedCode::Paint)
            .count();
        assert_eq!(paint, 2);
    }

    #[test]
    fn referenced_parts_are_deduplicated() {
        let items = "<item objectid=\"1\" p:path=\"/3D/Objects/o.model\"/>".repeat(5);
        let xml = model(
            &format!(" xmlns:p=\"{PRODUCTION_NS}\""),
            &format!("<resources/><build>{items}</build>"),
        );
        let mut parser = ModelParser::new(
            "3D/3dmodel.model",
            true,
            &ZipLimits::SPEC,
            Counts::default(),
        );
        let mut reader = quick_xml::Reader::from_str(&xml);
        loop {
            match reader.read_event().unwrap() {
                Event::Eof => break,
                event => parser.event(event).unwrap(),
            }
        }
        assert_eq!(parser.part.build.len(), 5);
        assert_eq!(
            parser
                .part
                .referenced_parts()
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["3D/Objects/o.model".to_string()]
        );
    }
}
