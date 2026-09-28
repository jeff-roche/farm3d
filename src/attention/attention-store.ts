import { createSignal } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { command, desktopAvailable, needsDesktopError, retryOnTransportFailure } from "../ipc/client";
import { createSequencedStream } from "../ipc/sequenced-stream";
import { isAttentionStreamEvent, printerRemovedId } from "./types";
import type {
  AttentionBackfill,
  AttentionChange,
  AttentionCursor,
  AttentionEvent,
  AttentionSeverity,
  AttentionStreamEvent,
  CameraHealth,
  CameraSnapshot,
  Incident,
} from "./types";

/** The only owner of Attention Events, Incidents (the open list; full
 *  detail is `src/incidents/incident-store.ts`'s job), and camera health:
 *  listen-before-backfill over the `attention` stream, copying
 *  `queue/queue-store.ts`'s pattern. Rust is the only source of a
 *  Condition, severity, resolution, or allowed action (global constraint
 *  4) -- this store presents what Rust returns and never derives any of
 *  it. */

export type AttentionStatus = "idle" | "loading" | "ready" | "error";
export type AttentionSyncState = "syncing" | "current" | "uncertain";

interface AttentionState {
  events: AttentionEvent[];
  incidents: Incident[];
  cameraHealth: CameraHealth[];
  resolvedCursor: AttentionCursor | null;
  status: AttentionStatus;
  syncState: AttentionSyncState;
}

const [state, setState] = createStore<AttentionState>({
  events: [],
  incidents: [],
  cameraHealth: [],
  resolvedCursor: null,
  status: "idle",
  syncState: "current",
});

/** decision 8 has no ordering table for severity, so this is purely a
 *  presentation choice, shared by `open()`'s sort and `highestOpenSeverity()`. */
const SEVERITY_RANK: Record<AttentionSeverity, number> = { fatal: 0, warning: 1, info: 2 };

/** `AttentionBackfill.open`'s order: severity, then `firstObservedAt`
 *  descending, then id. */
function compareOpen(a: AttentionEvent, b: AttentionEvent): number {
  return SEVERITY_RANK[a.severity] - SEVERITY_RANK[b.severity]
    || b.firstObservedAt.localeCompare(a.firstObservedAt)
    || a.id.localeCompare(b.id);
}

/** `AttentionBackfill.resolved`'s order: `resolvedAt` descending, then id
 *  descending. */
function compareResolved(a: AttentionEvent, b: AttentionEvent): number {
  return (b.resolvedAt ?? "").localeCompare(a.resolvedAt ?? "") || b.id.localeCompare(a.id);
}

export const attention = {
  /** Every open Event, in `AttentionBackfill.open`'s order. */
  open: (): AttentionEvent[] => state.events.filter((event) => event.resolvedAt === null).sort(compareOpen),
  /** Every resolved Event this store has loaded, newest first. Paged
   *  further by `loadMoreResolved`/`resolvedCursor`. */
  resolved: (): AttentionEvent[] => state.events.filter((event) => event.resolvedAt !== null).sort(compareResolved),
  event: (id: string): AttentionEvent | undefined => state.events.find((event) => event.id === id),
  incident: (id: string): Incident | undefined => state.incidents.find((record) => record.id === id),
  openIncidents: (): Incident[] => state.incidents.filter((record) => record.state === "open"),
  /** One per Printer with a camera source (`AttentionBackfill.cameraHealth`). */
  cameraHealth: (printerId: string): CameraHealth | undefined =>
    state.cameraHealth.find((health) => health.printerId === printerId),
  allCameraHealth: (): CameraHealth[] => state.cameraHealth,
  /** Open Events that `requiresAction`, acknowledged or not (spec
   *  "Frontend architecture"). */
  actionableCount: (): number => attention.open().filter((event) => event.requiresAction).length,
  /** Open, unread Events (the "Unread" filter's own count). */
  unreadCount: (): number => attention.open().filter((event) => event.readAt === null).length,
  highestOpenSeverity: (): AttentionSeverity | null => {
    const open = attention.open();
    return open.length > 0 ? open[0].severity : null;
  },
  /** Open Events tied to one Printer (its `printerId`, not just a
   *  `printer`-kind source -- a Job or Requirement Event on that Printer
   *  counts too), for the Printer detail dock's Status tab. */
  eventsForPrinter: (printerId: string): AttentionEvent[] =>
    attention.open().filter((event) => event.printerId === printerId),
  resolvedCursor: (): AttentionCursor | null => state.resolvedCursor,
  status: (): AttentionStatus => state.status,
  syncState: (): AttentionSyncState => state.syncState,
};

