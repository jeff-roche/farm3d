import { createEffect, createMemo, createSignal, on, Show } from "solid-js";
import { Button, Combobox, Dialog, RadioGroup, Select, TextField } from "../design-system";
import { isCommandError } from "../ipc/client";
import { materialLabel } from "../spools/materials";
import { moveSpool, spoolState } from "../spools/spool-store";
import { printers } from "../printers/printer-store";
import type { MaterialSlot } from "../generated/contracts/domain/MaterialSlot";
import type { MoveDestination } from "../generated/contracts/domain/MoveDestination";
import type { MoveSpoolData } from "../generated/contracts/command/MoveSpoolData";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import styles from "./MoveSpoolDialog.module.css";

export interface MoveSpoolDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  spool: SpoolRecord;
  /** Task 11: pre-targets a specific Printer/slot (e.g. "Load…" from the
   *  Setup tab), so the dialog opens straight into that choice instead of
   *  Storage. */
  presetDestination?: { printerId: string; slotId: string };
  onMoved?: (result: MoveSpoolData) => void;
}

type DestinationKind = "storage" | "printer";

function spoolLabel(spool: SpoolRecord): string {
  return `#${spool.spoolNumber} ${materialLabel(spool.materialFamily, spool.materialOther)} ${spool.colorName}`;
}

/** D6 §Components: destination is Storage (a label combobox of existing
 *  labels) or a Printer, then a Slot; an occupied slot shows a swap line
 *  and requires a label for the displaced Spool. Submits through the
 *  optimistic, rejecting `moveSpool` (unlike most spool-store mutations,
 *  which report to the banner instead) -- a `CONFLICT` is caught and shown
 *  inline, re-reading the slot's now-current occupant from `spoolState`
 *  (which `moveSpool` itself refetches before rethrowing) rather than
 *  trusting the stale occupant this dialog started with. */
