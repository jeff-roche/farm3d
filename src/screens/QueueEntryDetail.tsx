import { Dialog as KDialog } from "@kobalte/core/dialog";
import { createMemo, createResource, createSignal, createUniqueId, For, Show, type JSX } from "solid-js";
import { AlertDialog, Button, RadioGroup, Select, SeverityMarker, Tabs, Timeline } from "../design-system";
import type { TimelineItem } from "../design-system";
import { isCommandError } from "../ipc/client";
import {
  closeReasonLabel,
  copyLabel,
  dispatchPolicyLabel,
  dispatchPreferenceLabel,
  estimateSourceLabel,
  jobEventKindLabel,
  jobStateLabel,
  queueViewLabel,
  requirementKindLabel,
} from "../queue/presentation";
import {
  explainQueueEntry,
  getJobHistory,
  queue,
  removeQueueEntry,
  retryJob,
  updateQueueEntry,
  type UpdateQueueEntryPatch,
} from "../queue/queue-store";
import type { Blocker, Candidate, DispatchPolicy, DispatchPreference, QueueEntry, RequirementStatus } from "../queue/types";
import {
  estimateRows,
  FACT_KEYS,
  FACT_LABELS,
  factValueText,
  formatDateTime,
  revisionKindLabel,
} from "../slicing/revision-presentation";
import { loadSliceRevision } from "../slicing/slicing-store";
import { formatGrams } from "../spools/weight";
import { goTo, QueueRecoveryButton, showQueueEntry } from "./QueueRecoveryButton";
import styles from "./QueueEntryDetail.module.css";

export interface QueueEntryDetailProps {
  entry: QueueEntry;
  mode: "inline" | "overlay";
  onClose: () => void;
  /** Opens assignment for an entry (the Job dialog). Without it, **Assign…**
   *  shows but stays disabled with its reason. */
  onAssign?: (entryId: string) => void;
}

const POLICIES: DispatchPolicy[] = ["manual", "recommended", "automatic"];
const PREFERENCES: DispatchPreference[] = ["loadedFirst", "leastRecentlyUsed"];
const ASSIGN_LATER_REASON = "Assigning arrives in a later version.";
const REQUIREMENT_STATUS_LABEL: Record<RequirementStatus, string> = {
  pending: "Pending",
  deferred: "Deferred",
  resolved: "Resolved",
};

function entryTitle(entry: QueueEntry): string {
  return entry.display.plateLabel ? `${entry.display.modelName} — ${entry.display.plateLabel}` : entry.display.modelName;
}

function errorText(error: unknown, fallback: string): string {
  return isCommandError(error) ? error.message : fallback;
}

/** The Queue Entry dock (spec "Frontend architecture"): Dispatch, Artifact,
 *  and History. Inline beside the table, or a modal overlay when the
 *  workspace is too narrow for both, like `PrinterDetailDock`. */
export function QueueEntryDetail(props: QueueEntryDetailProps) {
  const content = () => (
    <DockContent entry={props.entry} overlay={props.mode === "overlay"} onClose={props.onClose} onAssign={props.onAssign} />
  );
  return (
    <Show
      when={props.mode === "overlay"}
      fallback={<aside class={styles.inline} aria-label={entryTitle(props.entry)}>{content()}</aside>}
    >
      <KDialog open onOpenChange={(open) => !open && props.onClose()}>
        <KDialog.Portal>
          <KDialog.Overlay class={styles.overlay} />
          <KDialog.Content class={styles.overlayContent} onCloseAutoFocus={(event) => event.preventDefault()}>
            {content()}
          </KDialog.Content>
        </KDialog.Portal>
      </KDialog>
    </Show>
  );
}

