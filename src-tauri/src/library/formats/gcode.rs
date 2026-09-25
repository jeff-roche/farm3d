//! D11: a streaming G-code line scanner.
//!
//! One pass over the bytes, one bounded line at a time. Each line is
//! decoded as UTF-8, falling back to Latin-1. Slicer metadata is kept only
//! as verbatim, allowlisted claims; nothing is interpreted as a Printer,
//! nozzle, or material fact.
//!
//! Everything retained is bounded: one claim per allowlisted key (the first
//! value wins, capped at [`MAX_CLAIM_VALUE_BYTES`]), and at most
//! [`MAX_LISTED`] tools and thumbnail blocks. Because a key can only ever
//! keep one value, claims are collected from any full-line comment rather
//! than only the header and trailing config blocks, which D11 accepts.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use base64::Engine;

use super::png::{self, MAX_THUMBNAIL_BYTES};
use super::{
    BoundsMm, GcodeClaim, GcodeInspection, InspectError, Producer, ThumbnailBytes,
    ThumbnailImageFormat, ThumbnailInfo,
};
use crate::library::content::CancelFlag;
use crate::library::{ImportWarning, ImportWarningCode};

/// D11: longer lines are counted and skipped with `LONG_LINE`.
const MAX_LINE: usize = 64 * 1024;
/// Lines between cancellation checks.
const CANCEL_STRIDE: u64 = 4096;
/// The base64 text of the largest PNG a block may carry (D12's 1 MiB).
const MAX_THUMBNAIL_BASE64: usize = MAX_THUMBNAIL_BYTES.div_ceil(3) * 4;

/// A claim value longer than this is cut on a char boundary.
pub(crate) const MAX_CLAIM_VALUE_BYTES: usize = 1024;
/// The most tools and thumbnail blocks listed.
pub(crate) const MAX_LISTED: usize = 64;

/// D11's claim allowlist, exactly. Any other key is ignored and not stored.
const CLAIM_KEYS: [&str; 16] = [
    "printer_model",
    "printer_settings_id",
    "nozzle_diameter",
    "filament_type",
    "filament_settings_id",
    "layer_height",
    "filament used [g]",
    "filament used [mm]",
    "estimated printing time (normal mode)",
    "total layer number",
    "max_z_height",
    "bed_temperature",
    "FLAVOR",
    "TIME",
    "Filament used",
    "Layer height",
];

type Inspected = (GcodeInspection, Option<ThumbnailBytes>, Vec<ImportWarning>);

/// D8's G-code rule over the first 64 KiB: text, with at least one command
/// or `;` comment line.
pub fn looks_like_gcode(head: &[u8]) -> bool {
    is_text(head)
        && head.split(|&byte| byte == b'\n').any(|line| {
            let line = decode(line);
            let line = line.trim_start();
            line.starts_with(';') || Command::parse(line).is_some()
        })
}

/// UTF-8 or Latin-1 text: every byte decodes under Latin-1, so this only
/// rules out control bytes other than tab, line feed, form feed, and
/// carriage return.
fn is_text(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .all(|&byte| byte >= 0x20 || matches!(byte, b'\t' | b'\n' | 0x0c | b'\r'))
}

fn decode(line: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(line) {
        Ok(text) => Cow::Borrowed(text),
        Err(_) => Cow::Owned(line.iter().map(|&byte| char::from(byte)).collect()),
    }
}

pub fn inspect(path: &Path, cancel: &CancelFlag) -> Result<Inspected, InspectError> {
    inspect_visiting(path, cancel, &mut |_| {})
}

/// [`inspect`], also handing `visit` every decoded line of at most the
/// line limit, in the same single pass (P5's printed bounds, D11). Longer
/// lines are skipped for `visit` too.
pub fn inspect_visiting(
    path: &Path,
    cancel: &CancelFlag,
    visit: &mut dyn FnMut(&str),
) -> Result<Inspected, InspectError> {
    if cancel.is_cancelled() {
        return Err(InspectError::Cancelled);
    }
    let mut lines = BoundedLines::new(BufReader::with_capacity(64 * 1024, File::open(path)?));
    let mut scanner = Scanner::default();
    while let Some(line) = lines.next_line()? {
        scanner.line_number += 1;
        if scanner.line_number.is_multiple_of(CANCEL_STRIDE) && cancel.is_cancelled() {
            return Err(InspectError::Cancelled);
        }
        match line {
            Some(bytes) => {
                let line = decode(bytes);
                visit(&line);
                scanner.line(&line);
            }
            None => scanner.long_line(),
        }
    }
    scanner.finish()
}

