//! P9 D12–D15 (ADR-0017): the safe-by-construction diagnostics log, the
//! diagnostics bundle, and the tiered reset.
//!
//! [`log`] accepts only typed, closed-vocabulary values; [`pseudonym`] holds
//! the one-way, per-process pseudonyms that stand in for ids and the
//! per-bundle pseudonymizer; [`collect`], [`egress`], and [`bundle`] build
//! the diagnostics bundle (D13) and [`about`] the About facts; [`reset`]
//! holds the three reset tiers (D15); [`commands`] the commands.

use std::path::PathBuf;
use std::sync::Arc;

pub mod about;
pub mod bundle;
pub mod collect;
pub mod commands;
pub mod egress;
pub mod log;
pub mod pseudonym;
pub mod reset;

/// A test-only hook over each serialized bundle entry, run after
/// serialization and before the egress scan (a deliberately leaking
/// collector). Production never sets one.
pub type EntryHook = Arc<dyn Fn(bundle::DiagnosticsSection, &str, &mut Vec<u8>) + Send + Sync>;

/// `RuntimeServices::diagnostics`: what the diagnostics commands need
/// beside the other services.
#[derive(Clone, Default)]
pub struct DiagnosticsServices {
    home_dir: Option<PathBuf>,
    entry_hook: Option<EntryHook>,
}

impl DiagnosticsServices {
    /// Adds `home` to the home directories the egress scan protects (the
    /// process's own is always included).
    pub fn with_home_dir(mut self, home: PathBuf) -> Self {
        self.home_dir = Some(home);
        self
    }

    #[doc(hidden)]
    pub fn with_entry_hook(mut self, hook: EntryHook) -> Self {
        self.entry_hook = Some(hook);
        self
    }

    /// The home directories the egress scan protects.
    pub fn home_dirs(&self) -> Vec<PathBuf> {
        self.home_dir
            .iter()
            .cloned()
            .chain(std::env::home_dir())
            .collect()
    }

    pub(crate) fn entry_hook(&self) -> Option<&EntryHook> {
        self.entry_hook.as_ref()
    }
}
