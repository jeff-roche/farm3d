import { createEffect, createSignal, createUniqueId, For, lazy, on, onMount, Show, Suspense, type JSX } from "solid-js";
import { AlertDialog, Button, Progress } from "../design-system";
import { capabilities } from "../host-ops/capabilities-store";
import { hostOperations, reconcileHostOperation } from "../host-ops/host-operations-store";
import {
  capabilityRefusalText,
  hostOperationFailureText,
  hostOperationLabel,
  inconclusiveReasonText,
  type Severity,
} from "../host-ops/presentation";
import { canAbandon, canReconcile, reconcileCapability } from "../host-ops/reconcile-rule";
import type { HostOperation } from "../host-ops/types";
import { printers } from "../printers/printer-store";
import type { ResolvedPrinter } from "../printers/types";
import {
  cancelReasonLabel,
  estimateSourceLabel,
  jobStateLabel,
  settlementLabel,
} from "../queue/presentation";
import {
  cancelJob,
  pauseJob,
  queue,
  refreshQueue,
  releaseJob,
  resumeJob,
  retryJob,
  stageJob,
} from "../queue/queue-store";
import { settlementPreviewText } from "../queue/settlement";
import type { Job, JobAction, JobFailure } from "../queue/types";
import { formatDateTime, formatPrintTime } from "../slicing/revision-presentation";
import { ensureInventoryLoaded } from "../spools/spool-store";
import { formatGrams } from "../spools/weight";
import { AbandonReconciliationDialog } from "./AbandonReconciliationDialog";
import { DeclareOutcomeDialog } from "./DeclareOutcomeDialog";
import { HostOperationAlert } from "./HostOperationAlert";
import { goTo, QueueRecoveryButton, showQueueEntry } from "./QueueRecoveryButton";
import { SeverityLabel } from "./SeverityLabel";
import { SettleMaterialDialog, spoolName } from "./SettleMaterialDialog";
import { StartJobDialog } from "./StartJobDialog";
import styles from "./JobPanel.module.css";

/** The Timeline (a live Job's events, or a settled Job's full timeline)
 *  loads on its own, so the Printer's Job tab stays light. */
const JobTimelineSection = lazy(() => import("./JobTimelineSection").then((m) => ({ default: m.JobTimelineSection })));

export interface JobPanelProps {
  job: Job;
  /** The Job's Printer, for its live status (the Start confirmation reads
   *  it). Defaults to the Printer store's record. */
  printer?: ResolvedPrinter;
  /** Show the Job's active Host Operation with P6's **Check again** and
   *  **Abandon check…**. Off in the Printer's Job tab, whose own "Printer
   *  operations" list already shows that row with the same buttons. */
  showHostOperation?: boolean;
}

type Dialog = "start" | "cancel" | "settle" | "declare";

/** D3's column order: what an operator reaches for, in lifecycle order. */
const ACTIONS: { action: JobAction; label: (job: Job) => string }[] = [
  { action: "start", label: () => "Start…" },
  { action: "stage", label: (job) => (job.state === "awaitingStart" ? "Stage again" : "Stage") },
  { action: "pause", label: () => "Pause" },
  { action: "resume", label: () => "Resume" },
  { action: "cancel", label: () => "Cancel…" },
  { action: "release", label: () => "Release" },
  { action: "settleMaterial", label: () => "Settle material…" },
  { action: "correctMaterial", label: () => "Correct weight…" },
  { action: "declareOutcome", label: () => "Declare outcome…" },
  { action: "retry", label: () => "Retry" },
];

/** Plain labels, not `SeverityMarker` (a `role="status"` region each):
 *  the panel has exactly one live region, `announcement` below. */
function jobSeverity(job: Job): Severity {
  if (job.state === "outcomeUnknown" || job.startBlockers.length > 0) return "warning";
  if (job.state === "failed") return "error";
  if (job.state === "completed") return "success";
  if (job.state === "cancelled") return "neutral";
  return "info";
}

function failureText(failure: JobFailure): string {
  switch (failure.kind) {
    case "refused": return failure.message;
    case "hostOperationFailed": return hostOperationFailureText(failure.failure.code);
    case "hostOperationAbandoned": return "farm3d stopped checking the last printer operation, so its result is unknown.";
  }
}

const PROGRESS_STATES = new Set<Job["state"]>(["printing", "paused", "completed", "failed", "cancelled", "outcomeUnknown"]);

