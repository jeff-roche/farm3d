import { createStore, reconcile } from "solid-js/store";
import { command, desktopAvailable, needsDesktopError, retryOnTransportFailure } from "../ipc/client";
import { createSequencedStream } from "../ipc/sequenced-stream";
import type { CommandError } from "../generated/contracts/command/CommandError";
import { isQueueEvent, isTerminalJobState } from "./types";
import type {
  AmountEntry,
  DeclaredOutcome,
  DispatchPolicy,
  DispatchPreference,
  EligibilitySummary,
  Job,
  JobHistory,
  MaterialEstimate,
  NextAutomaticAction,
  PriorState,
  QueueChange,
  QueueEntry,
  QueueEntryEligibility,
  QueueEvent,
  QueueSnapshot,
  ReconciliationRequirement,
  SettleChoice,
} from "./types";

/** The only owner of the Queue and Jobs (spec "Frontend architecture").
 *  Listen-before-backfill over `queue.*`, copying `host-ops/host-operations-store.ts`'s
 *  pattern: subscribe first, then backfill through `list_queue`, so nothing
 *  between the two is missed. Rust is the only source of state, eligibility,
 *  blockers, allowed actions, settlement, and order (global constraint 4) --
 *  this store presents what Rust returns and never derives any of it. */

export type QueueStatus = "idle" | "loading" | "ready" | "error";
export type QueueSyncState = "syncing" | "current" | "uncertain";

interface QueueState {
  entries: QueueEntry[];
  jobs: Job[];
  requirements: ReconciliationRequirement[];
  eligibility: EligibilitySummary[];
  nextAutomaticAction: NextAutomaticAction;
  status: QueueStatus;
  syncState: QueueSyncState;
}

const [state, setState] = createStore<QueueState>({
  entries: [],
  jobs: [],
  requirements: [],
  eligibility: [],
  nextAutomaticAction: { kind: "evaluatorNotRunning" },
  status: "idle",
  syncState: "current",
});

export const queue = {
  /** Open (`queued`/`assigned`) entries in `position` order. */
  entries: (): QueueEntry[] =>
    state.entries
      .filter((entry) => entry.state !== "closed")
      .sort((a, b) => (a.position ?? 0) - (b.position ?? 0)),
  /** Closed entries, newest-closed first. */
  history: (): QueueEntry[] =>
    state.entries
      .filter((entry) => entry.state === "closed")
      .sort((a, b) => (b.closedAt ?? "").localeCompare(a.closedAt ?? "") || b.id.localeCompare(a.id)),
  entry: (id: string): QueueEntry | undefined => state.entries.find((entry) => entry.id === id),
  jobFor: (entryId: string): Job | undefined => state.jobs.find((j) => j.queueEntryId === entryId),
  job: (id: string): Job | undefined => state.jobs.find((j) => j.id === id),
  /** At most one active Job per Printer (D3's partial unique index). */
  activeJobFor: (printerId: string): Job | undefined =>
    state.jobs.find((j) => j.printerId === printerId && !isTerminalJobState(j.state)),
  /** Every open (`pending` or `deferred`) Reconciliation Requirement, as
   *  `QueueSnapshot.requirements` describes them. */
  requirements: (): ReconciliationRequirement[] => state.requirements.filter((r) => r.status !== "resolved"),
  eligibility: (entryId: string): EligibilitySummary | undefined =>
    state.eligibility.find((summary) => summary.entryId === entryId),
  nextAutomaticAction: (): NextAutomaticAction => state.nextAutomaticAction,
  status: (): QueueStatus => state.status,
  syncState: (): QueueSyncState => state.syncState,
};

/** Events for one row can reach the stream out of revision order: the
 *  backend publishes some results after releasing the Printer lock, so an
 *  older copy can follow a newer one. A lower revision than the held row's
 *  is stale and ignored; an equal one still applies, because `startBlockers`
 *  and `allowedActions` are republished without a revision bump. Backfills
 *  (`applySnapshot`) replace every row wholesale and skip this check. */
function upsertEntry(record: QueueEntry): void {
  const index = state.entries.findIndex((entry) => entry.id === record.id);
  if (index >= 0 && record.revision < state.entries[index].revision) return;
  if (index >= 0) setState("entries", index, reconcile(record));
  else setState("entries", (list) => [...list, record]);
}

/** Same stale-revision rule as `upsertEntry`. */
function upsertJob(record: Job): void {
  const index = state.jobs.findIndex((j) => j.id === record.id);
  if (index >= 0 && record.revision < state.jobs[index].revision) return;
  if (index >= 0) setState("jobs", index, reconcile(record));
  else setState("jobs", (list) => [...list, record]);
}

function upsertRequirement(record: ReconciliationRequirement): void {
  const index = state.requirements.findIndex((r) => r.id === record.id);
  if (index >= 0) setState("requirements", index, reconcile(record));
  else setState("requirements", (list) => [...list, record]);
}