/** Same stale-revision rule as `queue-store`'s `upsertEntry`: a lower
 *  revision than the held row's is stale and ignored. Returns whether the
 *  store actually changed, so live-stream callers can notify only then. */
function upsertEvent(record: AttentionEvent): boolean {
  const index = state.events.findIndex((event) => event.id === record.id);
  if (index >= 0 && record.revision < state.events[index].revision) return false;
  if (index >= 0) setState("events", index, reconcile(record));
  else setState("events", (list) => [...list, record]);
  return true;
}

function upsertIncident(record: Incident): boolean {
  const index = state.incidents.findIndex((incident) => incident.id === record.id);
  if (index >= 0 && record.revision < state.incidents[index].revision) return false;
  if (index >= 0) setState("incidents", index, reconcile(record));
  else setState("incidents", (list) => [...list, record]);
  return true;
}

/** `CameraHealth` carries no revision; the latest reported row wins. */
function upsertCameraHealth(record: CameraHealth): void {
  const index = state.cameraHealth.findIndex((health) => health.printerId === record.printerId);
  if (index >= 0) setState("cameraHealth", index, reconcile(record));
  else setState("cameraHealth", (list) => [...list, record]);
}

/** A deleted Printer publishes no `camera.health.changed` removal (spec
 *  Task 12 "Controller carry"): the camera health row must be dropped
 *  explicitly, from `printer.status.removed` and from a fresh backfill
 *  (which never reports a removed Printer's health in the first place). */
function dropCameraHealth(printerId: string): void {
  setState("cameraHealth", (list) => list.filter((health) => health.printerId !== printerId));
}

// --- Cross-store notifications ---------------------------------------------
// `src/incidents/incident-store.ts` and `src/cameras/camera-store.ts` need to
// react to two payloads this store's single stream connection already sees
// (an Incident's detail refetch, and a snapshot/Printer-removal cache
// invalidation) without opening a second, redundant listener on the shared
// channel.

type IncidentListener = (incident: Incident) => void;
type SnapshotListener = (snapshot: CameraSnapshot) => void;
type PrinterRemovedListener = (printerId: string) => void;

const incidentListeners = new Set<IncidentListener>();
const snapshotListeners = new Set<SnapshotListener>();
const printerRemovedListeners = new Set<PrinterRemovedListener>();

/** Fires only when a *live* Incident change actually advanced the store's
 *  copy (spec "Events": "refetched ... when one with a higher revision
 *  arrives"), never for the initial backfill. */
export function onAttentionIncidentChanged(listener: IncidentListener): () => void {
  incidentListeners.add(listener);
  return () => incidentListeners.delete(listener);
}

export function onAttentionSnapshotChanged(listener: SnapshotListener): () => void {
  snapshotListeners.add(listener);
  return () => snapshotListeners.delete(listener);
}

export function onAttentionPrinterRemoved(listener: PrinterRemovedListener): () => void {
  printerRemovedListeners.add(listener);
  return () => printerRemovedListeners.delete(listener);
}

// --- The Attention center's open request ------------------------------------
// `farm3d-navigate-v1`'s `openAttentionCenter` (App.tsx) has nothing to open
// yet (the `AttentionTrigger`/`AttentionCenter` popover is Task 13); this
// signal is the seam it will subscribe to.

const [attentionCenterRequestId, setAttentionCenterRequestId] = createSignal(0);
export const attentionCenterRequest = attentionCenterRequestId;
export function requestAttentionCenterOpen(): void {
  setAttentionCenterRequestId((current) => current + 1);
}

function applyEvent(event: AttentionStreamEvent): void {
  const { payload } = event;
  switch (payload.type) {
    case "eventChanged":
      upsertEvent(payload.event);
      break;
    case "incidentChanged":
      if (upsertIncident(payload.incident)) {
        for (const listener of incidentListeners) listener(payload.incident);
      }
      break;
    case "snapshotChanged":
      for (const listener of snapshotListeners) listener(payload.snapshot);
      break;
    case "cameraHealthChanged":
      upsertCameraHealth(payload.health);
      break;
  }
}

function applySnapshot(backfill: AttentionBackfill): void {
  setState({
    events: [...backfill.open, ...backfill.resolved],
    incidents: backfill.openIncidents,
    cameraHealth: backfill.cameraHealth,
    resolvedCursor: backfill.resolvedCursor,
    status: "ready",
  });
}

type AttentionStream = ReturnType<typeof createSequencedStream<AttentionStreamEvent, AttentionBackfill>>;

