//! D13's egress scan (ADR-0017): the last check before a diagnostics bundle
//! is written. The corpus is read from the live Farm in one read
//! transaction; every serialized entry is checked against it; a hit aborts
//! the export with `DIAGNOSTICS_REDACTION_FAILED` naming the section, and
//! the matched term is never reported or logged.
//!
//! Two classes of term:
//!
//! - **Secret** terms (credential values and refs, hosts, URLs and their
//!   query values, absolute paths) are matched against every byte of every
//!   entry in their exact, lowercase, uppercase, percent-encoded, form-
//!   encoded, and JSON-escaped forms. The haystack and the forms are both
//!   ASCII-lowercased, so any mix of case also matches.
//! - **Name** terms (user-authored names and notes, host-supplied job and
//!   file names) longer than 3 characters are matched against the string
//!   values of each entry, except values at the entry's closed paths
//!   (enums, bundle pseudonyms, and timestamps farm3d re-renders), in their
//!   exact and percent-encoded forms, so a Printer named after a state word
//!   doesn't block the export.
//!
//! A secret term shorter than 4 bytes is matched only as a whole string
//! value. An entry that doesn't parse as its format is a hit (fail closed).
//!
//! Credential values are held as `Zeroizing` bytes, searched with `memmem`
//! (which borrows its needle), and dropped with the corpus. The other terms
//! go through one Aho-Corasick automaton per class.
//!
//! The scan catches known values, not a transformed leak it has no form
//! for; the typed collectors and the typed log are the primary control.

use std::path::Path;

use aho_corasick::AhoCorasick;
use zeroize::Zeroizing;

use super::bundle::DiagnosticsSection;

/// A secret term's minimum length for a byte match; shorter ones match only
/// a whole string value.
const MIN_BYTE_TERM: usize = 4;
/// A name term must be longer than this many characters.
const MAX_IGNORED_NAME_CHARS: usize = 3;
/// A query value that is only ASCII letters and at most this long is a
/// vocabulary word (`action=snapshot`), matched as a name, not as a secret.
const MAX_WORD_QUERY_VALUE: usize = 12;

/// The protected values of one Farm, read for one export.
#[derive(Default)]
pub struct Corpus {
    secrets: Vec<String>,
    credentials: Vec<Zeroizing<String>>,
    names: Vec<String>,
}

impl Corpus {
    /// A credential value, compared and dropped with the corpus.
    pub fn add_credential(&mut self, value: Zeroizing<String>) {
        if !value.is_empty() {
            self.credentials.push(value);
        }
    }

    /// A host, a credential ref, or any other secret-class text.
    pub fn add_secret(&mut self, term: &str) {
        if !term.is_empty() {
            self.secrets.push(term.to_string());
        }
    }

    /// An absolute path. Also its form without a trailing separator.
    pub fn add_path(&mut self, path: &Path) {
        let text = path.to_string_lossy();
        self.add_secret(&text);
        let trimmed = text.trim_end_matches(['/', '\\']);
        if trimmed.len() != text.len() {
            self.add_secret(trimmed);
        }
    }

    /// A URL: the whole text, its host, userinfo, and each query value.
    pub fn add_url(&mut self, url: &str) {
        self.add_secret(url);
        let Ok(parsed) = reqwest::Url::parse(url) else {
            return;
        };
        if let Some(host) = parsed.host_str() {
            self.add_secret(host);
        }
        self.add_secret(parsed.username());
        if let Some(password) = parsed.password() {
            self.add_secret(password);
        }
        for (_, value) in parsed.query_pairs() {
            let is_word = value.len() <= MAX_WORD_QUERY_VALUE
                && value.bytes().all(|byte| byte.is_ascii_alphabetic());
            if is_word {
                self.add_name(&value);
            } else {
                self.add_secret(&value);
            }
        }
    }

    /// A user-authored or host-supplied name or note. Three characters or
    /// fewer is never a term.
    pub fn add_name(&mut self, term: &str) {
        if term.chars().count() > MAX_IGNORED_NAME_CHARS {
            self.names.push(term.to_string());
        }
    }

