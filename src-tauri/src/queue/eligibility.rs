//! P7 D5: pure, deterministic eligibility, compatibility, and candidate
//! ranking. [`evaluate`] is the whole-entry pass the automatic evaluator
//! (D6), `list_queue`'s synchronous fallback (ruling R3), and the Assign
//! dialog read to show every candidate Printer, ranked. [`check_assignment`]
//! re-runs the same gates for one chosen Printer and Spool — the assign
//! transaction's in-transaction re-check, and the operator's Assign
//! command's own validation.
//!
//! Everything here is pure: no I/O, no clock (the caller passes `now` for
//! [`QueueEntryEligibility::evaluated_at`]). `EligibilityInput` is built by
//! the caller from rows already read (inside a transaction, for the assign
//! path, so the check sees exactly what it is about to commit against).
//!
//! Gates run in this order per Printer (D5): 0 (the entry's pin), 1 (the
//! Printer is schedulable), 2 (profile compatibility), 3 (capability), 4
//! (material). [`evaluate`] stops at a Printer's first failing gate — D5's
//! gates are each written as a chain of mutually exclusive conditions, so a
//! Printer only ever fails one of them at a time in the normal course of
//! `evaluate`'s single pass. [`check_assignment`] instead runs every gate
//! against its one Printer/Spool regardless of an earlier failure, and
//! collects every blocker found — gates 1 and 3 read independent facts
//! (Connection/Printer status vs. capabilities), so both can legitimately
//! fail at once, and the operator's Assign dialog wants the full account.

use std::collections::BTreeSet;

use crate::catalog::PrinterProfile;
use crate::connections::capabilities::{
    CapabilityKey, CapabilityState, EvidenceTier, PrinterCapabilities,
};
use crate::connections::{ConnectionState, PrinterStatus};
use crate::contracts::command::RecoveryCode;
use crate::printers::operational::{OperationalState, TelemetryFreshness};
use crate::printers::StoredPrinter;
use crate::slicing::compat::{self, CompatMismatch, MaterialMismatch};
use crate::slicing::facts::SliceFacts;
use crate::spools::{MaterialFamily, SpoolLifecycle, SpoolLocation, SpoolRecord};

use super::{
    Blocker, BlockerCode, Candidate, DispatchPolicy, DispatchPreference, EligibilityVerdict,
    PrinterEligibility, QueueEntry, QueueEntryEligibility, SpoolOption,
};

/// One Printer as `evaluate`/`check_assignment` see it — already resolved
/// and joined by the caller from `StoredPrinter`, its live `PrinterStatus`
/// (`None` before any status has ever been observed), its `PrinterCapabilities`,
/// whether it has an active Job, whether it has an unresolved Host
/// Operation not linked to a Job (`foreignHostOp` — one linked to a Job is
/// covered by `activeJob`/`JOB_ACTIVE` instead), its `lastUsedAt` (the
/// latest `endedAt` of its terminal Jobs), and the ids of the Spools
/// currently loaded in its Material Slots.
pub struct PrinterView<'a> {
    pub printer: &'a StoredPrinter,
    pub profile: &'a PrinterProfile,
    pub status: Option<&'a PrinterStatus>,
    pub capabilities: &'a PrinterCapabilities,
    pub active_job: bool,
    pub foreign_host_op: bool,
    pub last_used_at: Option<&'a str>,
    pub loaded_spool_ids: &'a [String],
}

/// Everything [`evaluate`]/[`check_assignment`] need for one Queue Entry.
///
/// `printers` is the unarchived Printers, plus the entry's
/// `manualPrinterId` even if archived (D5 gate 0) — this module never
/// applies the archived filter itself, so the caller builds that set.
/// [`check_assignment`]'s caller has a stricter precondition: it must
/// always include the one Printer it's about to check, archived or not,
/// even outside a pin. Gate 1 is what reports `PRINTER_ARCHIVED` (or any
/// other gate-1 blocker) for that Printer; a `printer_id` genuinely absent
/// from `printers` is a caller bug, not a gate failure, and
/// `check_assignment` reports it as
/// [`AssignmentCheckError::PrinterNotInView`] instead of a blocker.
///
/// `claimed_printers` are Printers already handed a Job earlier in the
/// same automatic-evaluator pass (D6) — they read as `JOB_ACTIVE`, same as
/// a Printer with a real active Job, so a later entry in the same run
/// never sees them offered twice.
pub struct EligibilityInput<'a> {
    pub entry: &'a QueueEntry,
    pub facts: &'a SliceFacts,
    pub printers: &'a [PrinterView<'a>],
    pub spools: &'a [SpoolRecord],
    pub claimed_printers: &'a BTreeSet<String>,
}

/// [`check_assignment`]'s caller: an operator's explicit Assign/Start (with
/// their own answer to "assign despite unconfirmed facts?"), or the
/// automatic evaluator's own re-check before it commits. `is_automatic`
/// (gates 1 and 3's automatic-only rows, and gate 4's loaded-only
/// candidate pool) is driven by this mode, not by the entry's own
/// `DispatchPolicy` — an operator may assign a `recommended` or even
/// `automatic` entry by hand, and that assignment is checked as manual
/// work. Tolerating an absent `printerProfile`/`nozzleDiameterMm`/
/// `materialFamily`/`filamentDiameterMm` fact (D5 gates 2 and 4) is
/// narrower: D5 allows it only for a Manual-policy entry, so
/// `check_assignment` requires *both* `entry.policy == Manual` and
/// `Operator { acknowledge_manual_facts: true }` — an operator's
/// acknowledgement alone never overrides a non-Manual policy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AssignMode {
    Operator { acknowledge_manual_facts: bool },
    Automatic,
}

/// [`check_assignment`]'s error. `Blocked` is the ordinary case — one or
/// more gates refused the pair. `PrinterNotInView` means `printer_id`
/// wasn't in `EligibilityInput::printers` at all, violating that struct's
/// documented precondition; it is never a gate result, so callers (Task 6's
/// `assign`) map it to `NOT_FOUND` rather than presenting it as a blocker.
#[derive(Clone, PartialEq, Debug)]
pub enum AssignmentCheckError {
    Blocked(Vec<Blocker>),
    PrinterNotInView,
}

fn blocker(
    code: BlockerCode,
    message: impl Into<String>,
    detail: Option<String>,
    recovery: Option<RecoveryCode>,
    printer_ids: Vec<String>,
) -> Blocker {
    Blocker {
        code,
        message: message.into(),
        detail,
        recovery,
        printer_ids,
    }
}

fn describe_mismatch(mismatch: &CompatMismatch) -> String {
    match mismatch {
        CompatMismatch::BedShape => "The bed shape doesn't match this Slice.".to_string(),
        CompatMismatch::PrintableHeight => {
            "The printable height doesn't match this Slice.".to_string()
        }
        CompatMismatch::NozzleCount { found } => {
            format!("This Printer has {found} nozzles; this Slice needs exactly one.")
        }
        CompatMismatch::NozzleDiameter { want, have } => {
            format!("Nozzle {have} mm; this Slice needs {want} mm.")
        }
        CompatMismatch::NozzleType => "The nozzle type doesn't match this Slice.".to_string(),
        CompatMismatch::GcodeFlavor => "The G-code flavor doesn't match this Slice.".to_string(),
        CompatMismatch::FactAbsent(_) => {
            unreachable!("FactAbsent is filtered out before this is ever formatted")
        }
    }
}

const NEEDS_MANUAL_PRINTER_MESSAGE: &str =
    "This Slice has facts nobody confirmed. Assign it by hand.";
const ADAPTER_NOT_PROVEN_MESSAGE: &str =
    "Automatic dispatch needs a simulator-proven Connection. Assign by hand.";
const SPOOL_NOT_LOADED_MESSAGE: &str =
    "Automatic assignment needs the Spool loaded on this Printer.";

fn operational_state_label(state: OperationalState) -> &'static str {
    match state {
        OperationalState::SetupIncomplete => "setup incomplete",
        OperationalState::Error => "error",
        OperationalState::Offline => "offline",
        OperationalState::Connecting => "connecting",
        OperationalState::Unknown => "unknown",
        OperationalState::Printing => "printing",
        OperationalState::Paused => "paused",
        OperationalState::Busy => "busy",
        OperationalState::Finished => "finished",
        OperationalState::Cancelled => "cancelled",
        OperationalState::Failed => "failed",
        OperationalState::Ready => "ready",
    }
}

/// D5 gate 1: the Printer is schedulable. `is_automatic` is whether the
/// caller's effective mode is Automatic (either the entry's own policy, in
/// `evaluate`, or `AssignMode::Automatic`, in `check_assignment`) — it
/// gates only the last row.
fn gate1(
    is_automatic: bool,
    view: &PrinterView<'_>,
    claimed: &BTreeSet<String>,
    printer_id: &str,
) -> Option<Blocker> {
    let ids = || vec![printer_id.to_string()];

    if view.printer.archived_at.is_some() {
        return Some(blocker(
            BlockerCode::PrinterArchived,
            "This Printer is archived.",
            None,
            Some(RecoveryCode::UnarchivePrinter),
            ids(),
        ));
    }

    let setup_incomplete = match &view.printer.connection {
        None => true,
        Some(connection) => !crate::connections::is_supported_kind(&connection.kind),
    };
    if setup_incomplete {
        return Some(blocker(
            BlockerCode::SetupIncomplete,
            "This Printer's setup is incomplete.",
            None,
            Some(RecoveryCode::OpenPrinterSetup),
            ids(),
        ));
    }

    let connection_state = view.status.map(|status| status.connection_state);
    if connection_state == Some(ConnectionState::Error) {
        return Some(blocker(
            BlockerCode::ConnectionError,
            "This Printer's Connection has an error.",
            None,
            Some(RecoveryCode::CheckConnection),
            ids(),
        ));
    }
    // No status at all reads the same as offline: there is nothing live to
    // trust that the Printer can accept a Job right now.
    if !matches!(connection_state, Some(ConnectionState::Online)) {
        return Some(blocker(
            BlockerCode::PrinterOffline,
            "This Printer is offline.",
            None,
            Some(RecoveryCode::CheckConnection),
            ids(),
        ));
    }

    if view.active_job || claimed.contains(printer_id) {
        return Some(blocker(
            BlockerCode::JobActive,
            "This Printer already has a Job.",
            None,
            Some(RecoveryCode::OpenJob),
            ids(),
        ));
    }

    if view.foreign_host_op {
        return Some(blocker(
            BlockerCode::HostOperationPending,
            "This Printer has a pending printer operation.",
            None,
            Some(RecoveryCode::OpenPrinterJob),
            ids(),
        ));
    }

    let operational_state = view.status.map(|status| status.operational_state);
    if matches!(
        operational_state,
        Some(OperationalState::Printing)
            | Some(OperationalState::Paused)
            | Some(OperationalState::Busy)
    ) {
        return Some(blocker(
            BlockerCode::PrinterBusyExternal,
            "This Printer is printing a file farm3d didn't start.",
            None,
            None,
            ids(),
        ));
    }

    if is_automatic {
        let freshness = view.status.map(|status| status.freshness);
        let idle_ok = matches!(
            operational_state,
            Some(OperationalState::Ready)
                | Some(OperationalState::Finished)
                | Some(OperationalState::Cancelled)
        );
        let fresh_ok = freshness == Some(TelemetryFreshness::Fresh);
        if !idle_ok || !fresh_ok {
            let label = operational_state
                .map(operational_state_label)
                .unwrap_or("unknown");
            return Some(blocker(
                BlockerCode::PrinterNotIdle,
                format!("Automatic assignment waits for this Printer to be idle: {label}."),
                None,
                None,
                ids(),
            ));
        }
    }

    None
}

