//! The adapter registry: the one place a `kind` string becomes an
//! observe-connection builder and its capability builders, so
//! `connections::is_supported_kind` and `supervisor::build_connection`
//! share a single source of truth. Moonraker and OctoPrint have no
//! capability builders yet (Task 8 adds Moonraker's) and no evidence rows
//! (Task 12 adds those), so `capabilities::capabilities_for` and
//! `capabilities::adapter_capability_matrix` report every capability
//! `notVerified` for both today.

use super::capabilities::{
    ArtifactStaging, CameraDiscovery, CapabilityEvidence, CapabilityKey, HostStateQuery,
    PrintControl,
};
use super::moonraker::MoonrakerConnection;
use super::octoprint::OctoPrintConnection;
use super::{ConnectionConfig, PrinterConnection, MOONRAKER_KIND, OCTOPRINT_KIND};

/// Builds the always-on observation connection for one adapter kind.
/// A plain `fn` pointer (not a `Fn` closure) so descriptors can live in a
/// `const` table.
pub type ObserveBuilder =
    fn(&ConnectionConfig, Option<zeroize::Zeroizing<String>>) -> Box<dyn PrinterConnection>;

pub type StagingBuilder =
    fn(&ConnectionConfig, Option<zeroize::Zeroizing<String>>) -> Box<dyn ArtifactStaging>;
pub type ControlBuilder =
    fn(&ConnectionConfig, Option<zeroize::Zeroizing<String>>) -> Box<dyn PrintControl>;
pub type HostStateBuilder =
    fn(&ConnectionConfig, Option<zeroize::Zeroizing<String>>) -> Box<dyn HostStateQuery>;
pub type CameraBuilder =
    fn(&ConnectionConfig, Option<zeroize::Zeroizing<String>>) -> Box<dyn CameraDiscovery>;

pub struct AdapterDescriptor {
    pub kind: &'static str,
    pub observe: ObserveBuilder,
    pub staging: Option<StagingBuilder>,
    pub control: Option<ControlBuilder>,
    pub host_state: Option<HostStateBuilder>,
    pub camera: Option<CameraBuilder>,
    /// Per-capability evidence (D6) — the UI shows each capability's own
    /// tier, so this is never one evidence value for the whole adapter.
    pub evidence: &'static [(CapabilityKey, CapabilityEvidence)],
}

fn moonraker_observe(
    config: &ConnectionConfig,
    api_key: Option<zeroize::Zeroizing<String>>,
) -> Box<dyn PrinterConnection> {
    Box::new(MoonrakerConnection::with_zeroizing_secret(
        config.clone(),
        api_key,
    ))
}

fn octoprint_observe(
    config: &ConnectionConfig,
    api_key: Option<zeroize::Zeroizing<String>>,
) -> Box<dyn PrinterConnection> {
    Box::new(OctoPrintConnection::with_zeroizing_secret(
        config.clone(),
        api_key,
    ))
}

const REGISTRY: &[AdapterDescriptor] = &[
    AdapterDescriptor {
        kind: MOONRAKER_KIND,
        observe: moonraker_observe,
        staging: None,
        control: None,
        host_state: None,
        camera: None,
        evidence: &[],
    },
    AdapterDescriptor {
        kind: OCTOPRINT_KIND,
        observe: octoprint_observe,
        staging: None,
        control: None,
        host_state: None,
        camera: None,
        evidence: &[],
    },
];

/// Every adapter this build can construct, in the order `SUPPORTED_KINDS`
/// must match.
pub fn registry() -> &'static [AdapterDescriptor] {
    REGISTRY
}

/// Looks up one adapter's descriptor by its `ConnectionConfig.kind`.
pub fn descriptor(kind: &str) -> Option<&'static AdapterDescriptor> {
    REGISTRY.iter().find(|d| d.kind == kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(kind: &str) -> ConnectionConfig {
        ConnectionConfig {
            kind: kind.to_string(),
            host: "printer.local".to_string(),
            port: 80,
            use_tls: false,
            credential_ref: None,
        }
    }

    #[test]
    fn descriptor_finds_moonraker_and_octoprint() {
        assert!(descriptor("moonraker").is_some());
        assert!(descriptor("octoprint").is_some());
    }

    #[test]
    fn descriptor_is_none_for_an_unknown_kind() {
        assert!(descriptor("elegoolink").is_none());
    }

    #[test]
    fn every_descriptor_observe_builds_a_connection() {
        for descriptor in registry() {
            // No panic building the connection; this only checks the
            // builder runs, not that it can reach a host.
            let _connection = (descriptor.observe)(&config(descriptor.kind), None);
        }
    }

    #[test]
    fn supported_kinds_matches_the_registry_in_order() {
        let registry_kinds: Vec<&str> = registry().iter().map(|d| d.kind).collect();
        assert_eq!(super::super::SUPPORTED_KINDS, registry_kinds.as_slice());
    }
}
