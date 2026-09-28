/** The Attention/Incident/Camera-health frontend types: re-exports of the
 *  generated contracts (the source of truth), plus the shared-channel
 *  filter every store needs. Mirrors `src/queue/types.ts`. */
import type { EventEnvelope } from "../generated/contracts/event/EventEnvelope";
import type { AttentionStreamEventType } from "../generated/contracts/domain/AttentionStreamEventType";
import type { AttentionStreamPayload } from "../generated/contracts/domain/AttentionStreamPayload";

export type { AttentionAction } from "../generated/contracts/domain/AttentionAction";
export type { AttentionBackfill } from "../generated/contracts/domain/AttentionBackfill";
export type { AttentionChange } from "../generated/contracts/domain/AttentionChange";
export type { AttentionCursor } from "../generated/contracts/domain/AttentionCursor";
export type { AttentionDetail } from "../generated/contracts/domain/AttentionDetail";
export type { AttentionEvent } from "../generated/contracts/domain/AttentionEvent";
export type { AttentionOrigin } from "../generated/contracts/domain/AttentionOrigin";
export type { AttentionResolution } from "../generated/contracts/domain/AttentionResolution";
export type { AttentionSeverity } from "../generated/contracts/domain/AttentionSeverity";
export type { AttentionSource } from "../generated/contracts/domain/AttentionSource";
export type { AttentionSourceKind } from "../generated/contracts/domain/AttentionSourceKind";
export type { AttentionStreamEventType } from "../generated/contracts/domain/AttentionStreamEventType";
export type { AttentionStreamPayload } from "../generated/contracts/domain/AttentionStreamPayload";
export type { AttentionSubject } from "../generated/contracts/domain/AttentionSubject";
export type { CameraErrorKind } from "../generated/contracts/domain/CameraErrorKind";
export type { CameraHealth } from "../generated/contracts/domain/CameraHealth";
export type { CameraHealthState } from "../generated/contracts/domain/CameraHealthState";
export type { CameraSource } from "../generated/contracts/domain/CameraSource";
export type { CameraSourceInput } from "../generated/contracts/domain/CameraSourceInput";
export type { CameraSourceKind } from "../generated/contracts/domain/CameraSourceKind";
export type { CameraSnapshot } from "../generated/contracts/domain/CameraSnapshot";
export type { HostWebcam } from "../generated/contracts/domain/HostWebcam";
export type { MediaUsage } from "../generated/contracts/domain/MediaUsage";
export type { SnapshotPage } from "../generated/contracts/domain/SnapshotPage";
export type { PrinterCamera } from "../generated/contracts/domain/PrinterCamera";
export type { PrinterCameraSummary } from "../generated/contracts/domain/PrinterCameraSummary";
export type { PrinterCameraCleared } from "../generated/contracts/domain/PrinterCameraCleared";
export type { AlertDefaults } from "../generated/contracts/domain/AlertDefaults";
export type { PrinterAlertDefaults } from "../generated/contracts/domain/PrinterAlertDefaults";
export type { NotificationMode } from "../generated/contracts/domain/NotificationMode";
export type { NotificationClassSettings } from "../generated/contracts/domain/NotificationClassSettings";
export type { NotifierStatus } from "../generated/contracts/domain/NotifierStatus";
export type { NotifierUnavailableReason } from "../generated/contracts/domain/NotifierUnavailableReason";
export type { TestNotificationSent } from "../generated/contracts/domain/TestNotificationSent";
export type { CameraTemplate } from "../generated/contracts/command/CameraTemplate";
export type { ConditionKind } from "../generated/contracts/domain/ConditionKind";
export type { EvidenceOutcome } from "../generated/contracts/domain/EvidenceOutcome";
export type { EvidenceSkipReason } from "../generated/contracts/domain/EvidenceSkipReason";
export type { Incident } from "../generated/contracts/domain/Incident";
export type { IncidentDetail } from "../generated/contracts/domain/IncidentDetail";
export type { IncidentEntry } from "../generated/contracts/domain/IncidentEntry";
export type { IncidentEntryDetail } from "../generated/contracts/domain/IncidentEntryDetail";
export type { IncidentEntryKind } from "../generated/contracts/domain/IncidentEntryKind";
export type { IncidentKind } from "../generated/contracts/domain/IncidentKind";
export type { IncidentPage } from "../generated/contracts/domain/IncidentPage";
export type { IncidentState } from "../generated/contracts/domain/IncidentState";
export type { IncidentTimelineItem } from "../generated/contracts/domain/IncidentTimelineItem";
export type { NotificationClass } from "../generated/contracts/domain/NotificationClass";
export type { PruneReason } from "../generated/contracts/domain/PruneReason";
export type { ResolutionMode } from "../generated/contracts/domain/ResolutionMode";
export type { SnapshotTrigger } from "../generated/contracts/domain/SnapshotTrigger";

/** The exact envelope emitted on the shared `farm3d-event-v1` channel for
 *  this stream (spec "Events"): `attention.*` and `camera.health.changed`
 *  share one sequence. */
export type AttentionStreamEvent = EventEnvelope<AttentionStreamEventType, AttentionStreamPayload>;

/** Shared-channel filter (mirrors `queue/types.ts`'s `isQueueEvent`):
 *  `farm3d-event-v1` also carries the Library, Printer status, inventory,
 *  slicing, host-ops, and Queue streams. `camera.health.changed` doesn't
 *  share the `attention.` prefix but is still this stream's own event
 *  (spec "Events" table). */
export function isAttentionStreamEvent(value: unknown): value is AttentionStreamEvent {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as { contractVersion?: unknown; type?: unknown };
  return candidate.contractVersion === 1
    && typeof candidate.type === "string"
    && (candidate.type.startsWith("attention.") || candidate.type === "camera.health.changed");
}

/** `printer.status.removed`'s shape, as it arrives on the same shared
 *  channel (from `printers/types.ts`'s own stream) -- the attention and
 *  camera stores watch for it to drop a deleted Printer's camera health
 *  and cached snapshots, since no `attention`-stream event announces that
 *  removal (spec Task 12 "Controller carry"). */
export function printerRemovedId(value: unknown): string | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  const candidate = value as { contractVersion?: unknown; type?: unknown; subject?: { id?: unknown } };
  if (candidate.contractVersion !== 1 || candidate.type !== "printer.status.removed") return undefined;
  return typeof candidate.subject?.id === "string" ? candidate.subject.id : undefined;
}
