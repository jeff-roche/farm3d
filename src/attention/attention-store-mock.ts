/** A stand-in for `attention-store` in screen/App tests. Test-only: nothing
 *  outside a test imports this module (mirrors
 *  `src/queue/queue-store-mock.ts`). Use it as
 *
 *    vi.mock("../attention/attention-store", async () =>
 *      (await import("../attention/attention-store-mock")).attentionStoreMock);
 *
 *  The read side is a real Solid store, so components react when a test
 *  changes it with `setAttentionStoreState` (or `loadWebAttentionFixture`);
 *  the actions are spies. */
import { createSignal } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { vi } from "vitest";
import { buildWebAttentionFixture } from "./web-fixtures";
import type {
  AttentionChange,
  AttentionCursor,
  AttentionEvent,
  AttentionSeverity,
  CameraHealth,
  Incident,
} from "./types";

interface MockAttentionState {
  events: AttentionEvent[];
  incidents: Incident[];
  cameraHealth: CameraHealth[];
  resolvedCursor: AttentionCursor | null;
  status: "idle" | "loading" | "ready" | "error";
  syncState: "syncing" | "current" | "uncertain";
}

const initialState = (): MockAttentionState => ({
  events: [],
  incidents: [],
  cameraHealth: [],
  resolvedCursor: null,
  status: "ready",
  syncState: "current",
});

const [state, setState] = createStore<MockAttentionState>(initialState());

const SEVERITY_RANK: Record<AttentionSeverity, number> = { fatal: 0, warning: 1, info: 2 };

// A real Solid signal, like the real store's -- `requestAttentionCenterOpen`
// increments it, so a component's `on(attentionCenterRequest, ...)` effect
// actually fires in a test (mirrors the real store's own pairing, not just
// a static stand-in).
const [attentionCenterRequestId, setAttentionCenterRequestId] = createSignal(0);

function emptyChange(): AttentionChange {
  return { events: [], incidents: [] };
}

export const attentionStoreMock = {
  attention: {
    open: (): AttentionEvent[] => state.events.filter((e) => e.resolvedAt === null)
      .sort((a, b) => SEVERITY_RANK[a.severity] - SEVERITY_RANK[b.severity] || b.firstObservedAt.localeCompare(a.firstObservedAt) || a.id.localeCompare(b.id)),
    resolved: (): AttentionEvent[] => state.events.filter((e) => e.resolvedAt !== null)
      .sort((a, b) => (b.resolvedAt ?? "").localeCompare(a.resolvedAt ?? "") || b.id.localeCompare(a.id)),
    event: (id: string) => state.events.find((e) => e.id === id),
    incident: (id: string) => state.incidents.find((i) => i.id === id),
    openIncidents: (): Incident[] => state.incidents.filter((i) => i.state === "open"),
    cameraHealth: (printerId: string) => state.cameraHealth.find((h) => h.printerId === printerId),
    allCameraHealth: (): CameraHealth[] => state.cameraHealth,
    actionableCount: () => attentionStoreMock.attention.open().filter((e) => e.requiresAction).length,
    unreadCount: () => attentionStoreMock.attention.open().filter((e) => e.readAt === null).length,
    highestOpenSeverity: (): AttentionSeverity | null => {
      const open = attentionStoreMock.attention.open();
      return open.length > 0 ? open[0].severity : null;
    },
    eventsForPrinter: (printerId: string) => attentionStoreMock.attention.open().filter((e) => e.printerId === printerId),
    resolvedCursor: () => state.resolvedCursor,
    status: () => state.status,
    syncState: () => state.syncState,
  },
  startAttention: vi.fn(async (): Promise<() => void> => () => {}),
  refreshAttention: vi.fn(),
  markAttentionRead: vi.fn(async (_eventIds: string[]): Promise<AttentionChange> => emptyChange()),
  acknowledgeAttentionEvent: vi.fn(async (_eventId: string): Promise<AttentionChange> => emptyChange()),
  resolveAttentionEvent: vi.fn(async (_eventId: string): Promise<AttentionChange> => emptyChange()),
  loadMoreResolved: vi.fn(async (_limit?: number): Promise<void> => {}),
  onAttentionIncidentChanged: vi.fn((_listener: (incident: Incident) => void) => () => {}),
  onAttentionSnapshotChanged: vi.fn((_listener: (snapshot: unknown) => void) => () => {}),
  onAttentionPrinterRemoved: vi.fn((_listener: (printerId: string) => void) => () => {}),
  attentionCenterRequest: attentionCenterRequestId,
  requestAttentionCenterOpen: vi.fn(() => setAttentionCenterRequestId((current) => current + 1)),
};

/** Replaces (a slice of) the mock's state, as events settling into the real
 *  store would. */
export function setAttentionStoreState(patch: Partial<Omit<MockAttentionState, "status" | "syncState">>): void {
  if (patch.events) setState("events", reconcile(patch.events));
  if (patch.incidents) setState("incidents", reconcile(patch.incidents));
  if (patch.cameraHealth) setState("cameraHealth", reconcile(patch.cameraHealth));
  if (patch.resolvedCursor !== undefined) setState("resolvedCursor", patch.resolvedCursor);
}

export function setAttentionStoreStatus(status: MockAttentionState["status"], syncState: MockAttentionState["syncState"] = "current"): void {
  setState({ status, syncState });
}

/** Loads `web-fixtures.ts`'s Attention world into the mock. */
export function loadWebAttentionFixture(): ReturnType<typeof buildWebAttentionFixture> {
  const fixture = buildWebAttentionFixture();
  setState({
    events: [...fixture.backfill.open, ...fixture.backfill.resolved],
    incidents: fixture.backfill.openIncidents,
    cameraHealth: fixture.backfill.cameraHealth,
    resolvedCursor: fixture.backfill.resolvedCursor,
  });
  return fixture;
}

export function resetAttentionStoreMock(): void {
  setState(initialState());
  setAttentionCenterRequestId(0);
  for (const action of Object.values(attentionStoreMock)) {
    if (typeof action === "function" && "mockClear" in action) action.mockClear();
  }
}