function DockContent(props: Omit<QueueEntryDetailProps, "mode"> & { overlay: boolean }) {
  const [tab, setTab] = createSignal("dispatch");
  const subtitle = () => {
    const parts: string[] = [];
    if (props.entry.copyCount > 1) parts.push(copyLabel(props.entry));
    if (props.entry.position !== null) parts.push(`Position ${props.entry.position}`);
    else if (props.entry.closeReason) parts.push(closeReasonLabel(props.entry.closeReason));
    return parts.join(" · ");
  };
  return (
    <div class={styles.dock}>
      <header class={styles.header}>
        <div>
          <Show when={props.overlay} fallback={<h2 class={styles.title}>{entryTitle(props.entry)}</h2>}>
            <KDialog.Title class={styles.title}>{entryTitle(props.entry)}</KDialog.Title>
          </Show>
          <p class={styles.subtitle}>{subtitle()}</p>
        </div>
        <Button variant="ghost" class={styles.close} onClick={props.onClose}>Close</Button>
      </header>
      <Tabs
        value={tab()}
        onChange={setTab}
        // Getters: Kobalte mounts only the selected tab, so the Artifact
        // and History tabs load nothing until they're opened.
        items={[
          { value: "dispatch", label: "Dispatch", get content() { return <DispatchTab entry={props.entry} onAssign={props.onAssign} />; } },
          { value: "artifact", label: "Artifact", get content() { return <ArtifactTab entry={props.entry} />; } },
          { value: "history", label: "History", get content() { return <HistoryTab entry={props.entry} />; } },
        ]}
      />
    </div>
  );
}

function Section(props: { title: string; children: JSX.Element }) {
  const id = createUniqueId();
  return (
    <section class={styles.section} aria-labelledby={id}>
      <h3 id={id} class={styles.sectionTitle}>{props.title}</h3>
      {props.children}
    </section>
  );
}

function BlockerLine(props: { blocker: Blocker; onAssignManually?: () => void }) {
  return (
    <li class={styles.blockerLine}>
      <SeverityMarker severity="warning" label={props.blocker.message} />
      <Show when={props.blocker.detail}>{(detail) => <span class={styles.muted}>{detail()}</span>}</Show>
      <QueueRecoveryButton blocker={props.blocker} onAssignManually={props.onAssignManually} />
    </li>
  );
}

/** Why a candidate ranks where it does, from the facts Rust ranked it by. */
function candidateReasons(candidate: Candidate): string[] {
  const reasons: string[] = [];
  const spool = candidate.spool;
  reasons.push(spool.loadedOnPrinter
    ? `Spool #${spool.spoolNumber} is loaded`
    : `Spool #${spool.spoolNumber}, ${formatGrams(spool.availableMg, 0)} available`);
  reasons.push(candidate.lastUsedAt ? `Last used ${formatDateTime(candidate.lastUsedAt)}` : "Not used for a Job yet");
  if (candidate.manualFactsAcknowledgementRequired) reasons.push("Needs a manual-facts acknowledgement");
  return reasons;
}