/// Reads `\n`-terminated lines of at most [`MAX_LINE`] bytes, without
/// buffering a longer one: its bytes are discarded as they arrive.
struct BoundedLines<R> {
    reader: R,
    line: Vec<u8>,
}

impl<R: BufRead> BoundedLines<R> {
    fn new(reader: R) -> Self {
        Self {
            reader,
            line: Vec::new(),
        }
    }

    /// `Some(Some(line))` for a line (without `\r\n`), `Some(None)` for an
    /// over-long line, `None` at the end.
    fn next_line(&mut self) -> std::io::Result<Option<Option<&[u8]>>> {
        self.line.clear();
        let mut too_long = false;
        let mut saw_bytes = false;
        loop {
            let buffer = self.reader.fill_buf()?;
            if buffer.is_empty() {
                if !saw_bytes {
                    return Ok(None);
                }
                break;
            }
            saw_bytes = true;
            let (chunk, used, ended) = match buffer.iter().position(|&byte| byte == b'\n') {
                Some(end) => (&buffer[..end], end + 1, true),
                None => (buffer, buffer.len(), false),
            };
            if !too_long {
                if self.line.len() + chunk.len() > MAX_LINE {
                    too_long = true;
                    self.line.clear();
                } else {
                    self.line.extend_from_slice(chunk);
                }
            }
            self.reader.consume(used);
            if ended {
                break;
            }
        }
        if too_long {
            return Ok(Some(None));
        }
        if self.line.ends_with(b"\r") {
            self.line.pop();
        }
        Ok(Some(Some(&self.line)))
    }
}

/// A command word: `G`, `M`, or `T`, then digits, optionally `.digits`.
struct Command<'a> {
    letter: char,
    number: &'a str,
}

impl<'a> Command<'a> {
    /// The command on `line`, ignoring any `;` comment.
    fn parse(line: &'a str) -> Option<Self> {
        let code = line.split(';').next().unwrap_or_default();
        let word = code.split_whitespace().next()?;
        let mut chars = word.chars();
        let letter = chars.next()?.to_ascii_uppercase();
        let number = chars.as_str();
        let (whole, fraction) = number.split_once('.').unwrap_or((number, "0"));
        let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
        (matches!(letter, 'G' | 'M' | 'T') && digits(whole) && digits(fraction))
            .then_some(Self { letter, number })
    }

    /// The whole-number code, when there is no fractional part.
    fn code(&self) -> Option<u32> {
        self.number.parse().ok()
    }

    fn is(&self, letter: char, code: u32) -> bool {
        self.letter == letter && self.code() == Some(code)
    }
}

struct Block {
    format: ThumbnailImageFormat,
    width: u32,
    height: u32,
    line: u64,
    base64: String,
    too_large: bool,
    /// Past [`MAX_LISTED`] blocks: consumed but neither listed nor decoded.
    ignored: bool,
}

#[derive(Default)]
struct Scanner {
    line_number: u64,
    command_count: u64,
    tools: BTreeSet<u32>,
    relative_positioning: bool,
    relative_extrusion: bool,
    /// Per-axis (min, max) of X, Y, Z words on `G0`/`G1`.
    extents: [Option<(f64, f64)>; 3],
    producer: Option<Producer>,
    claims: Vec<GcodeClaim>,
    thumbnails: Vec<ThumbnailInfo>,
    block: Option<Block>,
    best_thumbnail: Option<ThumbnailBytes>,
    warnings: Vec<ImportWarning>,
    long_lines: u64,
    first_long_line: u64,
}

impl Scanner {
    fn long_line(&mut self) {
        self.long_lines += 1;
        if self.first_long_line == 0 {
            self.first_long_line = self.line_number;
        }
    }

    fn line(&mut self, line: &str) {
        let line = line.trim();
        if let Some(comment) = line.strip_prefix(';') {
            self.comment(comment.trim());
        } else if let Some(command) = Command::parse(line) {
            self.command(&command, line);
        }
    }

