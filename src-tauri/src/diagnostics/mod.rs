//! P9 D12 (ADR-0017): the safe-by-construction diagnostics log.
//!
//! [`log`] accepts only typed, closed-vocabulary values; [`pseudonym`] holds
//! the one-way, per-process pseudonyms that stand in for ids.

pub mod log;
pub mod pseudonym;