function DispatchTab(props: { entry: QueueEntry; onAssign?: (entryId: string) => void }) {
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [confirmingRemove, setConfirmingRemove] = createSignal(false);
  let removeTrigger: HTMLButtonElement | undefined;
  const assignReasonId = createUniqueId();
  const summary = () => queue.eligibility(props.entry.id);
  const job = () => queue.jobFor(props.entry.id);
  const allowed = (action: QueueEntry["allowedActions"][number]) => props.entry.allowedActions.includes(action);

  // Refetched whenever the entry or its summary changes, so the
  // explanation never lags the table (the summary is the full set, D6).
  const explainKey = () => (props.entry.state === "queued"
    ? `${props.entry.id}|${props.entry.revision}|${JSON.stringify(summary() ?? null)}`
    : false);
  const [explanation, { refetch }] = createResource(explainKey, () => explainQueueEntry(props.entry.id));
  const loaded = () => (explanation.error ? undefined : explanation());

  async function run(action: () => Promise<unknown>, fallback: string) {
    if (pending()) return;
    setPending(true);
    setError(null);
    try {
      await action();
    } catch (e) {
      setError(errorText(e, fallback));
    } finally {
      setPending(false);
    }
  }

  const update = (patch: UpdateQueueEntryPatch) =>
    void run(() => updateQueueEntry(props.entry.id, props.entry.revision, patch), "The entry couldn't be updated.");

  /** `ASSIGN_MANUALLY`: switch the entry to Manual, then open Assign. */
  const assignManually = () => void run(async () => {
    if (props.entry.policy !== "manual") await updateQueueEntry(props.entry.id, props.entry.revision, { policy: "manual" });
    props.onAssign?.(props.entry.id);
  }, "The entry couldn't be switched to Manual.");

  const retryable = () => {
    const held = job();
    return props.entry.state === "closed" && held?.allowedActions.includes("retry") ? held : undefined;
  };
  const retry = () => void run(async () => {
    const held = retryable();
    if (!held) return;
    const change = await retryJob(held.id);
    const created = change.entries[0];
    if (created) showQueueEntry(created.id);
  }, "The Job couldn't be retried.");

  const pinnedToPrinter = () => props.entry.requiresManualPrinterSelection || props.entry.manualPrinterId !== null;

  return (
    <div class={styles.tab}>
      <Show when={props.entry.state === "queued"}>
        <Show when={summary()}>
          {(held) => (
            <SeverityMarker
              severity={held().verdict === "blocked" ? "warning" : held().verdict === "ready" ? "resolved" : "info"}
              label={queueViewLabel(held().verdict)}
            />
          )}
        </Show>
        <Show when={explanation.error}>
          <div class={styles.inlineError}>
            <p class={styles.error} role="alert">{errorText(explanation.error, "Couldn't check which Printers can take this.")}</p>
            <Button variant="secondary" size="sm" onClick={() => void refetch()}>Retry</Button>
          </div>
        </Show>
        <Show when={loaded()} fallback={<Show when={!explanation.error}><p class={styles.muted} role="status">Checking which Printers can take this…</p></Show>}>
          {(held) => (
            <>
              <Show when={held().blockers.length > 0}>
                <Section title="Blockers">
                  <ul class={styles.list}>
                    <For each={held().blockers}>
                      {(blocker) => <BlockerLine blocker={blocker} onAssignManually={props.onAssign ? assignManually : undefined} />}
                    </For>
                  </ul>
                </Section>
              </Show>
              <Section title="Printers">
                <Show
                  when={held().candidates.length > 0}
                  fallback={<p class={styles.muted}>No Printer can take this entry now.</p>}
                >
                  <ol class={styles.list} aria-label="Ranked Printers">
                    <For each={held().candidates}>
                      {(candidate) => (
                        <li class={styles.candidate}>
                          <span class={styles.rank}>{candidate.rank}</span>
                          <span class={styles.candidateName}>{candidate.printerName}</span>
                          <span class={styles.reasons}>{candidateReasons(candidate).join(" · ")}</span>
                        </li>
                      )}
                    </For>
                  </ol>
                </Show>
                <Show when={held().printers.some((printer) => !printer.eligible)}>
                  <ul class={styles.list} aria-label="Printers that can't take it">
                    <For each={held().printers.filter((printer) => !printer.eligible)}>
                      {(printer) => (
                        <li class={styles.ineligible}>
                          <span class={styles.candidateName}>{printer.printerName}</span>
                          <For each={printer.blockers}>
                            {(blocker) => <span class={styles.muted}>{blocker.message}</span>}
                          </For>
                        </li>
                      )}
                    </For>
                  </ul>
                </Show>
              </Section>
            </>
          )}
        </Show>
      </Show>

      <Show when={props.entry.state !== "queued" && job()}>
        {(held) => (
          <Section title="Job">
            <p class={styles.text}>
              {jobStateLabel(held().state)} on {held().printerSnapshot.name}
            </p>
          </Section>
        )}
      </Show>

      <Section title="Dispatch">
        <RadioGroup
          label="Dispatch Policy"
          options={POLICIES.map((policy) => ({
            value: policy,
            label: dispatchPolicyLabel(policy),
            disabled: policy !== "manual" && pinnedToPrinter(),
          }))}
          value={props.entry.policy}
          onChange={(value) => update({ policy: value as DispatchPolicy })}
          disabled={!allowed("update") || pending()}
        />
        <Show when={pinnedToPrinter() && allowed("update")}>
          <p class={styles.muted}>This entry names its Printer, so it stays Manual.</p>
        </Show>
        <Select<DispatchPreference>
          label="Dispatch preference"
          options={PREFERENCES}
          value={props.entry.preference}
          onChange={(value) => update({ preference: value })}
          optionLabel={dispatchPreferenceLabel}
          disabled={!allowed("update") || pending()}
        />
      </Section>

      <div class={styles.actions}>
        <Show when={allowed("assign")}>
          <Button
            variant="primary"
            disabled={!props.onAssign || pending()}
            aria-describedby={props.onAssign ? undefined : assignReasonId}
            onClick={() => props.onAssign?.(props.entry.id)}
          >
            Assign…
          </Button>
        </Show>
        <Show when={allowed("remove")}>
          <Button ref={removeTrigger} variant="secondary" disabled={pending()} onClick={() => setConfirmingRemove(true)}>
            Remove…
          </Button>
        </Show>
        <Show when={retryable()}>
          <Button variant="secondary" disabled={pending()} onClick={retry}>Retry</Button>
        </Show>
      </div>
      <Show when={allowed("assign") && !props.onAssign}>
        <p id={assignReasonId} class={styles.muted}>{ASSIGN_LATER_REASON}</p>
      </Show>
      <Show when={error()}>{(message) => <p class={styles.error} role="alert">{message()}</p>}</Show>
      <AlertDialog
        title="Remove this Queue Entry?"
        description={`${entryTitle(props.entry)} leaves the Queue and can't be put back. Add it to the Queue again to print it.`}
        open={confirmingRemove()}
        onOpenChange={setConfirmingRemove}
        returnFocus={() => removeTrigger}
      >
        <div class={styles.dialogActions}>
          <Button variant="secondary" onClick={() => setConfirmingRemove(false)}>Keep it</Button>
          <Button
            variant="danger"
            onClick={() => {
              setConfirmingRemove(false);
              void run(() => removeQueueEntry(props.entry.id, props.entry.revision), "The entry couldn't be removed.");
            }}
          >
            Remove
          </Button>
        </div>
      </AlertDialog>
    </div>
  );
}

