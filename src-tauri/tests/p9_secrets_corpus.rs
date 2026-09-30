//! The shared secret corpus scanners (`tests/common/secrets.rs`) catch what
//! they claim to: raw bytes, zip entry names, compressed and decompressed
//! entry bytes, nested zips, and the exact/lowercase/uppercase/percent-
//! encoded forms; and stay quiet on clean input and on data a backup may
//! legitimately hold.

use std::io::Write;

use zip::CompressionMethod::{Deflated, Stored};

#[path = "common/secrets.rs"]
mod secrets;
use secrets::*;

fn zip_of(entries: &[(&str, &[u8], zip::CompressionMethod)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes, method) in entries {
        let options = zip::write::SimpleFileOptions::default().compression_method(*method);
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn the_corpus_holds_the_brief_values() {
    assert_eq!(
        USERINFO_URL,
        "http://operator:s3cr3t-P9@192.0.2.19:8080/webcam?token=tok-P9-19"
    );
    assert_eq!(
        STORED_CAMERA_URL,
        "http://192.0.2.19:8080/webcam?token=tok-P9-19"
    );
    assert!(USERINFO_URL.contains(USERINFO) && USERINFO.contains(USERINFO_PASSWORD));
    assert!(HEADER_LINE.starts_with(HEADER_NAME) && HEADER_LINE.ends_with(HEADER_VALUE));
    assert!(
        !STORED_CAMERA_URL.contains('@'),
        "a stored URL carries no userinfo"
    );
}

#[test]
fn backup_forbidden_is_a_subset_of_the_full_corpus_without_farm_data() {
    for needle in BACKUP_FORBIDDEN {
        assert!(FULL_CORPUS.contains(needle), "{needle}");
    }
    for farm_data in [
        HOST,
        HOST_NAME,
        HOME_PATH,
        PRINTER_NAME,
        STORED_CAMERA_URL,
        QUERY_TOKEN,
    ] {
        assert!(
            !BACKUP_FORBIDDEN.contains(&farm_data),
            "{farm_data} is Farm data"
        );
    }
    for needle in [
        CREDENTIAL_VALUE,
        CREDENTIAL_VALUE_2,
        HEADER_VALUE,
        USERINFO_URL,
        USERINFO,
        USERINFO_PASSWORD,
    ] {
        assert!(BACKUP_FORBIDDEN.contains(&needle), "{needle}");
    }
}

#[test]
fn raw_bytes_are_scanned_in_every_form() {
    for form in [
        CREDENTIAL_VALUE.to_string(),
        CREDENTIAL_VALUE.to_uppercase(),
        CREDENTIAL_VALUE.to_lowercase(),
        USERINFO_URL
            .replace(':', "%3A")
            .replace('/', "%2F")
            .replace('@', "%40"),
    ] {
        let bytes = format!("prefix {form} suffix").into_bytes();
        assert!(!find_any(FULL_CORPUS, &bytes, "raw").is_empty(), "{form}");
    }
    assert!(find_any(FULL_CORPUS, b"nothing to see here", "raw").is_empty());
}

#[test]
fn zip_entries_are_scanned_stored_deflated_named_and_nested() {
    let secret = format!("value={CREDENTIAL_VALUE}").into_bytes();
    let stored = zip_of(&[("a.txt", &secret, Stored)]);
    let deflated = zip_of(&[("a.txt", &secret, Deflated)]);
    let named = zip_of(&[(&format!("dir/{HEADER_VALUE}.txt"), b"clean", Stored)]);
    let nested = zip_of(&[("inner.zip", &deflated, Stored)]);
    for (label, archive) in [
        ("stored", &stored),
        ("deflated", &deflated),
        ("named", &named),
        ("nested", &nested),
    ] {
        let hits = find_any(BACKUP_FORBIDDEN, archive, label);
        assert!(!hits.is_empty(), "{label} must be caught");
    }
    // Deflated: the plaintext is not in the raw or compressed bytes, only
    // in the decompressed entry.
    let hits = find_any(BACKUP_FORBIDDEN, &deflated, "deflated");
    assert!(
        hits.iter().all(|hit| hit.contains("(decompressed)")),
        "{hits:?}"
    );
    let clean = zip_of(&[("a.txt", b"nothing", Deflated), ("b.txt", b"here", Stored)]);
    assert_no_corpus(&clean, "clean zip");
}

#[test]
#[should_panic(expected = "seeded secrets found")]
fn assert_no_corpus_panics_on_a_hit() {
    assert_no_corpus(PRINTER_NAME.as_bytes(), "printer name");
}

#[test]
fn a_backup_may_hold_farm_data_but_not_credentials() {
    let farm_data = format!("{HOST} {HOST_NAME} {HOME_PATH} {PRINTER_NAME} {STORED_CAMERA_URL}");
    assert_no_backup_forbidden(farm_data.as_bytes(), "farm data");
    assert!(!find_any(FULL_CORPUS, farm_data.as_bytes(), "farm data").is_empty());
}

/// A corrupt archive must not scan clean: input that starts with `PK` but
/// doesn't parse as a zip fails the scan instead of being skipped.
#[test]
#[should_panic(expected = "is not a readable zip")]
fn a_pk_prefixed_input_that_is_not_a_zip_fails_the_scan() {
    let mut bytes = zip_of(&[("a.txt", b"nothing", Deflated)]);
    bytes.truncate(bytes.len() - 10);
    assert_no_backup_forbidden(&bytes, "truncated zip");
}

/// ...and so does an entry that can't be read (here, deflate data that
/// no longer decompresses to its CRC-32).
#[test]
#[should_panic(expected = "could not be read")]
fn an_unreadable_entry_fails_the_scan() {
    let original: Vec<u8> = (0..4096_u32).map(|value| (value % 7) as u8).collect();
    let mut bytes = zip_of(&[("a.bin", &original, Stored)]);
    // Flip a byte inside the stored data: the CRC-32 check fails on read.
    let at = bytes.windows(4).position(|w| w == [0, 1, 2, 3]).unwrap() + 2;
    bytes[at] ^= 0xff;
    assert_no_backup_forbidden(&bytes, "corrupt entry");
}

/// A nested entry that starts with `PK` is held to the same rule.
#[test]
#[should_panic(expected = "is not a readable zip")]
fn a_nested_pk_entry_that_is_not_a_zip_fails_the_scan() {
    let outer = zip_of(&[("inner.3mf", b"PK not a zip at all", Stored)]);
    assert_no_backup_forbidden(&outer, "nested");
}
