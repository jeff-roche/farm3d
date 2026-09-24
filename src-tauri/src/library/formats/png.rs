//! D12: PNG header checks for embedded thumbnails. Only the signature and
//! the IHDR dimensions are read; pixels are never decoded.

use super::ThumbnailBytes;
use crate::library::{ImportWarning, ImportWarningCode};

pub const MAX_THUMBNAIL_BYTES: usize = 1024 * 1024;
pub const MAX_THUMBNAIL_EDGE: u32 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotPng;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// The IHDR width and height of a PNG: the signature, then a 13-byte IHDR
/// as the first chunk, with non-zero dimensions.
pub fn dimensions(bytes: &[u8]) -> Result<(u32, u32), NotPng> {
    let header = bytes.get(..24).ok_or(NotPng)?;
    let be_u32 = |at: usize| u32::from_be_bytes(header[at..at + 4].try_into().unwrap());
    if header[..8] != SIGNATURE || be_u32(8) != 13 || &header[12..16] != b"IHDR" {
        return Err(NotPng);
    }
    let (width, height) = (be_u32(16), be_u32(20));
    if width == 0 || height == 0 {
        return Err(NotPng);
    }
    Ok((width, height))
}

/// Accepts `bytes` as the stored thumbnail if it is a PNG of at most 1 MiB
/// and 1024×1024; otherwise explains the skip as `THUMBNAIL_SKIPPED`.
/// `origin_part` names the source in the message and the result.
pub fn accept_thumbnail(
    bytes: Vec<u8>,
    origin_part: &str,
) -> Result<ThumbnailBytes, ImportWarning> {
    let skipped = |why: String| {
        ImportWarning::new(
            ImportWarningCode::ThumbnailSkipped,
            format!("The embedded thumbnail at {origin_part} was skipped: {why}."),
        )
    };
    if bytes.len() > MAX_THUMBNAIL_BYTES {
        return Err(skipped("it is larger than 1 MiB".to_string()));
    }
    let (width, height) =
        dimensions(&bytes).map_err(|NotPng| skipped("it isn't a PNG".to_string()))?;
    if width > MAX_THUMBNAIL_EDGE || height > MAX_THUMBNAIL_EDGE {
        return Err(skipped(format!(
            "it is {width}×{height}, over the {MAX_THUMBNAIL_EDGE} px limit"
        )));
    }
    Ok(ThumbnailBytes {
        origin_part: origin_part.to_string(),
        width,
        height,
        bytes,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal PNG signature plus IHDR with the given size. The CRC and
    /// the rest of the file are not checked, so they're left out.
    pub(crate) fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes
    }

    #[test]
    fn reads_the_ihdr_width_and_height() {
        assert_eq!(dimensions(&png_header(313, 173)), Ok((313, 173)));
    }

    #[test]
    fn rejects_bytes_that_are_not_a_png() {
        assert_eq!(dimensions(b"GIF89a....................."), Err(NotPng));
        assert_eq!(dimensions(&png_header(2, 2)[..20]), Err(NotPng));
        assert_eq!(dimensions(&png_header(0, 2)), Err(NotPng));
        let mut wrong_chunk = png_header(2, 2);
        wrong_chunk[12..16].copy_from_slice(b"IDAT");
        assert_eq!(dimensions(&wrong_chunk), Err(NotPng));
    }

    #[test]
    fn accepts_a_small_png() {
        let thumbnail = accept_thumbnail(png_header(16, 16), "Metadata/thumbnail.png").unwrap();
        assert_eq!(
            (
                thumbnail.width,
                thumbnail.height,
                thumbnail.origin_part.as_str()
            ),
            (16, 16, "Metadata/thumbnail.png")
        );
    }

    #[test]
    fn skips_a_png_over_1024_px() {
        let warning = accept_thumbnail(png_header(1025, 16), "line 9").unwrap_err();
        assert_eq!(warning.code, ImportWarningCode::ThumbnailSkipped);
        assert!(warning.message.contains("1025×16"), "{}", warning.message);
    }

    #[test]
    fn skips_a_png_over_1_mib_and_a_non_png() {
        let mut large = png_header(16, 16);
        large.resize(MAX_THUMBNAIL_BYTES + 1, 0);
        let warning = accept_thumbnail(large, "line 9").unwrap_err();
        assert_eq!(warning.code, ImportWarningCode::ThumbnailSkipped);
        let warning = accept_thumbnail(b"not a png".to_vec(), "line 9").unwrap_err();
        assert_eq!(warning.code, ImportWarningCode::ThumbnailSkipped);
        assert!(
            warning.message.contains("isn't a PNG"),
            "{}",
            warning.message
        );
    }
}
