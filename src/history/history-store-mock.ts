/** A stand-in for `history-store` in screen tests. Test-only. Use as
 *
 *    vi.mock("../history/history-store", async () =>
 *      (await import("../history/history-store-mock")).historyStoreMock);
 *
 *  The read side is a real Solid store (`setHistoryStoreState`); the actions
 *  are spies. */
import { createStore, reconcile } from "solid-js/store";
import { vi } from "vitest";
import { webHistoryRows, webJobTimeline } from "./web-fixtures";
import type { JobHistoryRow, JobTimeline } from "./types";
import type { HistoryFilters } from "./history-store";
import type { CommandError } from "../generated/contracts/command/CommandError";

interface MockHistoryState {
  filters: HistoryFilters;
  rows: JobHistoryRow[];
  hasMore: boolean;
  status: "idle" | "loading" | "ready" | "error";
  loadingMore: boolean;
  error: CommandError | null;
}

const initialState = (): MockHistoryState => ({
  filters: {}, rows: [], hasMore: false, status: "ready", loadingMore: false, error: null,
});
const [state, setState] = createStore<MockHistoryState>(initialState());

export const historyStoreMock = {
  HISTORY_DEBOUNCE_MS: 250,
  history: {
    filters: () => state.filters,
    rows: () => state.rows,
    hasMore: () => state.hasMore,
    status: () => state.status,
    loadingMore: () => state.loadingMore,
    error: () => state.error,
  },
  setHistoryFilters: vi.fn((patch: Partial<HistoryFilters>) => { setState("filters", patch); }),
  refreshHistory: vi.fn(async (): Promise<void> => {}),
  loadMoreHistory: vi.fn(async (): Promise<void> => {}),
  getJobTimeline: vi.fn(async (jobId: string): Promise<JobTimeline> => {
    const timeline = webJobTimeline(jobId);
    if (!timeline) throw new Error(`no timeline fixture for ${jobId}`);
    return timeline;
  }),
  resetHistoryStore: vi.fn(),
};

export function setHistoryStoreState(patch: Partial<MockHistoryState>): void {
  if (patch.rows) setState("rows", reconcile(patch.rows));
  const { rows: _rows, ...rest } = patch;
  setState(rest);
}

export function loadWebHistoryFixture(): JobHistoryRow[] {
  const rows = webHistoryRows();
  setState({ rows, status: "ready" });
  return rows;
}

export function resetHistoryStoreMock(): void {
  setState(reconcile(initialState()));
  for (const action of Object.values(historyStoreMock)) {
    if (typeof action === "function" && "mockClear" in action) action.mockClear();
  }
}
