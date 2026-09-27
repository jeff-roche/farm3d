import { createMemo, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Button, Chip, DataTable, ReorderHandle, SeverityMarker, Tabs } from "../design-system";
import type { DataTableColumn, SeverityMarkerProps, TabItem } from "../design-system";
import { isCommandError } from "../ipc/client";
import { library } from "../library/library-store";
import { navigation, serializeNavigationTarget, type NavigationTarget } from "../navigation/navigation-store";
import {
  closeReasonLabel,
  copyLabel,
  dispatchPolicyLabel,
  jobStateLabel,
  queueViewLabel,
  settlementLabel,
  startBlockerLabel,
} from "../queue/presentation";
import { moveQueueEntry, queue } from "../queue/queue-store";
import type { DispatchPolicy, Job, QueueEntry } from "../queue/types";
import { viewOf, type QueueView } from "../queue/views";
import { formatPrintTime } from "../slicing/revision-presentation";
import { materialLabel } from "../spools/materials";
import { formatGrams } from "../spools/weight";
import { QueueEntryDetail } from "./QueueEntryDetail";
import { goTo, QueueRecoveryButton } from "./QueueRecoveryButton";
import styles from "./QueueScreen.module.css";

/** The screen's tabs: the whole open Queue in order, then each `QueueView`. */
type ScreenView = "all" | QueueView;

const VIEW_ORDER: ScreenView[] = ["all", "awaitingOperator", "ready", "blocked", "assigned", "printing", "history"];
const POLICIES: DispatchPolicy[] = ["manual", "recommended", "automatic"];

function screenViewLabel(view: ScreenView): string {
  return view === "all" ? "Queue" : queueViewLabel(view);
}

/** Every entry the store holds, open ones first in position order. */
function allEntries(): QueueEntry[] {
  return [...queue.entries(), ...queue.history()];
}

/** The view an entry belongs to, or `undefined` for a queued entry whose
 *  eligibility summary hasn't arrived yet (it shows only under "Queue"). */
function entryView(entry: QueueEntry): QueueView | undefined {
  const summary = queue.eligibility(entry.id);
  if (entry.state === "queued" && !summary) return undefined;
  return viewOf(entry, queue.jobFor(entry.id), summary);
}

function rowsFor(view: ScreenView): QueueEntry[] {
  if (view === "all") return queue.entries();
  const source = view === "history" ? queue.history() : queue.entries();
  return source.filter((entry) => entryView(entry) === view);
}

function entryName(entry: QueueEntry): string {
  return entry.display.plateLabel ? `${entry.display.modelName} — ${entry.display.plateLabel}` : entry.display.modelName;
}

function projectNames(entry: QueueEntry): string {
  const model = library.models().find((candidate) => candidate.id === entry.display.modelId);
  if (!model || model.projectIds.length === 0) return "—";
  return model.projectIds
    .map((id) => library.projects().find((project) => project.id === id)?.name)
    .filter((name): name is string => name !== undefined)
    .join(", ") || "—";
}

function materialText(entry: QueueEntry): string {
  const family = entry.display.materialFamily;
  return family ? materialLabel(family, entry.display.materialOther ?? undefined) : "Not given";
}

function estimateText(entry: QueueEntry): string {
  const grams = formatGrams(entry.estimate.amountMg, 0);
  return entry.display.printSeconds === null ? grams : `${grams} · ${formatPrintTime(entry.display.printSeconds)}`;
}

function destinationText(entry: QueueEntry, job: Job | undefined): string {
  if (job) return job.printerSnapshot.name;
  if (entry.state !== "queued") return "—";
  const count = queue.eligibility(entry.id)?.eligibleCount;
  if (count === undefined) return "—";
  if (count === 0) return "No Printer eligible";
  return count === 1 ? "1 Printer eligible" : `${count} Printers eligible`;
}

function jobSeverity(job: Job): SeverityMarkerProps["severity"] {
  if (job.state === "outcomeUnknown" || job.startBlockers.length > 0) return "warning";
  if (job.state === "failed") return "fatal";
  if (job.state === "completed") return "resolved";
  return "info";
}

/** State, blockers, and settlement pair an icon, text, and color (spec
 *  "Accessibility and adaptation"). */
