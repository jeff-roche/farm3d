//! Versioned wire contracts shared by the Rust backend and TypeScript frontend.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub mod command;
pub mod domain;
pub mod event;
pub mod inventory;
pub mod navigation;

pub(crate) const JS_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// The only contract version accepted by F1 wire types.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, TS)]
#[ts(type = "1")]
pub struct ContractVersion(());

impl ContractVersion {
    pub const V1: Self = Self(());
}

impl Serialize for ContractVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u8(1)
    }
}

impl<'de> Deserialize<'de> for ContractVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match u8::deserialize(deserializer)? {
            1 => Ok(Self::V1),
            received => Err(serde::de::Error::custom(format_args!(
                "expected contract version 1, received {received}"
            ))),
        }
    }
}
