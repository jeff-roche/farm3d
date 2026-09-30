/** The Job History query (spec "Frontend architecture"): one paged query.
 *  A filter or text change is debounced 250 ms, resets the cursor, and
 *  replaces the rows; "Load more" appends the next page. Each request
 *  carries a sequence number and a response older than the latest is
 *  dropped. Rust decides every filter's meaning (D11); this only presents. */
import { createStore, reconcile } from "solid-js/store";
import { command, desktopAvailable, isCommandError, retryOnTransportFailure } from "../ipc/client";
import type { CommandError } from "../generated/contracts/command/CommandError";
import type { JobHistoryPage, JobHistoryQuery, JobHistoryRow, JobTimeline } from "./types";
import { clearHistoryKnownIds } from "./known-ids-actions";
import { commandError, notFound } from "../ipc/local-errors";

export const HISTORY_DEBOUNCE_MS = 250;

/** The filters an operator edits; `after` and `limit` belong to the store. */
export type HistoryFilters = Omit<JobHistoryQuery, "after" | "limit">;

interface HistoryState {
  filters: HistoryFilters;
  rows: JobHistoryRow[];
  nextCursor: string | null;
  status: "idle" | "loading" | "ready" | "error";
  loadingMore: boolean;
  error: CommandError | null;
}

const [state, setState] = createStore<HistoryState>({
  filters: {}, rows: [], nextCursor: null, status: "idle", loadingMore: false, error: null,
});

let sequence = 0;
let timer: ReturnType<typeof setTimeout> | undefined;

export const history = {
  filters: (): HistoryFilters => state.filters,
  rows: (): JobHistoryRow[] => state.rows,
  hasMore: (): boolean => state.nextCursor !== null,
  status: () => state.status,
  loadingMore: () => state.loadingMore,
  error: () => state.error,
};

function toQuery(filters: HistoryFilters, after?: string): JobHistoryQuery {
  const query: JobHistoryQuery = {};
  for (const [key, value] of Object.entries(filters)) {
    if (value === undefined || value === "" || (key === "states" && (value as unknown[]).length === 0)) continue;
    (query as Record<string, unknown>)[key] = value;
  }
  const text = filters.text?.trim();
  if (text) query.text = text; else delete query.text;
  if (after) query.after = after;
  return query;
}

async function fetchPage(query: JobHistoryQuery): Promise<JobHistoryPage> {
  if (desktopAvailable()) return retryOnTransportFailure(() => command("list_job_history", { query }));
  return import("./web-fixtures").then(({ webJobHistoryPage }) => webJobHistoryPage(query));
}

/** A valid `CommandError` as is; a transport failure (a plain `Error`, a
 *  string) as one in the backend's shape, so the view never renders a
 *  malformed error. */
function asCommandError(error: unknown): CommandError {
  return isCommandError(error) ? error : commandError("INTERNAL", "The history couldn't be read.");
}

async function runFirstPage(): Promise<void> {
  const mine = ++sequence;
  setState({ status: "loading", loadingMore: false, error: null });
  try {
    const page = await fetchPage(toQuery(state.filters));
    if (mine !== sequence) return;
    setState("rows", reconcile(page.rows, { key: "jobId" }));
    setState({ nextCursor: page.nextCursor, status: "ready" });
  } catch (error) {
    if (mine !== sequence) return;
    setState({ status: "error", error: asCommandError(error) });
  }
}

/** Merges a filter change, resets the cursor, and queries after 250 ms of
 *  quiet. The current rows stay visible until the new ones arrive. */
export function setHistoryFilters(patch: Partial<HistoryFilters>): void {
  setState("filters", { ...state.filters, ...patch });
  setState({ nextCursor: null });
  sequence += 1; // anything in flight is now stale
  if (timer !== undefined) clearTimeout(timer);
  setState({ status: "loading", loadingMore: false });
  timer = setTimeout(() => {
    timer = undefined;
    void runFirstPage();
  }, HISTORY_DEBOUNCE_MS);
}

/** Runs the query now (first open, retry, or after a change elsewhere). */
export async function refreshHistory(): Promise<void> {
  if (timer !== undefined) {
    clearTimeout(timer);
    timer = undefined;
  }
  await runFirstPage();
}

/** Appends the next page with the same filters. A no-op with no next page,
 *  while a load is in flight, or while a debounced change is pending. */
export async function loadMoreHistory(): Promise<void> {
  const after = state.nextCursor;
  if (after === null || state.loadingMore || state.status === "loading" || timer !== undefined) return;
  const mine = ++sequence;
  setState({ loadingMore: true, error: null });
  try {
    const page = await fetchPage(toQuery(state.filters, after));
    if (mine !== sequence) return;
    setState("rows", (rows) => [...rows, ...page.rows]);
    setState({ nextCursor: page.nextCursor, loadingMore: false });
  } catch (error) {
    if (mine !== sequence) return;
    setState({ loadingMore: false, error: asCommandError(error) });
  }
}

/** `get_job_timeline`: a settled Job's immutable view. */
export async function getJobTimeline(jobId: string): Promise<JobTimeline> {
  if (desktopAvailable()) return retryOnTransportFailure(() => command("get_job_timeline", { jobId }));
  const timeline = await import("./web-fixtures").then(({ webJobTimeline }) => webJobTimeline(jobId));
  if (!timeline) throw notFound(jobId);
  return timeline;
}

/** Test seam: clears the module state and any pending debounce. */
export function resetHistoryStore(): void {
  if (timer !== undefined) clearTimeout(timer);
  timer = undefined;
  sequence += 1;
  clearHistoryKnownIds();
  setState({ filters: {}, rows: [], nextCursor: null, status: "idle", loadingMore: false, error: null });
}