function Rows(props: { rows: { label: string; value: string }[] }) {
  return (
    <dl class={styles.rows}>
      <For each={props.rows}>
        {(row) => (
          <>
            <dt>{row.label}</dt>
            <dd>{row.value}</dd>
          </>
        )}
      </For>
    </dl>
  );
}

function ArtifactTab(props: { entry: QueueEntry }) {
  const [revision] = createResource(() => props.entry.sliceRevisionId, (id) => loadSliceRevision(id));
  const loaded = () => (revision.error ? undefined : revision());
  const estimate = () =>
    `${formatGrams(props.entry.estimate.amountMg, 1)} (${estimateSourceLabel(props.entry.estimate.source)})`;
  return (
    <div class={styles.tab}>
      <Rows rows={[
        { label: "Material estimate", value: estimate() },
        { label: "Target", value: props.entry.display.targetLabel },
      ]} />
      <Show when={revision.error}>
        <p class={styles.error} role="alert">
          {isCommandError(revision.error) && revision.error.code === "NOT_FOUND"
            ? "This Slice Revision was deleted."
            : "This Slice Revision couldn't be loaded."}
        </p>
      </Show>
      <Show when={loaded()} fallback={<Show when={!revision.error}><p class={styles.muted} role="status">Loading the Slice Revision…</p></Show>}>
        {(held) => (
          <>
            <Section title="Slice Revision">
              <Rows rows={[
                { label: "Kind", value: revisionKindLabel(held().kind) },
                { label: "Created", value: formatDateTime(held().createdAt) },
              ]} />
            </Section>
            <Section title="Facts">
              <Rows rows={FACT_KEYS.map((key) => ({ label: FACT_LABELS[key], value: factValueText(held().facts, key) ?? "Not provided" }))} />
            </Section>
            <Show when={held().estimates ?? held().claimedEstimates}>
              {(estimates) => (
                <Section title={held().kind === "external" ? "What the file says (not verified)" : "Slice estimates"}>
                  <Rows rows={estimateRows(estimates())} />
                </Section>
              )}
            </Show>
            <div class={styles.actions}>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => goTo({ version: 1, destination: "library", selection: { kind: "model", id: held().modelId } })}
              >
                Open the Model
              </Button>
            </div>
          </>
        )}
      </Show>
    </div>
  );
}