function StateCell(props: { entry: QueueEntry }) {
  const job = () => queue.jobFor(props.entry.id);
  const summary = () => queue.eligibility(props.entry.id);
  return (
    <span class={styles.state}>
      <Show when={props.entry.state === "queued"}>
        <Show when={summary()} fallback={<span class={styles.muted}>Checking…</span>}>
          {(held) => (
            <>
              <SeverityMarker
                severity={held().verdict === "blocked" ? "warning" : held().verdict === "ready" ? "resolved" : "info"}
                label={queueViewLabel(held().verdict)}
              />
              <Show when={held().topBlocker}>
                {(blocker) => (
                  <>
                    <span class={styles.blocker}>{blocker().message}</span>
                    <QueueRecoveryButton blocker={blocker()} />
                  </>
                )}
              </Show>
            </>
          )}
        </Show>
      </Show>
      <Show when={props.entry.state === "assigned" && job()}>
        {(held) => (
          <>
            <SeverityMarker severity={jobSeverity(held())} label={jobStateLabel(held().state)} />
            <Show when={held().startBlockers[0]}>
              {(blocker) => <span class={styles.blocker}>{startBlockerLabel(blocker())}</span>}
            </Show>
          </>
        )}
      </Show>
      <Show when={props.entry.state === "closed" && props.entry.closeReason}>
        {(reason) => (
          <>
            <SeverityMarker
              severity={reason() === "completed" ? "resolved" : reason() === "failed" ? "fatal" : "info"}
              label={closeReasonLabel(reason())}
            />
            <Show when={job() && (job()!.settlement === "pending" || job()!.settlement === "deferred") ? job() : undefined}>
              {(held) => <SeverityMarker severity="warning" label={`Material: ${settlementLabel(held().settlement)}`} />}
            </Show>
          </>
        )}
      </Show>
    </span>
  );
}

/** §Queue Entries and Job dispatch: the Queue destination. View tabs and
 *  Dispatch Policy chips over one ordered `DataTable`, a `ReorderHandle`
 *  per open row, and the Queue Entry dock. Everything shown -- order,
 *  verdicts, blockers, allowed actions -- is what Rust sent; this screen
 *  only groups and filters it. Self-contained like `SpoolInventory`: it
 *  reads the navigation target itself. */
