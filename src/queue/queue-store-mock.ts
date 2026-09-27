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
  JobHistory,
  NextAutomaticAction,
  PriorState,
  QueueChange,
  QueueEntry,
  QueueEntryEligibility,
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

/** Minimal-but-valid fallbacks for the two query mocks below, the same
 *  role `host-ops-store-mock.ts`'s `mockRow` plays. */
function mockJob(id: string): Job {
  return {
    id, revision: 1, queueEntryId: "qen-mock", sliceRevisionId: "slr-mock", printerId: "prn-mock",
    printerSnapshot: {
      name: "Mock Printer", location: null, catalogRef: null, adapterKind: null,
      profile: {
        bedShape: { kind: "rectangular", widthMm: 200, depthMm: 200, originXMm: 0, originYMm: 0 },
        printableHeightMm: 200, bedExcludeAreas: [], defaultBedType: "0",
        nozzleDiameterMm: [0.4], nozzleType: "brass", gcodeFlavor: "marlin2",
        hasAuxiliaryFan: false, supportsAirFiltration: false, supportsMultiFilament: false,
        suggestedHostType: null,
      },
    },
    spoolId: "spl-mock", reservationId: "rsv-mock", estimateMg: 0, state: "assigned",
    cancelReason: null, settlement: "open", settlementMethod: null, settlementPreview: null,
    corrected: false, assignedBy: "operator", startConfirmation: null,
    uploadHostOperationId: null, activeHostOperationId: null, hostPath: null, maxProgressPct: 0,
    hostUnreachableSince: null, lastFailure: null, startBlockers: [], allowedActions: [],
    createdAt: "2026-09-25T00:00:00Z", updatedAt: "2026-09-25T00:00:00Z", startedAt: null, endedAt: null,
  };
}

function mockEntry(id: string): QueueEntry {
  return {
    id, revision: 1, sliceRevisionId: "slr-mock", lineageId: "qln-mock", copyIndex: 1, copyCount: 1,
    originEntryId: null, originKind: null, state: "assigned", closeReason: null, position: 1,
    policy: "manual", preference: "loadedFirst", estimate: { amountMg: 1, source: "operatorEntered" },
    manualPrinterId: null, jobId: null, requiresManualPrinterSelection: false, allowedActions: [],
    display: { modelId: "mdl-mock", modelName: "Mock", plateLabel: null, targetLabel: "Mock", materialFamily: null, materialOther: null, printSeconds: null },
    createdAt: "2026-09-25T00:00:00Z", updatedAt: "2026-09-25T00:00:00Z", closedAt: null,
  };
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
  explainQueueEntry: vi.fn(async (entryId: string): Promise<QueueEntryEligibility> => ({
    entryId, verdict: "blocked", candidates: [], printers: [], blockers: [], evaluatedAt: "2026-09-25T00:00:00Z",
  })),
  getJobHistory: vi.fn(async (jobId: string): Promise<JobHistory> => {
    const job = mockJob(jobId);
    const entry = mockEntry(job.queueEntryId);
    return { job, entry, lineage: [entry], events: [], reservations: [], hostOperations: [], requirements: [] };
  }),
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
