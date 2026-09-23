import { createEffect, createSignal, For, on, Show } from "solid-js";
import { Button, Dialog, NumberField, TextField } from "../design-system";
import { isCommandError } from "../ipc/client";
import { formatGrams } from "../spools/weight";
import { createTare, deleteTare, spoolState, updateTare } from "../spools/spool-store";
import type { Tare } from "../generated/contracts/domain/Tare";
import styles from "./TareManagerDialog.module.css";

export interface TareManagerDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

function gramsToMg(grams: number): number {
  return Math.round(grams * 10) * 100;
}

type FieldError = { path: string; message: string } | null;

/** D3 §Components: `TareManagerDialog`, opened from the inventory
 *  toolbar's menu. Add, rename, and delete a reusable tare. Renaming edits
 *  in place (name + weight together, since D3's snapshot semantics mean an
 *  edit here changes nothing about past measurements).
 *
 *  Fix round 1 ruling: `createTare`/`updateTare`/`deleteTare` now reject
 *  for inline handling instead of reporting to the store banner (which
 *  would render behind this dialog's own overlay). A field-level
 *  `VALIDATION` (`name` -- a duplicate, case-insensitive per D3, or
 *  `weightMg`) shows on the field of whichever mini-form (Add or the
 *  currently-edited row) triggered it; a `CONFLICT` or anything else shows
 *  as a dialog-level message and the dialog stays open. */
export function TareManagerDialog(props: TareManagerDialogProps) {
  const [newName, setNewName] = createSignal("");
  const [newGrams, setNewGrams] = createSignal<number | undefined>(undefined);
  const [editingId, setEditingId] = createSignal<string | null>(null);
  const [editName, setEditName] = createSignal("");
  const [editGrams, setEditGrams] = createSignal<number | undefined>(undefined);
  const [dialogError, setDialogError] = createSignal<string | null>(null);
  const [addFieldError, setAddFieldError] = createSignal<FieldError>(null);
  const [editFieldError, setEditFieldError] = createSignal<FieldError>(null);

  createEffect(on(() => props.open, (open) => {
    if (!open) return;
    setDialogError(null);
    setAddFieldError(null);
    setEditFieldError(null);
  }));

  const addFieldErrorFor = (path: string): string | undefined => (
    addFieldError()?.path === path ? addFieldError()!.message : undefined
  );
  const editFieldErrorFor = (path: string): string | undefined => (
    editFieldError()?.path === path ? editFieldError()!.message : undefined
  );

  async function onAdd() {
    const name = newName().trim();
    if (!name || newGrams() === undefined) return;
    setDialogError(null);
    setAddFieldError(null);
    try {
      await createTare(name, gramsToMg(newGrams()!));
      setNewName("");
      setNewGrams(undefined);
    } catch (e) {
      if (isCommandError(e) && typeof e.details?.fieldPath === "string" && (e.details.fieldPath === "name" || e.details.fieldPath === "weightMg")) {
        setAddFieldError({ path: e.details.fieldPath, message: e.message });
      } else {
        setDialogError(isCommandError(e) ? e.message : "This tare could not be added.");
      }
    }
  }

  function startEdit(tare: Tare) {
    setEditingId(tare.id);
    setEditName(tare.name);
    setEditGrams(tare.weightMg / 1000);
    setDialogError(null);
    setEditFieldError(null);
  }

  async function onSaveEdit(id: string) {
    const name = editName().trim();
    if (!name || editGrams() === undefined) return;
    setDialogError(null);
    setEditFieldError(null);
    try {
      await updateTare(id, name, gramsToMg(editGrams()!));
      setEditingId(null);
    } catch (e) {
      if (isCommandError(e) && e.code === "CONFLICT") {
        setDialogError("This tare changed since you opened it. It's been reloaded with the current values — check them and try again.");
      } else if (isCommandError(e) && typeof e.details?.fieldPath === "string" && (e.details.fieldPath === "name" || e.details.fieldPath === "weightMg")) {
        setEditFieldError({ path: e.details.fieldPath, message: e.message });
      } else {
        setDialogError(isCommandError(e) ? e.message : "This tare could not be renamed.");
      }
    }
  }

  async function onDelete(id: string) {
    setDialogError(null);
    try {
      await deleteTare(id);
    } catch (e) {
      setDialogError(isCommandError(e) ? e.message : "This tare could not be deleted.");
    }
  }

  return (
    <Dialog title="Manage tares" open={props.open} onOpenChange={props.onOpenChange}>
      <div class={styles.body}>
        <ul class={styles.list}>
          <For each={spoolState.tares}>
            {(tare) => (
              <li class={styles.row}>
                <Show
                  when={editingId() === tare.id}
                  fallback={
                    <>
                      <span class={styles.name}>{tare.name}</span>
                      <span class={styles.weight}>{formatGrams(tare.weightMg, 1)}</span>
                      <Button variant="ghost" onClick={() => startEdit(tare)}>Rename</Button>
                      <Button variant="ghost" onClick={() => void onDelete(tare.id)}>Delete</Button>
                    </>
                  }
                >
                  <TextField
                    aria-label={`Tare name for ${tare.name}`}
                    value={editName()}
                    onChange={setEditName}
                    error={editFieldErrorFor("name")}
                  />
                  <NumberField
                    aria-label={`Tare weight for ${tare.name}`}
                    value={editGrams()}
                    onChange={setEditGrams}
                    minValue={0}
                    maxValue={5_000}
                    step={0.1}
                    suffix="g"
                    error={editFieldErrorFor("weightMg")}
                  />
                  <Button onClick={() => void onSaveEdit(tare.id)}>Save</Button>
                  <Button variant="ghost" onClick={() => setEditingId(null)}>Cancel</Button>
                </Show>
              </li>
            )}
          </For>
        </ul>
        <div class={styles.addRow}>
          <TextField
            label="New tare name"
            value={newName()}
            onChange={setNewName}
            error={addFieldErrorFor("name")}
          />
          <NumberField
            label="Weight (g)"
            value={newGrams()}
            onChange={setNewGrams}
            minValue={0}
            maxValue={5_000}
            step={0.1}
            suffix="g"
            error={addFieldErrorFor("weightMg")}
          />
          <Button disabled={!newName().trim() || newGrams() === undefined} onClick={() => void onAdd()}>
            Add tare
          </Button>
        </div>
        <Show when={dialogError()}>
          {(message) => <p class={styles.error} role="alert">{message()}</p>}
        </Show>
        <div class={styles.actions}>
          <Button variant="secondary" onClick={() => props.onOpenChange(false)}>Close</Button>
        </div>
      </div>
    </Dialog>
  );
}
