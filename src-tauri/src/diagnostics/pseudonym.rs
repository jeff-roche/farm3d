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