    /// Compiles the corpus for scanning.
    pub fn compile(self) -> CompiledCorpus {
        let mut long_secrets = Vec::new();
        let mut short_secrets = Vec::new();
        for term in &self.secrets {
            if term.len() < MIN_BYTE_TERM {
                short_secrets.push(term.to_ascii_lowercase());
            } else {
                long_secrets.extend(secret_forms(term));
            }
        }
        let mut credentials = Vec::new();
        for value in &self.credentials {
            if value.len() < MIN_BYTE_TERM {
                short_secrets.push(value.to_ascii_lowercase());
            } else {
                for form in secret_forms(value) {
                    credentials.push(Zeroizing::new(form));
                }
            }
        }
        long_secrets.sort();
        long_secrets.dedup();
        let mut names: Vec<String> = self
            .names
            .iter()
            .flat_map(|name| name_forms(name))
            .collect();
        names.sort();
        names.dedup();
        let secrets = automaton(&long_secrets);
        let names = automaton(&names);
        CompiledCorpus {
            unusable: secrets.is_err() || names.is_err(),
            secrets: secrets.unwrap_or(None),
            credentials,
            short_secrets,
            names: names.unwrap_or(None),
        }
    }
}

/// `Err` when the automaton can't be built (a size limit): the scan then
/// refuses every entry rather than skip the terms.
fn automaton(patterns: &[String]) -> Result<Option<AhoCorasick>, ()> {
    if patterns.is_empty() {
        return Ok(None);
    }
    AhoCorasick::new(patterns).map(Some).map_err(|_| ())
}

/// Percent-encodes every byte outside the unreserved set
/// (`encodeURIComponent`).
fn percent_encode_all(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Percent-encodes only what `encodeURI` would (reserved characters stay).
fn percent_encode_uri(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b':'
                    | b'/'
                    | b'?'
                    | b'#'
                    | b'['
                    | b']'
                    | b'@'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
            );
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Form encoding: `encodeURIComponent` with spaces as `+`.
fn form_encode(text: &str) -> String {
    percent_encode_all(text).replace("%20", "+")
}

/// The JSON string body of `text` (escapes, no quotes).
fn json_escaped(text: &str) -> String {
    let quoted = serde_json::to_string(text).unwrap_or_default();
    quoted
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_default()
        .to_string()
}

/// A secret term's forms, ASCII-lowercased (the haystack is lowercased
/// too, so exact, lowercase, uppercase, and mixed case all match, as do
/// both hex cases of a percent-encoding).
fn secret_forms(term: &str) -> Vec<String> {
    let mut forms = vec![
        term.to_string(),
        percent_encode_all(term),
        percent_encode_uri(term),
        form_encode(term),
        json_escaped(term),
    ];
    for form in &mut forms {
        form.make_ascii_lowercase();
    }
    forms.sort();
    forms.dedup();
    forms.retain(|form| form.len() >= MIN_BYTE_TERM);
    forms
}

/// A name term's forms: exact, and percent-encoded in both hex cases.
fn name_forms(term: &str) -> Vec<String> {
    let mut forms = vec![term.to_string()];
    for encoded in [
        percent_encode_all(term),
        percent_encode_uri(term),
        form_encode(term),
    ] {
        forms.push(encoded.to_ascii_lowercase_hex());
        forms.push(encoded);
    }
    forms.sort();
    forms.dedup();
    forms
}

trait LowercaseHex {
    fn to_ascii_lowercase_hex(&self) -> String;
}

impl LowercaseHex for String {
    /// `%2F` → `%2f`, leaving every other character alone.
    fn to_ascii_lowercase_hex(&self) -> String {
        let bytes = self.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' && index + 2 < bytes.len() {
                out.push(b'%');
                out.push(bytes[index + 1].to_ascii_lowercase());
                out.push(bytes[index + 2].to_ascii_lowercase());
                index += 3;
            } else {
                out.push(bytes[index]);
                index += 1;
            }
        }
        String::from_utf8(out).unwrap_or_else(|_| self.clone())
    }
}

