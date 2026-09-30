//! `AboutInfo` (spec "Wire types"): what `about_farm3d` returns and the
//! diagnostics bundle's `about` section. Typed facts only: the Slicer is
//! described by whether it is available and its version, never its path.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::catalog::Catalog;
use crate::connections::credentials::{CredentialStore, CredentialStoreKind};
use crate::slicing::runtime::{EngineState, SlicerRuntimeStatus};

/// `backupFormatVersion` is always 1 in this release.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AboutInfo.ts")]
pub struct AboutInfo {
    pub app_version: String,
    #[ts(type = "number")]
    pub schema_version: i64,
    #[ts(type = "1")]
    pub backup_format_version: u8,
    pub platform: AboutPlatform,
    pub catalog: AboutCatalog,
    pub slicer: AboutSlicer,
    pub credential_store: AboutCredentialStore,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AboutPlatform.ts")]
pub struct AboutPlatform {
    pub os: String,
    pub arch: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AboutCatalog.ts")]
pub struct AboutCatalog {
    pub source_tag: String,
    pub generated_at: String,
}

/// Never a path.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AboutSlicer.ts")]
pub struct AboutSlicer {
    pub configured: bool,
    pub version: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AboutCredentialStore.ts")]
pub struct AboutCredentialStore {
    pub kind: CredentialStoreKind,
    pub available: bool,
}

/// A version is reported only as a token of 1–64 characters from
/// `[0-9A-Za-z.+-]` (D13's host-software rule); anything else is `None`.
pub fn version_token(text: &str) -> Option<String> {
    let valid = (1..=64).contains(&text.len())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'));
    valid.then(|| text.to_string())
}

/// The Slicer as About reports it: `configured` when an OrcaSlicer engine
/// farm3d accepts is available; its version (as a token) whenever the
/// probe read one.
pub fn slicer(status: Option<&SlicerRuntimeStatus>) -> AboutSlicer {
    match status.map(|status| &status.engine) {
        Some(EngineState::Available { version, .. }) => AboutSlicer {
            configured: true,
            version: version_token(version),
        },
        Some(EngineState::UnsupportedVersion { version, .. }) => AboutSlicer {
            configured: false,
            version: version_token(version),
        },
        _ => AboutSlicer {
            configured: false,
            version: None,
        },
    }
}

/// Builds `AboutInfo`. `app_version` comes from `app.package_info()`.
pub fn about(
    app_version: &str,
    catalog: &Catalog,
    slicer_status: Option<&SlicerRuntimeStatus>,
    credentials: &CredentialStore,
) -> AboutInfo {
    AboutInfo {
        app_version: app_version.to_string(),
        schema_version: crate::persistence::CURRENT_SCHEMA_VERSION,
        backup_format_version: 1,
        platform: AboutPlatform {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        },
        catalog: AboutCatalog {
            source_tag: catalog.source_tag.clone(),
            generated_at: catalog.generated_at.clone(),
        },
        slicer: slicer(slicer_status),
        credential_store: AboutCredentialStore {
            kind: credentials.kind(),
            available: credentials.unavailable_reason_code().is_none(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_a_short_token_or_nothing() {
        assert_eq!(
            version_token("v0.9.3-1+g1a2b"),
            Some("v0.9.3-1+g1a2b".into())
        );
        assert_eq!(version_token("2.3.0"), Some("2.3.0".into()));
        for rejected in [
            "",
            "Moonraker v0.9",
            "/usr/bin/orca",
            "a_b",
            &"9".repeat(65),
        ] {
            assert_eq!(version_token(rejected), None, "{rejected}");
        }
    }
}
