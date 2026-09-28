import { createEffect, createSignal, For, Show, type JSX } from "solid-js";
import { Button } from "../design-system";
import { printers } from "../printers/printer-store";
import { jobStateLabel, queueViewLabel, requirementKindLabel, requirementStatusLabel, startBlockerLabel } from "../queue/presentation";
import { queue } from "../queue/queue-store";
import type { Job, NextAutomaticAction, QueueEntry, ReconciliationRequirement } from "../queue/types";
import { SettleMaterialDialog } from "./SettleMaterialDialog";
import { goTo, showQueueEntry } from "./QueueRecoveryButton";
import { SeverityLabel } from "./SeverityLabel";
import type { Severity } from "../host-ops/presentation";
import styles from "./QueuePreview.module.css";

export interface QueuePreviewProps {
  /** Opens a Job in the Monitor dock (spec: "selecting a Job opens
   *  `JobPanel`"). Without it, a Job opens in the Queue. */
  onSelectJob?: (jobId: string) => void;
  /** Shown as **Close** where the preview is an overlay. */
  onClose?: () => void;
}

const NEXT_COUNT = 5;

function entryName(entry: QueueEntry | undefined): string {
  if (!entry) return "A Queue Entry";
  return entry.display.plateLabel ? `${entry.display.modelName} — ${entry.display.plateLabel}` : entry.display.modelName;
}

function printerName(printerId: string): string {
  return printers().find((printer) => printer.id === printerId)?.name ?? "a Printer";
}

/** D6's `NextAutomaticAction`, in words. */
function nextActionText(action: NextAutomaticAction): string {
  switch (action.kind) {
    case "evaluatorNotRunning": return "Automatic dispatch hasn't run yet.";
    case "noAutomaticEntries": return "No Automatic entries are waiting.";
    case "waiting": return `${entryName(queue.entry(action.entryId))} is waiting: ${action.blocker.message}`;
    case "assigned": return `Assigned ${entryName(queue.entry(action.entryId))} to ${printerName(action.printerId)}.`;
  }
}

function jobSeverity(job: Job): Severity {
  if (job.state === "outcomeUnknown" || job.startBlockers.length > 0) return "warning";
  return "info";
}

/** The Monitor dock's default content: the next 5 queued entries, the
 *  active Jobs, open Reconciliation Requirements, and what automatic
 *  dispatch will do next -- all as Rust sent them. */