/** One Job (spec "Frontend architecture"): its state, progress, the
 *  Slice's facts, the reservation and its settlement, `startBlockers`, the
 *  Timeline from `get_job_history`, and every action Rust's
 *  `allowedActions` lists -- and only those. A disabled action says why in
 *  visible text. State changes are announced through one polite live
 *  region; failures use `role="alert"` with their recovery. */
export function JobPanel(props: JobPanelProps) {
  const headingId = createUniqueId();
  const blockersId = createUniqueId();
  const [open, setOpen] = createSignal<Dialog | null>(null);
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<unknown>(null);
  const [announcement, setAnnouncement] = createSignal("");
  const triggers: Partial<Record<Dialog, HTMLButtonElement>> = {};

  // `spoolName` reads the inventory; the Queue and Monitor don't load it.
  onMount(() => void ensureInventoryLoaded().catch(() => undefined));

  const printer = () => props.printer ?? printers().find((candidate) => candidate.id === props.job.printerId);
  const entry = () => queue.entry(props.job.queueEntryId);
  const allowed = (action: JobAction) => props.job.allowedActions.includes(action);
  const offered = () => ACTIONS.filter((item) => allowed(item.action));

  createEffect(on(() => props.job.state, (state, previous) => {
    if (previous !== undefined && state !== previous) setAnnouncement(`Job ${jobStateLabel(state).toLowerCase()}`);
  }));

  async function run(action: () => Promise<unknown>) {
    if (pending()) return;
    setPending(true);
    setError(null);
    try {
      await action();
    } catch (e) {
      setError(e);
    } finally {
      setPending(false);
    }
  }

  function perform(action: JobAction, trigger: HTMLButtonElement) {
    switch (action) {
      case "start":
      case "cancel":
      case "declareOutcome":
      case "settleMaterial":
      case "correctMaterial": {
        const dialog: Dialog = action === "declareOutcome" ? "declare"
          : action === "settleMaterial" || action === "correctMaterial" ? "settle" : action;
        triggers[dialog] = trigger;
        setOpen(dialog);
        return;
      }
      case "stage": return void run(() => stageJob(props.job.id));
      case "pause": return void run(() => pauseJob(props.job.id));
      case "resume": return void run(() => resumeJob(props.job.id));
      case "release": return void run(() => releaseJob(props.job.id));
      case "retry": return void run(async () => {
        const created = (await retryJob(props.job.id)).entries[0];
        if (created) showQueueEntry(created.id);
      });
    }
  }

  const disabledReason = (action: JobAction) =>
    action === "start" && props.job.startBlockers.length > 0 ? blockersId : undefined;

  const dialogOpenChange = (dialog: Dialog) => (next: boolean) => setOpen(next ? dialog : null);
  const facts = () => {
    const held = entry();
    const rows: { label: string; value: string; mono?: boolean }[] = [];
    if (held) {
      rows.push({ label: "Model", value: held.display.plateLabel ? `${held.display.modelName} — ${held.display.plateLabel}` : held.display.modelName });
      rows.push({ label: "Target", value: held.display.targetLabel });
      if (held.display.printSeconds !== null) rows.push({ label: "Print time", value: formatPrintTime(held.display.printSeconds) });
    }
    rows.push({ label: "Printer", value: props.job.printerSnapshot.name });
    if (props.job.hostPath) rows.push({ label: "File", value: props.job.hostPath, mono: true });
    return rows;
  };
  const reservation = () => {
    const rows = [
      { label: "Estimate", value: formatGrams(props.job.estimateMg, 1) },
      {
        label: "Material",
        value: props.job.settlementMethod
          ? `${settlementLabel(props.job.settlement)} (${props.job.settlementMethod === "measured" ? "measured" : "from the estimate"})`
          : settlementLabel(props.job.settlement),
      },
    ];
    const held = entry();
    const preview = settlementPreviewText(props.job.settlementPreview);
    if (preview) rows.push({ label: "Estimated use", value: preview });
    return { rows, source: held ? estimateSourceLabel(held.estimate.source) : undefined };
  };
  const foreignPrint = () => {
    const state = printer()?.runtimeStatus?.operationalState;
    return props.job.state === "awaitingStart" && (state === "printing" || state === "paused");
  };
  /** D3's "cancel after start" (a host write) versus "before start". */
  const started = () => props.job.state === "printing" || props.job.state === "paused";
  const activeOperation = () =>
    props.job.activeHostOperationId ? hostOperations.operation(props.job.activeHostOperationId) : undefined;

  return (
    <section class={styles.panel} aria-labelledby={headingId}>
      <h3 id={headingId} class={styles.heading}>Job</h3>
      <div class={styles.summary}>
        <SeverityLabel severity={jobSeverity(props.job)} text={jobStateLabel(props.job.state)} />
        <Show when={props.job.cancelReason}>{(reason) => <span class={styles.muted}>{cancelReasonLabel(reason())}</span>}</Show>
        <Show when={props.job.assignedBy === "automatic"}><span class={styles.muted}>Assigned automatically</span></Show>
      </div>
      <p class={styles.live} aria-live="polite">{announcement()}</p>

      <Show when={PROGRESS_STATES.has(props.job.state)}>
        <Progress
          label="Progress"
          value={props.job.maxProgressPct}
          valueLabel={`${props.job.maxProgressPct}%`}
          showValue
        />
      </Show>

      <Show when={props.job.startBlockers.length > 0}>
        <ul id={blockersId} class={styles.list} aria-label="Why it can't start">
          <For each={props.job.startBlockers}>
            {(blocker) => (
              <li class={styles.blocker}>
                {/* Rust's message is the whole sentence ("Awaiting material:
                    load Spool #12 on Bay 1."), so it is shown once, as the
                    label. */}
                <SeverityLabel severity="warning" text={blocker.message} />
                <QueueRecoveryButton blocker={blocker} />
              </li>
            )}
          </For>
        </ul>
      </Show>
      <Show when={foreignPrint()}>
        <p class={styles.note}>The printer is running a print farm3d didn't start. farm3d won't control it; use the printer.</p>
      </Show>
      <Show when={props.job.lastFailure}>
        {(failure) => (
          <p class={styles.warning}>
            <SeverityLabel severity="warning" text="Last attempt failed" /> {failureText(failure())}
          </p>
        )}
      </Show>
      <Show when={props.job.state === "outcomeUnknown"}>
        <p class={styles.note}>farm3d couldn't prove how this print ended and has stopped checking. Check the printer, then declare the outcome.</p>
      </Show>
      <Show when={props.job.hostUnreachableSince && !allowed("declareOutcome") ? props.job.hostUnreachableSince : undefined}>
        {(since) => (
          <p class={styles.note}>
            farm3d hasn't been able to read this printer's history since {formatDateTime(since())}. Declare outcome becomes
            available 30 minutes after that.
          </p>
        )}
      </Show>

      <Show when={props.showHostOperation !== false ? activeOperation() : undefined}>
        {(operation) => <LinkedOperation operation={operation()} printerName={props.job.printerSnapshot.name} />}
      </Show>

      <Show when={offered().length > 0}>
        <div class={styles.actions}>
          <For each={offered()}>
            {(item) => (
              <Button
                size="sm"
                variant={item.action === "start" ? "primary" : item.action === "cancel" ? "danger" : "secondary"}
                disabled={pending() || disabledReason(item.action) !== undefined}
                aria-describedby={disabledReason(item.action)}
                onClick={(event: MouseEvent) => perform(item.action, event.currentTarget as HTMLButtonElement)}
              >
                {item.label(props.job)}
              </Button>
            )}
          </For>
        </div>
      </Show>
      <Show when={error()}>
        {(held) => (
          <HostOperationAlert error={held()} fallback="The Job couldn't be updated." printerId={props.job.printerId} onReload={refreshQueue} />
        )}
      </Show>

      <Rows title="Slice" rows={facts()} />
      <Rows title="Reservation" rows={reservation().rows}>
        <Show when={reservation().source}>{(source) => <p class={styles.muted}>Estimate from the {source()}.</p>}</Show>
        <div class={styles.actions}>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => goTo({ version: 1, destination: "spools", selection: { kind: "spool", id: props.job.spoolId } })}
          >
            Open {spoolName(props.job.spoolId)}
          </Button>
        </div>
      </Rows>

      <Suspense fallback={<p class={styles.muted} role="status">Loading the Job's timeline…</p>}>
        <JobTimelineSection job={props.job} />
      </Suspense>

      <StartJobDialog
        open={open() === "start"}
        onOpenChange={dialogOpenChange("start")}
        job={props.job}
        printer={printer()}
        returnFocus={() => triggers.start}
      />
      <AlertDialog
        title="Cancel this Job?"
        description={started()
          ? `farm3d asks ${props.job.printerSnapshot.name} to cancel ${props.job.hostPath ?? "the print"}. A cancelled print can't be resumed.`
          : "The Job ends and its Spool reservation is released. Its Queue Entry closes."}
        open={open() === "cancel"}
        onOpenChange={dialogOpenChange("cancel")}
        returnFocus={() => triggers.cancel}
      >
        <div class={styles.dialogActions}>
          <Button variant="secondary" onClick={() => setOpen(null)}>Keep the Job</Button>
          <Button
            variant="danger"
            onClick={() => {
              setOpen(null);
              void run(() => cancelJob(props.job.id));
            }}
          >
            Cancel Job
          </Button>
        </div>
      </AlertDialog>
      <SettleMaterialDialog open={open() === "settle"} onOpenChange={dialogOpenChange("settle")} job={props.job} returnFocus={() => triggers.settle} />
      <DeclareOutcomeDialog open={open() === "declare"} onOpenChange={dialogOpenChange("declare")} job={props.job} returnFocus={() => triggers.declare} />
    </section>
  );
}

