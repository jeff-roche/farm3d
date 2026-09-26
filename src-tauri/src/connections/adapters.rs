//! The adapter registry: the one place a `kind` string becomes an
//! observe-connection builder and its capability builders, so
//! `connections::is_supported_kind` and `supervisor::build_connection`
//! share a single source of truth. Moonraker has all four capability
//! builders (`moonraker::control`); OctoPrint has none. Neither has evidence
//! rows yet (Task 12 adds Moonraker's), so `capabilities::capabilities_for`
//! and `capabilities::adapter_capability_matrix` report every capability
//! `notVerified` for both today.

use super::capabilities::{
    ArtifactStaging, CameraDiscovery, CapabilityEvidence, CapabilityKey, HostStateQuery,
    PrintControl,
};
use super::moonraker::control::{MoonrakerCapabilities, MoonrakerTimings};
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

/// The capability builders all build the same HTTP adapter with the
/// production timings (D10).
fn moonraker_capabilities(
    config: &ConnectionConfig,
    api_key: Option<zeroize::Zeroizing<String>>,
) -> MoonrakerCapabilities {
    MoonrakerCapabilities::new(config, api_key, MoonrakerTimings::default())
}

fn moonraker_staging(
    config: &ConnectionConfig,
    api_key: Option<zeroize::Zeroizing<String>>,
) -> Box<dyn ArtifactStaging> {
    Box::new(moonraker_capabilities(config, api_key))
}

fn moonraker_control(
    config: &ConnectionConfig,
    api_key: Option<zeroize::Zeroizing<String>>,
) -> Box<dyn PrintControl> {
    Box::new(moonraker_capabilities(config, api_key))
}

fn moonraker_host_state(
    config: &ConnectionConfig,
    api_key: Option<zeroize::Zeroizing<String>>,
) -> Box<dyn HostStateQuery> {
    Box::new(moonraker_capabilities(config, api_key))
}

fn moonraker_camera(
    config: &ConnectionConfig,
    api_key: Option<zeroize::Zeroizing<String>>,
) -> Box<dyn CameraDiscovery> {
    Box::new(moonraker_capabilities(config, api_key))
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
        staging: Some(moonraker_staging),
        control: Some(moonraker_control),
        host_state: Some(moonraker_host_state),
        camera: Some(moonraker_camera),
        // Task 12 adds the simulator evidence; until then every capability
        // stays `notVerified` (D6 rule 4).
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
    fn moonraker_has_all_four_capability_builders_and_no_evidence_yet() {
        let moonraker = descriptor(MOONRAKER_KIND).unwrap();
        let config = config(MOONRAKER_KIND);
        let _staging = (moonraker.staging.expect("staging"))(&config, None);
        let _control = (moonraker.control.expect("control"))(&config, None);
        let _host_state = (moonraker.host_state.expect("host state"))(&config, None);
        let _camera = (moonraker.camera.expect("camera"))(&config, None);
        assert!(moonraker.evidence.is_empty());
    }

    #[test]
    fn octoprint_has_no_capability_builders() {
        let octoprint = descriptor(OCTOPRINT_KIND).unwrap();
        assert!(octoprint.staging.is_none());
        assert!(octoprint.control.is_none());
        assert!(octoprint.host_state.is_none());
        assert!(octoprint.camera.is_none());
    }

    #[test]
    fn supported_kinds_matches_the_registry_in_order() {
        let registry_kinds: Vec<&str> = registry().iter().map(|d| d.kind).collect();
        assert_eq!(super::super::SUPPORTED_KINDS, registry_kinds.as_slice());
    }
}
