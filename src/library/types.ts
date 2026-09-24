/** The Library's frontend types: re-exports of the generated contracts
 *  (the source of truth), plus the shared-channel filter. */
import type { EventEnvelope } from "../generated/contracts/event/EventEnvelope";
import type { ImportModelsData } from "../generated/contracts/command/ImportModelsData";
import type { LibraryEventPayload } from "../generated/contracts/domain/LibraryEventPayload";
import type { LibraryEventType } from "../generated/contracts/domain/LibraryEventType";

export type { DeleteProjectData } from "../generated/contracts/command/DeleteProjectData";
export type { DuplicateAction } from "../generated/contracts/command/DuplicateAction";
export type { DuplicateMatch } from "../generated/contracts/command/DuplicateMatch";
export type { ImportCandidate } from "../generated/contracts/command/ImportCandidate";
export type { ImportInspection } from "../generated/contracts/command/ImportInspection";
export type { ImportItemRequest } from "../generated/contracts/command/ImportItemRequest";
export type { ImportItemResult } from "../generated/contracts/command/ImportItemResult";
export type { ImportModelsData } from "../generated/contracts/command/ImportModelsData";
export type { ImportOutcome } from "../generated/contracts/command/ImportOutcome";
export type { ImportProgress } from "../generated/contracts/command/ImportProgress";
export type { ImportSelectionSummary } from "../generated/contracts/command/ImportSelectionSummary";
export type { ImportWarning } from "../generated/contracts/command/ImportWarning";
export type { LibraryContentInfo } from "../generated/contracts/command/LibraryContentInfo";
export type { SelectionPurpose } from "../generated/contracts/command/SelectionPurpose";
export type { Inspection } from "../generated/contracts/domain/Inspection";
export type { InspectionSummary } from "../generated/contracts/domain/InspectionSummary";
export type { LibrarySnapshot } from "../generated/contracts/domain/LibrarySnapshot";
export type { ModelFormat } from "../generated/contracts/domain/ModelFormat";
export type { ModelRecord } from "../generated/contracts/domain/ModelRecord";
export type { ModelSourceRevisionRecord } from "../generated/contracts/domain/ModelSourceRevisionRecord";
export type { ModelSourceRevisionSummary } from "../generated/contracts/domain/ModelSourceRevisionSummary";
export type { ProjectRecord } from "../generated/contracts/domain/ProjectRecord";
export type { RevisionOrigin } from "../generated/contracts/domain/RevisionOrigin";
export type { RevisionThumbnail } from "../generated/contracts/domain/RevisionThumbnail";
export type { SourceState } from "../generated/contracts/domain/SourceState";
export type { StorageMode } from "../generated/contracts/domain/StorageMode";
export type { LibraryEventPayload, LibraryEventType };

/** `import_models`' result data. The generated `ImportModelsResult` is the
 *  command's `CommandSuccess` wrapper; the plan's interfaces use this name
 *  for the unwrapped data the store returns. */
export type ImportModelsResult = ImportModelsData;

/** D19's saved views, in sidebar order. */
export type SavedViewId = "all" | "unfiled" | "recent" | "attention" | "gcode";

export type LibraryEvent = EventEnvelope<LibraryEventType, LibraryEventPayload>;

/** D17: `farm3d-event-v1` also carries the Printer status and inventory
 *  streams. The Library keeps only `library.*` types, so a foreign
 *  `streamId`/`sequence` never reaches its reconciliation. */
export function isLibraryEvent(value: unknown): value is LibraryEvent {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as { contractVersion?: unknown; type?: unknown };
  return candidate.contractVersion === 1
    && typeof candidate.type === "string"
    && candidate.type.startsWith("library.");
}