/// Reads the corpus from the live Farm inside the caller's read
/// transaction. Returns it with every credential ref (both forms: each
/// stored ref, and the ref farm3d derives for every Printer id), whose
/// values the caller reads from the credential store.
pub fn read_corpus(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<(Corpus, Vec<String>)> {
    let mut corpus = Corpus::default();
    let mut refs = std::collections::BTreeSet::new();
    let texts = |sql: &str| -> rusqlite::Result<Vec<String>> {
        let mut statement = tx.prepare(sql)?;
        let rows = statement
            .query_map([], |row| row.get::<_, Option<String>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.into_iter().flatten().collect())
    };
    let json = |column: &str, pointer: &str, table: &str| {
        format!(
            "SELECT CASE WHEN json_valid({column}) THEN
                      CASE WHEN json_type({column}, '{pointer}') = 'text'
                           THEN json_extract({column}, '{pointer}') END
                    END
               FROM {table}"
        )
    };

    // Credential refs, both forms.
    for id in texts("SELECT id FROM printers")? {
        refs.insert(crate::connections::credentials::credential_ref_for(&id));
    }
    refs.extend(texts(&json(
        "connection_json",
        "$.credentialRef",
        "printers",
    ))?);
    refs.extend(texts(
        "SELECT credential_ref FROM pending_credential_cleanup",
    )?);
    for reference in &refs {
        corpus.add_secret(reference);
    }

    // Hosts: every Connection and Host Operation endpoint.
    for host in texts(&json("connection_json", "$.host", "printers"))?
        .into_iter()
        .chain(texts(&json("endpoint_json", "$.host", "host_operations"))?)
    {
        corpus.add_secret(&host);
    }
    // Camera URLs, with each query value.
    for url in texts("SELECT snapshot_url FROM printer_cameras")? {
        corpus.add_url(&url);
    }
    // Absolute paths: linked Models, source revisions, the Slicer runtime.
    for path in texts("SELECT linked_path FROM library_models")?
        .into_iter()
        .chain(texts("SELECT source_path FROM model_source_revisions")?)
        .chain(texts("SELECT engine_path FROM slicer_runtime_config")?)
        .chain(texts(
            "SELECT preset_source_path FROM slicer_runtime_config",
        )?)
    {
        corpus.add_path(Path::new(&path));
    }
    // Names and notes, and host-supplied names.
    for sql in [
        "SELECT name FROM printers",
        "SELECT location FROM printers",
        "SELECT notes FROM printers",
        "SELECT manufacturer FROM spools",
        "SELECT product FROM spools",
        "SELECT color_name FROM spools",
        "SELECT storage_label FROM spools",
        "SELECT name FROM spool_tares",
        "SELECT name FROM library_models",
        "SELECT name FROM library_projects",
        "SELECT source_file_name FROM model_source_revisions",
        "SELECT webcam_name FROM printer_cameras",
        "SELECT host_path FROM jobs",
        "SELECT host_path FROM host_operations",
        "SELECT source_name FROM migration_warnings",
    ] {
        for name in texts(sql)? {
            corpus.add_name(&name);
        }
    }
    for note in texts(
        "SELECT CASE WHEN json_valid(detail_json) THEN
                  CASE WHEN json_type(detail_json, '$.text') = 'text'
                       THEN json_extract(detail_json, '$.text') END
                END
           FROM incident_events WHERE kind = 'noteAdded'",
    )? {
        corpus.add_name(&note);
    }
    for job_name in texts(&json(
        "telemetry_json",
        "$.jobName",
        "printer_status_snapshots",
    ))? {
        corpus.add_name(&job_name);
    }
    Ok((corpus, refs.into_iter().collect()))
}

/// The compiled corpus of one export.
pub struct CompiledCorpus {
    /// An automaton couldn't be built: every entry is refused.
    unusable: bool,
    secrets: Option<AhoCorasick>,
    credentials: Vec<Zeroizing<String>>,
    short_secrets: Vec<String>,
    names: Option<AhoCorasick>,
}

/// How an entry is parsed for the string-value checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryFormat {
    /// One JSON document.
    Json,
    /// JSON lines (a log file).
    JsonLines,
}

/// One serialized entry, as the scan sees it.
pub struct ScanEntry<'a> {
    pub section: DiagnosticsSection,
    pub bytes: &'a [u8],
    pub format: EntryFormat,
    /// Paths whose string values are closed vocabulary (skipped by the
    /// name and short-secret checks). `a.b`, `a[].b`; a trailing `.*`
    /// matches every child.
    pub closed_paths: &'a [&'a str],
}

