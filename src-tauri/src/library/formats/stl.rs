//! D9: farm3d's own STL reader.
//!
//! Detection follows D9 rather than trusting a `solid` prefix: a binary
//! STL's 80-byte header may start with `solid `, and an ASCII file's first
//! line may be a bare `solid`. Both readers stream triangles and keep only a
//! count and bounds.

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use super::{include_point, BoundsMm, InspectError, StlEncoding, StlInspection};
use crate::library::content::CancelFlag;
use crate::library::{ImportWarning, ImportWarningCode};

const HEADER_LEN: u64 = 84;
const RECORD_LEN: u64 = 50;
/// D9: the first `facet` must appear within this many bytes.
const ASCII_FACET_WINDOW: usize = 4 * 1024;
/// The first line (`solid <name>`) and any one token must fit in this.
const MAX_ASCII_LINE: usize = 64 * 1024;
const MAX_ASCII_TOKEN: usize = 256;
/// Triangles between cancellation checks.
const CANCEL_STRIDE: u64 = 64 * 1024;

/// D9's detection rule over the first bytes of the file (`head`, at least
/// the first 4 KiB when the file has them) and the file's length. ASCII
/// wins when its rule holds; binary needs the declared count to fit.
pub fn detect(head: &[u8], file_len: u64) -> Option<StlEncoding> {
    if is_ascii_stl(head) {
        Some(StlEncoding::Ascii)
    } else if declared_count(head).is_some_and(|count| binary_len(count) <= file_len) {
        Some(StlEncoding::Binary)
    } else {
        None
    }
}

/// D9: for a file named `.stl` that no reader claimed, the truncation
/// reason when it isn't ASCII and declares more triangles than it holds
/// complete records, however it was cut. The caller checks the name.
pub fn truncated_binary_reason(head: &[u8], file_len: u64) -> Option<String> {
    let declared = declared_count(head)?;
    let present = file_len.checked_sub(HEADER_LEN)? / RECORD_LEN;
    (!is_ascii_stl(head) && u64::from(declared) > present).then(|| {
        format!(
            "This binary STL is truncated: it declares {declared} triangles but holds {present}."
        )
    })
}

fn is_ascii_stl(head: &[u8]) -> bool {
    let window = &head[..head.len().min(ASCII_FACET_WINDOW)];
    let start = window.iter().position(|byte| !byte.is_ascii_whitespace());
    start.is_some_and(|start| window[start..].starts_with(b"solid"))
        && window
            .windows(5)
            .any(|word| word.eq_ignore_ascii_case(b"facet"))
}

