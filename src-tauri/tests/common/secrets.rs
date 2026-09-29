//! The shared P9 seeded-secret corpus and its scanners (spec acceptance
//! criteria 5 and 13). Every value is fake. Hosts use RFC 5737 space.
//!
//! Two sets:
//!
//! - [`FULL_CORPUS`] (diagnostics bundles and log lines): every credential
//!   value, the header value, the userinfo URL and its parts, the host, a
//!   home-directory path, a Printer name, and the stored camera URL with its
//!   query token. None may appear in a bundle or a log line.
//! - [`BACKUP_FORBIDDEN`] (a whole-Farm backup, a journal, an error, an
//!   event, frontend state): only what must never be persisted or sent:
//!   every credential value, the header value, the userinfo URL, its
//!   `user:password@` part, and its password. Hosts, Printer names, paths,
//!   and the stored camera URL are Farm data a backup legitimately holds
//!   (D10), so they are not in this set.
//!
//! Stored camera URL seeds carry only a query token (P8 rejects userinfo at
//! save); the userinfo form is for error and log paths only.
//!
//! [`assert_no_corpus`] and [`assert_none_of`] scan raw bytes and, for a zip,
//! every entry name, every entry's compressed bytes, and every entry's
//! decompressed bytes (recursing into nested zips). Each needle is matched
//! in its exact, lowercase, uppercase, and percent-encoded forms.
#![allow(dead_code)]

use std::io::{Cursor, Read};

/// A credential value, as stored in the credential store.
pub const CREDENTIAL_VALUE: &str = "cred-P9-8f2a61c4d7e0";
/// A second credential value (an API key held for another Printer).
pub const CREDENTIAL_VALUE_2: &str = "cred-P9-b13d90a5e2c8";
/// A request header's name and value; the value is the secret.
pub const HEADER_NAME: &str = "X-Api-Key";
pub const HEADER_VALUE: &str = "hdr-P9-4c2e88b1a9f3";
pub const HEADER_LINE: &str = "X-Api-Key: hdr-P9-4c2e88b1a9f3";
/// The userinfo camera URL: error and log paths only.
pub const USERINFO_URL: &str = "http://operator:s3cr3t-P9@192.0.2.19:8080/webcam?token=tok-P9-19";
/// Its `user:password@` part.
pub const USERINFO: &str = "operator:s3cr3t-P9@";
/// Its password.
pub const USERINFO_PASSWORD: &str = "s3cr3t-P9";
/// Its query token (also in the stored URL).
pub const QUERY_TOKEN: &str = "tok-P9-19";
/// What a *stored* camera URL looks like: a query token, no userinfo.
pub const STORED_CAMERA_URL: &str = "http://192.0.2.19:8080/webcam?token=tok-P9-19";
/// A Printer host.
pub const HOST: &str = "192.0.2.19";
pub const HOST_NAME: &str = "printer-p9.farm.example";
/// A home-directory path.
pub const HOME_PATH: &str = "/home/p9-operator/farm3d";
/// A Printer name (Farm data).
pub const PRINTER_NAME: &str = "P9 Secret Printer Alpha";

/// Everything a diagnostics bundle or a log line must not contain.
pub const FULL_CORPUS: &[&str] = &[
    CREDENTIAL_VALUE,
    CREDENTIAL_VALUE_2,
    HEADER_VALUE,
    HEADER_LINE,
    USERINFO_URL,
    USERINFO,
    USERINFO_PASSWORD,
    QUERY_TOKEN,
    STORED_CAMERA_URL,
    HOST,
    HOST_NAME,
    HOME_PATH,
    PRINTER_NAME,
];

/// Everything a whole-Farm backup (and a journal, error, event, or
/// frontend state) must not contain.
pub const BACKUP_FORBIDDEN: &[&str] = &[
    CREDENTIAL_VALUE,
    CREDENTIAL_VALUE_2,
    HEADER_VALUE,
    HEADER_LINE,
    USERINFO_URL,
    USERINFO,
    USERINFO_PASSWORD,
];

/// Exact, lowercase, uppercase, and percent-encoded (both hex cases)
/// spellings of `needle`, deduplicated.
pub fn variants(needle: &str) -> Vec<Vec<u8>> {
    let mut all = vec![
        needle.as_bytes().to_vec(),
        needle.to_lowercase().into_bytes(),
        needle.to_uppercase().into_bytes(),
        percent_encode(needle, false).into_bytes(),
        percent_encode(needle, true).into_bytes(),
    ];
    all.sort();
    all.dedup();
    all
}

fn percent_encode(text: &str, lowercase_hex: bool) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else if lowercase_hex {
            encoded.push_str(&format!("%{byte:02x}"));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Where each needle was found, as `label: needle (form)`; empty when clean.
/// Scans raw bytes, then, when `bytes` is a zip, every entry name and every
/// entry's compressed and decompressed bytes, recursively.
pub fn find_any(needles: &[&str], bytes: &[u8], label: &str) -> Vec<String> {
    let mut hits = Vec::new();
    scan(needles, bytes, label, &mut hits);
    hits
}

fn scan(needles: &[&str], bytes: &[u8], label: &str, hits: &mut Vec<String>) {
    for needle in needles {
        for variant in variants(needle) {
            if contains(bytes, &variant) {
                hits.push(format!(
                    "{label}: {needle} (as {})",
                    String::from_utf8_lossy(&variant)
                ));
            }
        }
    }
    if !bytes.starts_with(b"PK") {
        return;
    }
    let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(bytes)) else {
        return;
    };
    for index in 0..archive.len() {
        let name = match archive.by_index_raw(index) {
            Ok(mut raw) => {
                let name = raw.name().to_string();
                scan(
                    needles,
                    name.as_bytes(),
                    &format!("{label}!{name} (entry name)"),
                    hits,
                );
                let mut compressed = Vec::new();
                let _ = raw.read_to_end(&mut compressed);
                scan(
                    needles,
                    &compressed,
                    &format!("{label}!{name} (compressed)"),
                    hits,
                );
                name
            }
            Err(_) => continue,
        };
        if let Ok(mut entry) = archive.by_index(index) {
            let mut decompressed = Vec::new();
            let _ = entry.read_to_end(&mut decompressed);
            scan(
                needles,
                &decompressed,
                &format!("{label}!{name} (decompressed)"),
                hits,
            );
        }
    }
}

/// Panics if any of `needles` appears in `bytes` (see [`find_any`]).
pub fn assert_none_of(needles: &[&str], bytes: &[u8], label: &str) {
    let hits = find_any(needles, bytes, label);
    assert!(
        hits.is_empty(),
        "seeded secrets found:\n{}",
        hits.join("\n")
    );
}

/// Panics if any of the full corpus appears in `bytes`.
pub fn assert_no_corpus(bytes: &[u8], label: &str) {
    assert_none_of(FULL_CORPUS, bytes, label);
}

/// Panics if any of [`BACKUP_FORBIDDEN`] appears in `bytes`.
pub fn assert_no_backup_forbidden(bytes: &[u8], label: &str) {
    assert_none_of(BACKUP_FORBIDDEN, bytes, label);
}

/// [`assert_no_corpus`] on a file's bytes.
pub fn assert_file_has_no_corpus(path: &std::path::Path) {
    let bytes =
        std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert_no_corpus(&bytes, &path.display().to_string());
}