/// `Err(section)` for the first entry that holds a corpus term. The term
/// itself is never returned.
pub fn scan(corpus: &CompiledCorpus, entries: &[ScanEntry<'_>]) -> Result<(), DiagnosticsSection> {
    for entry in entries {
        if !entry_is_clean(corpus, entry) {
            return Err(entry.section);
        }
    }
    Ok(())
}

fn entry_is_clean(corpus: &CompiledCorpus, entry: &ScanEntry<'_>) -> bool {
    if corpus.unusable {
        return false;
    }
    let lowered = Zeroizing::new(entry.bytes.to_ascii_lowercase());
    if let Some(secrets) = &corpus.secrets {
        if secrets.is_match(lowered.as_slice()) {
            return false;
        }
    }
    for form in &corpus.credentials {
        if memchr::memmem::find(&lowered, form.as_bytes()).is_some() {
            return false;
        }
    }
    let mut values = Vec::new();
    match entry.format {
        EntryFormat::Json => {
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(entry.bytes) else {
                return false;
            };
            collect_strings(&value, String::new(), entry.closed_paths, &mut values);
        }
        EntryFormat::JsonLines => {
            let Ok(text) = std::str::from_utf8(entry.bytes) else {
                return false;
            };
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                    return false;
                };
                collect_strings(&value, String::new(), entry.closed_paths, &mut values);
            }
        }
    }
    values.iter().all(|value| {
        let lowered = value.to_ascii_lowercase();
        if corpus.short_secrets.iter().any(|term| *term == lowered) {
            return false;
        }
        corpus
            .names
            .as_ref()
            .is_none_or(|names| !names.is_match(value.as_str()))
    })
}

fn is_closed(path: &str, closed_paths: &[&str]) -> bool {
    closed_paths
        .iter()
        .any(|closed| match closed.strip_suffix(".*") {
            Some(prefix) => path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('.')),
            None => path == *closed,
        })
}

