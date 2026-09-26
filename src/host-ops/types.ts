/** The Host Operations frontend types: re-exports of the generated
 *  contracts (the source of truth), plus the shared-channel filter and a
 *  couple of small predicates every store/component needs. */
import type { EventEnvelope } from "../generated/contracts/event/EventEnvelope";
import type { HostOperation } from "../generated/contracts/domain/HostOperation";
import type { HostOperationState } from "../generated/contracts/domain/HostOperationState";
import type { HostOperationsEventType } from "../generated/contracts/domain/HostOperationsEventType";

export type { AdapterCapabilityRow } from "../generated/contracts/domain/AdapterCapabilityRow";
export type { CapabilityEvidence } from "../generated/contracts/domain/CapabilityEvidence";
export type { CapabilityKey } from "../generated/contracts/domain/CapabilityKey";
export type { CapabilityState } from "../generated/contracts/domain/CapabilityState";
export type { EvidenceTier } from "../generated/contracts/domain/EvidenceTier";
export type { HostFacts } from "../generated/contracts/domain/HostFacts";
export type { HostOperation } from "../generated/contracts/domain/HostOperation";
export type { HostOperationEndpoint } from "../generated/contracts/domain/HostOperationEndpoint";
export type { HostOperationFailure } from "../generated/contracts/domain/HostOperationFailure";
export type { HostOperationFailureCode } from "../generated/contracts/domain/HostOperationFailureCode";
export type { HostOperationKind } from "../generated/contracts/domain/HostOperationKind";
export type { HostOperationLastAttempt } from "../generated/contracts/domain/HostOperationLastAttempt";
export type { HostOperationObservedState } from "../generated/contracts/domain/HostOperationObservedState";
export type { HostOperationResolution } from "../generated/contracts/domain/HostOperationResolution";
export type { HostOperationsEventType } from "../generated/contracts/domain/HostOperationsEventType";
export type { HostOperationsSnapshot } from "../generated/contracts/domain/HostOperationsSnapshot";
export type { HostOperationState } from "../generated/contracts/domain/HostOperationState";
export type { InconclusiveReason } from "../generated/contracts/domain/InconclusiveReason";
export type { OperationalState } from "../generated/contracts/domain/OperationalState";
export type { PrinterCapabilities } from "../generated/contracts/domain/PrinterCapabilities";
export type { PrinterStatus } from "../generated/contracts/domain/PrinterStatus";
export type { PriorState } from "../generated/contracts/domain/PriorState";
export type { StartEvidenceSource } from "../generated/contracts/domain/StartEvidenceSource";
export type { TelemetryFreshness } from "../generated/contracts/domain/TelemetryFreshness";
export type { UnsupportedReason } from "../generated/contracts/domain/UnsupportedReason";

/** The exact envelope emitted on the shared `farm3d-event-v1` channel for
 *  this stream (spec "Events"). */
export type HostOperationsEvent = EventEnvelope<HostOperationsEventType, HostOperation>;

/** D3: the states a Host Operation never leaves. */
export function isTerminalHostOperationState(state: HostOperationState): boolean {
  return state === "succeeded" || state === "failed" || state === "abandoned";
}

/** D17-style shared-channel filter (mirrors `slicing/types.ts`'
 *  `isSlicingEvent`): `farm3d-event-v1` also carries the Library, Printer
 *  status, inventory, and slicing streams. This store keeps only
 *  `hostOperations.*` types, so a foreign `streamId`/`sequence` never
 *  reaches its reconciliation. */
export function isHostOperationsEvent(value: unknown): value is HostOperationsEvent {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as { contractVersion?: unknown; type?: unknown };
  return candidate.contractVersion === 1
    && typeof candidate.type === "string"
    && candidate.type.startsWith("hostOperations.");
}
