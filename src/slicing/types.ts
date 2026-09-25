/** The slicing frontend types: re-exports of the generated contracts (the
 *  source of truth), plus the shared-channel filter. */
import type { SlicingEvent } from "../generated/contracts/domain/SlicingEvent";
import type { SliceOperationState } from "../generated/contracts/domain/SliceOperationState";
import type { SliceRevisionRecord } from "../generated/contracts/domain/SliceRevisionRecord";
import type { SliceRevisionSummary } from "../generated/contracts/domain/SliceRevisionSummary";

export type { ConfirmedFactRequest } from "../generated/contracts/command/ConfirmedFactRequest";
export type { CreateExternalSliceRevisionFacts } from "../generated/contracts/command/CreateExternalSliceRevisionFacts";
export type { PresetSourceKind } from "../generated/contracts/command/PresetSourceKind";
export type { ReloadPreparationData } from "../generated/contracts/command/ReloadPreparationData";
export type { SliceOperationLog } from "../generated/contracts/command/SliceOperationLog";
export type { StartSliceData } from "../generated/contracts/command/StartSliceData";
export type { BedShape } from "../generated/contracts/domain/BedShape";
export type { BoundsMm } from "../generated/contracts/domain/BoundsMm";
export type { BrimType } from "../generated/contracts/domain/BrimType";
export type { CatalogRef } from "../generated/contracts/domain/CatalogRef";
export type { ClaimedEstimates } from "../generated/contracts/domain/ClaimedEstimates";
export type { EngineCandidate } from "../generated/contracts/domain/EngineCandidate";
export type { EngineCandidateResult } from "../generated/contracts/domain/EngineCandidateResult";
export type { EngineSource } from "../generated/contracts/domain/EngineSource";
export type { EngineState } from "../generated/contracts/domain/EngineState";
export type { Fact } from "../generated/contracts/domain/Fact";
export type { FactProvenance } from "../generated/contracts/domain/FactProvenance";
export type { FilamentPresetOption } from "../generated/contracts/domain/FilamentPresetOption";
export type { GeometryBuildItem } from "../generated/contracts/domain/GeometryBuildItem";
export type { GeometryObject } from "../generated/contracts/domain/GeometryObject";
export type { InfillPattern } from "../generated/contracts/domain/InfillPattern";
export type { InstanceDoc } from "../generated/contracts/domain/InstanceDoc";
export type { InstanceTransform } from "../generated/contracts/domain/InstanceTransform";
export type { LayFlatFace } from "../generated/contracts/domain/LayFlatFace";
export type { MaterialFamily } from "../generated/contracts/domain/MaterialFamily";
export type { PointMm } from "../generated/contracts/domain/PointMm";
export type { PlateDoc } from "../generated/contracts/domain/PlateDoc";
export type { PreparationDocument } from "../generated/contracts/domain/PreparationDocument";
export type { PreparationRecord } from "../generated/contracts/domain/PreparationRecord";
export type { PresetSourceOrigin } from "../generated/contracts/domain/PresetSourceOrigin";
export type { PresetSourceState } from "../generated/contracts/domain/PresetSourceState";
export type { ProcessPresetOption } from "../generated/contracts/domain/ProcessPresetOption";
export type { ProfileSnapshot } from "../generated/contracts/domain/ProfileSnapshot";
export type { RevisionGeometry } from "../generated/contracts/domain/RevisionGeometry";
export type { RuntimeChannel } from "../generated/contracts/domain/RuntimeChannel";
export type { SliceControls } from "../generated/contracts/domain/SliceControls";
export type { SliceEstimates } from "../generated/contracts/domain/SliceEstimates";
export type { SliceFacts } from "../generated/contracts/domain/SliceFacts";
export type { SliceFailure } from "../generated/contracts/domain/SliceFailure";
export type { SliceFailureCode } from "../generated/contracts/domain/SliceFailureCode";
export type { SliceOperationRecord } from "../generated/contracts/domain/SliceOperationRecord";
export type { SliceOptionDefaults } from "../generated/contracts/domain/SliceOptionDefaults";
export type { SliceOptions } from "../generated/contracts/domain/SliceOptions";
export type { SlicePlateRef } from "../generated/contracts/domain/SlicePlateRef";
export type { SliceProgress } from "../generated/contracts/domain/SliceProgress";
export type { SliceRevisionBlob } from "../generated/contracts/domain/SliceRevisionBlob";
export type { SliceRevisionBlobRole } from "../generated/contracts/domain/SliceRevisionBlobRole";
export type { SliceRevisionKind } from "../generated/contracts/domain/SliceRevisionKind";
export type { SliceRevisionTarget } from "../generated/contracts/domain/SliceRevisionTarget";
export type { SliceRuntimeInfo } from "../generated/contracts/domain/SliceRuntimeInfo";
export type { SliceTarget } from "../generated/contracts/domain/SliceTarget";
export type { SlicerRuntimeStatus } from "../generated/contracts/domain/SlicerRuntimeStatus";
export type { SlicingEventPayload } from "../generated/contracts/domain/SlicingEventPayload";
export type { SlicingEventType } from "../generated/contracts/domain/SlicingEventType";
export type { SlicingSnapshot } from "../generated/contracts/domain/SlicingSnapshot";
export type { SupportMode } from "../generated/contracts/domain/SupportMode";
export type { SliceOperationState, SliceRevisionRecord, SliceRevisionSummary, SlicingEvent };

/** D10: the states an operation never leaves. */
export function isTerminalOperationState(state: SliceOperationState): boolean {
  return state === "succeeded" || state === "failed" || state === "cancelled" || state === "interrupted";
}

/** A record without the fields only the full record has: what lists and
 *  `slicing.revision.created` carry. */
export function revisionSummaryOf(record: SliceRevisionRecord): SliceRevisionSummary {
  const { target: _target, claimedEstimates: _claimed, producer: _producer, blobs: _blobs, ...summary } = record;
  return summary;
}

/** D17: `farm3d-event-v1` also carries the Library, Printer status and
 *  inventory streams. Slicing keeps only `slicing.*` types, so a foreign
 *  `streamId`/`sequence` never reaches its reconciliation. */
export function isSlicingEvent(value: unknown): value is SlicingEvent {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as { contractVersion?: unknown; type?: unknown };
  return candidate.contractVersion === 1
    && typeof candidate.type === "string"
    && candidate.type.startsWith("slicing.");
}
