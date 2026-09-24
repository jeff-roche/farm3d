import { createEffect, createMemo, createSignal, For, on, Show } from "solid-js";
import { createStore } from "solid-js/store";
import { Button, ColorSwatch, Dialog, Select, TextField } from "../design-system";
import { isCommandError } from "../ipc/client";
import { archivePrinter, printers } from "../printers/printer-store";
import type { MaterialSlot, ResolvedPrinter } from "../printers/types";
import { materialLabel } from "../spools/materials";
import { spoolState } from "../spools/spool-store";
import type { SpoolDisposition } from "../generated/contracts/domain/SpoolDisposition";
import type { SpoolDispositionInput } from "../generated/contracts/domain/SpoolDispositionInput";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import styles from "./ArchivePrinterDialog.module.css";

export interface ArchivePrinterDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  printer: ResolvedPrinter;
  /** From `lifecycleEligibility(...).loadedSpools` -- never re-derived here. */
  loadedSpools: SpoolRecord[];
  onArchived?: () => void;
}

type DispositionKind = SpoolDisposition["kind"];
const KIND_OPTIONS: DispositionKind[] = ["storage", "slot", "markEmpty"];
/** "Mark empty" is a lifecycle action on an `active` Spool only. An empty
 *  Spool can still be loaded (D5), so it can appear here. */
const kindOptionsFor = (spool: SpoolRecord): DispositionKind[] =>
  spool.lifecycle === "active" ? KIND_OPTIONS : KIND_OPTIONS.filter((kind) => kind !== "markEmpty");
const KIND_LABELS: Record<DispositionKind, string> = {
  storage: "Storage",
  slot: "Another Printer's slot",
  markEmpty: "Mark empty (used up)",
};

interface RowState {
  kind?: DispositionKind;
  storageLabel: string;
  slotId?: string;
  displacedLabel: string;
  /** From a `CONFLICT`'s `details.currentOccupantSpoolId`: the slot's real
   *  occupant, which the Printer list may not show yet. */
  conflictOccupantId?: string | null;
  error?: string;
}

interface SlotChoice {
  printer: ResolvedPrinter;
  slot: MaterialSlot;
}

function spoolLabel(spool: SpoolRecord): string {
  return `#${spool.spoolNumber} ${materialLabel(spool.materialFamily, spool.materialOther)} ${spool.colorName}`;
}

const findSpool = (id: string | null | undefined) => (id ? spoolState.spools.find((s) => s.id === id) : undefined);

/** D10 / spec "Archive flow": one row per loaded Spool, each choosing
 *  Storage, another Printer's slot (with Move's swap handling), or Mark
 *  empty. Submits every disposition in one `archivePrinter` call, which
 *  rolls back entirely on failure -- so a `CONFLICT` is shown on the row it
 *  concerns and the dialog stays open for a retry. */