export function QueueScreen() {
  const [view, setView] = createSignal<ScreenView>("all");
  const [policies, setPolicies] = createSignal<ReadonlySet<DispatchPolicy>>(new Set());
  const [hoveredLineage, setHoveredLineage] = createSignal<string | null>(null);
  const [actionError, setActionError] = createSignal<string | null>(null);
  const [dockMode, setDockMode] = createSignal<"inline" | "overlay">("overlay");
  let workspace: HTMLDivElement | undefined;

  onMount(() => {
    if (!workspace) return;
    const rem = Number.parseFloat(window.getComputedStyle(document.documentElement).fontSize) || 16;
    const minimumInlineWidth = 66 * rem; // 44rem table plus the 22rem dock -- PrinterDashboard's rule.
    const updateMode = (width: number) => setDockMode(width >= minimumInlineWidth ? "inline" : "overlay");
    updateMode(workspace.clientWidth);
    const observer = new ResizeObserver((entries) => updateMode(entries[0]?.contentRect.width ?? workspace!.clientWidth));
    observer.observe(workspace);
    onCleanup(() => observer.disconnect());
  });

  const rows = createMemo(() => rowsFor(view()).filter((entry) => policies().size === 0 || policies().has(entry.policy)));

  /** Lineages with more than one entry in the store: only those highlight. */
  const linkedLineages = createMemo(() => {
    const counts = new Map<string, number>();
    for (const entry of allEntries()) counts.set(entry.lineageId, (counts.get(entry.lineageId) ?? 0) + 1);
    return new Set([...counts].filter(([, count]) => count > 1).map(([lineageId]) => lineageId));
  });

  const togglePolicy = (policy: DispatchPolicy) => setPolicies((current) => {
    const next = new Set(current);
    if (next.has(policy)) next.delete(policy);
    else next.add(policy);
    return next;
  });
  const clearPolicies = () => setPolicies(new Set<DispatchPolicy>());

  // The selection is a Queue Entry id or a Job id, resolved by prefix
  // (spec "Navigation").
  const selectedEntry = createMemo<QueueEntry | undefined>(() => {
    const target = navigation.target();
    if (target.destination !== "queue" || target.selection?.kind !== "job") return undefined;
    const id = target.selection.id;
    const entryId = id.startsWith("job-") ? queue.job(id)?.queueEntryId : id;
    return entryId ? queue.entry(entryId) : undefined;
  });

  function select(id: string | null) {
    const target: NavigationTarget = id
      ? { version: 1, destination: "queue", selection: { kind: "job", id } }
      : { version: 1, destination: "queue" };
    const entries = allEntries();
    navigation.navigate(target, {
      availableDestinations: ["queue"],
      availableIds: [...entries.map((entry) => entry.id), ...entries.flatMap((entry) => (entry.jobId ? [entry.jobId] : []))],
    });
    window.location.hash = serializeNavigationTarget(target).slice(1);
  }

  /** A `ReorderHandle` move within the visible rows, sent as the target
   *  row's own (absolute) position -- Rust renumbers everything else. */
  function move(from: number, to: number) {
    const list = rows();
    const entry = list[from];
    const toPosition = list[to]?.position;
    if (!entry || toPosition == null) return;
    setActionError(null);
    moveQueueEntry(entry.id, entry.revision, toPosition).catch((error: unknown) => {
      setActionError(isCommandError(error) ? error.message : "The entry couldn't be moved.");
    });
  }

  const lineageAt = (target: EventTarget | null): string | null =>
    target instanceof Element ? target.closest("[data-lineage]")?.getAttribute("data-lineage") ?? null : null;

  const columns: DataTableColumn<QueueEntry>[] = [
    {
      id: "position", header: "#", width: "4.5rem",
      cell: (entry) => (
        <Show when={entry.state !== "closed"} fallback={<span class={styles.muted}>—</span>}>
          <span class={styles.position}>
            <ReorderHandle
              label={`${entryName(entry)}, position ${entry.position}`}
              index={rows().findIndex((row) => row.id === entry.id)}
              count={rows().length}
              onMove={move}
              disabled={!entry.allowedActions.includes("move")}
            />
            <span>{entry.position}</span>
          </span>
        </Show>
      ),
    },
    {
      id: "name", header: "Name",
      cell: (entry) => (
        <span class={styles.name}>
          <span>{entryName(entry)}</span>
          <Show when={entry.copyCount > 1}>
            <span class={styles.copy}>{copyLabel(entry)}</span>
          </Show>
          <Show when={entry.originKind}>
            {(kind) => <span class={styles.copy}>{kind() === "retry" ? "Retry" : "Replacement"}</span>}
          </Show>
        </span>
      ),
    },
    { id: "project", header: "Project", cell: projectNames },
    { id: "material", header: "Material", cell: materialText },
    { id: "estimate", header: "Estimate", align: "end", cell: estimateText },
    { id: "destination", header: "Destination", cell: (entry) => destinationText(entry, queue.jobFor(entry.id)) },
    { id: "state", header: "State", cell: (entry) => <StateCell entry={entry} /> },
    { id: "policy", header: "Policy", cell: (entry) => dispatchPolicyLabel(entry.policy) },
  ];

  /** The table's empty state. The filters stay as they are; each state
   *  offers one way out. */
  const empty = (): JSX.Element => {
    if (rowsFor(view()).length > 0) {
      return (
        <div class={styles.empty}>
          <p>No entries match these filters</p>
          <Button variant="ghost" onClick={clearPolicies}>Clear policy filters</Button>
        </div>
      );
    }
    if (view() === "all") {
      return (
        <div class={styles.empty}>
          <p>The Queue is empty</p>
          <p class={styles.muted}>Add to the Queue from a Slice Revision in the Library.</p>
          <Button onClick={() => goTo({ version: 1, destination: "library" })}>Open the Library</Button>
        </div>
      );
    }
    return (
      <div class={styles.empty}>
        <p>No entries in this view</p>
        <Button variant="ghost" onClick={() => setView("all")}>Show the whole Queue</Button>
      </div>
    );
  };

  const table = () => (
    <div
      class={styles.tableArea}
      onPointerOver={(event) => setHoveredLineage(lineageAt(event.target))}
      onPointerLeave={() => setHoveredLineage(null)}
      onFocusIn={(event) => setHoveredLineage(lineageAt(event.target))}
      onFocusOut={(event) => setHoveredLineage(lineageAt(event.relatedTarget))}
    >
      <DataTable
        label="Queue"
        rows={rows()}
        rowId={(entry) => entry.id}
        columns={columns}
        selectedId={selectedEntry()?.id ?? null}
        onSelect={select}
        onActivate={select}
        empty={empty()}
        rowAttributes={(entry) => {
          const linked = linkedLineages().has(entry.lineageId);
          return {
            "data-entry-id": entry.id,
            "data-reorder-row": entry.state !== "closed" ? "" : undefined,
            "data-lineage": linked ? entry.lineageId : undefined,
            "data-lineage-highlight": linked && hoveredLineage() === entry.lineageId ? "" : undefined,
          };
        }}
      />
    </div>
  );

  // Stable tab items: a getter keeps each label's count live without
  // recreating the tab (which would remount its table), and Kobalte
  // mounts only the selected tab's content.
  const tabItems: TabItem[] = VIEW_ORDER.map((value) => ({
    value,
    get label() {
      return `${screenViewLabel(value)} (${rowsFor(value).length})`;
    },
    get content() {
      return table();
    },
  }));

  return (
    <div class={styles.screen}>
      <Show when={actionError()}>
        {(message) => (
          <div class={styles.errorBanner} role="alert">
            <p class={styles.errorMessage}>{message()}</p>
            <Button variant="ghost" onClick={() => setActionError(null)}>Dismiss</Button>
          </div>
        )}
      </Show>
      <div ref={workspace} class={styles.workspace}>
        <div class={styles.main}>
          <div class={styles.toolbar} role="group" aria-label="Filter by Dispatch Policy">
            <span class={styles.toolbarLabel}>Dispatch Policy</span>
            <For each={POLICIES}>
              {(policy) => (
                <Chip selected={policies().has(policy)} onSelectedChange={() => togglePolicy(policy)}>
                  {dispatchPolicyLabel(policy)}
                </Chip>
              )}
            </For>
          </div>
          <Tabs class={styles.tabs} value={view()} onChange={(value) => setView(value as ScreenView)} items={tabItems} />
        </div>
        <Show when={selectedEntry()}>
          {(entry) => <QueueEntryDetail entry={entry()} mode={dockMode()} onClose={() => select(null)} />}
        </Show>
      </div>
    </div>
  );
}
