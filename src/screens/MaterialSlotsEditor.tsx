import { createEffect, createMemo, createSignal, Index, on, onMount, Show } from "solid-js";
import { Button, ColorSwatch, Select, TextField } from "../design-system";
import { isCommandError } from "../ipc/client";
import { reloadPrinter, setSlotLayout } from "../printers/printer-store";
import type { MaterialSlot, ResolvedPrinter, SlotSpec } from "../printers/types";
import { materialLabel } from "../spools/materials";
import { ensureInventoryLoaded, moveSpool, spoolState } from "../spools/spool-store";
import { formatGrams } from "../spools/weight";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import { MoveSpoolDialog } from "./MoveSpoolDialog";
import styles from "./MaterialSlotsEditor.module.css";

/** D4: every Printer has 1 to 16 slots; names and feeder labels are 1-32
 *  characters, trimmed; names are unique per Printer, ignoring case. */
export const MAX_SLOTS = 16;
const MAX_SLOT_TEXT = 32;
export const MULTI_MATERIAL_HINT =
  "This model can feed more than one material. Add a slot for each spool position on your feeder.";
/** The Setup tab's Material Slots section; batch Results' Equip targets it. */
export const MATERIAL_SLOTS_ANCHOR_ID = "printer-setup-material-slots";

/** One editable slot. `key` is local identity; `id` is set for a slot that
 *  already exists on a saved Printer (D12: an entry without one is new). */
export interface SlotDraft {
  key: string;
  id?: string;
  name: string;
  feederLabel: string;
}

export interface InitialLoad {
  slotIndex: number;
  spoolId: string;
}

const NO_SPOOL = "none";

export function newSlotDraft(name: string): SlotDraft {
  return { key: crypto.randomUUID(), name, feederLabel: "" };
}

export function defaultSlotDrafts(): SlotDraft[] {
  return [newSlotDraft("Main")];
}

export function draftsFromSlots(slots: MaterialSlot[]): SlotDraft[] {
  return slots.map((slot) => ({ key: slot.id, id: slot.id, name: slot.name, feederLabel: slot.feederLabel ?? "" }));
}

export function toSlotSpecs(drafts: SlotDraft[]): SlotSpec[] {
  return drafts.map((draft) => {
    const feederLabel = draft.feederLabel.trim();
    return {
      ...(draft.id !== undefined ? { id: draft.id } : {}),
      name: draft.name.trim(),
      ...(feederLabel !== "" ? { feederLabel } : {}),
    };
  });
}

function textError(value: string, required: boolean): string | undefined {
  const trimmed = value.trim();
  if (trimmed === "") return required ? "Name is required" : undefined;
  return trimmed.length > MAX_SLOT_TEXT ? `Use ${MAX_SLOT_TEXT} characters or fewer` : undefined;
}

/** Per-draft field errors, keyed by `SlotDraft.key`. */
export function slotDraftErrors(drafts: SlotDraft[]): Map<string, { name?: string; feederLabel?: string }> {
  const counts = new Map<string, number>();
  for (const draft of drafts) {
    const folded = draft.name.trim().toLowerCase();
    if (folded) counts.set(folded, (counts.get(folded) ?? 0) + 1);
  }
  const errors = new Map<string, { name?: string; feederLabel?: string }>();
  for (const draft of drafts) {
    const duplicate = (counts.get(draft.name.trim().toLowerCase()) ?? 0) > 1;
    const name = textError(draft.name, true) ?? (duplicate ? "Another slot already has this name" : undefined);
    const feederLabel = textError(draft.feederLabel, false);
    if (name || feederLabel) errors.set(draft.key, { name, feederLabel });
  }
  return errors;
}

export function slotDraftsValid(drafts: SlotDraft[]): boolean {
  return drafts.length >= 1 && drafts.length <= MAX_SLOTS && slotDraftErrors(drafts).size === 0;
}