function HistoryTab(props: { entry: QueueEntry }) {
  const [history] = createResource(() => props.entry.jobId ?? false, (jobId) => getJobHistory(jobId));
  const loaded = () => (history.error ? undefined : history());
  // The Job history's lineage includes closed entries the store may not
  // hold; until it loads (or with no Job), the store's own entries do.
  const lineage = createMemo(() => {
    const fromHistory = loaded()?.lineage;
    const source = fromHistory ?? [...queue.entries(), ...queue.history()].filter((e) => e.lineageId === props.entry.lineageId);
    return [...source].sort((a, b) => a.copyIndex - b.copyIndex || a.createdAt.localeCompare(b.createdAt));
  });
  const origin = () => (props.entry.originEntryId ? props.entry.originEntryId : undefined);
  const timeline = (): TimelineItem[] => (loaded()?.events ?? []).map((event) => ({
    id: event.id,
    at: event.at,
    title: jobEventKindLabel(event.kind),
    marker: event.kind === "assigned" ? "muted" : "default",
  }));

  return (
    <div class={styles.tab}>
      <Section title="Job">
        <Show when={props.entry.jobId} fallback={<p class={styles.muted}>No Job yet: this entry hasn't been assigned.</p>}>
          <Show when={history.error}>
            <p class={styles.error} role="alert">{errorText(history.error, "The Job's history couldn't be loaded.")}</p>
          </Show>
          <Show when={loaded()} fallback={<Show when={!history.error}><p class={styles.muted} role="status">Loading the Job's history…</p></Show>}>
            {(held) => (
              <>
                <Timeline label={`Job timeline for ${entryTitle(props.entry)}`} items={timeline()} />
                <Show when={held().requirements.length > 0}>
                  <ul class={styles.list}>
                    <For each={held().requirements}>
                      {(requirement) => (
                        <li>
                          <SeverityMarker
                            severity={requirement.status === "resolved" ? "resolved" : "warning"}
                            label={`${requirementKindLabel(requirement.kind)}: ${REQUIREMENT_STATUS_LABEL[requirement.status]}`}
                          />
                        </li>
                      )}
                    </For>
                  </ul>
                </Show>
              </>
            )}
          </Show>
        </Show>
      </Section>
      <Section title="Lineage">
        <Show when={origin()}>
          {(id) => (
            <Button variant="ghost" size="sm" onClick={() => showQueueEntry(id())}>
              {props.entry.originKind === "release" ? "Replaces an earlier entry" : "Retry of an earlier entry"}
            </Button>
          )}
        </Show>
        <Show when={lineage().length > 1} fallback={<p class={styles.muted}>No other entries in this lineage.</p>}>
          <ul class={styles.list} aria-label="Lineage">
            <For each={lineage()}>
              {(sibling) => (
                <li class={styles.lineageItem}>
                  <Show when={sibling.id !== props.entry.id} fallback={<span aria-current="true">{copyLabel(sibling)} (this entry)</span>}>
                    <Button variant="ghost" size="sm" onClick={() => showQueueEntry(sibling.id)}>{copyLabel(sibling)}</Button>
                  </Show>
                  <span class={styles.muted}>
                    {sibling.state === "closed" && sibling.closeReason
                      ? closeReasonLabel(sibling.closeReason)
                      : sibling.position !== null ? `Position ${sibling.position}` : ""}
                  </span>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </Section>
    </div>
  );
}
