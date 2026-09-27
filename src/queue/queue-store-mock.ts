/** A stand-in for `queue-store` in screen tests. Test-only: nothing outside
 *  a test imports this module (mirrors
 *  `src/host-ops/host-operations-store-mock.ts`). Use it as
 *
 *    vi.mock("../queue/queue-store", async () =>
 *      (await import("../queue/queue-store-mock")).queueStoreMock);
 *
 *  The read side is a real Solid store, so components react when a test
 *  changes it with `setQueueStoreState` (or `loadWebQueueFixture`); the
 *  actions are spies. */
import { createStore, reconcile } from "solid-js/store";
import { vi } from "vitest";
import { buildWebQueueFixture, type WebQueueFixture } from "./web-fixtures";
import { isTerminalJobState } from "./types";
import type {
  AmountEntry,
  DeclaredOutcome,
  DispatchPolicy,
  DispatchPreference,
  EligibilitySummary,
  Job,
  NextAutomaticAction,
  PriorState,
  QueueChange,
  QueueEntry,
  ReconciliationRequirement,
  SettleChoice,
} from "./types";
import type { AddToQueueOptions, UpdateQueueEntryPatch } from "./queue-store";

interface MockQueueState {
  entries: QueueEntry[];
  jobs: Job[];
  requirements: ReconciliationRequirement[];
  eligibility: EligibilitySummary[];
  nextAutomaticAction: NextAutomaticAction;
  status: "idle" | "loading" | "ready" | "error";
  syncState: "syncing" | "current" | "uncertain";
}

const initialState = (): MockQueueState => ({
  entries: [],
  jobs: [],
  requirements: [],
  eligibility: [],
  nextAutomaticAction: { kind: "evaluatorNotRunning" },
  status: "ready",
  syncState: "current",
});

const [state, setState] = createStore<MockQueueState>(initialState());

function emptyChange(): QueueChange {
  return { entries: [], jobs: [], requirements: [] };
}

export const queueStoreMock = {
  queue: {
    entries: (): QueueEntry[] =>
      state.entries.filter((entry) => entry.state !== "closed").sort((a, b) => (a.position ?? 0) - (b.position ?? 0)),
    history: (): QueueEntry[] => state.entries.filter((entry) => entry.state === "closed"),
    entry: (id: string) => state.entries.find((entry) => entry.id === id),
    jobFor: (entryId: string) => state.jobs.find((j) => j.queueEntryId === entryId),
    job: (id: string) => state.jobs.find((j) => j.id === id),
    activeJobFor: (printerId: string) => state.jobs.find((j) => j.printerId === printerId && !isTerminalJobState(j.state)),
    requirements: (): ReconciliationRequirement[] => state.requirements.filter((r) => r.status !== "resolved"),
    eligibility: (entryId: string) => state.eligibility.find((summary) => summary.entryId === entryId),
    nextAutomaticAction: () => state.nextAutomaticAction,
    status: () => state.status,
    syncState: () => state.syncState,
  },
  startQueue: vi.fn(async (): Promise<() => void> => () => {}),
  refreshQueue: vi.fn(),
  addToQueue: vi.fn(async (
    _sliceRevisionId: string,
    _quantity: number,
    _policy: DispatchPolicy,
    _preference: DispatchPreference,
    _options?: AddToQueueOptions,
  ): Promise<QueueChange> => emptyChange()),
  updateQueueEntry: vi.fn(async (_entryId: string, _expectedRevision: number, _patch: UpdateQueueEntryPatch): Promise<QueueChange> => emptyChange()),
  moveQueueEntry: vi.fn(async (_entryId: string, _expectedRevision: number, _toPosition: number): Promise<QueueChange> => emptyChange()),
  removeQueueEntry: vi.fn(async (_entryId: string, _expectedRevision: number): Promise<QueueChange> => emptyChange()),
  assignQueueEntry: vi.fn(async (_entryId: string, _printerId: string, _spoolId: string, _acknowledgeManualFacts?: boolean): Promise<QueueChange> => emptyChange()),
  stageJob: vi.fn(async (_jobId: string): Promise<QueueChange> => emptyChange()),
  startJob: vi.fn(async (_jobId: string, _priorState: PriorState): Promise<QueueChange> => emptyChange()),
  pauseJob: vi.fn(async (_jobId: string): Promise<QueueChange> => emptyChange()),
  resumeJob: vi.fn(async (_jobId: string): Promise<QueueChange> => emptyChange()),
  cancelJob: vi.fn(async (_jobId: string): Promise<QueueChange> => emptyChange()),
  releaseJob: vi.fn(async (_jobId: string): Promise<QueueChange> => emptyChange()),
  retryJob: vi.fn(async (_jobId: string): Promise<QueueChange> => emptyChange()),
  declareJobOutcome: vi.fn(async (_jobId: string, _outcome: DeclaredOutcome): Promise<QueueChange> => emptyChange()),
  settleJobMaterial: vi.fn(async (_jobId: string, _choice: SettleChoice): Promise<QueueChange> => emptyChange()),
  correctJobMaterial: vi.fn(async (_jobId: string, _entry: AmountEntry): Promise<QueueChange> => emptyChange()),
};

/** Replaces (a slice of) the mock's state, as events settling into the real
 *  store would. */
export function setQueueStoreState(patch: Partial<Omit<MockQueueState, "status" | "syncState">>): void {
  if (patch.entries) setState("entries", reconcile(patch.entries));
  if (patch.jobs) setState("jobs", reconcile(patch.jobs));
  if (patch.requirements) setState("requirements", reconcile(patch.requirements));
  if (patch.eligibility) setState("eligibility", reconcile(patch.eligibility));
  if (patch.nextAutomaticAction) setState("nextAutomaticAction", patch.nextAutomaticAction);
}

/** Loads `web-fixtures.ts`'s Queue into the mock, and returns the full
 *  fixture for the test to read from too. */
export function loadWebQueueFixture(): WebQueueFixture {
  const fixture = buildWebQueueFixture();
  setState({
    entries: fixture.entries,
    jobs: fixture.jobs,
    requirements: fixture.requirements,
    eligibility: fixture.eligibility,
    nextAutomaticAction: fixture.nextAutomaticAction,
  });
  return fixture;
}

export function resetQueueStoreMock(): void {
  setState(initialState());
  for (const action of Object.values(queueStoreMock)) {
    if (typeof action === "function" && "mockClear" in action) action.mockClear();
  }
}