export function spoolSummary(spool: SpoolRecord): string {
  return `#${spool.spoolNumber} ${materialLabel(spool.materialFamily, spool.materialOther)} ${spool.colorName}`;
}

function spoolOptionLabel(spool: SpoolRecord): string {
  return `${spoolSummary(spool)} — ${formatGrams(spool.availability.currentMg, 0)}`;
}

/** D12: only an active Spool in storage can be loaded at creation. */
function storageSpools(): SpoolRecord[] {
  return spoolState.spools.filter((s) => s.lifecycle === "active" && s.location.kind === "storage");
}

export interface MaterialSlotsEditorProps {
  /** `layout` edits names/order only; `occupancy` also shows each saved
   *  slot's Spool with Load…/Swap…/Unload (needs `printer`). */
  mode: "layout" | "occupancy";
  slots: SlotDraft[];
  onChange?: (slots: SlotDraft[]) => void;
  printer?: ResolvedPrinter;
  /** Wizard only: Spools to load at creation, by slot index. */
  initialLoads?: InitialLoad[];
  onInitialLoadsChange?: (loads: InitialLoad[]) => void;
  multiMaterialHint: boolean;
}

/** One component for the wizard's Equip step, batch Shared (layout only),
 *  and the Setup tab (layout and occupancy) -- spec §Components. Fully
 *  controlled: every edit reports a new `slots` array. */