    fn command(&mut self, command: &Command, line: &str) {
        self.command_count += 1;
        if command.letter == 'T' {
            if let Some(tool) = command.code() {
                if self.tools.len() < MAX_LISTED {
                    self.tools.insert(tool);
                }
            }
        } else if command.is('M', 83) {
            self.relative_extrusion = true;
        } else if command.is('G', 91) {
            self.relative_positioning = true;
        } else if command.is('G', 0) || command.is('G', 1) {
            let code = line.split(';').next().unwrap_or_default();
            for word in code.split_whitespace().skip(1) {
                let mut chars = word.chars();
                let axis = match chars.next().map(|c| c.to_ascii_uppercase()) {
                    Some('X') => 0,
                    Some('Y') => 1,
                    Some('Z') => 2,
                    _ => continue,
                };
                let Ok(value) = chars.as_str().parse::<f64>() else {
                    continue;
                };
                if value.is_finite() {
                    let extent = self.extents[axis].get_or_insert((value, value));
                    extent.0 = extent.0.min(value);
                    extent.1 = extent.1.max(value);
                }
            }
        }
    }

    fn comment(&mut self, text: &str) {
        if self.block.is_some() {
            let mut words = text.split_whitespace();
            let is_end = words.next().is_some_and(|tag| tag.starts_with("thumbnail"))
                && words.next() == Some("end");
            if is_end {
                self.finish_block();
            } else if let Some(block) = &mut self.block {
                if block.format == ThumbnailImageFormat::Png && !block.too_large && !block.ignored {
                    block.base64.push_str(text);
                    block.too_large = block.base64.len() > MAX_THUMBNAIL_BASE64;
                }
            }
            return;
        }
        if let Some(block) = self.block_begin(text) {
            self.block = Some(block);
            return;
        }
        if self.producer.is_none() {
            self.producer = parse_producer(text);
        }
        let separator = text.find(['=', ':']);
        if let Some(at) = separator {
            let key = text[..at].trim();
            let seen = self.claims.iter().any(|claim| claim.key == key);
            if CLAIM_KEYS.contains(&key) && !seen {
                self.claims.push(GcodeClaim {
                    key: key.to_string(),
                    value: capped(text[at + 1..].trim(), MAX_CLAIM_VALUE_BYTES).to_string(),
                    line: self.line_number,
                });
            }
        }
    }

    /// `thumbnail[_PNG|_QOI|_JPG] begin <W>x<H> [<length>]`.
    fn block_begin(&self, text: &str) -> Option<Block> {
        let mut words = text.split_whitespace();
        let format = match words.next()? {
            "thumbnail" | "thumbnail_PNG" => ThumbnailImageFormat::Png,
            "thumbnail_QOI" => ThumbnailImageFormat::Qoi,
            "thumbnail_JPG" => ThumbnailImageFormat::Jpg,
            _ => return None,
        };
        if words.next()? != "begin" {
            return None;
        }
        let (width, height) = words.next()?.split_once('x')?;
        Some(Block {
            format,
            width: width.parse().ok()?,
            height: height.parse().ok()?,
            line: self.line_number,
            base64: String::new(),
            too_large: false,
            ignored: self.thumbnails.len() >= MAX_LISTED,
        })
    }

    fn finish_block(&mut self) {
        let Some(block) = self.block.take() else {
            return;
        };
        if block.ignored {
            return;
        }
        self.thumbnails.push(ThumbnailInfo {
            format: block.format,
            width: block.width,
            height: block.height,
            part: None,
            line: Some(block.line),
        });
        if block.format != ThumbnailImageFormat::Png {
            return;
        }
        let origin = format!("line {}", block.line);
        let skipped = |why: &str| {
            ImportWarning::new(
                ImportWarningCode::ThumbnailSkipped,
                format!("The embedded thumbnail at {origin} was skipped: {why}."),
            )
        };
        if block.too_large {
            self.warnings.push(skipped("it is larger than 1 MiB"));
            return;
        }
        let bytes = match base64::engine::general_purpose::STANDARD.decode(&block.base64) {
            Ok(bytes) => bytes,
            Err(_) => {
                self.warnings.push(skipped("it isn't valid base64"));
                return;
            }
        };
        match png::accept_thumbnail(bytes, &origin) {
            Ok(thumbnail) => {
                let area = |t: &ThumbnailBytes| u64::from(t.width) * u64::from(t.height);
                if self
                    .best_thumbnail
                    .as_ref()
                    .is_none_or(|best| area(&thumbnail) > area(best))
                {
                    self.best_thumbnail = Some(thumbnail);
                }
            }
            Err(warning) => self.warnings.push(warning),
        }
    }