/// D5 gate 2's real (non-`FactAbsent`) mismatches, filtered from
/// `slicing::compat::profile_compatible`'s full list, plus whether any
/// fact was absent.
struct ProfileCheck {
    fact_absent: bool,
    mismatches: Vec<CompatMismatch>,
}

fn profile_check(facts: &SliceFacts, profile: &PrinterProfile) -> ProfileCheck {
    let all = compat::profile_compatible(facts, profile);
    let fact_absent = all
        .iter()
        .any(|m| matches!(m, CompatMismatch::FactAbsent(_)));
    let mismatches = all
        .into_iter()
        .filter(|m| !matches!(m, CompatMismatch::FactAbsent(_)))
        .collect();
    ProfileCheck {
        fact_absent,
        mismatches,
    }
}

/// D5 gate 2: profile compatibility. `tolerate_absent_facts` is whether the
/// caller's effective mode tolerates an absent `printerProfile`/
/// `nozzleDiameterMm` fact (Manual policy in `evaluate`; an operator's own
/// acknowledgement in `check_assignment`). Returns the blocker, if any, and
/// whether `manualFactsAcknowledgementRequired` should be set on a passing
/// Candidate.
fn gate2(
    tolerate_absent_facts: bool,
    facts: &SliceFacts,
    profile: &PrinterProfile,
    printer_id: &str,
) -> (Option<Blocker>, bool) {
    let check = profile_check(facts, profile);
    if check.fact_absent && !tolerate_absent_facts {
        return (
            Some(blocker(
                BlockerCode::NeedsManualPrinter,
                NEEDS_MANUAL_PRINTER_MESSAGE,
                None,
                Some(RecoveryCode::AssignManually),
                vec![printer_id.to_string()],
            )),
            false,
        );
    }
    if let Some(first) = check.mismatches.first() {
        let detail = describe_mismatch(first);
        return (
            Some(blocker(
                BlockerCode::ProfileMismatch,
                detail.clone(),
                Some(detail),
                None,
                vec![printer_id.to_string()],
            )),
            false,
        );
    }
    (None, check.fact_absent)
}

/// D5 gate 3: capability. `is_automatic` gates the `ADAPTER_NOT_PROVEN`
/// row only.
fn gate3(
    is_automatic: bool,
    capabilities: &PrinterCapabilities,
    printer_id: &str,
) -> Option<Blocker> {
    const KEYS: [CapabilityKey; 4] = [
        CapabilityKey::Upload,
        CapabilityKey::Start,
        CapabilityKey::HostState,
        CapabilityKey::ArtifactIdentity,
    ];

    for key in KEYS {
        if let CapabilityState::Unsupported { detail, .. } = &capabilities.capabilities[key] {
            return Some(blocker(
                BlockerCode::CapabilityUnsupported,
                detail.clone(),
                Some(detail.clone()),
                None,
                vec![printer_id.to_string()],
            ));
        }
    }

    if is_automatic {
        for key in KEYS {
            if let CapabilityState::Supported { evidence } = &capabilities.capabilities[key] {
                if evidence.tier != EvidenceTier::Sim {
                    return Some(blocker(
                        BlockerCode::AdapterNotProven,
                        ADAPTER_NOT_PROVEN_MESSAGE,
                        None,
                        Some(RecoveryCode::AssignManually),
                        vec![printer_id.to_string()],
                    ));
                }
            }
        }
    }

    None
}

fn spool_is_active_and_material_matches(
    facts: &SliceFacts,
    spool: &SpoolRecord,
    tolerate_absent_facts: bool,
) -> bool {
    if spool.lifecycle != SpoolLifecycle::Active {
        return false;
    }
    match compat::material_compatible(facts, spool) {
        Ok(()) => true,
        Err(MaterialMismatch::FactAbsent(_)) => tolerate_absent_facts,
        Err(_) => false,
    }
}

fn is_loaded_here(view: &PrinterView<'_>, spool_id: &str) -> bool {
    view.loaded_spool_ids.iter().any(|id| id == spool_id)
}

fn is_relevant_to(view: &PrinterView<'_>, spool: &SpoolRecord) -> bool {
    is_loaded_here(view, &spool.id) || matches!(spool.location, SpoolLocation::Storage { .. })
}

fn to_spool_option(spool: &SpoolRecord, view: &PrinterView<'_>) -> SpoolOption {
    SpoolOption {
        spool_id: spool.id.clone(),
        spool_number: spool.spool_number,
        loaded_on_printer: is_loaded_here(view, &spool.id),
        available_mg: spool.availability.available_mg,
    }
}

fn material_fact_absent(facts: &SliceFacts) -> bool {
    facts.material_family.value().is_none() || facts.filament_diameter_mm.value().is_none()
}

/// `MaterialFamily`'s wire name (`"PLA"`, `"PLA-CF"`, ...; see its
/// `#[serde(rename = ...)]`s), for a message an operator reads — never its
/// Rust `Debug` form (`Pla`).
fn material_family_wire_name(family: MaterialFamily) -> &'static str {
    match family {
        MaterialFamily::Pla => "PLA",
        MaterialFamily::PlaCf => "PLA-CF",
        MaterialFamily::Petg => "PETG",
        MaterialFamily::PetCf => "PET-CF",
        MaterialFamily::Abs => "ABS",
        MaterialFamily::Asa => "ASA",
        MaterialFamily::Tpu => "TPU",
        MaterialFamily::Pa => "PA",
        MaterialFamily::PaCf => "PA-CF",
        MaterialFamily::Pc => "PC",
        MaterialFamily::Pva => "PVA",
        MaterialFamily::Hips => "HIPS",
        MaterialFamily::Pp => "PP",
        MaterialFamily::Other => "OTHER",
    }
}

pub(crate) fn no_compatible_spool_message(facts: &SliceFacts) -> String {
    match (
        facts.material_family.value(),
        facts.filament_diameter_mm.value(),
    ) {
        (Some(&family), Some(diameter)) => {
            format!(
                "No Spool of {} {diameter} mm is available.",
                material_family_wire_name(family)
            )
        }
        _ => "No compatible Spool is available.".to_string(),
    }
}

/// D5's `INSUFFICIENT_MATERIAL` message states grams, not raw milligrams
/// (`"No matching Spool has <estimate> g available."`) — `detail` still
/// carries the best `availableMg` as a raw mg figure. Whole grams, as the
/// frontend's tables show them, rounded up so the message never states
/// less than the estimate needs.
pub(crate) fn insufficient_material_message(estimate_mg: i64) -> String {
    let grams = (estimate_mg.max(0) + 999) / 1000;
    format!("No matching Spool has {grams} g available.")
}

struct Gate4Passed {
    spool: SpoolOption,
    spool_options: Vec<SpoolOption>,
    loaded_match: bool,
    /// Whether a `materialFamily`/`filamentDiameterMm` fact was absent and
    /// tolerated (always alongside `tolerate_absent_facts`, i.e. Manual, or
    /// an operator's acknowledgement).
    fact_absent: bool,
}

/// D5 gate 4: material. `is_automatic` restricts the candidate pool to
/// Spools loaded on this Printer (Manual/Recommended also allow storage);
/// `tolerate_absent_facts` is the same Manual-only (or acknowledged)
/// tolerance gate 2 uses.
fn gate4(
    is_automatic: bool,
    tolerate_absent_facts: bool,
    facts: &SliceFacts,
    view: &PrinterView<'_>,
    spools: &[SpoolRecord],
    estimate_mg: i64,
    printer_id: &str,
) -> Result<Gate4Passed, Blocker> {
    let fact_absent = material_fact_absent(facts);
    if fact_absent && !tolerate_absent_facts {
        return Err(blocker(
            BlockerCode::NeedsManualPrinter,
            NEEDS_MANUAL_PRINTER_MESSAGE,
            None,
            Some(RecoveryCode::AssignManually),
            vec![printer_id.to_string()],
        ));
    }

    let matching: Vec<&SpoolRecord> = spools
        .iter()
        .filter(|spool| is_relevant_to(view, spool))
        .filter(|spool| spool_is_active_and_material_matches(facts, spool, tolerate_absent_facts))
        .collect();

    if matching.is_empty() {
        return Err(blocker(
            BlockerCode::NoCompatibleSpool,
            no_compatible_spool_message(facts),
            None,
            Some(RecoveryCode::LoadSpool),
            vec![printer_id.to_string()],
        ));
    }

    let sufficient: Vec<&SpoolRecord> = matching
        .iter()
        .copied()
        .filter(|spool| spool.availability.available_mg >= estimate_mg)
        .collect();

    if sufficient.is_empty() {
        let best = matching
            .iter()
            .map(|spool| spool.availability.available_mg)
            .max()
            .unwrap_or(0);
        return Err(blocker(
            BlockerCode::InsufficientMaterial,
            insufficient_material_message(estimate_mg),
            Some(best.to_string()),
            Some(RecoveryCode::LoadSpool),
            vec![printer_id.to_string()],
        ));
    }

    let policy_candidates: Vec<&SpoolRecord> = if is_automatic {
        sufficient
            .iter()
            .copied()
            .filter(|spool| is_loaded_here(view, &spool.id))
            .collect()
    } else {
        sufficient
    };

    if policy_candidates.is_empty() {
        return Err(blocker(
            BlockerCode::SpoolNotLoaded,
            SPOOL_NOT_LOADED_MESSAGE,
            None,
            Some(RecoveryCode::LoadSpool),
            vec![printer_id.to_string()],
        ));
    }

    let mut sorted = policy_candidates;
    sorted.sort_by(|a, b| {
        let a_loaded = is_loaded_here(view, &a.id);
        let b_loaded = is_loaded_here(view, &b.id);
        (!a_loaded)
            .cmp(&!b_loaded)
            .then(
                a.availability
                    .available_mg
                    .cmp(&b.availability.available_mg),
            )
            .then(a.spool_number.cmp(&b.spool_number))
    });

    let spool_options: Vec<SpoolOption> = sorted
        .iter()
        .map(|spool| to_spool_option(spool, view))
        .collect();
    let chosen = spool_options[0].clone();
    let loaded_match = chosen.loaded_on_printer;

    Ok(Gate4Passed {
        spool: chosen,
        spool_options,
        loaded_match,
        fact_absent,
    })
}