export function QueuePreview(props: QueuePreviewProps) {
  /** The Job being settled, by id: the dialog reads it live from the
   *  store, so a settlement made elsewhere shows there, and a Job that
   *  leaves the store closes it. */
  const [settlingId, setSettlingId] = createSignal<string | null>(null);
  const settling = () => {
    const id = settlingId();
    return id ? queue.job(id) : undefined;
  };
  createEffect(() => {
    if (settlingId() && !settling()) setSettlingId(null);
  });
  let settleTrigger: HTMLButtonElement | undefined;

  const next = () => queue.entries().filter((entry) => entry.state === "queued").slice(0, NEXT_COUNT);
  /** An `assigned` entry's Job is active: its entry closes only when the
   *  Job ends (D2). */
  const activeJobs = () => queue.entries()
    .filter((entry) => entry.state === "assigned")
    .map((entry) => queue.jobFor(entry.id))
    .filter((job): job is Job => job !== undefined);
  const openJob = (jobId: string) => (props.onSelectJob ? props.onSelectJob(jobId) : showQueueEntry(jobId));

  const requirementAction = (requirement: ReconciliationRequirement) => {
    const job = queue.job(requirement.jobId);
    if (requirement.kind === "materialReconciliation" && job?.allowedActions.includes("settleMaterial")) {
      return (
        <Button
          size="sm"
          variant="secondary"
          onClick={(event: MouseEvent) => {
            settleTrigger = event.currentTarget as HTMLButtonElement;
            setSettlingId(job.id);
          }}
        >
          Settle…
        </Button>
      );
    }
    return <Button size="sm" variant="ghost" onClick={() => openJob(requirement.jobId)}>Open the Job</Button>;
  };

  return (
    <div class={styles.preview}>
      <header class={styles.header}>
        <h2 class={styles.title}>Queue</h2>
        <div class={styles.headerActions}>
          <Button variant="ghost" size="sm" onClick={() => goTo({ version: 1, destination: "queue" })}>Open the Queue</Button>
          <Show when={props.onClose}>
            <Button variant="ghost" size="sm" onClick={() => props.onClose?.()}>Close</Button>
          </Show>
        </div>
      </header>

      <Section title="Next up" empty="Nothing is queued." count={next().length}>
        <ol class={styles.list} aria-label="Next up">
          <For each={next()}>
            {(entry) => (
              <li>
                <button type="button" class={styles.rowButton} onClick={() => showQueueEntry(entry.id)}>
                  <span class={styles.position}>{entry.position}</span>
                  <span class={styles.name}>{entryName(entry)}</span>
                  <Show when={queue.eligibility(entry.id)}>
                    {(summary) => <span class={styles.muted}>{queueViewLabel(summary().verdict)}</span>}
                  </Show>
                </button>
              </li>
            )}
          </For>
        </ol>
      </Section>

      <Section title="Active Jobs" empty="No Job is active." count={activeJobs().length}>
        <ul class={styles.list} aria-label="Active Jobs">
          <For each={activeJobs()}>
            {(job) => (
              <li>
                <button type="button" class={styles.rowButton} onClick={() => openJob(job.id)}>
                  <span class={styles.name}>{job.printerSnapshot.name}</span>
                  <span class={styles.muted}>{entryName(queue.entry(job.queueEntryId))}</span>
                  <SeverityLabel
                    severity={jobSeverity(job)}
                    text={job.startBlockers[0] ? startBlockerLabel(job.startBlockers[0]) : jobStateLabel(job.state)}
                  />
                  <Show when={job.state === "printing" || job.state === "paused"}>
                    <span class={styles.muted}>{job.maxProgressPct}%</span>
                  </Show>
                </button>
              </li>
            )}
          </For>
        </ul>
      </Section>

      <Section title="Needs reconciliation" empty="Nothing to settle." count={queue.requirements().length}>
        <ul class={styles.list} aria-label="Needs reconciliation">
          <For each={queue.requirements()}>
            {(requirement) => {
              const job = () => queue.job(requirement.jobId);
              return (
                <li class={styles.item}>
                  <div class={styles.itemText}>
                    <SeverityLabel severity="warning" text={requirementKindLabel(requirement.kind)} />
                    <span class={styles.muted}>
                      {requirementStatusLabel(requirement.status)}
                      {job() ? ` · ${entryName(queue.entry(job()!.queueEntryId))} on ${job()!.printerSnapshot.name}` : ""}
                    </span>
                  </div>
                  {requirementAction(requirement)}
                </li>
              );
            }}
          </For>
        </ul>
      </Section>

      <Section title="Automatic dispatch">
        <p class={styles.muted}>{nextActionText(queue.nextAutomaticAction())}</p>
      </Section>

      <Show when={settling()}>
        {(job) => (
          <SettleMaterialDialog
            open
            onOpenChange={(open) => !open && setSettlingId(null)}
            job={job()}
            returnFocus={() => settleTrigger}
          />
        )}
      </Show>
    </div>
  );
}

function Section(props: { title: string; empty?: string; count?: number; children: JSX.Element }) {
  return (
    <section class={styles.section} aria-label={props.title}>
      <h3 class={styles.sectionTitle}>{props.title}</h3>
      <Show when={props.count === undefined || props.count > 0} fallback={<p class={styles.muted}>{props.empty}</p>}>
        {props.children}
      </Show>
    </section>
  );
}
