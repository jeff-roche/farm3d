/** Deterministic `just web` fixtures for the history store (spec "Frontend
 *  architecture"). Rows are derived from the queue web fixture's settled
 *  Jobs, so every id resolves to the same Job/Printer/Model in `just web`.
 *  Synthetic names only; no real network details. */
import { buildWebSlicingFixture } from "../slicing/web-fixtures";
import { buildWebQueueFixture, jobHistoryForWeb } from "../queue/web-fixtures";
import type { JobHistoryPage, JobHistoryQuery, JobHistoryRow, JobHistoryState, JobTimeline, JobTimelineItem } from "./types";

const DEFAULT_STATES: JobHistoryState[] = ["completed", "failed", "cancelled"];
const HISTORY_STATES = new Set<string>(["completed", "failed", "cancelled", "outcomeUnknown"]);

export function webHistoryRows(): JobHistoryRow[] {
  const fixture = buildWebQueueFixture();
  const rows: JobHistoryRow[] = [];
  for (const job of fixture.jobs) {
    if (!HISTORY_STATES.has(job.state)) continue;
    const entry = fixture.entries.find((candidate) => candidate.id === job.queueEntryId);
    if (!entry) continue;
    rows.push({
      jobId: job.id,
      state: job.state as JobHistoryState,
      cancelReason: job.cancelReason,
      historyAt: job.endedAt ?? job.createdAt,
      startedAt: job.startedAt,
      endedAt: job.endedAt,
      printerId: job.printerId,
      printerSnapshotName: job.printerSnapshot.name,
      printerArchived: false,
      spoolId: job.spoolId,
      spoolNumber: Number(job.spoolId.split("-").pop()) || 1,
      modelId: entry.display.modelId,
      modelName: entry.display.modelName,
      sliceRevisionId: job.sliceRevisionId,
      plateName: entry.display.plateLabel,
      settlement: job.settlement,
      incidentId: null,
      snapshotCount: 0,
    });
  }
  return rows.sort((a, b) => b.historyAt.localeCompare(a.historyAt) || b.jobId.localeCompare(a.jobId));
}

/** `list_job_history`'s web-mode answer: the same filters the query names,
 *  paged by an offset cursor (opaque to callers). */
export function webJobHistoryPage(query: JobHistoryQuery = {}): JobHistoryPage {
  const states = new Set(query.states ?? DEFAULT_STATES);
  const text = query.text?.trim().toLowerCase() ?? "";
  const filtered = webHistoryRows().filter((row) =>
    states.has(row.state)
    && (query.printerLifecycle !== "archived" || row.printerArchived)
    && (query.printerLifecycle !== "active" || !row.printerArchived)
    && (!query.printerId || row.printerId === query.printerId)
    && (!query.spoolId || row.spoolId === query.spoolId)
    && (!query.modelId || row.modelId === query.modelId)
    && (!query.endedAfter || (row.endedAt !== null && row.endedAt >= query.endedAfter))
    && (!query.endedBefore || (row.endedAt !== null && row.endedAt < query.endedBefore))
    && (!text || `${row.modelName} ${row.printerSnapshotName}`.toLowerCase().includes(text)));
  const offset = query.after ? Number(query.after) || 0 : 0;
  const limit = query.limit ?? 50;
  const rows = filtered.slice(offset, offset + limit);
  const next = offset + limit;
  return { rows, nextCursor: next < filtered.length ? String(next) : null };
}

/** `get_job_timeline`'s web-mode answer, built from the queue fixture's own
 *  `get_job_history` view. `undefined` for an unknown or unsettled Job. */
export function webJobTimeline(jobId: string): JobTimeline | undefined {
  const history = jobHistoryForWeb(jobId);
  if (!history || !HISTORY_STATES.has(history.job.state)) return undefined;
  const record = buildWebSlicingFixture().revisionRecords[history.job.sliceRevisionId];
  if (!record) return undefined;
  const items: JobTimelineItem[] = [
    ...history.events.map((event): JobTimelineItem => ({ source: "job", at: event.at, event })),
    ...history.hostOperations.map((hostOperation): JobTimelineItem => ({ source: "hostOperation", at: hostOperation.createdAt, hostOperation })),
    ...history.reservations.map((reservation): JobTimelineItem => ({ source: "reservation", at: reservation.createdAt, reservation })),
    ...history.requirements.map((requirement): JobTimelineItem => ({ source: "requirement", at: requirement.openedAt, requirement })),
  ].sort((a, b) => a.at.localeCompare(b.at));
  const row = webHistoryRows().find((candidate) => candidate.jobId === jobId);
  return {
    job: history.job,
    entry: history.entry,
    lineage: history.lineage,
    printerSnapshot: history.job.printerSnapshot,
    sliceRevision: {
      id: record.id, kind: record.kind, modelId: record.modelId, sourceRevisionId: record.sourceRevisionId,
      plate: record.plate ?? null, target: record.target ?? null, runtime: record.runtime ?? null,
      estimates: record.estimates ?? null, facts: record.facts, createdAt: record.createdAt,
    },
    spoolId: history.job.spoolId,
    spoolNumber: row?.spoolNumber ?? 1,
    incident: null,
    items,
  };
}