struct CandidateDraft {
    spool: SpoolOption,
    spool_options: Vec<SpoolOption>,
    loaded_match: bool,
    manual_facts_acknowledgement_required: bool,
}

fn evaluate_printer(
    entry: &QueueEntry,
    facts: &SliceFacts,
    view: &PrinterView<'_>,
    spools: &[SpoolRecord],
    claimed_printers: &BTreeSet<String>,
) -> Result<CandidateDraft, Blocker> {
    let printer_id = view.printer.id.as_str();
    let is_automatic = entry.policy == DispatchPolicy::Automatic;
    let tolerate_absent_facts = entry.policy == DispatchPolicy::Manual;

    if let Some(b) = gate1(is_automatic, view, claimed_printers, printer_id) {
        return Err(b);
    }

    let (gate2_blocker, ack_required) =
        gate2(tolerate_absent_facts, facts, view.profile, printer_id);
    if let Some(b) = gate2_blocker {
        return Err(b);
    }

    if let Some(b) = gate3(is_automatic, view.capabilities, printer_id) {
        return Err(b);
    }

    let passed = gate4(
        is_automatic,
        tolerate_absent_facts,
        facts,
        view,
        spools,
        entry.estimate.amount_mg,
        printer_id,
    )?;

    Ok(CandidateDraft {
        spool: passed.spool,
        spool_options: passed.spool_options,
        loaded_match: passed.loaded_match,
        manual_facts_acknowledgement_required: ack_required || passed.fact_absent,
    })
}

/// D5's "Aggregated blockers": every Printer contributes at most one
/// Blocker in `evaluate` (its first failing gate). This groups them by
/// code — in the order gates run (gate 0's pin first, then gate 1's rows,
/// then gates 2-4) — combining every affected Printer's id into one
/// `Blocker` per code.
const CODE_ORDER: &[BlockerCode] = &[
    BlockerCode::PinnedToOtherPrinter,
    BlockerCode::PrinterArchived,
    BlockerCode::SetupIncomplete,
    BlockerCode::ConnectionError,
    BlockerCode::PrinterOffline,
    BlockerCode::JobActive,
    BlockerCode::HostOperationPending,
    BlockerCode::PrinterBusyExternal,
    BlockerCode::PrinterNotIdle,
    BlockerCode::ProfileMismatch,
    BlockerCode::NeedsManualPrinter,
    BlockerCode::CapabilityUnsupported,
    BlockerCode::AdapterNotProven,
    BlockerCode::NoCompatibleSpool,
    BlockerCode::InsufficientMaterial,
    BlockerCode::SpoolNotLoaded,
];

fn aggregate_blockers(hits: Vec<Blocker>) -> Vec<Blocker> {
    let mut aggregated = Vec::new();
    for code in CODE_ORDER {
        let matching: Vec<&Blocker> = hits.iter().filter(|b| b.code == *code).collect();
        if let Some(first) = matching.first() {
            let printer_ids: Vec<String> = matching
                .iter()
                .flat_map(|b| b.printer_ids.clone())
                .collect();
            aggregated.push(Blocker {
                printer_ids,
                ..(*first).clone()
            });
        }
    }
    aggregated
}