export function MoveSpoolDialog(props: MoveSpoolDialogProps) {
  const [destKind, setDestKind] = createSignal<DestinationKind>("storage");
  const [storageLabel, setStorageLabel] = createSignal("");
  const [printerId, setPrinterId] = createSignal<string | undefined>(undefined);
  const [slotId, setSlotId] = createSignal<string | undefined>(undefined);
  const [displacedLabel, setDisplacedLabel] = createSignal("");
  const [submitting, setSubmitting] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  /** D6/errors table: set from a caught `CONFLICT`'s
   *  `details.currentOccupantSpoolId` -- the slot's occupant at *select*
   *  time (`occupant()`, from `selectedSlot()`) is stale once that's
   *  happened, since nothing here re-reads the racer's own slot write. */
  const [conflictOccupantId, setConflictOccupantId] = createSignal<string | null>(null);

  createEffect(on(() => props.open, (open) => {
    if (!open) return;
    setError(null);
    setConflictOccupantId(null);
    setStorageLabel("");
    setDisplacedLabel("");
    if (props.presetDestination) {
      setDestKind("printer");
      setPrinterId(props.presetDestination.printerId);
      setSlotId(props.presetDestination.slotId);
    } else {
      setDestKind("storage");
      setPrinterId(undefined);
      setSlotId(undefined);
    }
  }));

  const availablePrinters = createMemo(() => printers().filter((p) => !p.archivedAt));
  const storageLabelOptions = createMemo<string[]>(() => {
    const labels = new Set<string>();
    for (const s of spoolState.spools) {
      if (s.location.kind === "storage" && s.location.storageLabel) labels.add(s.location.storageLabel);
    }
    return [...labels].sort();
  });

  const selectedPrinter = createMemo(() => availablePrinters().find((p) => p.id === printerId()));
  const slots = createMemo<MaterialSlot[]>(() => selectedPrinter()?.materialSlots ?? []);
  const selectedSlot = createMemo(() => slots().find((s) => s.id === slotId()));
  const occupant = createMemo<SpoolRecord | undefined>(() => {
    const slot = selectedSlot();
    if (!slot?.occupantSpoolId || slot.occupantSpoolId === props.spool.id) return undefined;
    return spoolState.spools.find((s) => s.id === slot.occupantSpoolId);
  });
  /** The occupant to show in the swap line: a fresher `CONFLICT` occupant
   *  once one has been reported, otherwise the selected slot's own. */
  const displayedOccupant = createMemo<SpoolRecord | undefined>(() => {
    const conflictId = conflictOccupantId();
    if (conflictId) return spoolState.spools.find((s) => s.id === conflictId);
    return occupant();
  });

  function slotOptionLabel(slot: MaterialSlot): string {
    const occ = slot.occupantSpoolId ? spoolState.spools.find((s) => s.id === slot.occupantSpoolId) : undefined;
    const feeder = slot.feederLabel ? ` (${slot.feederLabel})` : "";
    return `${slot.name}${feeder} — ${occ ? `occupied by ${spoolLabel(occ)}` : "empty"}`;
  }

  const canSubmit = createMemo(() => {
    if (submitting()) return false;
    if (destKind() === "storage") return true;
    if (!slotId()) return false;
    if (displayedOccupant() && displacedLabel().trim().length === 0) return false;
    return true;
  });

  async function onSubmit() {
    if (!canSubmit()) return;
    const slot = selectedSlot();
    const destination: MoveDestination = destKind() === "storage"
      ? { kind: "storage", storageLabel: storageLabel().trim() || null }
      : {
          kind: "slot",
          slotId: slot!.id,
          expectedOccupantSpoolId: conflictOccupantId() ?? slot!.occupantSpoolId ?? null,
          ...(displayedOccupant() ? { displacedStorageLabel: displacedLabel().trim() || null } : {}),
        };
    setSubmitting(true);
    setError(null);
    try {
      const result = await moveSpool({
        spoolId: props.spool.id,
        expectedSpoolRevision: props.spool.revision,
        destination,
      });
      props.onOpenChange(false);
      props.onMoved?.(result);
    } catch (e) {
      if (isCommandError(e) && e.code === "CONFLICT") {
        const occupantId = e.details?.currentOccupantSpoolId;
        setConflictOccupantId(typeof occupantId === "string" ? occupantId : null);
        setError(e.message);
      } else {
        setError(isCommandError(e) ? e.message : "This Spool could not be moved.");
      }
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Dialog title="Move Spool" open={props.open} onOpenChange={props.onOpenChange}>
      <div class={styles.body}>
        <RadioGroup
          label="Destination"
          options={[{ value: "storage", label: "Storage" }, { value: "printer", label: "Printer" }]}
          value={destKind()}
          onChange={(v) => setDestKind(v as DestinationKind)}
        />
        <Show when={destKind() === "storage"}>
          <Combobox
            label="Storage label (optional)"
            options={storageLabelOptions()}
            value={storageLabel()}
            onChange={setStorageLabel}
            onInputChange={setStorageLabel}
            placeholder="e.g. Shelf A2"
          />
        </Show>
        <Show when={destKind() === "printer"}>
          <Select
            label="Printer"
            options={availablePrinters().map((p) => p.id)}
            value={printerId()}
            onChange={(id) => { setPrinterId(id); setSlotId(undefined); }}
            optionLabel={(id) => availablePrinters().find((p) => p.id === id)?.name ?? id}
          />
          <Show when={selectedPrinter()}>
            <Select
              label="Slot"
              options={slots().map((s) => s.id)}
              value={slotId()}
              onChange={setSlotId}
              optionLabel={(id) => {
                const slot = slots().find((s) => s.id === id);
                return slot ? slotOptionLabel(slot) : id;
              }}
            />
          </Show>
          <Show when={displayedOccupant()}>
            {(occ) => (
              <div class={styles.swap}>
                <p class={styles.swapLine}>Swap: {spoolLabel(occ())} goes to storage</p>
                <TextField
                  label="Displaced Spool storage label"
                  value={displacedLabel()}
                  onChange={setDisplacedLabel}
                  required
                />
              </div>
            )}
          </Show>
        </Show>
        <Show when={error()}>
          {(message) => <p class={styles.error} role="alert">{message()}</p>}
        </Show>
        <div class={styles.actions}>
          <Button variant="secondary" onClick={() => props.onOpenChange(false)}>Cancel</Button>
          <Button disabled={!canSubmit()} onClick={() => void onSubmit()}>Move</Button>
        </div>
      </div>
    </Dialog>
  );
}
