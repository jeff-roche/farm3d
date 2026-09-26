//! P6 D1/D6: the composable capability traits an adapter can implement
//! (staging, print control, host-state queries, camera discovery), the
//! capability matrix that reports which of them a Printer or an adapter
//! `kind` actually supports, and the pure Moonraker host-fact derivation
//! (D6 "Host facts") the matrix's host rules read.
//!
//! A command builds a capability object per operation, straight from the
//! Printer's `ConnectionConfig` and credential — these never share the
//! supervisor's always-on observation socket (`PrinterConnection`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::adapters::AdapterDescriptor;
use super::{ConnectionError, TLS_UNSUPPORTED_MESSAGE};
use crate::connections::status_repository::ToolTemperature;
use crate::printers::StoredPrinter;

// --- D1: capability traits ---------------------------------------------

/// Streams a Slice Revision's G-code to the host and can read it back.
/// Never starts a print — that is `PrintControl::start`.
#[async_trait::async_trait]
pub trait ArtifactStaging: Send + Sync {
    /// Streams `artifact` to `artifact.host_path`.
    async fn upload(
        &self,
        artifact: &StagedArtifact,
        body: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
    ) -> Result<(), CommandFailure>;
    /// Reads `artifact.host_path` and compares size and SHA-256 (D4).
    async fn locate(&self, artifact: &StagedArtifact) -> Result<LocateOutcome, ConnectionError>;
}

/// Starts, pauses, resumes, and cancels a print already staged on the host.
#[async_trait::async_trait]
pub trait PrintControl: Send + Sync {
    async fn start(&self, host_path: &str) -> Result<(), CommandFailure>;
    async fn pause(&self) -> Result<(), CommandFailure>;
    async fn resume(&self) -> Result<(), CommandFailure>;
    async fn cancel(&self) -> Result<(), CommandFailure>;
}

/// Read-only queries about the host's current state and job history.
#[async_trait::async_trait]
pub trait HostStateQuery: Send + Sync {
    async fn host_facts(&self) -> Result<HostFacts, ConnectionError>;
    async fn host_job_state(&self) -> Result<HostJobState, ConnectionError>;
    /// Requests newest first (`order=desc`) with `since_epoch_s` on the
    /// job's `start_time`. Callers never rely on either; they re-check (D5).
    async fn job_history(&self, query: HistoryQuery) -> Result<Vec<HistoryJob>, ConnectionError>;
}

/// Discovers the cameras a host knows about. URLs are never kept — a real
/// host's `stream_url` embeds its LAN address.
#[async_trait::async_trait]
pub trait CameraDiscovery: Send + Sync {
    async fn cameras(&self) -> Result<Vec<CameraInfo>, ConnectionError>;
}

// --- D1: supporting types (Rust-only unless noted) ----------------------

#[derive(Clone, PartialEq, Debug)]
pub struct StagedArtifact {
    pub host_path: String,
    /// Lower-case hex.
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, PartialEq, Debug)]
pub enum LocateOutcome {
    Absent,
    Matches,
    Differs { reason: DiffersReason },
}

#[derive(Clone, PartialEq, Debug)]
pub enum DiffersReason {
    Size { actual: u64 },
    Hash,
}