/// Every string value outside the closed paths.
fn collect_strings(
    value: &serde_json::Value,
    path: String,
    closed_paths: &[&str],
    out: &mut Vec<String>,
) {
    match value {
        serde_json::Value::String(text) => {
            if !is_closed(&path, closed_paths) {
                out.push(text.clone());
            }
        }
        serde_json::Value::Array(items) => {
            let child = format!("{path}[]");
            for item in items {
                collect_strings(item, child.clone(), closed_paths, out);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, item) in map {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                collect_strings(item, child, closed_paths, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOSED: &[&str] = &["printers[].adapterKind", "fields.*"];

    fn corpus(build: impl FnOnce(&mut Corpus)) -> CompiledCorpus {
        let mut corpus = Corpus::default();
        build(&mut corpus);
        corpus.compile()
    }

    fn json(corpus: &CompiledCorpus, bytes: &str) -> bool {
        entry_is_clean(
            corpus,
            &ScanEntry {
                section: DiagnosticsSection::Health,
                bytes: bytes.as_bytes(),
                format: EntryFormat::Json,
                closed_paths: CLOSED,
            },
        )
    }

    #[test]
    fn secret_terms_match_any_byte_in_any_case_or_encoding() {
        let compiled = corpus(|corpus| {
            corpus.add_secret("printer-p9.farm.example");
            corpus.add_path(Path::new("/home/p9 operator/farm3d/"));
            corpus.add_credential(Zeroizing::new("cred-P9-8f2a61c4d7e0".to_string()));
        });
        assert!(json(&compiled, r#"{"a":"clean"}"#));
        for leak in [
            r#"{"a":"x PRINTER-P9.FARM.EXAMPLE y"}"#,
            r#"{"a":"Printer-P9.Farm.Example"}"#,
            r#"{"printers":[{"adapterKind":"printer-p9.farm.example"}]}"#,
            r#"{"a":"%2Fhome%2Fp9%20operator%2Ffarm3d"}"#,
            r#"{"a":"%2fhome%2fp9%20operator%2ffarm3d"}"#,
            r#"{"a":"/home/p9%20operator/farm3d"}"#,
            r#"{"a":"/home/p9 operator/farm3d"}"#,
            r#"{"a":"%2Fhome%2Fp9+operator%2Ffarm3d"}"#,
            r#"{"cred-p9-8f2a61c4d7e0":1}"#,
            r#"{"a":"CRED-P9-8F2A61C4D7E0"}"#,
        ] {
            assert!(!json(&compiled, leak), "{leak}");
        }
    }

    #[test]
    fn a_windows_path_is_caught_json_escaped() {
        let compiled = corpus(|corpus| corpus.add_path(Path::new(r"C:\Users\op\farm3d")));
        let escaped = serde_json::json!({ "a": r"C:\Users\op\farm3d\x" }).to_string();
        assert!(!json(&compiled, &escaped));
    }

    #[test]
    fn name_terms_match_string_values_outside_closed_paths_only() {
        let compiled = corpus(|corpus| {
            corpus.add_name("moonraker");
            corpus.add_name("P9 Secret Printer");
            corpus.add_name("job");
        });
        // Closed: an enum value equal to a name.
        assert!(json(
            &compiled,
            r#"{"printers":[{"adapterKind":"moonraker"}]}"#
        ));
        assert!(!json(&compiled, r#"{"printers":[{"other":"moonraker"}]}"#));
        // Exact case only; percent-encoded in both hex cases.
        assert!(json(&compiled, r#"{"a":"MOONRAKER"}"#));
        assert!(!json(&compiled, r#"{"a":"P9%20Secret%20Printer"}"#));
        assert!(!json(&compiled, r#"{"a":"P9+Secret+Printer"}"#));
        // A key is not a value, and a three-character name is no term.
        assert!(json(&compiled, r#"{"P9 Secret Printer":1,"b":"job"}"#));
    }

    #[test]
    fn a_short_secret_matches_only_a_whole_string_value() {
        let compiled = corpus(|corpus| corpus.add_secret("ab1"));
        assert!(json(&compiled, r#"{"a":"xab1y"}"#));
        assert!(!json(&compiled, r#"{"a":"AB1"}"#));
        // Closed values are skipped.
        assert!(json(&compiled, r#"{"printers":[{"adapterKind":"ab1"}]}"#));
    }

    #[test]
    fn query_values_are_terms_and_vocabulary_words_are_names() {
        let compiled = corpus(|corpus| {
            corpus.add_url("http://192.0.2.19:8080/webcam/?action=snapshot&token=tok-P9-19");
        });
        assert!(!json(&compiled, r#"{"a":"tok-p9-19"}"#));
        assert!(!json(&compiled, r#"{"a":"192.0.2.19"}"#));
        // `snapshot` is a word: a key or a closed value may hold it.
        assert!(json(&compiled, r#"{"snapshotRetention":1}"#));
        assert!(!json(&compiled, r#"{"a":"snapshot"}"#));
    }

    #[test]
    fn unparseable_entries_fail_closed_and_log_lines_are_checked_one_by_one() {
        let compiled = corpus(|corpus| corpus.add_name("P9 Secret Printer"));
        assert!(!json(&compiled, "{not json"));
        let lines = |text: &str| {
            entry_is_clean(
                &compiled,
                &ScanEntry {
                    section: DiagnosticsSection::Logs,
                    bytes: text.as_bytes(),
                    format: EntryFormat::JsonLines,
                    closed_paths: CLOSED,
                },
            )
        };
        assert!(lines(
            "{\"fields\":{\"e\":\"P9 Secret Printer\"}}\n{\"a\":1}\n"
        ));
        assert!(!lines("{\"a\":1}\n{\"leak\":\"P9 Secret Printer\"}\n"));
        assert!(!lines("{\"a\":1}\nnot json\n"));
    }

    #[test]
    fn scan_names_the_section_of_the_first_hit() {
        let compiled = corpus(|corpus| corpus.add_secret("192.0.2.19"));
        let entry = |section, bytes: &'static str| ScanEntry {
            section,
            bytes: bytes.as_bytes(),
            format: EntryFormat::Json,
            closed_paths: &[],
        };
        assert_eq!(
            scan(
                &compiled,
                &[
                    entry(DiagnosticsSection::About, "{}"),
                    entry(DiagnosticsSection::Storage, r#"{"a":"192.0.2.19"}"#),
                ]
            ),
            Err(DiagnosticsSection::Storage)
        );
    }
}