export function MaterialSlotsEditor(props: MaterialSlotsEditorProps) {
  const rows: HTMLLIElement[] = [];
  const errors = createMemo(() => slotDraftErrors(props.slots));
  const [pickingSlotId, setPickingSlotId] = createSignal<string | null>(null);
  const [moveTarget, setMoveTarget] = createSignal<{ spoolId: string; slotId: string } | null>(null);
  const [actionError, setActionError] = createSignal<string | null>(null);

  onMount(() => {
    if (props.mode === "occupancy" || props.initialLoads) void ensureInventoryLoaded();
  });

  const slotLabel = (slot: SlotDraft, index: number) => slot.name.trim() || `slot ${index + 1}`;
  const liveSlot = (slot: SlotDraft) =>
    slot.id === undefined ? undefined : props.printer?.materialSlots.find((s) => s.id === slot.id);
  const occupantId = (slot: SlotDraft) => (props.mode === "occupancy" ? liveSlot(slot)?.occupantSpoolId : undefined);
  const findSpool = (id: string | undefined) => (id ? spoolState.spools.find((s) => s.id === id) : undefined);

  function emitLoads(remap: (index: number) => number | null) {
    if (!props.initialLoads) return;
    const next = props.initialLoads.flatMap((load) => {
      const slotIndex = remap(load.slotIndex);
      return slotIndex === null ? [] : [{ ...load, slotIndex }];
    });
    props.onInitialLoadsChange?.(next);
  }

  function update(index: number, patch: Partial<SlotDraft>) {
    props.onChange?.(props.slots.map((slot, i) => (i === index ? { ...slot, ...patch } : slot)));
  }

  function add() {
    if (props.slots.length >= MAX_SLOTS) return;
    const taken = new Set(props.slots.map((slot) => slot.name.trim().toLowerCase()));
    let n = props.slots.length + 1;
    while (taken.has(`slot ${n}`)) n += 1;
    props.onChange?.([...props.slots, newSlotDraft(`Slot ${n}`)]);
  }

  function remove(index: number) {
    props.onChange?.(props.slots.filter((_, i) => i !== index));
    emitLoads((i) => (i === index ? null : i > index ? i - 1 : i));
  }

  /** Swaps two adjacent slots, then puts focus back on the control the user
   *  was on, now in the moved slot's new row (rows are index-bound). */
  function move(from: number, to: number) {
    if (to < 0 || to >= props.slots.length) return;
    const fromRow = rows[from];
    const controls = fromRow ? [...fromRow.querySelectorAll<HTMLElement>("input, button")] : [];
    const focusIndex = controls.indexOf(document.activeElement as HTMLElement);
    const next = [...props.slots];
    [next[from], next[to]] = [next[to], next[from]];
    props.onChange?.(next);
    emitLoads((i) => (i === from ? to : i === to ? from : i));
    if (focusIndex < 0) return;
    const target = rows[to]?.querySelectorAll<HTMLElement>("input, button")[focusIndex];
    const fallback = rows[to]?.querySelector<HTMLElement>("input");
    (target && !(target as HTMLButtonElement).disabled ? target : fallback)?.focus();
  }

  function onRowKeyDown(e: KeyboardEvent, index: number) {
    if (!e.altKey || e.defaultPrevented || (e.key !== "ArrowUp" && e.key !== "ArrowDown")) return;
    e.preventDefault();
    move(index, e.key === "ArrowUp" ? index - 1 : index + 1);
  }

  function setLoad(index: number, spoolId: string) {
    // Kobalte re-reports the current value when its options change; a
    // no-op here keeps that from looping through `onInitialLoadsChange`.
    const current = props.initialLoads?.find((load) => load.slotIndex === index)?.spoolId ?? NO_SPOOL;
    if (current === spoolId) return;
    const others = (props.initialLoads ?? []).filter((load) => load.slotIndex !== index);
    props.onInitialLoadsChange?.(spoolId === NO_SPOOL ? others : [...others, { slotIndex: index, spoolId }]);
  }

  function loadOptions(index: number): string[] {
    const chosenElsewhere = new Set((props.initialLoads ?? []).filter((l) => l.slotIndex !== index).map((l) => l.spoolId));
    return [NO_SPOOL, ...storageSpools().filter((s) => !chosenElsewhere.has(s.id)).map((s) => s.id)];
  }

  async function unload(spool: SpoolRecord) {
    setActionError(null);
    try {
      await moveSpool({ spoolId: spool.id, expectedSpoolRevision: spool.revision, destination: { kind: "storage", storageLabel: null } });
    } catch (e) {
      setActionError(isCommandError(e) ? e.message : "This Spool could not be unloaded.");
    }
  }

  const moveSpoolRecord = () => findSpool(moveTarget()?.spoolId);

  return (
    <div class={styles.editor}>
      <Show when={props.multiMaterialHint}>
        <p class={styles.hint}>{MULTI_MATERIAL_HINT}</p>
      </Show>
      <ol class={styles.list} aria-label="Material slots">
        <Index each={props.slots}>
          {(slot, index) => {
            const label = () => slotLabel(slot(), index);
            const occupant = () => findSpool(occupantId(slot()));
            const canEquip = () => props.mode === "occupancy" && liveSlot(slot()) !== undefined && !props.printer?.archivedAt;
            return (
              <li class={styles.row} ref={(el) => (rows[index] = el)} onKeyDown={(e) => onRowKeyDown(e, index)}>
                <span class={styles.position} aria-hidden="true">{index + 1}</span>
                <TextField
                  aria-label={`Name for slot ${index + 1}`}
                  value={slot().name}
                  onChange={(name) => update(index, { name })}
                  error={errors().get(slot().key)?.name}
                />
                <TextField
                  aria-label={`Feeder label for slot ${index + 1}`}
                  placeholder="Feeder (optional)"
                  value={slot().feederLabel}
                  onChange={(feederLabel) => update(index, { feederLabel })}
                  error={errors().get(slot().key)?.feederLabel}
                />
                <Show when={props.initialLoads}>
                  <div class={styles.wide}>
                    <Select
                      label={`Load into ${label()}`}
                      options={loadOptions(index)}
                      value={props.initialLoads?.find((l) => l.slotIndex === index)?.spoolId ?? NO_SPOOL}
                      optionLabel={(id) => {
                        const spool = findSpool(id);
                        return spool ? spoolOptionLabel(spool) : "None";
                      }}
                      onChange={(id) => setLoad(index, id)}
                    />
                  </div>
                </Show>
                <Show when={props.mode === "occupancy"}>
                  <div class={[styles.wide, styles.occupancy].join(" ")}>
                    <Show
                      when={occupant()}
                      fallback={<span class={styles.muted}>{slot().id === undefined ? "Save to load" : occupantId(slot()) ? "Loaded" : "Empty"}</span>}
                    >
                      {(spool) => (
                        <span class={styles.occupant}>
                          <ColorSwatch hex={spool().colorHex ?? null} name={spool().colorName} size="sm" />
                          {spoolSummary(spool())}
                        </span>
                      )}
                    </Show>
                    <Show when={canEquip()}>
                      <Show
                        when={occupant()}
                        fallback={
                          <Button variant="secondary" size="sm" aria-label={`Load… into ${label()}`} onClick={() => setPickingSlotId(slot().id!)}>
                            Load…
                          </Button>
                        }
                      >
                        {(spool) => (
                          <>
                            <Button variant="secondary" size="sm" aria-label={`Swap… in ${label()}`} onClick={() => setPickingSlotId(slot().id!)}>
                              Swap…
                            </Button>
                            <Button variant="ghost" size="sm" aria-label={`Unload ${label()}`} onClick={() => void unload(spool())}>
                              Unload
                            </Button>
                          </>
                        )}
                      </Show>
                    </Show>
                  </div>
                  <Show when={pickingSlotId() !== null && pickingSlotId() === slot().id}>
                    <div class={[styles.wide, styles.picker].join(" ")}>
                      <Show when={storageSpools().length > 0} fallback={<span class={styles.muted}>No active Spools in storage.</span>}>
                        <Select
                          label={`Spool to load into ${label()}`}
                          options={storageSpools().map((s) => s.id)}
                          optionLabel={(id) => {
                            const spool = findSpool(id);
                            return spool ? spoolOptionLabel(spool) : id;
                          }}
                          placeholder="Choose a Spool"
                          onChange={(spoolId) => {
                            setPickingSlotId(null);
                            setMoveTarget({ spoolId, slotId: slot().id! });
                          }}
                        />
                      </Show>
                      <Button variant="ghost" size="sm" onClick={() => setPickingSlotId(null)}>Cancel</Button>
                    </div>
                  </Show>
                </Show>
                <div class={[styles.wide, styles.rowActions].join(" ")}>
                  <Button variant="ghost" size="sm" aria-label={`Move ${label()} up`} disabled={index === 0} onClick={() => move(index, index - 1)}>
                    ↑
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    aria-label={`Move ${label()} down`}
                    disabled={index === props.slots.length - 1}
                    onClick={() => move(index, index + 1)}
                  >
                    ↓
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    aria-label={`Remove ${label()}`}
                    disabled={props.slots.length <= 1 || occupantId(slot()) !== undefined}
                    onClick={() => remove(index)}
                  >
                    Remove
                  </Button>
                  <Show when={occupantId(slot()) !== undefined}>
                    <span class={styles.muted}>Unload first</span>
                  </Show>
                </div>
              </li>
            );
          }}
        </Index>
      </ol>
      <div class={styles.footer}>
        <Button variant="secondary" size="sm" disabled={props.slots.length >= MAX_SLOTS} onClick={add}>
          Add slot
        </Button>
        <Show when={props.slots.length >= MAX_SLOTS}>
          <span class={styles.muted}>{MAX_SLOTS} slots is the most a Printer can have.</span>
        </Show>
      </div>
      <Show when={actionError()}>
        {(message) => <p class={styles.error} role="alert">{message()}</p>}
      </Show>
      <Show when={moveSpoolRecord()}>
        {(spool) => (
          <MoveSpoolDialog
            open
            onOpenChange={(open) => !open && setMoveTarget(null)}
            spool={spool()}
            presetDestination={{ printerId: props.printer!.id, slotId: moveTarget()!.slotId }}
          />
        )}
      </Show>
    </div>
  );
}

