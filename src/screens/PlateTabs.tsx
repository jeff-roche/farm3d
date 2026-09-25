import { Tabs as KTabs } from "@kobalte/core/tabs";
import { IconDots, IconPlus } from "@tabler/icons-solidjs";
import { createSignal, createUniqueId, For, Show, type JSX } from "solid-js";
import { Button, Dialog, DropdownMenu, TextField, type DropdownMenuEntry } from "../design-system";
import { MAX_PLATE_NAME_CHARS, MAX_PLATES, plateLabel } from "../slicing/preparation-edits";
import type { PlateDoc } from "../slicing/types";
import styles from "./PlateTabs.module.css";

export interface PlateTabsProps {
  plates: PlateDoc[];
  value: string | undefined;
  onChange: (plateKey: string) => void;
  onAdd: () => void;
  onRename: (plateKey: string, name: string) => void;
  onMove: (plateKey: string, step: -1 | 1) => void;
  onDelete: (plateKey: string) => void;
  /** Plate keys with a validation issue, marked on their tab in text. */
  withIssues?: ReadonlySet<string>;
  /** The shown plate's content. */
  children: JSX.Element;
}

/** D19's plate tabs (Kobalte `Tabs`): one tab per plate, **+ Plate**, and a
 *  menu for the shown plate with Rename…, Move left/right and Delete… (the
 *  last plate can't be deleted). One panel follows the chosen tab, so the
 *  viewport inside it stays mounted as plates change. */
export function PlateTabs(props: PlateTabsProps) {
  const [renaming, setRenaming] = createSignal<PlateDoc | undefined>();
  const [draft, setDraft] = createSignal("");
  const [deleting, setDeleting] = createSignal<PlateDoc | undefined>();

  const index = () => props.plates.findIndex((plate) => plate.plateKey === props.value);
  const shown = () => props.plates[index()];
  const label = (plate: PlateDoc) => plateLabel(plate, props.plates.indexOf(plate));
  const labelOf = (plateKey: string) => {
    const at = props.plates.findIndex((plate) => plate.plateKey === plateKey);
    return at < 0 ? "" : plateLabel(props.plates[at], at);
  };

  const startRename = (plate: PlateDoc) => {
    setDraft(plate.name ?? "");
    setRenaming(plate);
  };
  const finishRename = () => {
    const plate = renaming();
    if (plate) props.onRename(plate.plateKey, draft());
    setRenaming(undefined);
  };
  const requestDelete = (plate: PlateDoc) => {
    if (plate.instances.length === 0) props.onDelete(plate.plateKey);
    else setDeleting(plate);
  };

  const limitId = createUniqueId();
  const menu = (): DropdownMenuEntry[] => {
    const plate = shown();
    if (!plate) return [];
    return [
      { label: "Rename…", onSelect: () => startRename(plate) },
      { label: "Move left", onSelect: () => props.onMove(plate.plateKey, -1), disabled: index() <= 0 },
      { label: "Move right", onSelect: () => props.onMove(plate.plateKey, 1), disabled: index() >= props.plates.length - 1 },
      { type: "separator" },
      props.plates.length <= 1
        ? { label: "Delete… (a Preparation keeps at least one plate)", onSelect: () => {}, disabled: true }
        : { label: "Delete…", onSelect: () => requestDelete(plate) },
    ];
  };

  return (
    <KTabs value={props.value} onChange={props.onChange} class={styles.tabs}>
      <div class={styles.bar}>
        <KTabs.List class={styles.list} aria-label="Plates">
          {/* Keyed by plate key: a plate's record is replaced on every edit,
              and a remounted trigger would make Kobalte move the selection. */}
          <For each={props.plates.map((plate) => plate.plateKey)}>
            {(plateKey) => (
              <KTabs.Trigger value={plateKey} class={styles.trigger}>
                {labelOf(plateKey)}
                <Show when={props.withIssues?.has(plateKey)}>
                  <span class={styles.issue}> · issues</span>
                </Show>
              </KTabs.Trigger>
            )}
          </For>
          <KTabs.Indicator class={styles.indicator} />
        </KTabs.List>
        <Button
          variant="ghost"
          size="sm"
          onClick={props.onAdd}
          disabled={props.plates.length >= MAX_PLATES}
          aria-describedby={props.plates.length >= MAX_PLATES ? limitId : undefined}
        >
          <IconPlus size={14} aria-hidden="true" /> Plate
        </Button>
        <Show when={props.plates.length >= MAX_PLATES}>
          <span id={limitId} class={styles.hint}>{MAX_PLATES} plates, the most a Preparation can have</span>
        </Show>
        <Show when={shown()}>
          {(plate) => (
            <DropdownMenu
              trigger={
                <span class={styles.menuTrigger} aria-label={`Plate actions for ${label(plate())}`}>
                  <IconDots size={16} aria-hidden="true" />
                </span>
              }
              items={menu()}
            />
          )}
        </Show>
      </div>
      <KTabs.Content value={props.value ?? ""} forceMount class={styles.panel}>
        {props.children}
      </KTabs.Content>

      <Dialog
        title="Rename plate"
        open={renaming() !== undefined}
        onOpenChange={(open) => { if (!open) setRenaming(undefined); }}
      >
        <form
          class={styles.form}
          onSubmit={(event) => {
            event.preventDefault();
            finishRename();
          }}
        >
          <TextField
            label="Plate name"
            description={`Leave it empty to call it by its position. At most ${MAX_PLATE_NAME_CHARS} characters.`}
            value={draft()}
            onChange={setDraft}
          />
          <div class={styles.actions}>
            <Button type="submit" variant="primary">Rename</Button>
            <Button type="button" variant="ghost" onClick={() => setRenaming(undefined)}>Cancel</Button>
          </div>
        </form>
      </Dialog>

      <Dialog
        title={`Delete ${deleting() ? label(deleting()!) : "plate"}?`}
        description={deleting()
          ? `Its ${deleting()!.instances.length === 1 ? "object is" : `${deleting()!.instances.length} objects are`} removed from this Preparation. The Model is not changed.`
          : undefined}
        open={deleting() !== undefined}
        onOpenChange={(open) => { if (!open) setDeleting(undefined); }}
      >
        <div class={styles.actions}>
          <Button
            variant="danger"
            onClick={() => {
              const plate = deleting();
              setDeleting(undefined);
              if (plate) props.onDelete(plate.plateKey);
            }}
          >
            Delete plate
          </Button>
          <Button variant="ghost" onClick={() => setDeleting(undefined)}>Cancel</Button>
        </div>
      </Dialog>
    </KTabs>
  );
}