#[derive(Clone, PartialEq, Debug)]
pub enum CommandFailure {
    Definitive(HostOperationFailureCode),
    Indeterminate {
        reason: InconclusiveReason,
        no_longer_pending: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KlippyState {
    Ready,
    Startup,
    Shutdown,
    Error,
    Disconnected,
}

#[derive(Clone, PartialEq, Debug)]
pub struct HostJobState {
    pub klippy_state: KlippyState,
    /// Present only when `klippy_state` is `Ready`.
    pub print: Option<PrintSnapshot>,
    /// Every tool, in index order (#29's `ToolTemperature` — never a single
    /// "nozzle" field).
    pub tools: Vec<ToolTemperature>,
    /// Absent on a no-bed printer, never zero.
    pub bed: Option<BedTemperature>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BedTemperature {
    pub temp_c: f64,
    pub target_c: f64,
}

#[derive(Clone, PartialEq, Debug)]
pub enum PrintStatsState {
    Standby,
    Printing,
    Paused,
    Complete,
    Cancelled,
    Error,
    Other(String),
}

#[derive(Clone, PartialEq, Debug)]
pub struct PrintSnapshot {
    pub state: PrintStatsState,
    /// Moonraker's empty string maps to `None`.
    pub filename: Option<String>,
    pub is_paused: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct HistoryQuery {
    pub since_epoch_s: Option<f64>,
    pub limit: u32,
}

#[derive(Clone, PartialEq, Debug)]
pub struct HistoryJob {
    /// Parsed from Moonraker's hex string; a job whose id does not parse is
    /// dropped by the caller.
    pub job_id: u64,
    pub filename: String,
    /// A hint only (spike 6).
    pub status: String,
    pub start_time_epoch_s: f64,
}

#[derive(Clone, PartialEq, Debug)]
pub struct CameraInfo {
    pub name: String,
    pub service: String,
}

/// D11 (`failure_json.code`). ts-rs/camelCase because Task 6+'s
/// `HostOperationFailure` (`host_ops`) wraps it on the wire; defined here so
/// `CommandFailure::Definitive` has somewhere to live before that module
/// exists.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/HostOperationFailureCode.ts"
)]
pub enum HostOperationFailureCode {
    NeverSent,
    HostUnreachable,
    AuthRejected,
    ChecksumRejected,
    FileLoaded,
    HostBusy,
    FileMissing,
    HostRejected,
    HostNotReady,
    NotApplied,
    HostFileDiffers,
}

/// D11 (`last_attempt_reason`, and why an executor result became
/// `uncertain`). See `HostOperationFailureCode`'s doc comment.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/InconclusiveReason.ts")]
pub enum InconclusiveReason {
    ResponseLost,
    UnexpectedResponse,
    InterruptedByRestart,
    KlipperRestarted,
    HostUnreachable,
    AuthRejected,
    HostNotReady,
    IdentityCheckFailed,
    UploadSettling,
    NoStartEvidence,
    DifferentFileOnHost,
    EffectNotObserved,
}

// --- D6: the capability matrix's ts-rs wire types -----------------------

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/CapabilityKey.ts")]
pub enum CapabilityKey {
    Upload,
    Start,
    Pause,
    Resume,
    Cancel,
    HostState,
    ArtifactIdentity,
    Camera,
}

impl CapabilityKey {
    pub const ALL: [CapabilityKey; 8] = [
        CapabilityKey::Upload,
        CapabilityKey::Start,
        CapabilityKey::Pause,
        CapabilityKey::Resume,
        CapabilityKey::Cancel,
        CapabilityKey::HostState,
        CapabilityKey::ArtifactIdentity,
        CapabilityKey::Camera,
    ];

    /// Upload, start, pause, resume, and cancel change the host. Reads
    /// (`hostState`, `artifactIdentity`, `camera`) never do.
    fn is_write(self) -> bool {
        matches!(
            self,
            CapabilityKey::Upload
                | CapabilityKey::Start
                | CapabilityKey::Pause
                | CapabilityKey::Resume
                | CapabilityKey::Cancel
        )
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/EvidenceTier.ts")]
pub enum EvidenceTier {
    Sim,
    ReadOnlyHardware,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/CapabilityEvidence.ts")]
pub struct CapabilityEvidence {
    /// e.g. `"sim-runs/<UTC>/manifest.json"`.
    pub source: String,
    pub tier: EvidenceTier,
    /// e.g. `["Moonraker v0.11.0-1 API 1.5.0"]`.
    pub verified_host_versions: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/UnsupportedReason.ts")]
pub enum UnsupportedReason {
    Adapter,
    NotVerified,
    Host,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/CapabilityState.ts"
)]
pub enum CapabilityState {
    Supported {
        evidence: CapabilityEvidence,
    },
    Unsupported {
        reason: UnsupportedReason,
        detail: String,
    },
}

impl CapabilityState {
    fn supported(evidence: CapabilityEvidence) -> Self {
        CapabilityState::Supported { evidence }
    }

    fn unsupported(reason: UnsupportedReason, detail: impl Into<String>) -> Self {
        CapabilityState::Unsupported {
            reason,
            detail: detail.into(),
        }
    }
}

/// `Record<CapabilityKey, CapabilityState>`, with every key of
/// `CapabilityKey::ALL` always present — [`CapabilityMap::complete`] is the
/// only constructor, so that guarantee holds by construction.
///
/// `ts-rs`'s automatic mapping for an enum-keyed `BTreeMap`/`HashMap`
/// renders `{ [key in K]?: V }` (every key optional), which is weaker than
/// the spec's `Record<K, V>` (every key required) — `#[ts(type = "...")]`
/// on the field alone would fix the rendered text but drops that field's
/// automatic dependency tracking (`ts-rs` skips it whenever a field has a
/// literal type override), which would silently drop the
/// `import type { CapabilityKey }`/`{ CapabilityState }` lines the
/// generated file needs. This newtype's hand-written `TS` impl below picks
/// the exact TypeScript text AND keeps the dependency hint
/// (`visit_dependencies`), so the generated file still imports both.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(transparent)]
pub struct CapabilityMap(BTreeMap<CapabilityKey, CapabilityState>);

impl CapabilityMap {
    /// Builds a map with `state_for(key)` for every key of
    /// `CapabilityKey::ALL` — never a partial one.
    pub fn complete(mut state_for: impl FnMut(CapabilityKey) -> CapabilityState) -> Self {
        CapabilityMap(
            CapabilityKey::ALL
                .into_iter()
                .map(|key| (key, state_for(key)))
                .collect(),
        )
    }
}

impl std::ops::Index<CapabilityKey> for CapabilityMap {
    type Output = CapabilityState;

    fn index(&self, key: CapabilityKey) -> &CapabilityState {
        &self.0[&key]
    }
}

impl TS for CapabilityMap {
    type WithoutGenerics = Self;
    type OptionInnerType = Self;

    fn name(_: &ts_rs::Config) -> String {
        "Record<CapabilityKey, CapabilityState>".to_string()
    }

    fn inline(_: &ts_rs::Config) -> String {
        "Record<CapabilityKey, CapabilityState>".to_string()
    }

    // A field's dependency-on-`CapabilityMap` reaches its consumer (here,
    // `PrinterCapabilities`/`AdapterCapabilityRow`'s derived
    // `visit_dependencies`) through `visit_generics`, exactly as `ts-rs`'s
    // own blanket `HashMap<K, V>` impl does (`K`/`V` are its "generics"),
    // not through `visit_dependencies` — that method only matters if
    // `CapabilityMap` were ever exported directly, which it isn't. Both are
    // overridden identically so the dependency hint holds either way.
    fn visit_dependencies(visitor: &mut impl ts_rs::TypeVisitor)
    where
        Self: 'static,
    {
        visitor.visit::<CapabilityKey>();
        visitor.visit::<CapabilityState>();
    }

    fn visit_generics(visitor: &mut impl ts_rs::TypeVisitor)
    where
        Self: 'static,
    {
        visitor.visit::<CapabilityKey>();
        visitor.visit::<CapabilityState>();
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/HostFacts.ts")]
pub struct HostFacts {
    pub components: Vec<String>,
    pub has_virtual_sdcard: bool,
    pub has_pause_resume: bool,
    pub has_history: bool,
    pub has_heater_bed: bool,
    pub tool_count: u32,
    pub camera_count: u32,
    pub host_software: String,
    pub api_version: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PrinterCapabilities.ts")]
pub struct PrinterCapabilities {
    pub printer_id: String,
    pub adapter_kind: Option<String>,
    pub capabilities: CapabilityMap,
    /// When absent, no host rule applied (D6): only the adapter/evidence/TLS
    /// rules ran.
    pub host_facts: Option<HostFacts>,
    /// When `hostFacts` were read. `None` whenever `hostFacts` is `None`.
    pub observed_at: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AdapterCapabilityRow.ts")]
pub struct AdapterCapabilityRow {
    pub adapter_kind: String,
    /// No host rules applied — this is what the adapter TYPE supports, not
    /// what one Printer's host currently reports.
    pub capabilities: CapabilityMap,
}

const NOT_VERIFIED_DETAIL: &str = "Not verified for this Connection type yet.";

fn has_builder(key: CapabilityKey, descriptor: &AdapterDescriptor) -> bool {
    match key {
        CapabilityKey::Upload | CapabilityKey::ArtifactIdentity => descriptor.staging.is_some(),
        CapabilityKey::Start => descriptor.control.is_some() && descriptor.host_state.is_some(),
        CapabilityKey::Pause | CapabilityKey::Resume | CapabilityKey::Cancel => {
            descriptor.control.is_some()
        }
        CapabilityKey::HostState => descriptor.host_state.is_some(),
        CapabilityKey::Camera => descriptor.camera.is_some(),
    }
}

/// D6 rule 6, applied only when `host_facts` is `Some`. Returns the detail
/// message for the first violated rule for `key`, if any.
fn host_rule_violation(key: CapabilityKey, facts: &HostFacts) -> Option<&'static str> {
    match key {
        CapabilityKey::Upload | CapabilityKey::Start if !facts.has_virtual_sdcard => {
            Some("This printer has no virtual SD card.")
        }
        CapabilityKey::Start if !facts.has_history => {
            Some("This printer's Moonraker keeps no job history, so farm3d can't confirm a start.")
        }
        CapabilityKey::Pause | CapabilityKey::Resume if !facts.has_pause_resume => {
            Some("This printer has no pause and resume support.")
        }
        CapabilityKey::Camera if facts.camera_count == 0 => {
            Some("No camera is configured on this printer.")
        }
        _ => None,
    }
}

/// D6 rules 3-7 for one capability of one adapter. `use_tls`/`host_facts`
/// are `false`/`None` for `adapter_capability_matrix` (kind-level, no
/// per-Printer Connection or host reading behind it) and the Printer's own
/// values for `capabilities_for`.
fn capability_state(
    key: CapabilityKey,
    descriptor: &AdapterDescriptor,
    use_tls: bool,
    host_facts: Option<&HostFacts>,
) -> CapabilityState {
    // Rule 3.
    if use_tls && key.is_write() {
        return CapabilityState::unsupported(
            UnsupportedReason::NotVerified,
            TLS_UNSUPPORTED_MESSAGE,
        );
    }
    // Rule 4.
    if !has_builder(key, descriptor) {
        return CapabilityState::unsupported(UnsupportedReason::NotVerified, NOT_VERIFIED_DETAIL);
    }
    let Some(evidence) = descriptor
        .evidence
        .iter()
        .find(|(evidence_key, _)| *evidence_key == key)
        .map(|(_, evidence)| evidence.clone())
    else {
        return CapabilityState::unsupported(UnsupportedReason::NotVerified, NOT_VERIFIED_DETAIL);
    };
    // Rule 5.
    if evidence.tier == EvidenceTier::ReadOnlyHardware && key.is_write() {
        return CapabilityState::unsupported(UnsupportedReason::NotVerified, NOT_VERIFIED_DETAIL);
    }
    // Rule 6.
    if let Some(facts) = host_facts {
        if let Some(detail) = host_rule_violation(key, facts) {
            return CapabilityState::unsupported(UnsupportedReason::Host, detail);
        }
    }
    // Rule 7.
    CapabilityState::supported(evidence)
}

fn all_unsupported(reason: UnsupportedReason, detail: impl Into<String>) -> CapabilityMap {
    let detail = detail.into();
    CapabilityMap::complete(|_key| CapabilityState::unsupported(reason, detail.clone()))
}

/// D6. Public for P7. The first matching rule wins, in the order the spec
/// lists them (rules 1-2 here; rules 3-7 in [`capability_state`]).
pub fn capabilities_for(
    printer: &StoredPrinter,
    host_facts: Option<&HostFacts>,
) -> PrinterCapabilities {
    let printer_id = printer.id.clone();
    // Rule 1.
    let Some(connection) = printer.connection.as_ref() else {
        return PrinterCapabilities {
            printer_id,
            adapter_kind: None,
            capabilities: all_unsupported(UnsupportedReason::Adapter, "No Connection"),
            host_facts: None,
            observed_at: None,
        };
    };
    // Rule 2.
    let Some(descriptor) = super::adapters::descriptor(&connection.kind) else {
        return PrinterCapabilities {
            printer_id,
            adapter_kind: Some(connection.kind.clone()),
            capabilities: all_unsupported(
                UnsupportedReason::Adapter,
                "farm3d can't use this Connection type.",
            ),
            host_facts: None,
            observed_at: None,
        };
    };
    let capabilities = CapabilityMap::complete(|key| {
        capability_state(key, descriptor, connection.use_tls, host_facts)
    });
    PrinterCapabilities {
        printer_id,
        adapter_kind: Some(connection.kind.clone()),
        capabilities,
        host_facts: host_facts.cloned(),
        // The host-facts cache (`host_ops`, not built yet) owns the read
        // time; nothing supplies one here until it exists.
        observed_at: None,
    }
}

/// D6 "Rows at the end of P6": the registry's own capabilities, in
/// registry order, with no host rules applied (`useTls: false`,
/// `host_facts: None`) — what the adapter TYPE supports, not one Printer.
pub fn adapter_capability_matrix() -> Vec<AdapterCapabilityRow> {
    super::adapters::registry()
        .iter()
        .map(|descriptor| AdapterCapabilityRow {
            adapter_kind: descriptor.kind.to_string(),
            capabilities: CapabilityMap::complete(|key| {
                capability_state(key, descriptor, false, None)
            }),
        })
        .collect()
}

// --- D6 "Host facts": pure derivation from Moonraker's responses --------

/// Whether `name` is a tool object: `^extruder\d*$` exactly, never a
/// prefix. The real host also reports `extruder_offset_calibration`, which
/// must NOT count.
fn is_tool_object(name: &str) -> bool {
    match name.strip_prefix("extruder") {
        Some(rest) => rest.bytes().all(|byte| byte.is_ascii_digit()),
        None => false,
    }
}

/// Combines Moonraker's `server.info` (`components`), `printer.objects.list`
/// (`objects`), and `server.webcams.list` (`camera_count`) into `HostFacts`.
/// Pure: the HTTP client that calls this (`moonraker::control`) owns the
/// JSON deserialization.
///
/// `history` is a Moonraker component; `virtual_sdcard`, `pause_resume`, and
/// `heater_bed` are Klipper objects, which Moonraker never lists among its
/// components (spike Gate H).
pub fn derive_host_facts(
    host_software: impl Into<String>,
    api_version: impl Into<String>,
    components: Vec<String>,
    objects: &[String],
    camera_count: usize,
) -> HostFacts {
    let has_object = |name: &str| objects.iter().any(|object| object == name);
    HostFacts {
        has_virtual_sdcard: has_object("virtual_sdcard"),
        has_pause_resume: has_object("pause_resume"),
        has_history: components.iter().any(|component| component == "history"),
        has_heater_bed: has_object("heater_bed"),
        tool_count: objects
            .iter()
            .filter(|object| is_tool_object(object))
            .count() as u32,
        camera_count: camera_count as u32,
        components,
        host_software: host_software.into(),
        api_version: api_version.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::{ConnectionConfig, MOONRAKER_KIND, OCTOPRINT_KIND};
    use crate::printers::CatalogRef;

    // --- test fixtures ---------------------------------------------------

    fn connection(kind: &str, use_tls: bool) -> ConnectionConfig {
        ConnectionConfig {
            kind: kind.to_string(),
            host: "printer.local".to_string(),
            port: 7125,
            use_tls,
            credential_ref: None,
        }
    }

    fn a_stored_printer(connection: Option<ConnectionConfig>) -> StoredPrinter {
        StoredPrinter {
            id: "printer-a".to_string(),
            name: "Printer A".to_string(),
            catalog_ref: CatalogRef::default(),
            connection,
            ..Default::default()
        }
    }

    fn sim_evidence(source: &str) -> CapabilityEvidence {
        CapabilityEvidence {
            source: source.to_string(),
            tier: EvidenceTier::Sim,
            verified_host_versions: vec!["Moonraker v0.11.0-1 API 1.5.0".to_string()],
        }
    }

    fn read_only_evidence() -> CapabilityEvidence {
        CapabilityEvidence {
            source: "readonly-tier".to_string(),
            tier: EvidenceTier::ReadOnlyHardware,
            verified_host_versions: vec![],
        }
    }

    fn dummy_observe(
        config: &ConnectionConfig,
        _api_key: Option<zeroize::Zeroizing<String>>,
    ) -> Box<dyn crate::connections::PrinterConnection> {
        Box::new(
            super::super::moonraker::MoonrakerConnection::with_zeroizing_secret(
                config.clone(),
                None,
            ),
        )
    }

    fn dummy_staging(
        _config: &ConnectionConfig,
        _api_key: Option<zeroize::Zeroizing<String>>,
    ) -> Box<dyn ArtifactStaging> {
        unreachable!("not exercised by capability-matrix tests")
    }

    fn dummy_control(
        _config: &ConnectionConfig,
        _api_key: Option<zeroize::Zeroizing<String>>,
    ) -> Box<dyn PrintControl> {
        unreachable!("not exercised by capability-matrix tests")
    }

    fn dummy_host_state(
        _config: &ConnectionConfig,
        _api_key: Option<zeroize::Zeroizing<String>>,
    ) -> Box<dyn HostStateQuery> {
        unreachable!("not exercised by capability-matrix tests")
    }

    fn dummy_camera(
        _config: &ConnectionConfig,
        _api_key: Option<zeroize::Zeroizing<String>>,
    ) -> Box<dyn CameraDiscovery> {
        unreachable!("not exercised by capability-matrix tests")
    }

    /// A fully-equipped fake descriptor: every builder present, and `Sim`
    /// evidence for every capability. Individual tests strip a builder or
    /// an evidence row back out to exercise one rule at a time.
    fn fully_equipped_descriptor(
        evidence: Vec<(CapabilityKey, CapabilityEvidence)>,
    ) -> AdapterDescriptor {
        AdapterDescriptor {
            kind: "test-adapter",
            observe: dummy_observe,
            staging: Some(dummy_staging),
            control: Some(dummy_control),
            host_state: Some(dummy_host_state),
            camera: Some(dummy_camera),
            evidence: Box::leak(evidence.into_boxed_slice()),
        }
    }

    fn every_capability_sim_evidence() -> Vec<(CapabilityKey, CapabilityEvidence)> {
        CapabilityKey::ALL
            .into_iter()
            .map(|key| (key, sim_evidence("sim-runs/test/manifest.json")))
            .collect()
    }

    fn a_host_facts() -> HostFacts {
        HostFacts {
            components: vec![
                "virtual_sdcard".to_string(),
                "history".to_string(),
                "pause_resume".to_string(),
            ],
            has_virtual_sdcard: true,
            has_pause_resume: true,
            has_history: true,
            has_heater_bed: true,
            tool_count: 1,
            camera_count: 1,
            host_software: "Moonraker".to_string(),
            api_version: "1.5.0".to_string(),
        }
    }

    fn detail(state: &CapabilityState) -> &str {
        match state {
            CapabilityState::Unsupported { detail, .. } => detail,
            CapabilityState::Supported { .. } => panic!("expected an unsupported state"),
        }
    }

    fn reason(state: &CapabilityState) -> UnsupportedReason {
        match state {
            CapabilityState::Unsupported { reason, .. } => *reason,
            CapabilityState::Supported { .. } => panic!("expected an unsupported state"),
        }
    }

    // --- D6 rule 1: no Connection -----------------------------------------

    #[test]
    fn no_connection_makes_every_capability_unsupported_adapter() {
        let printer = a_stored_printer(None);
        let result = capabilities_for(&printer, None);

        assert_eq!(result.adapter_kind, None);
        assert_eq!(result.host_facts, None);
        for key in CapabilityKey::ALL {
            let state = &result.capabilities[key];
            assert_eq!(reason(state), UnsupportedReason::Adapter);
            assert_eq!(detail(state), "No Connection");
        }
    }

    // --- D6 rule 2: unknown kind --------------------------------------------

    #[test]
    fn an_unknown_kind_makes_every_capability_unsupported_adapter() {
        let printer = a_stored_printer(Some(connection("elegoolink", false)));
        let result = capabilities_for(&printer, None);

        assert_eq!(result.adapter_kind.as_deref(), Some("elegoolink"));
        for key in CapabilityKey::ALL {
            let state = &result.capabilities[key];
            assert_eq!(reason(state), UnsupportedReason::Adapter);
            assert_eq!(detail(state), "farm3d can't use this Connection type.");
        }
    }

    // --- D6 rule 3: useTls ---------------------------------------------------

    #[test]
    fn use_tls_makes_every_write_capability_not_verified_with_the_tls_message() {
        let descriptor = fully_equipped_descriptor(every_capability_sim_evidence());

        for key in [
            CapabilityKey::Upload,
            CapabilityKey::Start,
            CapabilityKey::Pause,
            CapabilityKey::Resume,
            CapabilityKey::Cancel,
        ] {
            let state = capability_state(key, &descriptor, true, None);
            assert_eq!(reason(&state), UnsupportedReason::NotVerified);
            assert_eq!(detail(&state), TLS_UNSUPPORTED_MESSAGE);
        }
    }

    #[test]
    fn use_tls_never_blocks_a_read_capability() {
        let descriptor = fully_equipped_descriptor(every_capability_sim_evidence());

        for key in [
            CapabilityKey::HostState,
            CapabilityKey::ArtifactIdentity,
            CapabilityKey::Camera,
        ] {
            let state = capability_state(key, &descriptor, true, None);
            assert!(matches!(state, CapabilityState::Supported { .. }));
        }
    }

    // --- D6 rule 4: missing builder, or missing evidence -------------------

    #[test]
    fn a_missing_builder_is_not_verified() {
        let mut descriptor = fully_equipped_descriptor(every_capability_sim_evidence());
        descriptor.staging = None;

        for key in [CapabilityKey::Upload, CapabilityKey::ArtifactIdentity] {
            let state = capability_state(key, &descriptor, false, None);
            assert_eq!(reason(&state), UnsupportedReason::NotVerified);
            assert_eq!(detail(&state), NOT_VERIFIED_DETAIL);
        }
    }

    #[test]
    fn start_needs_both_control_and_host_state_builders() {
        let mut descriptor = fully_equipped_descriptor(every_capability_sim_evidence());
        descriptor.host_state = None;

        let state = capability_state(CapabilityKey::Start, &descriptor, false, None);
        assert_eq!(reason(&state), UnsupportedReason::NotVerified);
    }

    #[test]
    fn a_builder_with_no_evidence_row_is_not_verified() {
        let descriptor = fully_equipped_descriptor(vec![]);

        let state = capability_state(CapabilityKey::Upload, &descriptor, false, None);
        assert_eq!(reason(&state), UnsupportedReason::NotVerified);
        assert_eq!(detail(&state), NOT_VERIFIED_DETAIL);
    }

    // --- D6 rule 5: read-only-hardware tier never supports a write ---------

    #[test]
    fn read_only_hardware_evidence_never_makes_a_write_capability_supported() {
        let descriptor =
            fully_equipped_descriptor(vec![(CapabilityKey::Upload, read_only_evidence())]);

        let state = capability_state(CapabilityKey::Upload, &descriptor, false, None);
        assert_eq!(reason(&state), UnsupportedReason::NotVerified);
    }

    #[test]
    fn read_only_hardware_evidence_still_supports_a_read_capability() {
        let descriptor =
            fully_equipped_descriptor(vec![(CapabilityKey::Camera, read_only_evidence())]);

        let state = capability_state(CapabilityKey::Camera, &descriptor, false, None);
        assert!(matches!(state, CapabilityState::Supported { .. }));
    }

    // --- D6 rule 6: host rules ----------------------------------------------

    #[test]
    fn no_virtual_sdcard_blocks_upload_and_start_as_host() {
        let descriptor = fully_equipped_descriptor(every_capability_sim_evidence());
        let mut facts = a_host_facts();
        facts.has_virtual_sdcard = false;

        for key in [CapabilityKey::Upload, CapabilityKey::Start] {
            let state = capability_state(key, &descriptor, false, Some(&facts));
            assert_eq!(reason(&state), UnsupportedReason::Host);
            assert_eq!(detail(&state), "This printer has no virtual SD card.");
        }
    }

    #[test]
    fn no_history_component_blocks_start_as_host() {
        let descriptor = fully_equipped_descriptor(every_capability_sim_evidence());
        let mut facts = a_host_facts();
        facts.has_history = false;

        let state = capability_state(CapabilityKey::Start, &descriptor, false, Some(&facts));
        assert_eq!(reason(&state), UnsupportedReason::Host);
        assert_eq!(
            detail(&state),
            "This printer's Moonraker keeps no job history, so farm3d can't confirm a start."
        );
    }

    #[test]
    fn no_pause_resume_blocks_pause_and_resume_as_host() {
        let descriptor = fully_equipped_descriptor(every_capability_sim_evidence());
        let mut facts = a_host_facts();
        facts.has_pause_resume = false;

        for key in [CapabilityKey::Pause, CapabilityKey::Resume] {
            let state = capability_state(key, &descriptor, false, Some(&facts));
            assert_eq!(reason(&state), UnsupportedReason::Host);
            assert_eq!(
                detail(&state),
                "This printer has no pause and resume support."
            );
        }
    }

    #[test]
    fn no_cameras_blocks_camera_as_host() {
        let descriptor = fully_equipped_descriptor(every_capability_sim_evidence());
        let mut facts = a_host_facts();
        facts.camera_count = 0;

        let state = capability_state(CapabilityKey::Camera, &descriptor, false, Some(&facts));
        assert_eq!(reason(&state), UnsupportedReason::Host);
        assert_eq!(detail(&state), "No camera is configured on this printer.");
    }

    #[test]
    fn host_facts_none_applies_no_host_rule() {
        let descriptor = fully_equipped_descriptor(every_capability_sim_evidence());

        // Every host rule's condition is violated by `a_host_facts`'s
        // opposite — but with `host_facts: None`, nothing downgrades these.
        for key in CapabilityKey::ALL {
            let state = capability_state(key, &descriptor, false, None);
            assert!(
                matches!(state, CapabilityState::Supported { .. }),
                "{key:?} should be supported with no host facts, got {state:?}"
            );
        }
    }

    // --- D6 rule 7: supported ------------------------------------------------

    #[test]
    fn a_fully_equipped_adapter_with_satisfied_host_facts_is_supported() {
        let descriptor = fully_equipped_descriptor(every_capability_sim_evidence());
        let facts = a_host_facts();

        for key in CapabilityKey::ALL {
            let state = capability_state(key, &descriptor, false, Some(&facts));
            assert!(
                matches!(state, CapabilityState::Supported { .. }),
                "{key:?}: {state:?}"
            );
        }
    }

    // --- capabilities_for wiring (rules 3-7 via a real, if empty, adapter) --

    #[test]
    fn moonraker_is_supported_from_its_sim_evidence_and_octoprint_is_not_verified() {
        let moonraker = capabilities_for(
            &a_stored_printer(Some(connection(MOONRAKER_KIND, false))),
            None,
        );
        assert_eq!(moonraker.adapter_kind.as_deref(), Some(MOONRAKER_KIND));
        for key in CapabilityKey::ALL {
            match &moonraker.capabilities[key] {
                CapabilityState::Supported { evidence } => {
                    assert_eq!(evidence.tier, EvidenceTier::Sim, "{key:?}")
                }
                other => panic!("{key:?}: {other:?}"),
            }
        }

        let octoprint = capabilities_for(
            &a_stored_printer(Some(connection(OCTOPRINT_KIND, false))),
            None,
        );
        for key in CapabilityKey::ALL {
            assert_eq!(
                reason(&octoprint.capabilities[key]),
                UnsupportedReason::NotVerified
            );
        }
    }

    /// D6 rule 6 still applies over the evidence: the simulator has no
    /// webcam, so with its host facts `camera` is a host limit.
    #[test]
    fn moonraker_host_rules_apply_over_its_evidence() {
        let facts = HostFacts {
            camera_count: 0,
            ..a_host_facts()
        };
        let result = capabilities_for(
            &a_stored_printer(Some(connection(MOONRAKER_KIND, false))),
            Some(&facts),
        );
        assert_eq!(
            result.capabilities[CapabilityKey::Camera],
            CapabilityState::Unsupported {
                reason: UnsupportedReason::Host,
                detail: "No camera is configured on this printer.".to_string(),
            }
        );
        for key in CapabilityKey::ALL {
            if key != CapabilityKey::Camera {
                assert!(
                    matches!(result.capabilities[key], CapabilityState::Supported { .. }),
                    "{key:?}"
                );
            }
        }
    }

    // --- adapter_capability_matrix -------------------------------------------

    #[test]
    fn the_matrix_lists_moonraker_supported_and_octoprint_not_verified() {
        let rows = adapter_capability_matrix();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].adapter_kind, MOONRAKER_KIND);
        assert_eq!(rows[1].adapter_kind, OCTOPRINT_KIND);
        for key in CapabilityKey::ALL {
            assert!(
                matches!(rows[0].capabilities[key], CapabilityState::Supported { .. }),
                "{key:?}"
            );
            assert_eq!(
                reason(&rows[1].capabilities[key]),
                UnsupportedReason::NotVerified
            );
        }
    }

    #[test]
    fn every_capability_map_always_carries_all_eight_keys() {
        // A `Record<CapabilityKey, CapabilityState>` promises every key is
        // present, never a partial map — `CapabilityMap::complete` is the
        // only constructor, so this holds for every path that builds one.
        let no_connection = capabilities_for(&a_stored_printer(None), None);
        assert_eq!(no_connection.capabilities.0.len(), CapabilityKey::ALL.len());

        let unknown_kind = capabilities_for(
            &a_stored_printer(Some(connection("elegoolink", false))),
            None,
        );
        assert_eq!(unknown_kind.capabilities.0.len(), CapabilityKey::ALL.len());

        let real_adapter = capabilities_for(
            &a_stored_printer(Some(connection(MOONRAKER_KIND, false))),
            None,
        );
        assert_eq!(real_adapter.capabilities.0.len(), CapabilityKey::ALL.len());

        for row in adapter_capability_matrix() {
            assert_eq!(row.capabilities.0.len(), CapabilityKey::ALL.len());
        }

        // Every key indexes without panicking, for every map above.
        for key in CapabilityKey::ALL {
            let _ = &no_connection.capabilities[key];
            let _ = &unknown_kind.capabilities[key];
            let _ = &real_adapter.capabilities[key];
        }
    }

    // --- the registry consistency test (R1) ---------------------------------

    /// A capability can only ever compute as `supported` when the
    /// descriptor has the builder(s) `has_builder` requires for it AND a
    /// `Sim`-tier evidence row — never from a builder alone, an evidence
    /// row alone, or `ReadOnlyHardware` evidence. Runs over the real
    /// registry (Moonraker: every builder and a `sim` row for each
    /// capability) and over deliberately inconsistent fixtures, so a future registry entry that
    /// violates the invariant fails this test rather than shipping a UI
    /// that offers a write the adapter cannot actually perform.
    #[test]
    fn registry_never_reports_supported_without_a_builder_and_sim_evidence() {
        fn assert_consistent(descriptor: &AdapterDescriptor) {
            for key in CapabilityKey::ALL {
                let state = capability_state(key, descriptor, false, None);
                if let CapabilityState::Supported { evidence } = state {
                    assert!(
                        has_builder(key, descriptor),
                        "{}/{key:?} reported supported with no builder",
                        descriptor.kind
                    );
                    assert_eq!(
                        evidence.tier,
                        EvidenceTier::Sim,
                        "{}/{key:?} reported supported from non-sim evidence",
                        descriptor.kind
                    );
                }
            }
        }

        for descriptor in super::super::adapters::registry() {
            assert_consistent(descriptor);
        }

        // A builder alone (no evidence row) must never be supported.
        assert_consistent(&AdapterDescriptor {
            evidence: &[],
            ..fully_equipped_descriptor(vec![])
        });
        // An evidence row alone (no builder) must never be supported.
        assert_consistent(&AdapterDescriptor {
            staging: None,
            control: None,
            host_state: None,
            camera: None,
            ..fully_equipped_descriptor(every_capability_sim_evidence())
        });
        // Read-only-hardware evidence for a write capability must never be
        // supported.
        assert_consistent(&fully_equipped_descriptor(vec![(
            CapabilityKey::Upload,
            read_only_evidence(),
        )]));
    }

    // --- serde round-trip snapshots ------------------------------------------

    #[test]
    fn capability_key_serializes_camel_case() {
        let pairs = [
            (CapabilityKey::Upload, "\"upload\""),
            (CapabilityKey::Start, "\"start\""),
            (CapabilityKey::Pause, "\"pause\""),
            (CapabilityKey::Resume, "\"resume\""),
            (CapabilityKey::Cancel, "\"cancel\""),
            (CapabilityKey::HostState, "\"hostState\""),
            (CapabilityKey::ArtifactIdentity, "\"artifactIdentity\""),
            (CapabilityKey::Camera, "\"camera\""),
        ];
        for (key, expected) in pairs {
            let json = serde_json::to_string(&key).unwrap();
            assert_eq!(json, expected);
            assert_eq!(serde_json::from_str::<CapabilityKey>(&json).unwrap(), key);
        }
    }

    #[test]
    fn capability_state_round_trips_both_variants() {
        let supported = CapabilityState::supported(sim_evidence("sim-runs/x/manifest.json"));
        let json = serde_json::to_string(&supported).unwrap();
        assert_eq!(
            json,
            r#"{"status":"supported","evidence":{"source":"sim-runs/x/manifest.json","tier":"sim","verifiedHostVersions":["Moonraker v0.11.0-1 API 1.5.0"]}}"#
        );
        assert_eq!(
            serde_json::from_str::<CapabilityState>(&json).unwrap(),
            supported
        );

        let unsupported = CapabilityState::unsupported(
            UnsupportedReason::Host,
            "No camera is configured on this printer.",
        );
        let json = serde_json::to_string(&unsupported).unwrap();
        assert_eq!(
            json,
            r#"{"status":"unsupported","reason":"host","detail":"No camera is configured on this printer."}"#
        );
        assert_eq!(
            serde_json::from_str::<CapabilityState>(&json).unwrap(),
            unsupported
        );
    }

    #[test]
    fn printer_capabilities_round_trips_and_omits_nothing_as_null_rather_than_missing() {
        let printer = a_stored_printer(None);
        let result = capabilities_for(&printer, None);
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""printerId":"printer-a""#));
        assert!(json.contains(r#""adapterKind":null"#));
        assert!(json.contains(r#""hostFacts":null"#));
        assert!(json.contains(r#""observedAt":null"#));
        assert_eq!(
            serde_json::from_str::<PrinterCapabilities>(&json).unwrap(),
            result
        );
    }

    #[test]
    fn adapter_capability_row_round_trips() {
        let rows = adapter_capability_matrix();
        let json = serde_json::to_string(&rows).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<AdapterCapabilityRow>>(&json).unwrap(),
            rows
        );
    }

    #[test]
    fn host_facts_round_trips_camel_case() {
        let facts = a_host_facts();
        let json = serde_json::to_string(&facts).unwrap();
        assert!(json.contains(r#""hasVirtualSdcard":true"#));
        assert!(json.contains(r#""toolCount":1"#));
        assert_eq!(serde_json::from_str::<HostFacts>(&json).unwrap(), facts);
    }

    #[test]
    fn host_operation_failure_code_and_inconclusive_reason_are_camel_case() {
        assert_eq!(
            serde_json::to_string(&HostOperationFailureCode::HostFileDiffers).unwrap(),
            "\"hostFileDiffers\""
        );
        assert_eq!(
            serde_json::to_string(&InconclusiveReason::DifferentFileOnHost).unwrap(),
            "\"differentFileOnHost\""
        );
    }

    // --- host-fact derivation -------------------------------------------------

    #[test]
    fn derive_host_facts_counts_a_single_extruder() {
        let facts = derive_host_facts(
            "Moonraker",
            "1.5.0",
            vec!["history".to_string()],
            &[
                "extruder".to_string(),
                "heater_bed".to_string(),
                "virtual_sdcard".to_string(),
            ],
            0,
        );
        assert_eq!(facts.tool_count, 1);
        assert!(facts.has_heater_bed);
        assert!(facts.has_virtual_sdcard);
        assert!(!facts.has_pause_resume);
        assert!(facts.has_history);
    }

    #[test]
    fn derive_host_facts_reads_klipper_objects_from_objects_not_components() {
        // A component named like a Klipper object proves nothing, and an
        // object named like a Moonraker component proves nothing either.
        let facts = derive_host_facts(
            "Moonraker",
            "1.5.0",
            vec!["virtual_sdcard".to_string(), "pause_resume".to_string()],
            &["history".to_string()],
            0,
        );
        assert!(!facts.has_virtual_sdcard);
        assert!(!facts.has_pause_resume);
        assert!(!facts.has_history);
    }

    #[test]
    fn derive_host_facts_counts_four_extruders_out_of_order_and_excludes_calibration() {
        let objects = [
            "extruder2".to_string(),
            "extruder".to_string(),
            "extruder3".to_string(),
            "extruder1".to_string(),
            "extruder_offset_calibration".to_string(),
        ];
        let facts = derive_host_facts("Moonraker", "1.5.0", vec![], &objects, 2);
        assert_eq!(facts.tool_count, 4);
        assert_eq!(facts.camera_count, 2);
    }
}