export function ArchivePrinterDialog(props: ArchivePrinterDialogProps) {
  const [rows, setRows] = createStore<Record<string, RowState>>({});
  const [submitting, setSubmitting] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  createEffect(on(() => props.open, (open) => {
    if (!open) return;
    setError(null);
    setRows(Object.fromEntries(props.loadedSpools.map((s) => [s.id, { storageLabel: "", displacedLabel: "" }])));
  }));

  const slotChoices = createMemo<SlotChoice[]>(() =>
    printers()
      .filter((p) => p.id !== props.printer.id && !p.archivedAt)
      .flatMap((printer) => printer.materialSlots.map((slot) => ({ printer, slot }))),
  );
  const choiceFor = (slotId: string | undefined) => slotChoices().find((c) => c.slot.id === slotId);

  const occupantIdOf = (row: RowState): string | null => {
    if (row.conflictOccupantId !== undefined) return row.conflictOccupantId;
    return choiceFor(row.slotId)?.slot.occupantSpoolId ?? null;
  };

  function choiceLabel(choice: SlotChoice): string {
    const feeder = choice.slot.feederLabel ? ` (${choice.slot.feederLabel})` : "";
    const occupant = findSpool(choice.slot.occupantSpoolId);
    return `${choice.printer.name} — ${choice.slot.name}${feeder} — ${occupant ? `occupied by ${spoolLabel(occupant)}` : "empty"}`;
  }

  /** Two rows can't both target one slot: each row's options leave out
   *  slots another row has already taken. */
  const optionsFor = (spoolId: string) => {
    const taken = new Set(
      Object.entries(rows).filter(([id, row]) => id !== spoolId && row.kind === "slot").map(([, row]) => row.slotId),
    );
    return slotChoices().filter((c) => !taken.has(c.slot.id)).map((c) => c.slot.id);
  };

  const rowComplete = (row: RowState | undefined) => {
    if (!row?.kind) return false;
    if (row.kind !== "slot") return true;
    if (!row.slotId) return false;
    return occupantIdOf(row) === null || row.displacedLabel.trim() !== "";
  };
  const canSubmit = () => !submitting() && props.loadedSpools.every((s) => rowComplete(rows[s.id]));

  function patchRow(spoolId: string, patch: Partial<RowState>) {
    setRows(spoolId, { ...patch, error: undefined });
  }

  function disposition(row: RowState): SpoolDisposition {
    const label = (value: string) => value.trim() || null;
    if (row.kind === "storage") return { kind: "storage", storageLabel: label(row.storageLabel) };
    if (row.kind === "markEmpty") return { kind: "markEmpty", storageLabel: label(row.storageLabel) };
    const occupantId = occupantIdOf(row);
    return {
      kind: "slot",
      slotId: row.slotId!,
      expectedOccupantSpoolId: occupantId,
      ...(occupantId ? { displacedStorageLabel: label(row.displacedLabel) } : {}),
    };
  }

  function showFailure(e: unknown) {
    const details = isCommandError(e) ? e.details : undefined;
    const message = isCommandError(e) ? e.message : "This Printer could not be archived.";
    const bySlot = props.loadedSpools.find((s) => details?.slotId !== undefined && rows[s.id]?.slotId === details.slotId);
    const bySpool = props.loadedSpools.find((s) => s.id === details?.entityId);
    const target = bySlot ?? bySpool;
    if (!target) {
      setError(message);
      return;
    }
    const occupant = details?.currentOccupantSpoolId;
    setRows(target.id, {
      error: message,
      ...(bySlot && isCommandError(e) && e.code === "CONFLICT"
        ? { conflictOccupantId: typeof occupant === "string" ? occupant : null }
        : {}),
    });
  }

  async function onSubmit() {
    if (!canSubmit()) return;
    const dispositions: SpoolDispositionInput[] = props.loadedSpools.map((s) => ({
      spoolId: s.id,
      expectedSpoolRevision: findSpool(s.id)?.revision ?? s.revision,
      disposition: disposition(rows[s.id]),
    }));
    setSubmitting(true);
    setError(null);
    try {
      await archivePrinter(props.printer.id, dispositions);
      props.onOpenChange(false);
      props.onArchived?.();
    } catch (e) {
      showFailure(e);
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Dialog title={`Archive ${props.printer.name}`} open={props.open} onOpenChange={props.onOpenChange}>
      <div class={styles.body}>
        <p class={styles.note}>Choose where each loaded Spool goes. Nothing moves unless the whole archive succeeds.</p>
        <For each={props.loadedSpools}>
          {(spool) => {
            const row = () => rows[spool.id] ?? { storageLabel: "", displacedLabel: "" };
            const slotName = () => props.printer.materialSlots.find((s) => s.occupantSpoolId === spool.id)?.name;
            /** The slot's occupant id, when this row targets an occupied
             *  slot -- the swap block keys off this, not the Spool record,
             *  so a label is still asked for when the record isn't loaded. */
            const occupantId = () => (row().kind === "slot" && row().slotId ? occupantIdOf(row()) : null);
            const headingId = `archive-row-${spool.id}`;
            return (
              <div role="group" aria-labelledby={headingId} class={styles.row}>
                <p id={headingId} class={styles.rowTitle}>
                  <ColorSwatch hex={spool.colorHex ?? null} name={spool.colorName} size="sm" />
                  <span>{spoolLabel(spool)}</span>
                  <Show when={slotName()}>{(name) => <span class={styles.muted}>in {name()}</span>}</Show>
                </p>
                <Select
                  label={`Where #${spool.spoolNumber} goes`}
                  options={kindOptionsFor(spool)}
                  optionLabel={(kind) => KIND_LABELS[kind]}
                  value={row().kind}
                  placeholder="Choose…"
                  onChange={(kind) =>
                    kind !== row().kind &&
                    patchRow(spool.id, { kind, slotId: undefined, conflictOccupantId: undefined, displacedLabel: "" })}
                />
                <Show when={row().kind === "storage" || row().kind === "markEmpty"}>
                  <TextField
                    label="Storage label (optional)"
                    value={row().storageLabel}
                    onChange={(storageLabel) => patchRow(spool.id, { storageLabel })}
                  />
                </Show>
                <Show when={row().kind === "slot"}>
                  <Select
                    label="Destination slot"
                    options={optionsFor(spool.id)}
                    optionLabel={(id) => {
                      const choice = choiceFor(id);
                      return choice ? choiceLabel(choice) : id;
                    }}
                    value={row().slotId}
                    placeholder="Choose a slot"
                    onChange={(slotId) => slotId !== row().slotId && patchRow(spool.id, { slotId, conflictOccupantId: undefined })}
                  />
                  <Show when={occupantId()}>
                    {(id) => (
                      <div class={styles.swap}>
                        <p class={styles.swapLine}>
                          Swap: {findSpool(id()) ? spoolLabel(findSpool(id())!) : "the Spool in that slot"} goes to storage
                        </p>
                        <TextField
                          label="Displaced Spool storage label"
                          value={row().displacedLabel}
                          onChange={(displacedLabel) => patchRow(spool.id, { displacedLabel })}
                          required
                        />
                      </div>
                    )}
                  </Show>
                </Show>
                <Show when={row().error}>
                  {(message) => <p class={styles.error} role="alert">{message()}</p>}
                </Show>
              </div>
            );
          }}
        </For>
        <Show when={error()}>
          {(message) => <p class={styles.error} role="alert">{message()}</p>}
        </Show>
        <div class={styles.actions}>
          <Button variant="secondary" onClick={() => props.onOpenChange(false)}>Cancel</Button>
          <Button variant="danger" disabled={!canSubmit()} onClick={() => void onSubmit()}>
            {submitting() ? "Archiving…" : "Archive"}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