fn declared_count(head: &[u8]) -> Option<u32> {
    let bytes = head.get(80..84)?;
    Some(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn binary_len(count: u32) -> u64 {
    HEADER_LEN + RECORD_LEN * u64::from(count)
}

pub fn inspect(
    path: &Path,
    encoding: StlEncoding,
    cancel: &CancelFlag,
) -> Result<(StlInspection, Vec<ImportWarning>), InspectError> {
    if cancel.is_cancelled() {
        return Err(InspectError::Cancelled);
    }
    let file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let reader = BufReader::with_capacity(64 * 1024, file);
    let (solid_name, triangle_count, bounds, warnings) = match encoding {
        StlEncoding::Binary => {
            let (count, bounds, warnings) = read_binary(reader, file_len, cancel)?;
            (None, count, bounds, warnings)
        }
        StlEncoding::Ascii => {
            let (name, count, bounds) = read_ascii(reader, cancel)?;
            (name, count, bounds, Vec::new())
        }
    };
    let bounds_mm = match bounds {
        Some(bounds) if triangle_count > 0 => bounds,
        _ => return Err(InspectError::invalid("This STL has no triangles.")),
    };
    Ok((
        StlInspection {
            encoding,
            solid_name,
            triangle_count,
            bounds_mm,
            units_assumed: true,
        },
        warnings,
    ))
}

fn non_finite(triangle: u64) -> InspectError {
    InspectError::invalid(format!(
        "This STL has a non-finite coordinate in triangle {triangle}."
    ))
}

fn read_binary(
    mut reader: impl Read,
    file_len: u64,
    cancel: &CancelFlag,
) -> Result<(u64, Option<BoundsMm>, Vec<ImportWarning>), InspectError> {
    let mut header = [0u8; HEADER_LEN as usize];
    reader.read_exact(&mut header)?;
    let count = declared_count(&header).expect("the header holds the count");
    let expected_len = binary_len(count);
    if file_len < expected_len {
        let present = (file_len - HEADER_LEN) / RECORD_LEN;
        return Err(InspectError::invalid(format!(
            "This binary STL is truncated: it declares {count} triangles but holds {present}."
        )));
    }

    let mut bounds = None;
    let mut record = [0u8; RECORD_LEN as usize];
    for triangle in 0..u64::from(count) {
        if triangle.is_multiple_of(CANCEL_STRIDE) && cancel.is_cancelled() {
            return Err(InspectError::Cancelled);
        }
        reader.read_exact(&mut record)?;
        // 12 bytes of normal, then three vertices of three f32 each.
        for vertex in record[12..48].as_chunks::<12>().0 {
            let mut point = [0.0; 3];
            for (axis, value) in vertex.as_chunks::<4>().0.iter().enumerate() {
                let value = f32::from_le_bytes(*value);
                if !value.is_finite() {
                    return Err(non_finite(triangle + 1));
                }
                point[axis] = f64::from(value);
            }
            include_point(&mut bounds, point);
        }
    }

    let mut warnings = Vec::new();
    let trailing = file_len - expected_len;
    if trailing > 0 {
        warnings.push(ImportWarning::new(
            ImportWarningCode::TrailingBytes,
            format!("{trailing} bytes after the last triangle were ignored."),
        ));
    }
    Ok((u64::from(count), bounds, warnings))
}

fn read_ascii(
    mut reader: impl BufRead,
    cancel: &CancelFlag,
) -> Result<(Option<String>, u64, Option<BoundsMm>), InspectError> {
    let solid_name = read_solid_line(&mut reader)?;
    let mut tokens = Tokens { reader };
    let mut triangles = 0u64;
    let mut bounds = None;
    while let Some(token) = tokens.next()? {
        if token.eq_ignore_ascii_case(b"endsolid") {
            break;
        }
        if !token.eq_ignore_ascii_case(b"facet") {
            return Err(unexpected(&token));
        }
        if triangles.is_multiple_of(CANCEL_STRIDE) && cancel.is_cancelled() {
            return Err(InspectError::Cancelled);
        }
        triangles += 1;
        tokens.expect(b"normal")?;
        for _ in 0..3 {
            tokens.number()?;
        }
        tokens.expect(b"outer")?;
        tokens.expect(b"loop")?;
        for _ in 0..3 {
            tokens.expect(b"vertex")?;
            let mut point = [0.0; 3];
            for value in &mut point {
                *value = tokens.number()?;
                if !value.is_finite() {
                    return Err(non_finite(triangles));
                }
            }
            include_point(&mut bounds, point);
        }
        tokens.expect(b"endloop")?;
        tokens.expect(b"endfacet")?;
    }
    Ok((solid_name, triangles, bounds))
}

/// Reads through the first non-blank line, which starts with `solid`, and
/// returns the name after it, if any.
fn read_solid_line(reader: &mut impl BufRead) -> Result<Option<String>, InspectError> {
    loop {
        let mut line = Vec::new();
        let read = reader
            .by_ref()
            .take(MAX_ASCII_LINE as u64)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            return Err(InspectError::invalid("This ASCII STL is empty."));
        }
        if !line.ends_with(b"\n") && read == MAX_ASCII_LINE {
            return Err(InspectError::invalid(
                "This ASCII STL's first line is too long.",
            ));
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let name = text
            .strip_prefix("solid")
            .ok_or_else(|| InspectError::invalid("This ASCII STL doesn't start with `solid`."))?
            .trim();
        return Ok((!name.is_empty()).then(|| name.to_string()));
    }
}

fn unexpected(token: &[u8]) -> InspectError {
    InspectError::invalid(format!(
        "This ASCII STL is malformed near `{}`.",
        String::from_utf8_lossy(token)
    ))
}

/// A whitespace-split token stream with a bounded token length.
struct Tokens<R> {
    reader: R,
}

impl<R: BufRead> Tokens<R> {
    fn next(&mut self) -> Result<Option<Vec<u8>>, InspectError> {
        let mut token = Vec::new();
        loop {
            let buffer = self.reader.fill_buf()?;
            if buffer.is_empty() {
                return Ok((!token.is_empty()).then_some(token));
            }
            let mut used = 0;
            let mut done = false;
            for &byte in buffer {
                used += 1;
                if byte.is_ascii_whitespace() {
                    if !token.is_empty() {
                        done = true;
                        break;
                    }
                } else {
                    token.push(byte);
                    if token.len() > MAX_ASCII_TOKEN {
                        return Err(InspectError::invalid(
                            "This ASCII STL has a token that is too long.",
                        ));
                    }
                }
            }
            self.reader.consume(used);
            if done {
                return Ok(Some(token));
            }
        }
    }

    fn expect(&mut self, keyword: &[u8]) -> Result<(), InspectError> {
        match self.next()? {
            Some(token) if token.eq_ignore_ascii_case(keyword) => Ok(()),
            Some(token) => Err(unexpected(&token)),
            None => Err(InspectError::invalid("This ASCII STL ends mid-facet.")),
        }
    }

    fn number(&mut self) -> Result<f64, InspectError> {
        let token = self
            .next()?
            .ok_or_else(|| InspectError::invalid("This ASCII STL ends mid-facet."))?;
        std::str::from_utf8(&token)
            .ok()
            .and_then(|text| text.parse::<f64>().ok())
            .ok_or_else(|| unexpected(&token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::formats::BoundsMm;
    use crate::library::ImportWarningCode;

    const TRIANGLE: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 5.0, 2.0]];

    fn binary(header: &[u8], declared: u32, triangles: &[[[f32; 3]; 3]]) -> Vec<u8> {
        let mut out = header.to_vec();
        out.resize(80, 0);
        out.extend_from_slice(&declared.to_le_bytes());
        for triangle in triangles {
            out.extend_from_slice(&[0u8; 12]);
            for value in triangle.iter().flatten() {
                out.extend_from_slice(&value.to_le_bytes());
            }
            out.extend_from_slice(&[0, 0]);
        }
        out
    }

    fn ascii(first_line: &str, triangles: &[[[&str; 3]; 3]]) -> Vec<u8> {
        let mut out = format!("{first_line}\n");
        for triangle in triangles {
            out.push_str("facet normal 0 0 1\n outer loop\n");
            for [x, y, z] in triangle {
                out.push_str(&format!("  vertex {x} {y} {z}\n"));
            }
            out.push_str(" endloop\nendfacet\n");
        }
        out.push_str("endsolid\n");
        out.into_bytes()
    }

    const ASCII_TRIANGLE: [[&str; 3]; 3] = [["0", "0", "0"], ["10", "0", "0"], ["0", "5", "2"]];

    fn detect_bytes(bytes: &[u8]) -> Option<StlEncoding> {
        detect(&bytes[..bytes.len().min(64 * 1024)], bytes.len() as u64)
    }

    fn inspect_bytes(bytes: &[u8]) -> Result<(StlInspection, Vec<ImportWarning>), InspectError> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("0.part");
        std::fs::write(&path, bytes).unwrap();
        let encoding = detect_bytes(bytes).expect("test input is an STL");
        inspect(&path, encoding, &CancelFlag::never())
    }

    #[test]
    fn detection_rule_table() {
        let far_facet = {
            let mut bytes = b"solid far\n".to_vec();
            bytes.extend(std::iter::repeat_n(b' ', 5000));
            bytes.extend_from_slice(b"facet normal 0 0 1\n");
            bytes
        };
        let cases: Vec<(&str, Vec<u8>, Option<StlEncoding>)> = vec![
            (
                "ASCII with a name",
                ascii("solid part", &[ASCII_TRIANGLE]),
                Some(StlEncoding::Ascii),
            ),
            (
                "ASCII whose first line is bare `solid`",
                ascii("solid", &[ASCII_TRIANGLE]),
                Some(StlEncoding::Ascii),
            ),
            (
                "ASCII after leading whitespace",
                ascii("  \n solid part", &[ASCII_TRIANGLE]),
                Some(StlEncoding::Ascii),
            ),
            (
                "binary with a `solid ` header",
                binary(b"solid exported by CAD", 1, &[TRIANGLE]),
                Some(StlEncoding::Binary),
            ),
            (
                "binary with a plain header",
                binary(b"plain", 1, &[TRIANGLE]),
                Some(StlEncoding::Binary),
            ),
            (
                "binary with zero triangles is still binary",
                binary(b"empty", 0, &[]),
                Some(StlEncoding::Binary),
            ),
            ("`solid` whose first `facet` is past 4 KiB", far_facet, None),
            ("shorter than the 84-byte header", vec![0u8; 83], None),
            (
                "binary shorter than its declared count",
                binary(b"short", 3, &[TRIANGLE]),
                None,
            ),
        ];
        for (name, bytes, expected) in cases {
            assert_eq!(detect_bytes(&bytes), expected, "{name}");
        }
    }

    #[test]
    fn a_binary_stl_cut_short_is_reported_as_truncated() {
        let whole_records = binary(b"cut", 12, &[TRIANGLE; 3]);
        let reason = truncated_binary_reason(&whole_records, whole_records.len() as u64).unwrap();
        assert!(reason.contains("truncated"), "{reason}");
        assert!(reason.contains("12") && reason.contains('3'), "{reason}");

        // Cut in the middle of the fourth record: still three whole ones.
        let mut mid_record = binary(b"cut", 12, &[TRIANGLE; 4]);
        mid_record.truncate(mid_record.len() - 17);
        let reason = truncated_binary_reason(&mid_record, mid_record.len() as u64).unwrap();
        assert!(reason.contains("holds 3"), "{reason}");

        // ASCII, and a binary file whose count fits, are not truncated.
        let text = ascii("solid part", &[ASCII_TRIANGLE]);
        assert_eq!(truncated_binary_reason(&text, text.len() as u64), None);
        let complete = binary(b"ok", 1, &[TRIANGLE]);
        assert_eq!(
            truncated_binary_reason(&complete, complete.len() as u64),
            None
        );
    }

    #[test]
    fn reads_a_binary_stl() {
        let (stl, warnings) = inspect_bytes(&binary(b"solid cad", 2, &[TRIANGLE; 2])).unwrap();
        assert_eq!(stl.encoding, StlEncoding::Binary);
        assert_eq!(stl.solid_name, None);
        assert_eq!(stl.triangle_count, 2);
        assert_eq!(
            stl.bounds_mm,
            BoundsMm {
                min: [0.0, 0.0, 0.0],
                max: [10.0, 5.0, 2.0]
            }
        );
        assert!(stl.units_assumed);
        assert!(warnings.is_empty());
    }

    #[test]
    fn reads_an_ascii_stl_and_its_name() {
        let (stl, _) = inspect_bytes(&ascii("solid  my part ", &[ASCII_TRIANGLE])).unwrap();
        assert_eq!(stl.encoding, StlEncoding::Ascii);
        assert_eq!(stl.solid_name.as_deref(), Some("my part"));
        assert_eq!(stl.triangle_count, 1);
        assert_eq!(stl.bounds_mm.max, [10.0, 5.0, 2.0]);
        let (bare, _) = inspect_bytes(&ascii("solid", &[ASCII_TRIANGLE])).unwrap();
        assert_eq!(bare.solid_name, None);
    }

    #[test]
    fn ascii_keywords_are_case_insensitive() {
        let upper = String::from_utf8(ascii("solid UP", &[ASCII_TRIANGLE]))
            .unwrap()
            .replace("facet", "FACET")
            .replace("vertex", "VERTEX");
        let (stl, _) = inspect_bytes(upper.as_bytes()).unwrap();
        assert_eq!(stl.triangle_count, 1);
    }

    #[test]
    fn trailing_bytes_after_the_last_binary_triangle_warn() {
        let mut bytes = binary(b"cad", 1, &[TRIANGLE]);
        bytes.extend_from_slice(b"extra");
        let (stl, warnings) = inspect_bytes(&bytes).unwrap();
        assert_eq!(stl.triangle_count, 1);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, ImportWarningCode::TrailingBytes);
        assert!(warnings[0].message.contains('5'), "{}", warnings[0].message);
    }

    #[test]
    fn non_finite_coordinates_are_rejected() {
        let mut nan = [TRIANGLE; 2];
        nan[1][2][0] = f32::NAN;
        let error = inspect_bytes(&binary(b"cad", 2, &nan)).unwrap_err();
        assert!(
            matches!(&error, InspectError::InvalidContent(reason) if reason.contains("non-finite"))
        );

        let mut infinite = [TRIANGLE; 1];
        infinite[0][0][1] = f32::INFINITY;
        assert!(matches!(
            inspect_bytes(&binary(b"cad", 1, &infinite)),
            Err(InspectError::InvalidContent(_))
        ));

        let ascii_nan = [["0", "0", "0"], ["nan", "0", "0"], ["0", "5", "2"]];
        let error = inspect_bytes(&ascii("solid x", &[ascii_nan])).unwrap_err();
        assert!(
            matches!(&error, InspectError::InvalidContent(reason) if reason.contains("non-finite"))
        );
    }

    #[test]
    fn zero_triangles_are_rejected() {
        let error = inspect_bytes(&binary(b"empty", 0, &[])).unwrap_err();
        assert!(
            matches!(&error, InspectError::InvalidContent(reason) if reason.contains("no triangles"))
        );
        let error = inspect_bytes(b"solid nothing\nfacet\nendsolid\n");
        assert!(matches!(error, Err(InspectError::InvalidContent(_))));
    }

    #[test]
    fn malformed_ascii_is_invalid_content() {
        let broken = b"solid x\nfacet normal 0 0 1\n outer loop\n vertex 0 0 0\n vertex 1 1\n";
        assert!(matches!(
            inspect_bytes(broken),
            Err(InspectError::InvalidContent(_))
        ));
    }

    #[test]
    fn a_cancelled_read_stops() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("0.part");
        std::fs::write(&path, binary(b"cad", 1, &[TRIANGLE])).unwrap();
        let (sender, receiver) = tokio::sync::watch::channel(false);
        sender.send(true).unwrap();
        assert_eq!(
            inspect(&path, StlEncoding::Binary, &CancelFlag::new(receiver)).unwrap_err(),
            InspectError::Cancelled
        );
    }
}