const layoutKey = (slots: MaterialSlot[]) => JSON.stringify(toSlotSpecs(draftsFromSlots(slots)));

/** The Setup tab's Material Slots section: the occupancy-mode editor over
 *  a local draft of this Printer's layout, saved with `setSlotLayout`.
 *  `SLOT_OCCUPIED` and `VALIDATION` show inline; `CONFLICT` reloads this
 *  Printer and resets the draft (P2's "reload and retry"). */
export function MaterialSlotsSection(props: { printer: ResolvedPrinter }) {
  const [baseline, setBaseline] = createSignal(props.printer.materialSlots);
  const [drafts, setDrafts] = createSignal(draftsFromSlots(props.printer.materialSlots));
  const [error, setError] = createSignal<string | null>(null);
  const [saving, setSaving] = createSignal(false);
  const dirty = () => JSON.stringify(toSlotSpecs(drafts())) !== layoutKey(baseline());

  function seed(slots: MaterialSlot[]) {
    setBaseline(slots);
    setDrafts(draftsFromSlots(slots));
  }

  // A different Printer always reseeds; a layout change from elsewhere
  // (another save, an event) reseeds only when there's no unsaved edit.
  createEffect(on(
    () => [props.printer.id, layoutKey(props.printer.materialSlots)] as const,
    ([id, key], previous) => {
      if (!previous) return;
      if (previous[0] !== id) {
        setError(null);
        seed(props.printer.materialSlots);
      } else if (key !== previous[1] && !dirty()) {
        seed(props.printer.materialSlots);
      }
    },
  ));

  async function save() {
    if (saving() || !dirty() || !slotDraftsValid(drafts())) return;
    setSaving(true);
    setError(null);
    try {
      const updated = await setSlotLayout(props.printer.id, toSlotSpecs(drafts()));
      seed(updated.materialSlots);
    } catch (e) {
      if (isCommandError(e) && e.code === "SLOT_OCCUPIED") {
        const slotId = e.details?.slotId;
        const name = baseline().find((slot) => slot.id === slotId)?.name ?? "That slot";
        setError(`${name} still holds a Spool. Unload first, then remove it.`);
      } else if (isCommandError(e) && e.code === "CONFLICT") {
        await reloadPrinter(props.printer.id).catch(() => undefined);
        seed(props.printer.materialSlots);
        setError(`${e.message} The slots were reloaded; make your change again.`);
      } else {
        setError(isCommandError(e) ? e.message : "The slots could not be saved.");
      }
    } finally {
      setSaving(false);
    }
  }

  return (
    <section id={MATERIAL_SLOTS_ANCHOR_ID} class={styles.section} aria-labelledby={`${MATERIAL_SLOTS_ANCHOR_ID}-title`}>
      <h3 id={`${MATERIAL_SLOTS_ANCHOR_ID}-title`} class={styles.title} tabIndex={-1}>Material Slots</h3>
      <MaterialSlotsEditor
        mode="occupancy"
        printer={props.printer}
        slots={drafts()}
        onChange={setDrafts}
        multiMaterialHint={props.printer.profile.supportsMultiFilament}
      />
      <Show when={error()}>
        {(message) => <p class={styles.error} role="alert">{message()}</p>}
      </Show>
      <div class={styles.footer}>
        <Button variant="primary" size="sm" disabled={saving() || !dirty() || !slotDraftsValid(drafts())} onClick={() => void save()}>
          {saving() ? "Saving…" : "Save slots"}
        </Button>
        <Button variant="ghost" size="sm" disabled={saving() || !dirty()} onClick={() => seed(baseline())}>
          Revert
        </Button>
      </div>
    </section>
  );
}