/// D5: evaluates every gate for `input.entry` against every Printer in
/// `input.printers`, ranks the resulting candidates, and reports the
/// entry's verdict. Pure — takes `now` only to stamp `evaluatedAt`.
/// Callers pass only `queued` entries; an `assigned` entry's standing
/// comes from its Job (`startBlockers`), not this function.
pub fn evaluate(input: &EligibilityInput<'_>, now: &str) -> QueueEntryEligibility {
    let entry = input.entry;

    let (considered, pinned_out_ids): (Vec<&PrinterView<'_>>, Vec<String>) =
        match entry.manual_printer_id.as_deref() {
            Some(pin) => {
                let considered = input
                    .printers
                    .iter()
                    .filter(|p| p.printer.id == pin)
                    .collect();
                let excluded = input
                    .printers
                    .iter()
                    .filter(|p| p.printer.id != pin)
                    .map(|p| p.printer.id.clone())
                    .collect();
                (considered, excluded)
            }
            None => (input.printers.iter().collect(), Vec::new()),
        };

    let mut printer_rows = Vec::with_capacity(considered.len());
    let mut passed: Vec<(&PrinterView<'_>, CandidateDraft)> = Vec::new();
    let mut blocker_hits: Vec<Blocker> = Vec::new();

    for view in &considered {
        match evaluate_printer(
            entry,
            input.facts,
            view,
            input.spools,
            input.claimed_printers,
        ) {
            Ok(draft) => {
                printer_rows.push(PrinterEligibility {
                    printer_id: view.printer.id.clone(),
                    printer_name: view.printer.name.clone(),
                    eligible: true,
                    blockers: Vec::new(),
                });
                passed.push((view, draft));
            }
            Err(b) => {
                printer_rows.push(PrinterEligibility {
                    printer_id: view.printer.id.clone(),
                    printer_name: view.printer.name.clone(),
                    eligible: false,
                    blockers: vec![b.clone()],
                });
                blocker_hits.push(b);
            }
        }
    }

    if !pinned_out_ids.is_empty() {
        let pinned_name = considered
            .first()
            .map(|view| view.printer.name.clone())
            .unwrap_or_default();
        blocker_hits.push(blocker(
            BlockerCode::PinnedToOtherPrinter,
            format!("This entry is pinned to {pinned_name}."),
            None,
            None,
            pinned_out_ids,
        ));
    }

    let mut ranked = passed;
    match entry.preference {
        DispatchPreference::LoadedFirst => {
            ranked.sort_by(|(view_a, draft_a), (view_b, draft_b)| {
                (!draft_a.loaded_match)
                    .cmp(&!draft_b.loaded_match)
                    .then(draft_b.spool.available_mg.cmp(&draft_a.spool.available_mg))
                    .then(
                        view_a
                            .printer
                            .name
                            .to_lowercase()
                            .cmp(&view_b.printer.name.to_lowercase()),
                    )
                    .then(view_a.printer.id.cmp(&view_b.printer.id))
            })
        }
        DispatchPreference::LeastRecentlyUsed => ranked.sort_by(|(view_a, _), (view_b, _)| {
            view_a
                .last_used_at
                .cmp(&view_b.last_used_at)
                .then(
                    view_a
                        .printer
                        .name
                        .to_lowercase()
                        .cmp(&view_b.printer.name.to_lowercase()),
                )
                .then(view_a.printer.id.cmp(&view_b.printer.id))
        }),
    }

    let candidates: Vec<Candidate> = ranked
        .into_iter()
        .enumerate()
        .map(|(index, (view, draft))| Candidate {
            printer_id: view.printer.id.clone(),
            printer_name: view.printer.name.clone(),
            rank: (index + 1) as i64,
            spool: draft.spool,
            spool_options: draft.spool_options,
            loaded_match: draft.loaded_match,
            last_used_at: view.last_used_at.map(|s| s.to_string()),
            manual_facts_acknowledgement_required: draft.manual_facts_acknowledgement_required,
        })
        .collect();

    let verdict = if candidates.is_empty() {
        EligibilityVerdict::Blocked
    } else if entry.policy == DispatchPolicy::Automatic {
        EligibilityVerdict::Ready
    } else {
        EligibilityVerdict::AwaitingOperator
    };

    QueueEntryEligibility {
        entry_id: entry.id.clone(),
        verdict,
        candidates,
        printers: printer_rows,
        blockers: aggregate_blockers(blocker_hits),
        evaluated_at: now.to_string(),
    }
}

/// A `Vec<Blocker>` in insertion order, with any later entry sharing an
/// earlier one's `code` dropped. `check_assignment` doesn't stop at a
/// Printer's first failing gate (unlike `evaluate`), so gates 2 and 4 can
/// each independently produce `NEEDS_MANUAL_PRINTER` for the same
/// Printer/Slice — an accepted overlap (D5), but the caller should see it
/// once, not twice.
fn dedupe_by_code(blockers: Vec<Blocker>) -> Vec<Blocker> {
    let mut seen_codes: Vec<BlockerCode> = Vec::new();
    blockers
        .into_iter()
        .filter(|b| {
            if seen_codes.contains(&b.code) {
                false
            } else {
                seen_codes.push(b.code);
                true
            }
        })
        .collect()
}

/// D5: re-runs every gate for one Printer and one Spool, and fails with
/// every blocker it finds (unlike `evaluate`, which stops at a Printer's
/// first failing gate). The operator may choose any of the entry's
/// `spoolOptions` for a Printer, not only its ranked choice, so this checks
/// exactly the pair given.
///
/// `printer_id` must name a Printer in `input.printers` — see
/// `EligibilityInput`'s doc comment for that precondition — or this
/// returns [`AssignmentCheckError::PrinterNotInView`] rather than a
/// blocker.
pub fn check_assignment(
    input: &EligibilityInput<'_>,
    printer_id: &str,
    spool_id: &str,
    mode: AssignMode,
) -> Result<(), AssignmentCheckError> {
    let entry = input.entry;
    let is_automatic = mode == AssignMode::Automatic;
    // D5: an absent fact is tolerated only for a Manual-policy entry, and
    // only once the operator has acknowledged it — an acknowledgement
    // alone never overrides a non-Manual policy (`AssignMode`'s doc
    // comment).
    let tolerate_absent_facts = entry.policy == DispatchPolicy::Manual
        && matches!(
            mode,
            AssignMode::Operator {
                acknowledge_manual_facts: true
            }
        );

    let mut blockers = Vec::new();

    if let Some(pin) = entry.manual_printer_id.as_deref() {
        if pin != printer_id {
            let pinned_name = input
                .printers
                .iter()
                .find(|p| p.printer.id == pin)
                .map(|p| p.printer.name.clone())
                .unwrap_or_else(|| pin.to_string());
            blockers.push(blocker(
                BlockerCode::PinnedToOtherPrinter,
                format!("This entry is pinned to {pinned_name}."),
                None,
                None,
                vec![printer_id.to_string()],
            ));
        }
    }

    let Some(view) = input.printers.iter().find(|p| p.printer.id == printer_id) else {
        return Err(AssignmentCheckError::PrinterNotInView);
    };

    if let Some(b) = gate1(is_automatic, view, input.claimed_printers, printer_id) {
        blockers.push(b);
    }

    let (gate2_blocker, _ack_required) =
        gate2(tolerate_absent_facts, input.facts, view.profile, printer_id);
    if let Some(b) = gate2_blocker {
        blockers.push(b);
    }

    if let Some(b) = gate3(is_automatic, view.capabilities, printer_id) {
        blockers.push(b);
    }

    // Gate 4's own absent-material-fact check (mirroring gate2's, and
    // gate4's in `evaluate_printer`): reported once, and only, instead of
    // falling through to the per-Spool checks below (which would otherwise
    // report the same absent fact as a spurious NO_COMPATIBLE_SPOOL).
    if material_fact_absent(input.facts) && !tolerate_absent_facts {
        blockers.push(blocker(
            BlockerCode::NeedsManualPrinter,
            NEEDS_MANUAL_PRINTER_MESSAGE,
            None,
            Some(RecoveryCode::AssignManually),
            vec![printer_id.to_string()],
        ));
    } else {
        match input.spools.iter().find(|s| s.id == spool_id) {
            None => blockers.push(blocker(
                BlockerCode::NoCompatibleSpool,
                no_compatible_spool_message(input.facts),
                None,
                Some(RecoveryCode::LoadSpool),
                vec![printer_id.to_string()],
            )),
            Some(spool) => {
                let relevant = is_relevant_to(view, spool);
                let material_ok =
                    spool_is_active_and_material_matches(input.facts, spool, tolerate_absent_facts);
                if !relevant || !material_ok {
                    blockers.push(blocker(
                        BlockerCode::NoCompatibleSpool,
                        no_compatible_spool_message(input.facts),
                        None,
                        Some(RecoveryCode::LoadSpool),
                        vec![printer_id.to_string()],
                    ));
                } else if spool.availability.available_mg < entry.estimate.amount_mg {
                    blockers.push(blocker(
                        BlockerCode::InsufficientMaterial,
                        insufficient_material_message(entry.estimate.amount_mg),
                        Some(spool.availability.available_mg.to_string()),
                        Some(RecoveryCode::LoadSpool),
                        vec![printer_id.to_string()],
                    ));
                } else if is_automatic && !is_loaded_here(view, spool_id) {
                    blockers.push(blocker(
                        BlockerCode::SpoolNotLoaded,
                        SPOOL_NOT_LOADED_MESSAGE,
                        None,
                        Some(RecoveryCode::LoadSpool),
                        vec![printer_id.to_string()],
                    ));
                }
            }
        }
    }

    let blockers = dedupe_by_code(blockers);
    if blockers.is_empty() {
        Ok(())
    } else {
        Err(AssignmentCheckError::Blocked(blockers))
    }
}

impl QueueEntryEligibility {
    /// Test/debug convenience: the ranked candidates' Printer ids.
    pub fn candidate_ids(&self) -> Vec<&str> {
        self.candidates
            .iter()
            .map(|c| c.printer_id.as_str())
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::catalog::BedShape;
    use crate::connections::capabilities::{CapabilityEvidence, CapabilityMap};
    use crate::connections::ConnectionConfig;
    use crate::printers::CatalogRef;
    use crate::slicing::facts::Farm3dFacts;
    use crate::spools::{AmountConfidence, Availability, MaterialFamily, SpoolFacets};

    // ---- fixtures ---------------------------------------------------

    fn a_bed_shape() -> BedShape {
        BedShape::Rectangular {
            width_mm: 256.0,
            depth_mm: 256.0,
            origin_x_mm: 0.0,
            origin_y_mm: 0.0,
        }
    }

    pub(crate) fn a_profile() -> PrinterProfile {
        PrinterProfile {
            bed_shape: a_bed_shape(),
            printable_height_mm: 256.0,
            bed_exclude_areas: Vec::new(),
            default_bed_type: "PEI".to_string(),
            nozzle_diameter_mm: vec![0.4],
            nozzle_type: "hardened_steel".to_string(),
            gcode_flavor: "klipper".to_string(),
            has_auxiliary_fan: false,
            supports_air_filtration: false,
            supports_multi_filament: false,
            suggested_host_type: None,
            suggested_port: None,
        }
    }

    fn a_profile_snapshot() -> crate::slicing::facts::ProfileSnapshot {
        crate::slicing::facts::ProfileSnapshot {
            catalog_ref: CatalogRef {
                vendor: "Elegoo".to_string(),
                model: "Centauri Carbon".to_string(),
                variant: "Elegoo Centauri Carbon 0.4 nozzle".to_string(),
                model_id: "ECC".to_string(),
                printer_variant: "0.4".to_string(),
            },
            bed_shape: a_bed_shape(),
            printable_height_mm: 256.0,
            bed_exclude_areas: Vec::new(),
            nozzle_type: "hardened_steel".to_string(),
            gcode_flavor: "klipper".to_string(),
        }
    }

    pub(crate) fn pla_175_facts() -> SliceFacts {
        Farm3dFacts::new(a_profile_snapshot(), 0.4, MaterialFamily::Pla, None, 1.75)
            .facts()
            .clone()
    }

    pub(crate) fn an_entry(
        policy: DispatchPolicy,
        preference: DispatchPreference,
        estimate_mg: i64,
    ) -> QueueEntry {
        QueueEntry {
            id: "qen-1".to_string(),
            revision: 1,
            slice_revision_id: "slr-1".to_string(),
            lineage_id: "qln-1".to_string(),
            copy_index: 0,
            copy_count: 1,
            origin_entry_id: None,
            origin_kind: None,
            state: super::super::QueueEntryState::Queued,
            close_reason: None,
            position: Some(1),
            policy,
            preference,
            estimate: super::super::MaterialEstimate {
                amount_mg: estimate_mg,
                source: super::super::EstimateSource::SliceEstimate,
            },
            manual_printer_id: None,
            job_id: None,
            requires_manual_printer_selection: false,
            allowed_actions: Vec::new(),
            display: super::super::QueueEntryDisplay {
                model_id: "mdl-1".to_string(),
                model_name: "Bracket".to_string(),
                plate_label: None,
                target_label: "Voron".to_string(),
                material_family: Some(MaterialFamily::Pla),
                material_other: None,
                print_seconds: None,
            },
            created_at: "2026-09-26T00:00:00Z".to_string(),
            updated_at: "2026-09-26T00:00:00Z".to_string(),
            closed_at: None,
        }
    }

    pub(crate) fn a_stored_printer(
        id: &str,
        name: &str,
        archived: bool,
        adapter: &str,
    ) -> StoredPrinter {
        StoredPrinter {
            id: id.to_string(),
            name: name.to_string(),
            connection: Some(ConnectionConfig {
                kind: adapter.to_string(),
                host: "192.0.2.10".to_string(),
                port: 7125,
                use_tls: false,
                credential_ref: None,
            }),
            archived_at: if archived {
                Some("2026-09-01T00:00:00Z".to_string())
            } else {
                None
            },
            ..Default::default()
        }
    }

    pub(crate) fn ready_status() -> PrinterStatus {
        let mut status = PrinterStatus::new(ConnectionState::Online);
        status.operational_state = OperationalState::Ready;
        status.freshness = TelemetryFreshness::Fresh;
        status
    }

    pub(crate) fn sim_capabilities(printer_id: &str) -> PrinterCapabilities {
        supported_capabilities(printer_id, EvidenceTier::Sim)
    }

    pub(crate) fn supported_capabilities(
        printer_id: &str,
        tier: EvidenceTier,
    ) -> PrinterCapabilities {
        PrinterCapabilities {
            printer_id: printer_id.to_string(),
            adapter_kind: Some("moonraker".to_string()),
            capabilities: CapabilityMap::complete(|_| CapabilityState::Supported {
                evidence: CapabilityEvidence {
                    source: "test".to_string(),
                    tier,
                    verified_host_versions: vec!["Moonraker v0.11.0-1 API 1.5.0".to_string()],
                },
            }),
            host_facts: None,
            observed_at: None,
        }
    }

    fn unsupported_capabilities(printer_id: &str, detail: &str) -> PrinterCapabilities {
        PrinterCapabilities {
            printer_id: printer_id.to_string(),
            adapter_kind: Some("octoprint".to_string()),
            capabilities: CapabilityMap::complete(|_| CapabilityState::Unsupported {
                reason: crate::connections::capabilities::UnsupportedReason::NotVerified,
                detail: detail.to_string(),
            }),
            host_facts: None,
            observed_at: None,
        }
    }

    /// Fixture spool ids follow the spec's `spl-<spoolNumber>` convention
    /// (`spl-7` is Spool #7), so the tie-break fixtures' spool-number
    /// ordering (F8) falls out of the id the fixture already names.
    fn spool_number_from_id(id: &str) -> i64 {
        id.trim_start_matches("spl-").parse().unwrap_or(0)
    }

    pub(crate) fn a_pla_spool(
        id: &str,
        number: i64,
        available_mg: i64,
        location: SpoolLocation,
    ) -> SpoolRecord {
        let loaded = matches!(location, SpoolLocation::Slot { .. });
        SpoolRecord {
            id: id.to_string(),
            revision: 1,
            spool_number: number,
            manufacturer: "Test".to_string(),
            product: None,
            material_family: MaterialFamily::Pla,
            material_other: None,
            color_name: "Black".to_string(),
            color_hex: None,
            diameter: crate::spools::FilamentDiameter::D175,
            nominal_mg: 1_000_000,
            low_threshold_mg: 50_000,
            tare_id: None,
            lifecycle: SpoolLifecycle::Active,
            location,
            availability: Availability {
                current_mg: available_mg,
                reserved_mg: 0,
                available_mg,
            },
            facets: SpoolFacets {
                loaded,
                reserved: false,
                low: false,
                reconciliation: false,
                confidence: AmountConfidence::Estimated,
            },
            last_measured_at: None,
            notes: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    /// A small builder for the tie-break fixtures: printers (by name, a
    /// loaded Spool, and a last-used timestamp), storage Spools, and one
    /// entry, matching the spec D5 fixtures F1-F8.
    struct World {
        printers: Vec<StoredPrinter>,
        profiles: Vec<PrinterProfile>,
        statuses: Vec<PrinterStatus>,
        capabilities: Vec<PrinterCapabilities>,
        loaded_spool_ids: Vec<Vec<String>>,
        last_used_at: Vec<Option<String>>,
        spools: Vec<SpoolRecord>,
        claimed: BTreeSet<String>,
    }

    impl World {
        fn new() -> Self {
            World {
                printers: Vec::new(),
                profiles: Vec::new(),
                statuses: Vec::new(),
                capabilities: Vec::new(),
                loaded_spool_ids: Vec::new(),
                last_used_at: Vec::new(),
                spools: Vec::new(),
                claimed: BTreeSet::new(),
            }
        }

        /// A Printer with `id`/`name`, loaded with a fresh Spool
        /// `(spool_id, available_mg)`, never used before.
        fn printer(mut self, id: &str, name: &str, loaded: (&str, i64)) -> Self {
            let (spool_id, available_mg) = loaded;
            self.spools.push(a_pla_spool(
                spool_id,
                spool_number_from_id(spool_id),
                available_mg,
                SpoolLocation::Slot {
                    slot_id: format!("slot-{id}"),
                    printer_id: id.to_string(),
                },
            ));
            self.printers
                .push(a_stored_printer(id, name, false, "moonraker"));
            self.profiles.push(a_profile());
            self.statuses.push(ready_status());
            self.capabilities.push(sim_capabilities(id));
            self.loaded_spool_ids.push(vec![spool_id.to_string()]);
            self.last_used_at.push(None);
            self
        }

        /// A Printer with nothing loaded.
        fn printer_unloaded(mut self, id: &str, name: &str) -> Self {
            self.printers
                .push(a_stored_printer(id, name, false, "moonraker"));
            self.profiles.push(a_profile());
            self.statuses.push(ready_status());
            self.capabilities.push(sim_capabilities(id));
            self.loaded_spool_ids.push(Vec::new());
            self.last_used_at.push(None);
            self
        }

        fn last_used(mut self, id: &str, at: &str) -> Self {
            let index = self
                .printers
                .iter()
                .position(|p| p.id == id)
                .expect("printer");
            self.last_used_at[index] = Some(at.to_string());
            self
        }

        fn stored(mut self, spool_id: &str, available_mg: i64) -> Self {
            self.spools.push(a_pla_spool(
                spool_id,
                spool_number_from_id(spool_id),
                available_mg,
                SpoolLocation::Storage {
                    storage_label: None,
                },
            ));
            self
        }

        fn entry(
            &self,
            policy: DispatchPolicy,
            preference: DispatchPreference,
            estimate_mg: i64,
        ) -> QueueEntry {
            an_entry(policy, preference, estimate_mg)
        }

        fn views(&self) -> Vec<PrinterView<'_>> {
            (0..self.printers.len())
                .map(|i| PrinterView {
                    printer: &self.printers[i],
                    profile: &self.profiles[i],
                    status: Some(&self.statuses[i]),
                    capabilities: &self.capabilities[i],
                    active_job: false,
                    foreign_host_op: false,
                    last_used_at: self.last_used_at[i].as_deref(),
                    loaded_spool_ids: &self.loaded_spool_ids[i],
                })
                .collect()
        }

        fn input<'a>(
            &'a self,
            entry: &'a QueueEntry,
            facts: &'a SliceFacts,
            views: &'a [PrinterView<'a>],
        ) -> EligibilityInput<'a> {
            EligibilityInput {
                entry,
                facts,
                printers: views,
                spools: &self.spools,
                claimed_printers: &self.claimed,
            }
        }
    }

    fn run(world: &World, entry: &QueueEntry, facts: &SliceFacts) -> QueueEntryEligibility {
        let views = world.views();
        let input = world.input(entry, facts, &views);
        evaluate(&input, "2026-09-27T00:00:00Z")
    }

    /// The ranked candidates' chosen Spool ids, in rank order.
    fn chosen_spool_ids(result: &QueueEntryEligibility) -> Vec<&str> {
        result
            .candidates
            .iter()
            .map(|c| c.spool.spool_id.as_str())
            .collect()
    }

    // ---- D5 gate 1 ----------------------------------------------------

    #[test]
    fn archived_pinned_printer_is_still_shown_with_printer_archived() {
        // D5 gate 0: an archived Printer is normally never in `printers`
        // at all — but the entry's `manualPrinterId` is the one exception
        // ("so the operator sees why"), and gate 1 still blocks it with
        // PRINTER_ARCHIVED rather than silently omitting it.
        let world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        let mut printer = world.printers[0].clone();
        printer.archived_at = Some("2026-09-01T00:00:00Z".to_string());
        let view = PrinterView {
            printer: &printer,
            ..world.views().remove(0)
        };
        let mut entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        entry.manual_printer_id = Some("prn-a".to_string());
        let facts = pla_175_facts();
        let input = world.input(&entry, &facts, std::slice::from_ref(&view));

        let result = evaluate(&input, "2026-09-27T00:00:00Z");

        assert_eq!(result.verdict, EligibilityVerdict::Blocked);
        assert_eq!(result.printers.len(), 1);
        assert_eq!(result.printers[0].printer_id, "prn-a");
        assert_eq!(
            result.printers[0].blockers[0].code,
            BlockerCode::PrinterArchived
        );
    }

    #[test]
    fn printer_with_no_connection_is_setup_incomplete() {
        let world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        let mut printer = world.printers[0].clone();
        printer.connection = None;
        let view = PrinterView {
            printer: &printer,
            ..world.views().remove(0)
        };
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();
        let input = world.input(&entry, &facts, std::slice::from_ref(&view));

        let result = evaluate(&input, "2026-09-27T00:00:00Z");

        assert_eq!(
            result.printers[0].blockers[0].code,
            BlockerCode::SetupIncomplete
        );
    }

    #[test]
    fn octoprint_not_verified_is_capability_unsupported() {
        let world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        let caps = unsupported_capabilities("prn-a", "OctoPrint's upload endpoint isn't verified.");
        let mut view = world.views().remove(0);
        view.capabilities = &caps;
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();
        let input = world.input(&entry, &facts, std::slice::from_ref(&view));

        let result = evaluate(&input, "2026-09-27T00:00:00Z");

        assert_eq!(result.verdict, EligibilityVerdict::Blocked);
        assert_eq!(
            result.printers[0].blockers[0].code,
            BlockerCode::CapabilityUnsupported
        );
    }

    #[test]
    fn readonly_hardware_evidence_never_qualifies_for_automatic() {
        let world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        let caps = supported_capabilities("prn-a", EvidenceTier::ReadOnlyHardware);
        let mut view = world.views().remove(0);
        view.capabilities = &caps;
        let entry = world.entry(
            DispatchPolicy::Automatic,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();
        let input = world.input(&entry, &facts, std::slice::from_ref(&view));

        let result = evaluate(&input, "2026-09-27T00:00:00Z");

        assert_eq!(result.verdict, EligibilityVerdict::Blocked);
        assert_eq!(
            result.printers[0].blockers[0].code,
            BlockerCode::AdapterNotProven
        );

        // The same Printer is fine under Recommended (not automatic-only).
        let recommended = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let recommended_input = world.input(&recommended, &facts, std::slice::from_ref(&view));
        let recommended_result = evaluate(&recommended_input, "2026-09-27T00:00:00Z");
        assert_eq!(
            recommended_result.verdict,
            EligibilityVerdict::AwaitingOperator
        );
    }

    #[test]
    fn claimed_printers_are_excluded_later_in_the_same_run() {
        let mut world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        world.claimed.insert("prn-a".to_string());
        let entry = world.entry(
            DispatchPolicy::Automatic,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();

        let result = run(&world, &entry, &facts);

        assert_eq!(result.verdict, EligibilityVerdict::Blocked);
        assert_eq!(result.printers[0].blockers[0].code, BlockerCode::JobActive);
        assert_eq!(
            result.printers[0].blockers[0].printer_ids,
            vec!["prn-a".to_string()]
        );
    }

    #[test]
    fn connection_error_blocks_and_clears() {
        let mut world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        world.statuses[0].connection_state = ConnectionState::Error;
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();

        let blocked = run(&world, &entry, &facts);
        assert_eq!(
            blocked.printers[0].blockers[0].code,
            BlockerCode::ConnectionError
        );

        world.statuses[0].connection_state = ConnectionState::Online;
        let passing = run(&world, &entry, &facts);
        assert_eq!(passing.verdict, EligibilityVerdict::AwaitingOperator);
    }

    #[test]
    fn printer_offline_blocks_including_when_status_is_none() {
        let mut world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        world.statuses[0].connection_state = ConnectionState::Offline;
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();

        let offline = run(&world, &entry, &facts);
        assert_eq!(
            offline.printers[0].blockers[0].code,
            BlockerCode::PrinterOffline
        );

        let view_no_status = PrinterView {
            status: None,
            ..world.views().remove(0)
        };
        let input_no_status = world.input(&entry, &facts, std::slice::from_ref(&view_no_status));
        let none_result = evaluate(&input_no_status, "2026-09-27T00:00:00Z");
        assert_eq!(
            none_result.printers[0].blockers[0].code,
            BlockerCode::PrinterOffline
        );

        world.statuses[0].connection_state = ConnectionState::Online;
        let passing = run(&world, &entry, &facts);
        assert_eq!(passing.verdict, EligibilityVerdict::AwaitingOperator);
    }

    #[test]
    fn host_operation_pending_blocks_and_clears() {
        let world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();

        let view = PrinterView {
            foreign_host_op: true,
            ..world.views().remove(0)
        };
        let input = world.input(&entry, &facts, std::slice::from_ref(&view));
        let blocked = evaluate(&input, "2026-09-27T00:00:00Z");
        assert_eq!(
            blocked.printers[0].blockers[0].code,
            BlockerCode::HostOperationPending
        );

        let passing = run(&world, &entry, &facts);
        assert_eq!(passing.verdict, EligibilityVerdict::AwaitingOperator);
    }

    #[test]
    fn printer_busy_external_blocks_and_clears() {
        let mut world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        world.statuses[0].operational_state = OperationalState::Printing;
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();

        let blocked = run(&world, &entry, &facts);
        assert_eq!(
            blocked.printers[0].blockers[0].code,
            BlockerCode::PrinterBusyExternal
        );

        world.statuses[0].operational_state = OperationalState::Ready;
        let passing = run(&world, &entry, &facts);
        assert_eq!(passing.verdict, EligibilityVerdict::AwaitingOperator);
    }

    #[test]
    fn job_active_via_the_active_job_flag() {
        let world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();

        let view = PrinterView {
            active_job: true,
            ..world.views().remove(0)
        };
        let input = world.input(&entry, &facts, std::slice::from_ref(&view));
        let result = evaluate(&input, "2026-09-27T00:00:00Z");

        assert_eq!(result.printers[0].blockers[0].code, BlockerCode::JobActive);
    }

    #[test]
    fn job_active_fires_before_printer_busy_external() {
        let mut world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        world.statuses[0].operational_state = OperationalState::Printing;
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();

        let view = PrinterView {
            active_job: true,
            ..world.views().remove(0)
        };
        let input = world.input(&entry, &facts, std::slice::from_ref(&view));
        let result = evaluate(&input, "2026-09-27T00:00:00Z");

        assert_eq!(result.printers[0].blockers[0].code, BlockerCode::JobActive);
    }

    #[test]
    fn printer_not_idle_by_state_and_by_freshness() {
        let mut world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        let automatic_entry = world.entry(
            DispatchPolicy::Automatic,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();

        // By state: `Failed` is neither ready/finished/cancelled nor one of
        // gate 1's own printing/paused/busy rows, isolating PRINTER_NOT_IDLE.
        world.statuses[0].operational_state = OperationalState::Failed;
        let by_state = run(&world, &automatic_entry, &facts);
        assert_eq!(
            by_state.printers[0].blockers[0].code,
            BlockerCode::PrinterNotIdle
        );

        // By freshness: `Ready`, but stale telemetry.
        world.statuses[0].operational_state = OperationalState::Ready;
        world.statuses[0].freshness = TelemetryFreshness::Stale;
        let by_freshness = run(&world, &automatic_entry, &facts);
        assert_eq!(
            by_freshness.printers[0].blockers[0].code,
            BlockerCode::PrinterNotIdle
        );

        // Passing: ready and fresh.
        world.statuses[0].freshness = TelemetryFreshness::Fresh;
        let passing = run(&world, &automatic_entry, &facts);
        assert_eq!(passing.verdict, EligibilityVerdict::Ready);
    }

    // ---- D5 gate 2 ------------------------------------------------------

    #[test]
    fn absent_material_fact_is_manual_only() {
        // F5: the revision's materialFamily fact is absent.
        let world = World::new()
            .printer("prn-a", "A", ("spl-1", 500_000))
            .printer("prn-b", "B", ("spl-4", 900_000))
            .stored("spl-2", 900_000);
        let mut world = world;
        world.spools[0].material_family = MaterialFamily::Petg; // prn-a's loaded spl-1 is PETG.

        let facts =
            crate::slicing::facts::ExternalFacts::new(crate::slicing::facts::ConfirmedFacts {
                printer_profile: crate::slicing::facts::ConfirmedFact::Confirmed(
                    a_profile_snapshot(),
                ),
                nozzle_diameter_mm: crate::slicing::facts::ConfirmedFact::Confirmed(0.4),
                material_family: crate::slicing::facts::ConfirmedFact::Absent,
                material_other: None,
                filament_diameter_mm: crate::slicing::facts::ConfirmedFact::Confirmed(1.75),
            })
            .facts()
            .clone();

        let mut manual_entry = world.entry(
            DispatchPolicy::Manual,
            DispatchPreference::LoadedFirst,
            100_000,
        );
        manual_entry.manual_printer_id = Some("prn-a".to_string());
        let manual_result = run(&world, &manual_entry, &facts);
        assert_eq!(manual_result.candidate_ids(), ["prn-a"]);
        assert!(manual_result.candidates[0].manual_facts_acknowledgement_required);
        assert_eq!(
            manual_result.candidates[0]
                .spool_options
                .iter()
                .map(|o| o.spool_id.as_str())
                .collect::<Vec<_>>(),
            vec!["spl-1", "spl-2"]
        );
        // F5: prn-b is omitted from `printers[]` entirely (gate 0), and the
        // aggregate `blockers` carries PINNED_TO_OTHER_PRINTER for it.
        assert!(!manual_result
            .printers
            .iter()
            .any(|p| p.printer_id == "prn-b"));
        assert_eq!(manual_result.printers.len(), 1);
        let pinned_blocker = manual_result
            .blockers
            .iter()
            .find(|b| b.code == BlockerCode::PinnedToOtherPrinter)
            .expect("PINNED_TO_OTHER_PRINTER for prn-b");
        assert_eq!(pinned_blocker.printer_ids, vec!["prn-b".to_string()]);

        let recommended_entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            100_000,
        );
        let recommended_result = run(&world, &recommended_entry, &facts);
        assert_eq!(recommended_result.verdict, EligibilityVerdict::Blocked);
        assert!(recommended_result
            .blockers
            .iter()
            .any(|b| b.code == BlockerCode::NeedsManualPrinter));
    }

    #[test]
    fn nozzle_mismatch_reports_profile_mismatch_with_detail() {
        let mut world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        world.profiles[0].nozzle_diameter_mm = vec![0.6];
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();

        let result = run(&world, &entry, &facts);

        assert_eq!(
            result.printers[0].blockers[0].code,
            BlockerCode::ProfileMismatch
        );
        // The spec's own example format: "Nozzle 0.6 mm; this Slice needs
        // 0.4 mm." (D5) — not "0.60"/"0.40".
        assert_eq!(
            result.printers[0].blockers[0].detail.as_deref(),
            Some("Nozzle 0.6 mm; this Slice needs 0.4 mm.")
        );
    }

    #[test]
    fn profile_mismatch_bed_shape() {
        let mut world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        world.profiles[0].bed_shape = BedShape::Rectangular {
            width_mm: 300.0,
            depth_mm: 300.0,
            origin_x_mm: 0.0,
            origin_y_mm: 0.0,
        };
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();

        let result = run(&world, &entry, &facts);

        assert_eq!(
            result.printers[0].blockers[0].code,
            BlockerCode::ProfileMismatch
        );
        assert_eq!(
            result.printers[0].blockers[0].detail.as_deref(),
            Some("The bed shape doesn't match this Slice.")
        );
    }

    #[test]
    fn profile_mismatch_printable_height() {
        let mut world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        world.profiles[0].printable_height_mm = 100.0;
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();

        let result = run(&world, &entry, &facts);

        assert_eq!(
            result.printers[0].blockers[0].code,
            BlockerCode::ProfileMismatch
        );
        assert_eq!(
            result.printers[0].blockers[0].detail.as_deref(),
            Some("The printable height doesn't match this Slice.")
        );
    }

    #[test]
    fn profile_mismatch_nozzle_type() {
        let mut world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        world.profiles[0].nozzle_type = "brass".to_string();
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();

        let result = run(&world, &entry, &facts);

        assert_eq!(
            result.printers[0].blockers[0].code,
            BlockerCode::ProfileMismatch
        );
        assert_eq!(
            result.printers[0].blockers[0].detail.as_deref(),
            Some("The nozzle type doesn't match this Slice.")
        );
    }

    #[test]
    fn profile_mismatch_gcode_flavor() {
        let mut world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        world.profiles[0].gcode_flavor = "marlin".to_string();
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();

        let result = run(&world, &entry, &facts);

        assert_eq!(
            result.printers[0].blockers[0].code,
            BlockerCode::ProfileMismatch
        );
        assert_eq!(
            result.printers[0].blockers[0].detail.as_deref(),
            Some("The G-code flavor doesn't match this Slice.")
        );
    }

    // ---- D5 gate 4 / spool selection -----------------------------------

    #[test]
    fn diameter_175_matches_1_75_fact_within_tolerance() {
        let mut world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        let facts =
            crate::slicing::facts::ExternalFacts::new(crate::slicing::facts::ConfirmedFacts {
                printer_profile: crate::slicing::facts::ConfirmedFact::Confirmed(
                    a_profile_snapshot(),
                ),
                nozzle_diameter_mm: crate::slicing::facts::ConfirmedFact::Confirmed(0.4),
                material_family: crate::slicing::facts::ConfirmedFact::Confirmed(
                    MaterialFamily::Pla,
                ),
                material_other: None,
                filament_diameter_mm: crate::slicing::facts::ConfirmedFact::Confirmed(1.751),
            })
            .facts()
            .clone();
        world.spools[0].diameter = crate::spools::FilamentDiameter::D175;
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );

        let result = run(&world, &entry, &facts);

        assert_eq!(result.candidate_ids(), ["prn-a"]);
    }

    #[test]
    fn no_compatible_spool_when_nothing_matches_material() {
        let mut world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        world.spools[0].material_family = MaterialFamily::Petg; // Facts need PLA.
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();

        let blocked = run(&world, &entry, &facts);
        assert_eq!(
            blocked.printers[0].blockers[0].code,
            BlockerCode::NoCompatibleSpool
        );
        // D5: the family's wire name (`PLA`), not its Rust `Debug` form.
        assert_eq!(
            blocked.printers[0].blockers[0].message,
            "No Spool of PLA 1.75 mm is available."
        );

        world.spools[0].material_family = MaterialFamily::Pla;
        let passing = run(&world, &entry, &facts);
        assert_eq!(passing.verdict, EligibilityVerdict::AwaitingOperator);
    }

    #[test]
    fn absent_family_with_a_diameter_mismatch_is_no_compatible_spool_not_needs_manual_printer() {
        // D5: an absent `materialFamily` fact only skips *that*
        // comparison — a concrete diameter mismatch (a 2.85 mm Spool
        // against a 1.75 mm fact) must still block, as
        // NO_COMPATIBLE_SPOOL, not be masked by the absent family into a
        // pass or into NEEDS_MANUAL_PRINTER.
        let mut world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        world.spools[0].diameter = crate::spools::FilamentDiameter::D285;
        let facts =
            crate::slicing::facts::ExternalFacts::new(crate::slicing::facts::ConfirmedFacts {
                printer_profile: crate::slicing::facts::ConfirmedFact::Confirmed(
                    a_profile_snapshot(),
                ),
                nozzle_diameter_mm: crate::slicing::facts::ConfirmedFact::Confirmed(0.4),
                material_family: crate::slicing::facts::ConfirmedFact::Absent,
                material_other: None,
                filament_diameter_mm: crate::slicing::facts::ConfirmedFact::Confirmed(1.75),
            })
            .facts()
            .clone();
        // Manual so the absent family is tolerated at all (else it would
        // block as NEEDS_MANUAL_PRINTER before the Spool search even
        // runs) — isolating the diameter mismatch this test is about.
        let entry = world.entry(
            DispatchPolicy::Manual,
            DispatchPreference::LoadedFirst,
            50_000,
        );

        let result = run(&world, &entry, &facts);

        assert_eq!(
            result.printers[0].blockers[0].code,
            BlockerCode::NoCompatibleSpool
        );
    }

    #[test]
    fn insufficient_material_rounds_a_fractional_gram_estimate_up() {
        assert_eq!(
            insufficient_material_message(12_345),
            "No matching Spool has 13 g available."
        );
        assert_eq!(
            insufficient_material_message(12_000),
            "No matching Spool has 12 g available."
        );
        assert_eq!(
            insufficient_material_message(1),
            "No matching Spool has 1 g available."
        );
    }

    #[test]
    fn insufficient_material_states_grams_and_details_the_best_available_mg() {
        let world = World::new().printer("prn-a", "A", ("spl-1", 50_000));
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            300_000,
        );
        let facts = pla_175_facts();

        let result = run(&world, &entry, &facts);

        assert_eq!(
            result.printers[0].blockers[0].code,
            BlockerCode::InsufficientMaterial
        );
        // D5's message format states grams (300_000 mg estimate -> "300 g"),
        // while `detail` keeps the best availableMg as a raw mg figure.
        assert_eq!(
            result.printers[0].blockers[0].message,
            "No matching Spool has 300 g available."
        );
        assert_eq!(
            result.printers[0].blockers[0].detail.as_deref(),
            Some("50000")
        );
    }

    #[test]
    fn nozzle_list_with_two_entries_is_nozzle_count_mismatch() {
        let mut world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        world.profiles[0].nozzle_diameter_mm = vec![0.4, 0.6];
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();

        let result = run(&world, &entry, &facts);

        assert_eq!(
            result.printers[0].blockers[0].code,
            BlockerCode::ProfileMismatch
        );
    }

    #[test]
    fn stored_spool_is_allowed_for_manual_and_blocked_for_automatic() {
        let world = World::new()
            .printer_unloaded("prn-a", "A")
            .stored("spl-9", 800_000);
        let facts = pla_175_facts();

        let mut manual_entry = world.entry(
            DispatchPolicy::Manual,
            DispatchPreference::LoadedFirst,
            100_000,
        );
        manual_entry.manual_printer_id = Some("prn-a".to_string());
        let views = world.views();
        let input = world.input(&manual_entry, &facts, &views);
        assert_eq!(
            check_assignment(
                &input,
                "prn-a",
                "spl-9",
                AssignMode::Operator {
                    acknowledge_manual_facts: false
                },
            ),
            Ok(())
        );

        let automatic_result = check_assignment(&input, "prn-a", "spl-9", AssignMode::Automatic);
        let AssignmentCheckError::Blocked(blockers) =
            automatic_result.expect_err("storage-only Spool should block Automatic")
        else {
            panic!("expected Blocked");
        };
        assert!(blockers
            .iter()
            .any(|b| b.code == BlockerCode::SpoolNotLoaded));
    }

    #[test]
    fn recommended_entry_with_absent_facts_is_blocked_even_when_acknowledged() {
        // D5: the Manual-only tolerance is keyed on the entry's own
        // policy, not on the operator's acknowledgement alone — a
        // Recommended entry stays NEEDS_MANUAL_PRINTER no matter what
        // `acknowledge_manual_facts` says.
        let world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        let facts =
            crate::slicing::facts::ExternalFacts::new(crate::slicing::facts::ConfirmedFacts {
                printer_profile: crate::slicing::facts::ConfirmedFact::Confirmed(
                    a_profile_snapshot(),
                ),
                nozzle_diameter_mm: crate::slicing::facts::ConfirmedFact::Confirmed(0.4),
                material_family: crate::slicing::facts::ConfirmedFact::Absent,
                material_other: None,
                filament_diameter_mm: crate::slicing::facts::ConfirmedFact::Confirmed(1.75),
            })
            .facts()
            .clone();
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            100_000,
        );
        let views = world.views();
        let input = world.input(&entry, &facts, &views);

        let result = check_assignment(
            &input,
            "prn-a",
            "spl-1",
            AssignMode::Operator {
                acknowledge_manual_facts: true,
            },
        );

        let AssignmentCheckError::Blocked(blockers) =
            result.expect_err("Recommended must stay blocked")
        else {
            panic!("expected Blocked");
        };
        assert!(blockers
            .iter()
            .any(|b| b.code == BlockerCode::NeedsManualPrinter));
    }

    #[test]
    fn check_assignment_dedupes_needs_manual_printer_from_gates_2_and_4() {
        // Both the printerProfile fact and the materialFamily fact are
        // absent: gate 2 and gate 4 each independently want to report
        // NEEDS_MANUAL_PRINTER (an accepted overlap, D5), but
        // `check_assignment`'s result must list it once.
        let world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        let facts =
            crate::slicing::facts::ExternalFacts::new(crate::slicing::facts::ConfirmedFacts {
                printer_profile: crate::slicing::facts::ConfirmedFact::Absent,
                nozzle_diameter_mm: crate::slicing::facts::ConfirmedFact::Absent,
                material_family: crate::slicing::facts::ConfirmedFact::Absent,
                material_other: None,
                filament_diameter_mm: crate::slicing::facts::ConfirmedFact::Absent,
            })
            .facts()
            .clone();
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            100_000,
        );
        let views = world.views();
        let input = world.input(&entry, &facts, &views);

        let result = check_assignment(
            &input,
            "prn-a",
            "spl-1",
            AssignMode::Operator {
                acknowledge_manual_facts: false,
            },
        );

        let AssignmentCheckError::Blocked(blockers) = result.expect_err("must be blocked") else {
            panic!("expected Blocked");
        };
        assert_eq!(
            blockers
                .iter()
                .filter(|b| b.code == BlockerCode::NeedsManualPrinter)
                .count(),
            1
        );
    }

    #[test]
    fn check_assignment_reports_printer_not_in_view_for_an_unknown_printer_id() {
        let world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();
        let views = world.views();
        let input = world.input(&entry, &facts, &views);

        let result = check_assignment(
            &input,
            "prn-does-not-exist",
            "spl-1",
            AssignMode::Operator {
                acknowledge_manual_facts: false,
            },
        );

        assert_eq!(result, Err(AssignmentCheckError::PrinterNotInView));
    }

    #[test]
    fn check_assignment_collects_several_blockers_at_once() {
        // Offline (gate 1) and an unsupported capability (gate 3) are
        // independent failures, so both should show up together — this is
        // exactly why `check_assignment` doesn't stop at the first gate,
        // unlike `evaluate`.
        let mut world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        world.statuses[0].connection_state = ConnectionState::Offline;
        world.capabilities[0] =
            unsupported_capabilities("prn-a", "OctoPrint's upload endpoint isn't verified.");
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();
        let views = world.views();
        let input = world.input(&entry, &facts, &views);

        let result = check_assignment(
            &input,
            "prn-a",
            "spl-1",
            AssignMode::Operator {
                acknowledge_manual_facts: false,
            },
        );

        let AssignmentCheckError::Blocked(blockers) = result.expect_err("must be blocked") else {
            panic!("expected Blocked");
        };
        assert!(blockers
            .iter()
            .any(|b| b.code == BlockerCode::PrinterOffline));
        assert!(blockers
            .iter()
            .any(|b| b.code == BlockerCode::CapabilityUnsupported));
        assert_eq!(blockers.len(), 2);
    }

    // ---- Tie-break fixtures (D5) ----------------------------------------

    #[test]
    fn tie_break_fixture_1_equal_names_order_by_id() {
        // F1: both Printers are named exactly "Voron" (equal names,
        // different ids) — id alone decides.
        let world = World::new()
            .printer("prn-b", "Voron", ("spl-1", 900_000))
            .printer("prn-a", "Voron", ("spl-2", 900_000));
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();

        let result = run(&world, &entry, &facts);

        assert_eq!(result.candidate_ids(), ["prn-a", "prn-b"]);
    }

    #[test]
    fn tie_break_fixture_2_case_only_name_differences() {
        let world = World::new()
            .printer("prn-3", "Bravo", ("spl-3", 900_000))
            .printer("prn-2", "alpha", ("spl-2", 900_000))
            .printer("prn-1", "Alpha", ("spl-1", 900_000));
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            50_000,
        );
        let facts = pla_175_facts();

        let result = run(&world, &entry, &facts);

        assert_eq!(result.candidate_ids(), ["prn-1", "prn-2", "prn-3"]);
    }

    #[test]
    fn tie_break_fixture_3_loaded_first_versus_least_recently_used() {
        let world = World::new()
            .printer("prn-a", "A", ("spl-1", 500_000))
            .last_used("prn-a", "2026-09-20T10:00:00Z")
            .printer_unloaded("prn-b", "B")
            .printer("prn-c", "C", ("spl-3", 700_000))
            .last_used("prn-c", "2026-09-25T10:00:00Z")
            .stored("spl-9", 800_000);
        let facts = pla_175_facts();

        let recommended_loaded_first = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            300_000,
        );
        let recommended_loaded_first_result = run(&world, &recommended_loaded_first, &facts);
        assert_eq!(
            recommended_loaded_first_result.candidate_ids(),
            ["prn-c", "prn-a", "prn-b"]
        );
        assert_eq!(
            chosen_spool_ids(&recommended_loaded_first_result),
            ["spl-3", "spl-1", "spl-9"]
        );

        let recommended_lru = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LeastRecentlyUsed,
            300_000,
        );
        let recommended_lru_result = run(&world, &recommended_lru, &facts);
        assert_eq!(
            recommended_lru_result.candidate_ids(),
            ["prn-b", "prn-a", "prn-c"]
        );
        assert_eq!(
            chosen_spool_ids(&recommended_lru_result),
            ["spl-9", "spl-1", "spl-3"]
        );

        let automatic_loaded_first = world.entry(
            DispatchPolicy::Automatic,
            DispatchPreference::LoadedFirst,
            300_000,
        );
        let automatic_loaded_first_result = run(&world, &automatic_loaded_first, &facts);
        assert_eq!(
            automatic_loaded_first_result.candidate_ids(),
            ["prn-c", "prn-a"]
        );
        assert_eq!(
            chosen_spool_ids(&automatic_loaded_first_result),
            ["spl-3", "spl-1"]
        );
        let prn_b = automatic_loaded_first_result
            .printers
            .iter()
            .find(|p| p.printer_id == "prn-b")
            .unwrap();
        assert_eq!(prn_b.blockers[0].code, BlockerCode::SpoolNotLoaded);

        let automatic_lru = world.entry(
            DispatchPolicy::Automatic,
            DispatchPreference::LeastRecentlyUsed,
            300_000,
        );
        let automatic_lru_result = run(&world, &automatic_lru, &facts);
        assert_eq!(automatic_lru_result.candidate_ids(), ["prn-a", "prn-c"]);
        assert_eq!(chosen_spool_ids(&automatic_lru_result), ["spl-1", "spl-3"]);
        let prn_b_lru = automatic_lru_result
            .printers
            .iter()
            .find(|p| p.printer_id == "prn-b")
            .unwrap();
        assert_eq!(prn_b_lru.blockers[0].code, BlockerCode::SpoolNotLoaded);
    }

    #[test]
    fn tie_break_fixture_4_insufficient_but_loaded_spool() {
        let world = World::new()
            .printer("prn-a", "A", ("spl-1", 200_000))
            .printer("prn-b", "B", ("spl-3", 400_000))
            .stored("spl-2", 1_000_000);
        let facts = pla_175_facts();

        let recommended = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            300_000,
        );
        let recommended_result = run(&world, &recommended, &facts);
        assert_eq!(recommended_result.candidate_ids(), ["prn-b", "prn-a"]);
        let prn_a = recommended_result
            .candidates
            .iter()
            .find(|c| c.printer_id == "prn-a")
            .unwrap();
        assert_eq!(prn_a.spool.spool_id, "spl-2");
        let prn_b = recommended_result
            .candidates
            .iter()
            .find(|c| c.printer_id == "prn-b")
            .unwrap();
        assert_eq!(
            prn_b
                .spool_options
                .iter()
                .map(|o| o.spool_id.as_str())
                .collect::<Vec<_>>(),
            vec!["spl-3", "spl-2"]
        );

        let automatic = world.entry(
            DispatchPolicy::Automatic,
            DispatchPreference::LoadedFirst,
            300_000,
        );
        let automatic_result = run(&world, &automatic, &facts);
        assert_eq!(automatic_result.candidate_ids(), ["prn-b"]);
        let prn_a_auto = automatic_result
            .printers
            .iter()
            .find(|p| p.printer_id == "prn-a")
            .unwrap();
        assert_eq!(prn_a_auto.blockers[0].code, BlockerCode::SpoolNotLoaded);
    }

    #[test]
    fn tie_break_fixture_5_absent_material_fact_under_manual() {
        // See `absent_material_fact_is_manual_only` above for this fixture
        // in full (F5). This test only re-confirms the Recommended branch
        // reports NEEDS_MANUAL_PRINTER for every Printer, with an empty
        // candidate list.
        let world = World::new()
            .printer("prn-a", "A", ("spl-1", 500_000))
            .printer("prn-b", "B", ("spl-4", 900_000))
            .stored("spl-2", 900_000);
        let facts =
            crate::slicing::facts::ExternalFacts::new(crate::slicing::facts::ConfirmedFacts {
                printer_profile: crate::slicing::facts::ConfirmedFact::Confirmed(
                    a_profile_snapshot(),
                ),
                nozzle_diameter_mm: crate::slicing::facts::ConfirmedFact::Confirmed(0.4),
                material_family: crate::slicing::facts::ConfirmedFact::Absent,
                material_other: None,
                filament_diameter_mm: crate::slicing::facts::ConfirmedFact::Confirmed(1.75),
            })
            .facts()
            .clone();
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            100_000,
        );

        let result = run(&world, &entry, &facts);

        assert_eq!(result.verdict, EligibilityVerdict::Blocked);
        assert!(result.candidates.is_empty());
        assert!(result
            .printers
            .iter()
            .all(|p| p.blockers[0].code == BlockerCode::NeedsManualPrinter));
    }

    #[test]
    fn tie_break_fixture_6_two_automatic_entries_competing_for_one_printer() {
        let mut world = World::new().printer("prn-a", "A", ("spl-1", 900_000));
        let e1 = world.entry(
            DispatchPolicy::Automatic,
            DispatchPreference::LoadedFirst,
            100_000,
        );
        let facts = pla_175_facts();

        let e1_result = run(&world, &e1, &facts);
        assert_eq!(e1_result.candidate_ids(), ["prn-a"]);

        // E1 is assigned; prn-a is claimed for the rest of this run.
        world.claimed.insert("prn-a".to_string());

        let e2 = world.entry(
            DispatchPolicy::Automatic,
            DispatchPreference::LoadedFirst,
            100_000,
        );
        let e2_result = run(&world, &e2, &facts);

        assert_eq!(e2_result.verdict, EligibilityVerdict::Blocked);
        assert!(e2_result.candidates.is_empty());
        let top_blocker = e2_result.blockers.first().expect("a blocker");
        assert_eq!(top_blocker.code, BlockerCode::JobActive);
        assert_eq!(top_blocker.printer_ids, vec!["prn-a".to_string()]);
    }

    #[test]
    fn tie_break_fixture_7_unproven_adapter_under_automatic() {
        let world = World::new()
            .printer("prn-m", "M", ("spl-1", 900_000))
            .printer("prn-o", "O", ("spl-2", 900_000))
            .printer("prn-s", "S", ("spl-3", 100_000));
        let mut world = world;
        world.capabilities[0] = supported_capabilities("prn-m", EvidenceTier::ReadOnlyHardware);
        world.capabilities[1] = unsupported_capabilities("prn-o", "OctoPrint isn't verified.");
        // F7: prn-o is OctoPrint (`notVerified`), not Moonraker — its
        // Connection, not only its capability row, says so.
        world.printers[1].connection.as_mut().unwrap().kind = "octoprint".to_string();
        // prn-s already has sim evidence via World::printer's default.
        let facts = pla_175_facts();

        let automatic = world.entry(
            DispatchPolicy::Automatic,
            DispatchPreference::LoadedFirst,
            100_000,
        );
        let automatic_result = run(&world, &automatic, &facts);
        assert_eq!(automatic_result.candidate_ids(), ["prn-s"]);
        assert_eq!(chosen_spool_ids(&automatic_result), ["spl-3"]);
        let prn_m = automatic_result
            .printers
            .iter()
            .find(|p| p.printer_id == "prn-m")
            .unwrap();
        assert_eq!(prn_m.blockers[0].code, BlockerCode::AdapterNotProven);
        let prn_o = automatic_result
            .printers
            .iter()
            .find(|p| p.printer_id == "prn-o")
            .unwrap();
        assert_eq!(prn_o.blockers[0].code, BlockerCode::CapabilityUnsupported);

        let recommended = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            100_000,
        );
        let recommended_result = run(&world, &recommended, &facts);
        assert_eq!(recommended_result.candidate_ids(), ["prn-m", "prn-s"]);
        // F7: "[prn-m (spl-1), prn-s (spl-3)]" — the chosen Spool per row.
        assert_eq!(chosen_spool_ids(&recommended_result), ["spl-1", "spl-3"]);
        let prn_o_recommended = recommended_result
            .printers
            .iter()
            .find(|p| p.printer_id == "prn-o")
            .unwrap();
        assert_eq!(
            prn_o_recommended.blockers[0].code,
            BlockerCode::CapabilityUnsupported
        );
    }

    #[test]
    fn tie_break_fixture_8_choosing_among_equal_spools() {
        let world = World::new()
            .printer_unloaded("prn-a", "A")
            .stored("spl-7", 400_000)
            .stored("spl-3", 400_000)
            .stored("spl-5", 250_000);
        let entry = world.entry(
            DispatchPolicy::Recommended,
            DispatchPreference::LoadedFirst,
            100_000,
        );
        let facts = pla_175_facts();

        let result = run(&world, &entry, &facts);

        assert_eq!(result.candidate_ids(), ["prn-a"]);
        assert_eq!(result.candidates[0].spool.spool_id, "spl-5");
        assert_eq!(
            result.candidates[0]
                .spool_options
                .iter()
                .map(|o| o.spool_id.as_str())
                .collect::<Vec<_>>(),
            vec!["spl-5", "spl-3", "spl-7"]
        );
    }
}
