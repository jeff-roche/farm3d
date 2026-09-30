//! `Pseudonym`: a stable-within-a-run, irreversible stand-in for an id.

use std::fmt::Write as _;
use std::sync::OnceLock;

use sha2::{Digest, Sha256};

/// `<kind>-` plus the first 8 hex digits of SHA-256 over a random 16-byte
/// salt, `0x00`, the kind, `0x00`, and the value. Built only by
/// [`Pseudonym::of`] (the process salt) or [`Pseudonym::of_with_salt`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pseudonym(String);

static PROCESS_SALT: OnceLock<[u8; 16]> = OnceLock::new();

/// The random salt of this process. Never persisted.
fn process_salt() -> &'static [u8; 16] {
    PROCESS_SALT.get_or_init(|| *uuid::Uuid::new_v4().as_bytes())
}

impl Pseudonym {
    /// A pseudonym under this process's salt: it correlates lines within one
    /// run and can't be reversed.
    pub fn of(kind: &'static str, value: &str) -> Self {
        Self::of_with_salt(process_salt(), kind, value)
    }

    /// A pseudonym under an explicit salt (a bundle's fresh salt, or a test).
    pub fn of_with_salt(salt: &[u8; 16], kind: &'static str, value: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(salt);
        hasher.update([0u8]);
        hasher.update(kind.as_bytes());
        hasher.update([0u8]);
        hasher.update(value.as_bytes());
        let digest = hasher.finalize();
        let mut text = String::with_capacity(kind.len() + 9);
        text.push_str(kind);
        text.push('-');
        for byte in &digest[..4] {
            let _ = write!(text, "{byte:02x}");
        }
        Self(text)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// D13's bundle pseudonymizer: per-bundle ordinals. Every id that appears
/// in a bundle is noted first; [`BundlePseudonyms::assign`] then sorts each
/// kind's ids by SHA-256(salt ‖ id) and numbers them from 1 (`printer-3`,
/// `job-12`). A fresh random salt per bundle makes the numbering stable
/// within one bundle and different between bundles; the salt is never
/// written anywhere.
pub struct BundlePseudonyms {
    salt: [u8; 16],
    noted: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
}

impl Default for BundlePseudonyms {
    fn default() -> Self {
        Self::new()
    }
}

impl BundlePseudonyms {
    /// A fresh random salt.
    pub fn new() -> Self {
        Self::with_salt(*uuid::Uuid::new_v4().as_bytes())
    }

    pub fn with_salt(salt: [u8; 16]) -> Self {
        Self {
            salt,
            noted: Default::default(),
        }
    }

    /// Records that `id` of `kind` appears in the bundle.
    pub fn note(&mut self, kind: &str, id: &str) {
        self.noted
            .entry(kind.to_string())
            .or_default()
            .insert(id.to_string());
    }

    /// Numbers every noted id.
    pub fn assign(self) -> AssignedPseudonyms {
        let mut names = std::collections::HashMap::new();
        for (kind, ids) in self.noted {
            let mut ordered: Vec<([u8; 32], String)> = ids
                .into_iter()
                .map(|id| {
                    let mut hasher = Sha256::new();
                    hasher.update(self.salt);
                    hasher.update(id.as_bytes());
                    (hasher.finalize().into(), id)
                })
                .collect();
            // Ties (never in practice) fall back to the hash-ordered id, so
            // the order is still total.
            ordered.sort();
            for (index, (_, id)) in ordered.into_iter().enumerate() {
                names.insert((kind.clone(), id), format!("{kind}-{}", index + 1));
            }
        }
        AssignedPseudonyms { names }
    }
}

/// The numbered pseudonyms of one bundle.
pub struct AssignedPseudonyms {
    names: std::collections::HashMap<(String, String), String>,
}

impl AssignedPseudonyms {
    /// `id`'s pseudonym. An id that wasn't noted gets `<kind>-0`, which no
    /// noted id has, so a collector bug never leaks the raw id.
    pub fn name(&self, kind: &str, id: &str) -> String {
        self.names
            .get(&(kind.to_string(), id.to_string()))
            .cloned()
            .unwrap_or_else(|| format!("{kind}-0"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbering(salt: [u8; 16], ids: &[String]) -> Vec<String> {
        let mut pseudonyms = BundlePseudonyms::with_salt(salt);
        for id in ids {
            pseudonyms.note("printer", id);
        }
        let assigned = pseudonyms.assign();
        ids.iter().map(|id| assigned.name("printer", id)).collect()
    }

    #[test]
    fn ordinals_are_stable_under_one_salt_and_differ_under_another() {
        let ids: Vec<String> = (0..16).map(|index| format!("prn-{index}")).collect();
        let first = numbering([1; 16], &ids);
        assert_eq!(first, numbering([1; 16], &ids));
        assert_ne!(first, numbering([2; 16], &ids));
        let mut sorted = first.clone();
        sorted.sort_by_key(|name| name[8..].parse::<usize>().unwrap());
        let expected: Vec<String> = (1..=16).map(|n| format!("printer-{n}")).collect();
        assert_eq!(sorted, expected);
    }

    #[test]
    fn kinds_are_numbered_apart_and_an_unnoted_id_never_leaks() {
        let mut pseudonyms = BundlePseudonyms::with_salt([3; 16]);
        pseudonyms.note("printer", "prn-a");
        pseudonyms.note("job", "job-a");
        pseudonyms.note("printer", "prn-a");
        let assigned = pseudonyms.assign();
        assert_eq!(assigned.name("printer", "prn-a"), "printer-1");
        assert_eq!(assigned.name("job", "job-a"), "job-1");
        assert_eq!(assigned.name("job", "prn-a"), "job-0");
    }
}