let activeStream: AttentionStream | undefined;
let activeUnlisten: (() => void) | undefined;

function disposeListener(): void {
  activeStream?.dispose();
  activeStream = undefined;
  activeUnlisten?.();
  activeUnlisten = undefined;
}

/** Subscribes to the `attention` stream, then backfills through
 *  `list_attention`, so nothing between the two is missed. Idempotent:
 *  calling it again disposes the previous listener first. In web mode it
 *  loads `web-fixtures.ts`'s deterministic Attention world instead (mirrors
 *  `queue-store.ts`'s `startQueue`). */
export async function startAttention(): Promise<() => void> {
  disposeListener();
  if (!desktopAvailable()) {
    const { buildWebAttentionFixture } = await import("./web-fixtures");
    const fixture = buildWebAttentionFixture();
    applySnapshot(fixture.backfill);
    setState("syncState", "current");
    return () => {};
  }

  setState({ syncState: "syncing", ...(state.status === "ready" ? {} : { status: "loading" }) });
  const stream: AttentionStream = createSequencedStream<AttentionStreamEvent, AttentionBackfill>({
    backfill: () => command("list_attention", {}),
    applySnapshot,
    applyEvent,
    onSyncState: (syncState) => setState("syncState", syncState),
    onBackfillError: () => {
      if (state.status !== "ready") setState("status", "error");
    },
  });
  activeStream = stream;
  const dispose = () => {
    if (activeStream !== stream) return;
    disposeListener();
  };

  try {
    const { listen } = await import("@tauri-apps/api/event");
    const unlisten = await listen<unknown>("farm3d-event-v1", (event) => {
      const candidate = event.payload;
      if (isAttentionStreamEvent(candidate)) {
        stream.receive(candidate);
        return;
      }
      const removedPrinterId = printerRemovedId(candidate);
      if (removedPrinterId !== undefined) {
        dropCameraHealth(removedPrinterId);
        for (const listener of printerRemovedListeners) listener(removedPrinterId);
      }
    });
    if (activeStream !== stream) {
      unlisten();
      return () => {};
    }
    activeUnlisten = unlisten;
  } catch {
    dispose();
    setState({ status: "error", syncState: "uncertain" });
    return () => {};
  }

  await stream.start();
  return dispose;
}

/** Backfill now rather than wait for the stream's backoff. Web mode has no
 *  stream to refresh. */
export function refreshAttention(): void {
  activeStream?.resync();
}

function adoptChange(change: AttentionChange): AttentionChange {
  for (const event of change.events) upsertEvent(event);
  for (const incident of change.incidents) upsertIncident(incident);
  return change;
}

/** Runs one user-initiated write, the same shape as `queue-store.ts`'s
 *  `write`: a fresh `operationId`, retried once on transport failure,
 *  settled from the returned `AttentionChange` (global constraint 4 --
 *  this never decides the change itself). */
async function write(action: string, send: (operationId: string) => Promise<AttentionChange>): Promise<AttentionChange> {
  if (!desktopAvailable()) throw needsDesktopError(action);
  const operationId = crypto.randomUUID();
  return adoptChange(await retryOnTransportFailure(() => send(operationId)));
}

export function markAttentionRead(eventIds: string[]): Promise<AttentionChange> {
  return write("Marking Attention Events read", (operationId) => command("mark_attention_read", { operationId, eventIds }));
}

export function acknowledgeAttentionEvent(eventId: string): Promise<AttentionChange> {
  return write("Acknowledging an Attention Event", (operationId) => command("acknowledge_attention_event", { operationId, eventId }));
}

/** `resolve_attention_event`: manual Events only (`ATTENTION_NOT_MANUAL`
 *  otherwise -- Rust's own answer, not something this store precomputes). */
export function resolveAttentionEvent(eventId: string): Promise<AttentionChange> {
  return write("Resolving an Attention Event", (operationId) => command("resolve_attention_event", { operationId, eventId }));
}

/** One more page of resolved Events, appended to what's already loaded. A
 *  `null` cursor means there's nothing older to fetch. Web mode's fixture
 *  has no further pages. */
export async function loadMoreResolved(limit?: number): Promise<void> {
  if (!desktopAvailable() || state.resolvedCursor === null) return;
  const page = await command("list_attention", { resolvedBefore: state.resolvedCursor, ...(limit !== undefined ? { limit } : {}) });
  for (const event of page.resolved) upsertEvent(event);
  setState("resolvedCursor", page.resolvedCursor);
}
