/** The Queue/Job frontend types: re-exports of the generated contracts
 *  (the source of truth), plus the shared-channel filter and a couple of
 *  small predicates every store/component needs. Mirrors
 *  `src/host-ops/types.ts`. */
import type { EventEnvelope } from "../generated/contracts/event/EventEnvelope";
import type { JobState } from "../generated/contracts/domain/JobState";
import type { QueueEventPayload } from "../generated/contracts/domain/QueueEventPayload";
import type { QueueEventType } from "../generated/contracts/domain/QueueEventType";

export type { AmountEntry } from "../generated/contracts/domain/AmountEntry";
export type { AssignedBy } from "../generated/contracts/domain/AssignedBy";
export type { Blocker } from "../generated/contracts/domain/Blocker";
export type { BlockerCode } from "../generated/contracts/domain/BlockerCode";
export type { Candidate } from "../generated/contracts/domain/Candidate";
export type { CancelReason } from "../generated/contracts/domain/CancelReason";
export type { CloseReason } from "../generated/contracts/domain/CloseReason";
export type { DeclaredOutcome } from "../generated/contracts/domain/DeclaredOutcome";
export type { DispatchPolicy } from "../generated/contracts/domain/DispatchPolicy";
export type { DispatchPreference } from "../generated/contracts/domain/DispatchPreference";
export type { EligibilitySummary } from "../generated/contracts/domain/EligibilitySummary";
export type { EligibilityVerdict } from "../generated/contracts/domain/EligibilityVerdict";
export type { EstimateSource } from "../generated/contracts/domain/EstimateSource";
export type { Job } from "../generated/contracts/domain/Job";
export type { JobAction } from "../generated/contracts/domain/JobAction";
export type { JobEvent } from "../generated/contracts/domain/JobEvent";
export type { JobEventKind } from "../generated/contracts/domain/JobEventKind";
export type { JobFailure } from "../generated/contracts/domain/JobFailure";
export type { JobHistory } from "../generated/contracts/domain/JobHistory";
export type { JobState } from "../generated/contracts/domain/JobState";
export type { MaterialEstimate } from "../generated/contracts/domain/MaterialEstimate";
export type { NextAutomaticAction } from "../generated/contracts/domain/NextAutomaticAction";
export type { OriginKind } from "../generated/contracts/domain/OriginKind";
export type { PriorState } from "../generated/contracts/domain/PriorState";
export type { PrinterEligibility } from "../generated/contracts/domain/PrinterEligibility";
export type { PrinterSnapshot } from "../generated/contracts/domain/PrinterSnapshot";
export type { QueueChange } from "../generated/contracts/domain/QueueChange";
export type { QueueEntry } from "../generated/contracts/domain/QueueEntry";
export type { QueueEntryAction } from "../generated/contracts/domain/QueueEntryAction";
export type { QueueEntryDisplay } from "../generated/contracts/domain/QueueEntryDisplay";
export type { QueueEntryEligibility } from "../generated/contracts/domain/QueueEntryEligibility";
export type { QueueEntryState } from "../generated/contracts/domain/QueueEntryState";
export type { QueueEventPayload } from "../generated/contracts/domain/QueueEventPayload";
export type { QueueEventType } from "../generated/contracts/domain/QueueEventType";
export type { QueueSnapshot } from "../generated/contracts/domain/QueueSnapshot";
export type { ReconciliationRequirement } from "../generated/contracts/domain/ReconciliationRequirement";
export type { RequirementKind } from "../generated/contracts/domain/RequirementKind";
export type { RequirementResolution } from "../generated/contracts/domain/RequirementResolution";
export type { RequirementStatus } from "../generated/contracts/domain/RequirementStatus";
export type { Reservation } from "../generated/contracts/domain/Reservation";
export type { ReservationHolder } from "../generated/contracts/domain/ReservationHolder";
export type { ReservationState } from "../generated/contracts/domain/ReservationState";
export type { SettleChoice } from "../generated/contracts/domain/SettleChoice";
export type { Settlement } from "../generated/contracts/domain/Settlement";
export type { SettlementMethod } from "../generated/contracts/domain/SettlementMethod";
export type { SettlementPreview } from "../generated/contracts/domain/SettlementPreview";
export type { SpoolOption } from "../generated/contracts/domain/SpoolOption";
export type { StartConfirmation } from "../generated/contracts/domain/StartConfirmation";

export type { RecoveryCode } from "../generated/contracts/command/RecoveryCode";

/** The exact envelope emitted on the shared `farm3d-event-v1` channel for
 *  this stream (spec "Events"). */
export type QueueEvent = EventEnvelope<QueueEventType, QueueEventPayload>;

/** D3: the states a Job never leaves. */
export function isTerminalJobState(state: JobState): boolean {
  return state === "completed" || state === "failed" || state === "cancelled";
}

/** Shared-channel filter (mirrors `host-ops/types.ts`'s `isHostOperationsEvent`):
 *  `farm3d-event-v1` also carries the Library, Printer status, inventory,
 *  slicing, and host-ops streams. This store keeps only `queue.*` types, so
 *  a foreign `streamId`/`sequence` never reaches its reconciliation. */
export function isQueueEvent(value: unknown): value is QueueEvent {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as { contractVersion?: unknown; type?: unknown };
  return candidate.contractVersion === 1
    && typeof candidate.type === "string"
    && candidate.type.startsWith("queue.");
}