function applyEvent(event: QueueEvent): void {
  const { payload } = event;
  switch (payload.type) {
    case "entryChanged":
      upsertEntry(payload.entry);
      break;
    case "jobChanged":
      upsertJob(payload.job);
      break;
    case "requirementChanged":
      upsertRequirement(payload.requirement);
      break;
    case "eligibilityChanged":
      // The full set, not a diff (spec "Events").
      setState({ eligibility: payload.summaries, nextAutomaticAction: payload.nextAutomaticAction });
      break;
  }
}

function applySnapshot(snapshot: QueueSnapshot): void {
  setState({
    entries: snapshot.entries,
    jobs: snapshot.jobs,
    requirements: snapshot.requirements,
    eligibility: snapshot.eligibility,
    nextAutomaticAction: snapshot.nextAutomaticAction,
    status: "ready",
  });
}

type QueueStream = ReturnType<typeof createSequencedStream<QueueEvent, QueueSnapshot>>;

let activeStream: QueueStream | undefined;
let activeUnlisten: (() => void) | undefined;

function disposeListener(): void {
  activeStream?.dispose();
  activeStream = undefined;
  activeUnlisten?.();
  activeUnlisten = undefined;
}

/** Subscribes to `queue.*` events, then backfills through `list_queue`, so
 *  nothing between the two is missed. Idempotent: calling it again disposes
 *  the previous listener first. Resolves once the first backfill has
 *  settled (successfully or not; a failure keeps retrying) with this
 *  start's own disposer. In web mode it loads `web-fixtures.ts`'s Queue
 *  instead (mirrors `host-operations-store.ts`). */
