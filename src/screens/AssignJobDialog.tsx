import { createEffect, createResource, createSignal, For, on, Show } from "solid-js";
import { Button, Checkbox, Dialog, RadioGroup, Select } from "../design-system";
import { isCommandError } from "../ipc/client";
import { assignQueueEntry, explainQueueEntry, refreshQueue } from "../queue/queue-store";
import type { Candidate, QueueChange, QueueEntry, SpoolOption } from "../queue/types";
import { formatDateTime } from "../slicing/revision-presentation";
import { formatGrams } from "../spools/weight";
import { HostOperationAlert } from "./HostOperationAlert";
import { QueueRecoveryButton } from "./QueueRecoveryButton";
import styles from "./JobDialogs.module.css";

export interface AssignJobDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  entry: QueueEntry;
  onAssigned?: (change: QueueChange) => void;
  returnFocus?: () => HTMLElement | null | undefined;
}

function candidateLabel(candidate: Candidate): string {
  return `${candidate.rank} · ${candidate.printerName}`;
}

function spoolLabel(option: SpoolOption): string {
  const where = option.loadedOnPrinter ? "loaded" : "not loaded";
  return `Spool #${option.spoolNumber} · ${formatGrams(option.availableMg, 0)} available · ${where}`;
}

/** Why Rust ranked a candidate where it is, from the facts it ranked by. */
function candidateReason(candidate: Candidate): string {
  const parts = [candidate.loadedMatch ? "A matching Spool is loaded" : "No matching Spool is loaded"];
  parts.push(candidate.lastUsedAt ? `last used ${formatDateTime(candidate.lastUsedAt)}` : "not used for a Job yet");
  return parts.join(", ");
}

/** D4's Assign: the ranked candidates from `explain_queue_entry`, in Rust's
 *  order, each with its own Spool choice (defaulting to the Spool Rust
 *  picked), and the manual-facts acknowledgement when Rust says the
 *  Slice's facts are absent. `assign_queue_entry` re-checks everything
 *  inside its transaction; a refusal renders inline. */
export function AssignJobDialog(props: AssignJobDialogProps) {
  const [printerId, setPrinterId] = createSignal<string | undefined>();
  /** Each candidate's own Spool choice, by Printer id. */
  const [spoolChoices, setSpoolChoices] = createSignal<Record<string, string>>({});
  const [acknowledged, setAcknowledged] = createSignal(false);
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<unknown>(null);

  const [explanation, { refetch }] = createResource(
    () => (props.open ? `${props.entry.id}|${props.entry.revision}` : false),
    () => explainQueueEntry(props.entry.id),
  );
  const loaded = () => (explanation.error ? undefined : explanation());
  const candidates = () => loaded()?.candidates ?? [];
  const selected = () => candidates().find((candidate) => candidate.printerId === printerId());

  createEffect(on(() => props.open, (open) => {
    if (!open) return;
    setPrinterId(undefined);
    setSpoolChoices({});
    setAcknowledged(false);
    setError(null);
  }));

  // Rust's rank 1 is the default, and stays chosen across a refetch while
  // it's still a candidate.
  createEffect(() => {
    const list = candidates();
    if (list.length === 0) return;
    if (!list.some((candidate) => candidate.printerId === printerId())) setPrinterId(list[0].printerId);
  });

  const spoolFor = (candidate: Candidate): SpoolOption => {
    const chosen = spoolChoices()[candidate.printerId];
    return candidate.spoolOptions.find((option) => option.spoolId === chosen) ?? candidate.spool;
  };
  const spoolOptions = (candidate: Candidate): SpoolOption[] =>
    candidate.spoolOptions.length > 0 ? candidate.spoolOptions : [candidate.spool];

  const needsAcknowledgement = () => selected()?.manualFactsAcknowledgementRequired === true;
  const canAssign = () => selected() !== undefined && (!needsAcknowledgement() || acknowledged()) && !pending();

  async function onAssign() {
    const candidate = selected();
    if (!candidate || !canAssign()) return;
    setPending(true);
    setError(null);
    try {
      const change = await assignQueueEntry(
        props.entry.id,
        candidate.printerId,
        spoolFor(candidate).spoolId,
        candidate.manualFactsAcknowledgementRequired ? true : undefined,
      );
      props.onOpenChange(false);
      props.onAssigned?.(change);
    } catch (e) {
      setError(e);
      // The world moved: show the ranking as it is now.
      if (isCommandError(e) && e.recovery.includes("RELOAD")) void refetch();
    } finally {
      setPending(false);
    }
  }

  return (
    <Dialog title="Assign to a Printer" open={props.open} onOpenChange={props.onOpenChange} returnFocus={props.returnFocus}>
      <div class={styles.body}>
        <p class={styles.text}>
          farm3d reserves the Spool and stages the file on the Printer. You start the print when the bed is ready.
        </p>
        <Show when={explanation.loading && !loaded()}>
          <p class={styles.note} role="status">Checking which Printers can take this…</p>
        </Show>
        <Show when={explanation.error}>
          <HostOperationAlert error={explanation.error} fallback="Couldn't check which Printers can take this." onRetry={() => void refetch()} />
        </Show>
        <Show when={loaded()}>
          {(held) => (
            <Show
              when={held().candidates.length > 0}
              fallback={
                <>
                  <p class={styles.text}>No Printer can take this entry now.</p>
                  <ul class={styles.list}>
                    <For each={held().blockers}>
                      {(blocker) => (
                        <li class={styles.reason}>
                          {blocker.message} <QueueRecoveryButton blocker={blocker} />
                        </li>
                      )}
                    </For>
                  </ul>
                </>
              }
            >
              <RadioGroup
                label="Printer"
                options={held().candidates.map((candidate) => ({ value: candidate.printerId, label: candidateLabel(candidate) }))}
                value={printerId() ?? ""}
                onChange={(value) => {
                  setPrinterId(value);
                  setAcknowledged(false);
                }}
                disabled={pending()}
              />
              <Show when={selected()}>
                {(candidate) => (
                  <>
                    <p class={styles.note}>{candidateReason(candidate())}</p>
                    <Select<SpoolOption>
                      label="Spool"
                      options={spoolOptions(candidate())}
                      value={spoolFor(candidate())}
                      optionValue={(option) => option.spoolId}
                      optionLabel={spoolLabel}
                      onChange={(option) => setSpoolChoices((current) => ({ ...current, [candidate().printerId]: option.spoolId }))}
                      disabled={pending()}
                    />
                    <Show when={candidate().manualFactsAcknowledgementRequired}>
                      <Checkbox checked={acknowledged()} onChange={setAcknowledged} disabled={pending()}>
                        This Slice is missing facts farm3d normally checks. I confirmed it suits {candidate().printerName}.
                      </Checkbox>
                    </Show>
                  </>
                )}
              </Show>
            </Show>
          )}
        </Show>
        <Show when={error()}>
          {(held) => (
            <HostOperationAlert
              error={held()}
              fallback="The entry couldn't be assigned."
              onOpenJob={() => props.onOpenChange(false)}
              onReload={refreshQueue}
            />
          )}
        </Show>
        <div class={styles.footer}>
          <Show when={needsAcknowledgement() && !acknowledged()}>
            <p class={styles.reason}>Tick the acknowledgement to assign.</p>
          </Show>
          <div class={styles.actions}>
            <Button variant="secondary" onClick={() => props.onOpenChange(false)}>Cancel</Button>
            <Button variant="primary" disabled={!canAssign()} onClick={() => void onAssign()}>Assign</Button>
          </div>
        </div>
      </div>
    </Dialog>
  );
}