function Rows(props: { title: string; rows: { label: string; value: string; mono?: boolean }[]; children?: JSX.Element }) {
  return (
    <div class={styles.group}>
      <h4 class={styles.subheading}>{props.title}</h4>
      <dl class={styles.rows}>
        <For each={props.rows}>
          {(row) => (
            <>
              <dt>{row.label}</dt>
              <dd class={row.mono ? styles.mono : undefined}>{row.value}</dd>
            </>
          )}
        </For>
      </dl>
      {props.children}
    </div>
  );
}

/** The Job's active Host Operation (D3: the only exit from `staging` or
 *  `starting`), with P6's **Check again** and **Abandon check…** under P6's
 *  own enabling rules. */
function LinkedOperation(props: { operation: HostOperation; printerName: string }) {
  const [checking, setChecking] = createSignal(false);
  const [abandoning, setAbandoning] = createSignal(false);
  const [error, setError] = createSignal<unknown>(null);
  let abandonTrigger: HTMLButtonElement | undefined;
  const record = () => capabilities.forPrinter(props.operation.printerId);
  const label = () => hostOperationLabel(props.operation);
  const uncertain = () => props.operation.state === "uncertain";

  async function checkAgain() {
    if (checking()) return;
    setChecking(true);
    setError(null);
    try {
      await reconcileHostOperation(props.operation.id);
    } catch (e) {
      setError(e);
    } finally {
      setChecking(false);
    }
  }

  return (
    <div class={styles.operation}>
      <div class={styles.summary}>
        <SeverityLabel severity={label().severity} text={label().text} />
        <span class={styles.mono}>{props.operation.hostPath}</span>
      </div>
      <Show when={props.operation.lastAttempt}>
        {(attempt) => <p class={styles.muted}>{inconclusiveReasonText(attempt().reason)}</p>}
      </Show>
      <Show when={uncertain()}>
        <div class={styles.actions}>
          <Button size="sm" variant="secondary" disabled={!canReconcile(props.operation, record()) || checking()} onClick={() => void checkAgain()}>
            Check again
          </Button>
          <Button
            ref={abandonTrigger}
            size="sm"
            variant="secondary"
            disabled={!canAbandon(props.operation, record())}
            onClick={() => setAbandoning(true)}
          >
            Abandon check…
          </Button>
        </div>
        <Show when={!canReconcile(props.operation, record()) ? record() : undefined}>
          {(held) => (
            <p class={styles.muted}>
              farm3d can't check this printer.{" "}
              {capabilityRefusalText(
                reconcileCapability(props.operation),
                held().capabilities[reconcileCapability(props.operation)],
                held().adapterKind,
              )}
            </p>
          )}
        </Show>
        <Show when={!canAbandon(props.operation, record())}>
          <p class={styles.muted}>farm3d can stop checking once it has checked at least once.</p>
        </Show>
      </Show>
      <Show when={error()}>
        {(held) => <HostOperationAlert error={held()} fallback="farm3d couldn't check the printer." printerId={props.operation.printerId} />}
      </Show>
      <AbandonReconciliationDialog
        open={abandoning()}
        onOpenChange={setAbandoning}
        operation={props.operation}
        printerName={props.printerName}
        returnFocus={() => abandonTrigger}
      />
    </div>
  );
}