export async function startQueue(): Promise<() => void> {
  disposeListener();
  if (!desktopAvailable()) {
    const { buildWebQueueFixture } = await import("./web-fixtures");
    const fixture = buildWebQueueFixture();
    setState({
      entries: fixture.entries,
      jobs: fixture.jobs,
      requirements: fixture.requirements,
      eligibility: fixture.eligibility,
      nextAutomaticAction: fixture.nextAutomaticAction,
      status: "ready",
      syncState: "current",
    });
    return () => {};
  }

  setState({ syncState: "syncing", ...(state.status === "ready" ? {} : { status: "loading" }) });
  const stream: QueueStream = createSequencedStream<QueueEvent, QueueSnapshot>({
    backfill: () => command("list_queue", {}),
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
      if (isQueueEvent(candidate)) stream.receive(candidate);
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

/** Backfill now rather than wait for the stream's backoff, e.g. after a
 *  `CONFLICT`-style rejection from a Queue or Job command. Web mode has no
 *  stream to refresh. */
export function refreshQueue(): void {
  activeStream?.resync();
}

/** A command's returned rows, for whichever the stream hasn't delivered
 *  yet. The command returns right after its write-ahead commit and the
 *  stream's events for it can arrive first, followed by later transitions,
 *  so a row the store already holds is never overwritten by this older
 *  copy. */
function adoptChange(change: QueueChange): QueueChange {
  for (const entry of change.entries) if (!state.entries.some((e) => e.id === entry.id)) upsertEntry(entry);
  for (const record of change.jobs) if (!state.jobs.some((j) => j.id === record.id)) upsertJob(record);
  for (const requirement of change.requirements) {
    if (!state.requirements.some((r) => r.id === requirement.id)) upsertRequirement(requirement);
  }
  return change;
}

/** Runs one user-initiated write. Each call is one user action, so it gets
 *  a fresh client `operationId`; a transport failure is retried once with
 *  that same id, so the backend replays a committed first try instead of
 *  writing twice (never a `CommandError`, which is the backend's answer).
 *  Rejects with the `CommandError` for the caller to render inline. Web
 *  mode has no backend to write to and refuses. */
async function write(action: string, send: (operationId: string) => Promise<QueueChange>): Promise<QueueChange> {
  if (!desktopAvailable()) throw needsDesktopError(action);
  const operationId = crypto.randomUUID();
  return adoptChange(await retryOnTransportFailure(() => send(operationId)));
}

export interface AddToQueueOptions {
  materialEstimate?: MaterialEstimate;
  manualPrinterId?: string;
}

/** `add_to_queue`: creates `quantity` linked copies. */
export function addToQueue(
  sliceRevisionId: string,
  quantity: number,
  policy: DispatchPolicy,
  preference: DispatchPreference,
  options: AddToQueueOptions = {},
): Promise<QueueChange> {
  return write("Adding to the Queue", (operationId) => command("add_to_queue", {
    operationId, sliceRevisionId, quantity, policy, preference, ...options,
  }));
}

export interface UpdateQueueEntryPatch {
  policy?: DispatchPolicy;
  preference?: DispatchPreference;
}

export function updateQueueEntry(entryId: string, expectedRevision: number, patch: UpdateQueueEntryPatch): Promise<QueueChange> {
  return write("Updating a Queue Entry", (operationId) => command("update_queue_entry", {
    operationId, entryId, expectedRevision, ...patch,
  }));
}

export function moveQueueEntry(entryId: string, expectedRevision: number, toPosition: number): Promise<QueueChange> {
  return write("Moving a Queue Entry", (operationId) => command("move_queue_entry", { operationId, entryId, expectedRevision, toPosition }));
}

export function removeQueueEntry(entryId: string, expectedRevision: number): Promise<QueueChange> {
  return write("Removing a Queue Entry", (operationId) => command("remove_queue_entry", { operationId, entryId, expectedRevision }));
}

export function assignQueueEntry(entryId: string, printerId: string, spoolId: string, acknowledgeManualFacts?: boolean): Promise<QueueChange> {
  return write("Assigning a Queue Entry", (operationId) => command("assign_queue_entry", {
    operationId, entryId, printerId, spoolId, ...(acknowledgeManualFacts !== undefined ? { acknowledgeManualFacts } : {}),
  }));
}

export function stageJob(jobId: string): Promise<QueueChange> {
  return write("Staging a Job", (operationId) => command("stage_job", { operationId, jobId }));
}

/** `start_job`, with the `priorState` the operator confirmed the bed for
 *  (D9). The acknowledgement is the one literal the backend accepts. */
export function startJob(jobId: string, priorState: PriorState): Promise<QueueChange> {
  return write("Starting a Job", (operationId) => command("start_job", { operationId, jobId, priorState, acknowledgement: "bedClear" }));
}

export function pauseJob(jobId: string): Promise<QueueChange> {
  return write("Pausing a Job", (operationId) => command("pause_job", { operationId, jobId }));
}

export function resumeJob(jobId: string): Promise<QueueChange> {
  return write("Resuming a Job", (operationId) => command("resume_job", { operationId, jobId }));
}

export function cancelJob(jobId: string): Promise<QueueChange> {
  return write("Cancelling a Job", (operationId) => command("cancel_job", { operationId, jobId }));
}

export function releaseJob(jobId: string): Promise<QueueChange> {
  return write("Releasing a Job", (operationId) => command("release_job", { operationId, jobId }));
}

export function retryJob(jobId: string): Promise<QueueChange> {
  return write("Retrying a Job", (operationId) => command("retry_job", { operationId, jobId }));
}

/** `declare_job_outcome` (D9). The acknowledgement is the one literal the
 *  backend accepts. */
export function declareJobOutcome(jobId: string, outcome: DeclaredOutcome): Promise<QueueChange> {
  return write("Declaring a Job's outcome", (operationId) => command("declare_job_outcome", {
    operationId, jobId, outcome, acknowledgement: "hostStateUnknown",
  }));
}

export function settleJobMaterial(jobId: string, choice: SettleChoice): Promise<QueueChange> {
  return write("Settling a Job's material", (operationId) => command("settle_job_material", { operationId, jobId, choice }));
}

export function correctJobMaterial(jobId: string, entry: AmountEntry): Promise<QueueChange> {
  return write("Correcting a Job's material", (operationId) => command("correct_job_material", { operationId, jobId, entry }));
}

function notFound(what: string): CommandError {
  return { contractVersion: 1, code: "NOT_FOUND", message: `${what} was not found.`, recovery: [], retryable: false };
}

/** `explain_queue_entry`: the full eligibility explanation for one entry
 *  (D5). Read-only, so it has no `operationId` and never touches the
 *  store. In web mode it's served from `web-fixtures.ts` instead of
 *  refusing (fix round 1) -- unlike a write, nothing about it needs the
 *  desktop app to answer honestly. */
export function explainQueueEntry(entryId: string): Promise<QueueEntryEligibility> {
  if (!desktopAvailable()) {
    return import("./web-fixtures").then(({ explainWebQueueEntry }) => {
      const result = explainWebQueueEntry(entryId);
      if (!result) throw notFound(`Queue Entry ${entryId}`);
      return result;
    });
  }
  return retryOnTransportFailure(() => command("explain_queue_entry", { entryId }));
}

/** `get_job_history`: one Job's full timeline (D1). Read-only, so it has
 *  no `operationId` and never touches the store. Web mode as above (fix
 *  round 1). */
export function getJobHistory(jobId: string): Promise<JobHistory> {
  if (!desktopAvailable()) {
    return import("./web-fixtures").then(({ jobHistoryForWeb }) => {
      const result = jobHistoryForWeb(jobId);
      if (!result) throw notFound(`Job ${jobId}`);
      return result;
    });
  }
  return retryOnTransportFailure(() => command("get_job_history", { jobId }));
}
