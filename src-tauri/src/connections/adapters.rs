//! The adapter registry: the one place a `kind` string becomes an
//! observe-connection builder and its capability builders, so
//! `connections::is_supported_kind` and `supervisor::build_connection`
//! share a single source of truth. Moonraker has all four capability
//! builders (`moonraker::control`) and a simulator evidence row for every
//! capability (P6 Task 12); OctoPrint has neither, so
//! `capabilities::capabilities_for` reports every OctoPrint capability
//! `notVerified`.

use std::sync::LazyLock;

use super::capabilities::{
    ArtifactStaging, CameraDiscovery, CapabilityEvidence, CapabilityKey, EvidenceTier,
    HostStateQuery, PrintControl,
};
use super::moonraker::control::{MoonrakerCapabilities, MoonrakerTimings};
use super::moonraker::MoonrakerConnection;
use super::octoprint::OctoPrintConnection;
use super::{ConnectionConfig, PrinterConnection, MOONRAKER_KIND, OCTOPRINT_KIND};

/// Builds the always-on observation connection for one adapter kind.
/// A plain `fn` pointer (not a `Fn` closure) so descriptors can live in a
/// `static` table.
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

/// The P6 simulator run (`just test-sim`) that is Moonraker's evidence:
/// `src-tauri/target/sim-runs/<UTC>/manifest.json`, recorded against the
/// commit that added the P6 scenarios to `tests/sim_moonraker.rs`.
pub const MOONRAKER_SIM_MANIFEST: &str = "sim-runs/20260926T033455Z/manifest.json";

/// The Moonraker that run tested (its manifest's `reported` versions).
pub const MOONRAKER_SIM_VERSION: &str = "Moonraker v0.11.0-1-g1cfb0c4-prind API 1.5.0";

/// A real host the read-only suite (`just p6-readonly`) passed against.
/// It verified reads only, so only the read capabilities carry it (D6: a
/// read-only result never makes a write capability supported).
pub const MOONRAKER_READ_ONLY_VERSION: &str = "Moonraker 1.5.2 API 1.4.0 (read-only hardware)";

/// D6 "Rows at the end of P6": a `sim` row for every Moonraker capability.
/// `camera` is the camera query only; the simulator has no webcam (Gate H).
static MOONRAKER_EVIDENCE: LazyLock<Vec<(CapabilityKey, CapabilityEvidence)>> =
    LazyLock::new(|| {
        CapabilityKey::ALL
            .into_iter()
            .map(|key| {
                let mut versions = vec![MOONRAKER_SIM_VERSION.to_string()];
                if matches!(
                    key,
                    CapabilityKey::HostState
                        | CapabilityKey::ArtifactIdentity
                        | CapabilityKey::Camera
                ) {
                    versions.push(MOONRAKER_READ_ONLY_VERSION.to_string());
                }
                (
                    key,
                    CapabilityEvidence {
                        source: MOONRAKER_SIM_MANIFEST.to_string(),
                        tier: EvidenceTier::Sim,
                        verified_host_versions: versions,
                    },
                )
            })
            .collect()
    });

static REGISTRY: LazyLock<[AdapterDescriptor; 2]> = LazyLock::new(|| {
    [
        AdapterDescriptor {
            kind: MOONRAKER_KIND,
            observe: moonraker_observe,
            staging: Some(moonraker_staging),
            control: Some(moonraker_control),
            host_state: Some(moonraker_host_state),
            camera: Some(moonraker_camera),
            evidence: MOONRAKER_EVIDENCE.as_slice(),
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
    ]
});

/// Every adapter this build can construct, in the order `SUPPORTED_KINDS`
/// must match.
pub fn registry() -> &'static [AdapterDescriptor] {
    REGISTRY.as_slice()
}

/// Looks up one adapter's descriptor by its `ConnectionConfig.kind`.
pub fn descriptor(kind: &str) -> Option<&'static AdapterDescriptor> {
    REGISTRY.iter().find(|d| d.kind == kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::capabilities::EvidenceTier;

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
    fn moonraker_has_all_four_capability_builders() {
        let moonraker = descriptor(MOONRAKER_KIND).unwrap();
        let config = config(MOONRAKER_KIND);
        let _staging = (moonraker.staging.expect("staging"))(&config, None);
        let _control = (moonraker.control.expect("control"))(&config, None);
        let _host_state = (moonraker.host_state.expect("host state"))(&config, None);
        let _camera = (moonraker.camera.expect("camera"))(&config, None);
    }

    /// Task 12 (D6): one `sim` row per capability, each naming the P6
    /// simulator run's manifest. Read-only hardware adds a version to the
    /// read capabilities only, never to a write.
    #[test]
    fn moonraker_has_sim_evidence_for_every_capability_from_the_p6_run() {
        let moonraker = descriptor(MOONRAKER_KIND).unwrap();
        assert_eq!(moonraker.evidence.len(), CapabilityKey::ALL.len());
        for key in CapabilityKey::ALL {
            let rows: Vec<_> = moonraker
                .evidence
                .iter()
                .filter(|(row_key, _)| *row_key == key)
                .collect();
            assert_eq!(rows.len(), 1, "{key:?}");
            let evidence = &rows[0].1;
            assert_eq!(evidence.tier, EvidenceTier::Sim, "{key:?}");
            assert_eq!(evidence.source, MOONRAKER_SIM_MANIFEST, "{key:?}");
            assert_eq!(
                evidence.verified_host_versions[0], MOONRAKER_SIM_VERSION,
                "{key:?}"
            );
            let read_only = evidence
                .verified_host_versions
                .iter()
                .any(|version| version == MOONRAKER_READ_ONLY_VERSION);
            let is_read = matches!(
                key,
                CapabilityKey::HostState | CapabilityKey::ArtifactIdentity | CapabilityKey::Camera
            );
            assert_eq!(read_only, is_read, "{key:?}");
        }
        assert!(MOONRAKER_SIM_MANIFEST.starts_with("sim-runs/"));
        assert!(MOONRAKER_SIM_MANIFEST.ends_with("/manifest.json"));
    }

    #[test]
    fn octoprint_has_no_evidence() {
        assert!(descriptor(OCTOPRINT_KIND).unwrap().evidence.is_empty());
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
