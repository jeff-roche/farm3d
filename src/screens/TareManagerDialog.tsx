import { createSignal, For, Show } from "solid-js";
import { Button, Dialog, NumberField, TextField } from "../design-system";
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

/** D3 §Components: `TareManagerDialog`, opened from the inventory
 *  toolbar's menu. Add, rename, and delete a reusable tare. Renaming edits
 *  in place (name + weight together, since D3's snapshot semantics mean an
 *  edit here changes nothing about past measurements). */
export function TareManagerDialog(props: TareManagerDialogProps) {
  const [newName, setNewName] = createSignal("");
  const [newGrams, setNewGrams] = createSignal<number | undefined>(undefined);
  const [editingId, setEditingId] = createSignal<string | null>(null);
  const [editName, setEditName] = createSignal("");
  const [editGrams, setEditGrams] = createSignal<number | undefined>(undefined);

  async function onAdd() {
    const name = newName().trim();
    if (!name || newGrams() === undefined) return;
    await createTare(name, gramsToMg(newGrams()!));
    setNewName("");
    setNewGrams(undefined);
  }

  function startEdit(tare: Tare) {
    setEditingId(tare.id);
    setEditName(tare.name);
    setEditGrams(tare.weightMg / 1000);
  }

  async function onSaveEdit(id: string) {
    const name = editName().trim();
    if (!name || editGrams() === undefined) return;
    await updateTare(id, name, gramsToMg(editGrams()!));
    setEditingId(null);
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
                      <Button variant="ghost" onClick={() => void deleteTare(tare.id)}>Delete</Button>
                    </>
                  }
                >
                  <TextField aria-label={`Tare name for ${tare.name}`} value={editName()} onChange={setEditName} />
                  <NumberField
                    aria-label={`Tare weight for ${tare.name}`}
                    value={editGrams()}
                    onChange={setEditGrams}
                    minValue={0}
                    maxValue={5_000}
                    step={0.1}
                    suffix="g"
                  />
                  <Button onClick={() => void onSaveEdit(tare.id)}>Save</Button>
                  <Button variant="ghost" onClick={() => setEditingId(null)}>Cancel</Button>
                </Show>
              </li>
            )}
          </For>
        </ul>
        <div class={styles.addRow}>
          <TextField label="New tare name" value={newName()} onChange={setNewName} />
          <NumberField
            label="Weight (g)"
            value={newGrams()}
            onChange={setNewGrams}
            minValue={0}
            maxValue={5_000}
            step={0.1}
            suffix="g"
          />
          <Button disabled={!newName().trim() || newGrams() === undefined} onClick={() => void onAdd()}>
            Add tare
          </Button>
        </div>
        <div class={styles.actions}>
          <Button variant="secondary" onClick={() => props.onOpenChange(false)}>Close</Button>
        </div>
      </div>
    </Dialog>
  );
}