    fn finish(mut self) -> Result<Inspected, InspectError> {
        if self.command_count == 0 {
            return Err(InspectError::invalid("This G-code file has no commands."));
        }
        if self.long_lines > 0 {
            self.warnings.push(ImportWarning::new(
                ImportWarningCode::LongLine,
                format!(
                    "{} line(s) over 64 KiB were skipped, starting at line {}.",
                    self.long_lines, self.first_long_line
                ),
            ));
        }
        let observed_bounds_mm = match self.extents {
            [Some(x), Some(y), Some(z)] if !self.relative_positioning => Some(BoundsMm {
                min: [x.0, y.0, z.0],
                max: [x.1, y.1, z.1],
            }),
            _ => None,
        };
        Ok((
            GcodeInspection {
                producer: self.producer,
                claims: self.claims,
                trusted: false,
                line_count: self.line_number,
                command_count: self.command_count,
                tools_used: self.tools.into_iter().collect(),
                relative_positioning_seen: self.relative_positioning,
                relative_extrusion_seen: self.relative_extrusion,
                observed_bounds_mm,
                thumbnails: self.thumbnails,
            },
            self.best_thumbnail,
            self.warnings,
        ))
    }
}

/// `text` cut to at most `max` bytes on a char boundary.
fn capped(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `generated by <name> <version>` (most slicers) or `Generated with
/// <name> <version>` (Cura).
fn parse_producer(text: &str) -> Option<Producer> {
    let lower = text.to_ascii_lowercase();
    let rest = ["generated by ", "generated with "]
        .iter()
        .find(|prefix| lower.starts_with(*prefix))
        .map(|prefix| &text[prefix.len()..])?;
    let mut words = rest.split_whitespace();
    Some(Producer {
        name: words.next()?.to_string(),
        version: words.next()?.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use base64::Engine;

    use super::*;
    use crate::library::formats::png::{self, tests::png_header};
    use crate::library::formats::{BoundsMm, GcodeClaim, Producer, ThumbnailImageFormat};
    use crate::library::ImportWarningCode;

    type Inspected = (GcodeInspection, Option<ThumbnailBytes>, Vec<ImportWarning>);

    fn inspect_bytes(bytes: &[u8]) -> Result<Inspected, InspectError> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("0.part");
        std::fs::write(&path, bytes).unwrap();
        inspect(&path, &CancelFlag::never())
    }

    fn claim(key: &str, value: &str, line: u64) -> GcodeClaim {
        GcodeClaim {
            key: key.into(),
            value: value.into(),
            line,
        }
    }

    fn thumbnail_block(tag: &str, width: u32, height: u32, bytes: &[u8]) -> String {
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        let mut out = format!("; {tag} begin {width}x{height} {}\n", encoded.len());
        for chunk in encoded.as_bytes().chunks(78) {
            out.push_str(&format!("; {}\n", std::str::from_utf8(chunk).unwrap()));
        }
        out.push_str(&format!("; {tag} end\n"));
        out
    }

    #[test]
    fn detection_needs_text_with_a_command_or_comment() {
        assert!(looks_like_gcode(b"G28\n"));
        assert!(looks_like_gcode(b"; just a comment\n"));
        assert!(looks_like_gcode(b"\n  M104 S200\n"));
        assert!(looks_like_gcode(b"; caf\xe9\nG28\n"), "Latin-1 text");
        assert!(!looks_like_gcode(b"hello world\n"));
        assert!(!looks_like_gcode(b"G28\n\x00\x01\x02"));
        assert!(!looks_like_gcode(b""));
    }

    #[test]
    fn extracts_producer_and_claims_from_header_and_trailer() {
        let text = "; generated by OrcaSlicer 2.5.0-dev on 2026-09-24 at 12:00:00\n\
                    ; HEADER_BLOCK_START\n\
                    ; total layer number: 50\n\
                    ; max_z_height: 10.00\n\
                    ; HEADER_BLOCK_END\n\
                    G28\n\
                    ; printer_model = Bambu Lab X1 Carbon\n\
                    ; first_layer_height = 0.2\n\
                    ; nozzle_diameter = 0.4\n\
                    ; secret_key = do not keep\n";
        let (gcode, _, _) = inspect_bytes(text.as_bytes()).unwrap();
        assert_eq!(
            gcode.producer,
            Some(Producer {
                name: "OrcaSlicer".into(),
                version: "2.5.0-dev".into()
            })
        );
        assert_eq!(
            gcode.claims,
            vec![
                claim("total layer number", "50", 3),
                claim("max_z_height", "10.00", 4),
                claim("printer_model", "Bambu Lab X1 Carbon", 7),
                claim("nozzle_diameter", "0.4", 9),
            ]
        );
        assert!(!gcode.trusted);
    }

    #[test]
    fn extracts_cura_header_claims() {
        let text = ";FLAVOR:Marlin\n;TIME:6000\n;Filament used: 2.5m\n;Layer height: 0.12\n\
                    ;Generated with Cura_SteamEngine 5.8.0\n;LAYER:0\nG1 X1 Y1\n";
        let (gcode, _, _) = inspect_bytes(text.as_bytes()).unwrap();
        assert_eq!(
            gcode.producer,
            Some(Producer {
                name: "Cura_SteamEngine".into(),
                version: "5.8.0".into()
            })
        );
        assert_eq!(
            gcode.claims,
            vec![
                claim("FLAVOR", "Marlin", 1),
                claim("TIME", "6000", 2),
                claim("Filament used", "2.5m", 3),
                claim("Layer height", "0.12", 4),
            ]
        );
    }

    #[test]
    fn counts_structure_and_bounds_in_absolute_mode() {
        let text = "G28 ; home\nM83\nT1\nM104 T0 S200\nG1 X-1 Y2 Z0.2 E1\nG0 X10.5 Y.5 Z3\nG1 E-1\n\n; end\nT0\n";
        let (gcode, _, warnings) = inspect_bytes(text.as_bytes()).unwrap();
        assert_eq!(gcode.line_count, 10);
        assert_eq!(gcode.command_count, 8);
        assert_eq!(gcode.tools_used, vec![0, 1]);
        assert!(gcode.relative_extrusion_seen);
        assert!(!gcode.relative_positioning_seen);
        assert_eq!(
            gcode.observed_bounds_mm,
            Some(BoundsMm {
                min: [-1.0, 0.5, 0.2],
                max: [10.5, 2.0, 3.0]
            })
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn g91_marks_relative_positioning_and_omits_bounds() {
        let text = "G1 X1 Y1 Z1\nG91\nG1 Z5\nG90\n";
        let (gcode, _, _) = inspect_bytes(text.as_bytes()).unwrap();
        assert!(gcode.relative_positioning_seen);
        assert_eq!(gcode.observed_bounds_mm, None);
    }

    #[test]
    fn bounds_are_omitted_when_an_axis_never_appears() {
        let (gcode, _, _) = inspect_bytes(b"G1 X1 Y1\nG1 X2 Y3\n").unwrap();
        assert_eq!(gcode.observed_bounds_mm, None);
    }

    #[test]
    fn a_png_thumbnail_block_is_decoded_and_matches_the_png_header() {
        let small = png_header(16, 16);
        let large = png_header(300, 300);
        let mut text = String::from("; generated by PrusaSlicer 2.9.6 on today\n");
        text.push_str(&thumbnail_block("thumbnail", 16, 16, &small));
        text.push_str(&thumbnail_block("thumbnail_QOI", 32, 32, b"qoif...."));
        text.push_str(&thumbnail_block("thumbnail", 300, 300, &large));
        text.push_str("G28\n");
        let (gcode, thumbnail, warnings) = inspect_bytes(text.as_bytes()).unwrap();
        let thumbnail = thumbnail.expect("the largest PNG is chosen");
        assert_eq!(thumbnail.bytes, large);
        assert_eq!(
            png::dimensions(&thumbnail.bytes),
            Ok((thumbnail.width, thumbnail.height))
        );
        assert_eq!((thumbnail.width, thumbnail.height), (300, 300));
        let formats: Vec<_> = gcode
            .thumbnails
            .iter()
            .map(|info| (info.format, info.width, info.height, info.line))
            .collect();
        assert_eq!(
            formats,
            vec![
                (ThumbnailImageFormat::Png, 16, 16, Some(2)),
                (ThumbnailImageFormat::Qoi, 32, 32, Some(5)),
                (ThumbnailImageFormat::Png, 300, 300, Some(8)),
            ]
        );
        assert_eq!(thumbnail.origin_part, "line 8");
        assert!(warnings.is_empty());
        // Base64 lines are not claims or commands.
        assert!(gcode.claims.is_empty());
        assert_eq!(gcode.command_count, 1);
    }

    #[test]
    fn a_png_over_1024_px_is_skipped_with_a_warning() {
        let mut text = thumbnail_block("thumbnail", 16, 16, &png_header(16, 16));
        text.push_str(&thumbnail_block(
            "thumbnail",
            2048,
            2048,
            &png_header(2048, 2048),
        ));
        text.push_str("G28\n");
        let (_, thumbnail, warnings) = inspect_bytes(text.as_bytes()).unwrap();
        assert_eq!(thumbnail.map(|t| t.width), Some(16));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, ImportWarningCode::ThumbnailSkipped);
    }

    #[test]
    fn a_70_kib_line_is_counted_and_skipped_with_long_line() {
        let mut bytes = b"G28\n".to_vec();
        bytes.extend(std::iter::repeat_n(b'G', 70 * 1024));
        bytes.extend_from_slice(b"\nG1 X1\n");
        let (gcode, _, warnings) = inspect_bytes(&bytes).unwrap();
        assert_eq!(gcode.line_count, 3);
        assert_eq!(gcode.command_count, 2);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, ImportWarningCode::LongLine);
        assert!(
            warnings[0].message.contains("line 2"),
            "{}",
            warnings[0].message
        );
    }

    #[test]
    fn latin_1_input_is_accepted() {
        let bytes = b"; filament_type = PLA caf\xe9\r\nG28\r\n";
        let (gcode, _, _) = inspect_bytes(bytes).unwrap();
        assert_eq!(gcode.claims, vec![claim("filament_type", "PLA café", 1)]);
        assert_eq!(gcode.line_count, 2);
        assert_eq!(gcode.command_count, 1);
    }

    #[test]
    fn a_file_without_commands_is_invalid_content() {
        assert!(matches!(
            inspect_bytes(b"; only a comment\n"),
            Err(InspectError::InvalidContent(_))
        ));
    }

    #[test]
    fn a_cancelled_scan_stops() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("0.part");
        std::fs::write(&path, b"G28\n").unwrap();
        let (sender, receiver) = tokio::sync::watch::channel(false);
        sender.send(true).unwrap();
        assert_eq!(
            inspect(&path, &CancelFlag::new(receiver)).unwrap_err(),
            InspectError::Cancelled
        );
    }

    #[test]
    fn a_repeated_claim_key_keeps_its_first_value() {
        let text =
            "; printer_model = First\nG28\n; printer_model = Second\n; printer_model = Third\n";
        let (gcode, _, _) = inspect_bytes(text.as_bytes()).unwrap();
        assert_eq!(gcode.claims, vec![claim("printer_model", "First", 1)]);
    }

    #[test]
    fn an_oversized_claim_value_is_truncated_on_a_char_boundary() {
        let text = format!("; filament_type = {}\nG28\n", "é".repeat(600));
        let (gcode, _, _) = inspect_bytes(text.as_bytes()).unwrap();
        let value = &gcode.claims[0].value;
        assert_eq!(value.len(), MAX_CLAIM_VALUE_BYTES);
        assert_eq!(value.chars().count(), MAX_CLAIM_VALUE_BYTES / 2);
    }

    #[test]
    fn the_tool_and_thumbnail_lists_are_capped() {
        let mut text = String::new();
        for tool in 0..100 {
            text.push_str(&format!("T{tool}\n"));
        }
        for _ in 0..70 {
            text.push_str(&thumbnail_block("thumbnail_QOI", 8, 8, b"qoif"));
        }
        let (gcode, _, _) = inspect_bytes(text.as_bytes()).unwrap();
        assert_eq!(gcode.tools_used.len(), MAX_LISTED);
        assert_eq!(gcode.thumbnails.len(), MAX_LISTED);
        assert_eq!(gcode.command_count, 100);
    }
}
