/** Deterministic `just web` fixtures for the history store (spec "Frontend
 *  architecture"). Rows are derived from the queue web fixture's settled
 *  Jobs, so every id resolves to the same Job/Printer/Model in `just web`.
 *  Synthetic names only; no real network details. */
import { buildWebAttentionFixture, webIncidentDetail } from "../attention/web-fixtures";
import { buildWebSlicingFixture } from "../slicing/web-fixtures";
import { buildWebQueueFixture, jobHistoryForWeb, WEB_QUEUE_JOB_COMPLETED } from "../queue/web-fixtures";
import type { AmountEvent } from "../generated/contracts/domain/AmountEvent";
import type { JobHistoryPage, JobHistoryQuery, JobHistoryRow, JobHistoryState, JobTimeline, JobTimelineItem } from "./types";

const DEFAULT_STATES: JobHistoryState[] = ["completed", "failed", "cancelled"];
/** The Incident `attention/web-fixtures.ts` builds, which the completed
 *  Job's timeline links (so its "Open Incident" link resolves in `just web`). */
const WEB_HISTORY_INCIDENT_ID = "inc-w-host-failed";
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
      incidentId: job.id === WEB_QUEUE_JOB_COMPLETED ? WEB_HISTORY_INCIDENT_ID : null,
      snapshotCount: job.id === WEB_QUEUE_JOB_COMPLETED ? 2 : 0,
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
    ...(history.job.id === WEB_QUEUE_JOB_COMPLETED ? completedJobExtras(history.job.id, history.job.reservationId, history.job.spoolId) : []),
  ].sort((a, b) => a.at.localeCompare(b.at));
  const row = webHistoryRows().find((candidate) => candidate.jobId === jobId);
  const incident = jobId === WEB_QUEUE_JOB_COMPLETED ? webIncidentDetail(WEB_HISTORY_INCIDENT_ID)?.incident : undefined;
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
    incident: incident ? { ...incident, jobId } : null,
    items,
  };
}

/** The completed Job's material ledger (a deduction and a correction), an
 *  Attention Event, its Incident's entries, and one unpruned and one
 *  pruned snapshot: every item kind a screenshot needs. */
function completedJobExtras(jobId: string, reservationId: string, spoolId: string): JobTimelineItem[] {
  const deduction: AmountEvent = {
    id: `${jobId}-amt-1`, spoolId, sequence: 4, kind: "consumption", beforeMg: 412_000, afterMg: 377_400,
    confidenceAfter: "estimated", reservationId, occurredAt: "2026-09-20T09:29:30Z", isCorrection: false,
  };
  const correction: AmountEvent = {
    id: `${jobId}-amt-2`, spoolId, sequence: 5, kind: "measurement", beforeMg: 377_400, afterMg: 379_100,
    confidenceAfter: "measured", reservationId, note: "Weighed on the scale", occurredAt: "2026-09-21T08:00:00Z", isCorrection: true,
  };
  const attention = buildWebAttentionFixture();
  const event = [...attention.backfill.open, ...attention.backfill.resolved].find((candidate) => candidate.jobId === jobId);
  const incident = webIncidentDetail(WEB_HISTORY_INCIDENT_ID);
  const snapshots = attention.snapshots.slice(0, 2).map((snapshot, index) => ({
    ...snapshot,
    jobId,
    ...(index === 1 ? { prunedAt: "2026-09-24T00:00:00Z", pruneReason: "age" as const } : { prunedAt: null, pruneReason: null }),
  }));
  return [
    { source: "amountEvent", at: deduction.occurredAt, amountEvent: deduction, isCorrection: false },
    { source: "amountEvent", at: correction.occurredAt, amountEvent: correction, isCorrection: true },
    ...(event ? [{ source: "attention" as const, at: event.firstObservedAt, event }] : []),
    ...(incident?.timeline.flatMap((entry) => (entry.source === "incident" ? [{ source: "incident" as const, at: entry.entry.at, entry: entry.entry }] : [])) ?? []),
    ...snapshots.map((snapshot): JobTimelineItem => ({ source: "snapshot", at: snapshot.capturedAt, snapshot })),
  ];
}
