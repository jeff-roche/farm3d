//! P9 D12 (ADR-0017): the safe-by-construction diagnostics log.
//!
//! [`log`] accepts only typed, closed-vocabulary values; [`pseudonym`] holds
//! the one-way, per-process pseudonyms that stand in for ids; [`reset`]
//! holds the three reset tiers (D15) and [`commands`] their commands.

pub mod commands;
pub mod log;
pub mod pseudonym;
pub mod reset;
