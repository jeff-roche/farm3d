import { createSignal, createUniqueId, For, Show } from "solid-js";
import { Button, Checkbox, Select } from "../design-system";
import { plateLabel } from "../slicing/preparation-edits";
import { formatFaceArea } from "../slicing/slice-presentation";
import { MAX_SCALE, MIN_SCALE } from "../slicing/transforms";
import type { GeometryObject, InstanceDoc, PlateDoc } from "../slicing/types";
import { CommitNumberField } from "./CommitNumberField";
import type { Axis, PreparationActions } from "./preparation-actions";
import styles from "./InstanceInspector.module.css";

export interface InstanceInspectorProps {
  instance: InstanceDoc;
  object: GeometryObject | undefined;
  name: string;
  plates: PlateDoc[];
  plateKey: string;
  actions: PreparationActions;
}

const AXES = ["X", "Y", "Z"] as const;
const round = (value: number, places: number) => Number(value.toFixed(places));

/** D19's numeric equivalents for the selected object: position (Z is
 *  derived, so it is only shown), rotation, scale with **Uniform**, the
 *  lay-flat faces, **Move to plate**, and **Duplicate** and **Delete**. */
export function InstanceInspector(props: InstanceInspectorProps) {
  const [uniform, setUniform] = createSignal(true);
  const headingId = createUniqueId();
  const transform = () => props.instance.transform;
  const faces = () => props.object?.layFlatFaces ?? [];

  return (
    <section class={styles.inspector} aria-labelledby={headingId}>
      <h3 id={headingId} class={styles.heading}>{props.name}</h3>

      <fieldset class={styles.group} data-field="position">
        <legend class={styles.legend}>Position</legend>
        <div class={styles.row}>
          <For each={[0, 1] as const}>
            {(axis) => (
              <CommitNumberField
                label={`${AXES[axis]} (mm)`}
                value={round(transform().translateMm[axis], 3)}
                step={1}
                onCommit={(value) => props.actions.setPosition(axis, value)}
              />
            )}
          </For>
          <div class={styles.derived}>
            <span class={styles.derivedLabel}>Z (mm)</span>
            <span>0, on the bed</span>
          </div>
        </div>
      </fieldset>

      <fieldset class={styles.group}>
        <legend class={styles.legend}>Rotation</legend>
        <div class={styles.row}>
          <For each={[0, 1, 2] as Axis[]}>
            {(axis) => (
              <CommitNumberField
                label={`${AXES[axis]} (°)`}
                value={round(transform().rotateDeg[axis], 3)}
                step={15}
                onCommit={(value) => props.actions.setRotation(axis, value)}
              />
            )}
          </For>
        </div>
      </fieldset>

      <fieldset class={styles.group}>
        <legend class={styles.legend}>Scale</legend>
        <div class={styles.row}>
          <For each={[0, 1, 2] as Axis[]}>
            {(axis) => (
              <CommitNumberField
                label={`${AXES[axis]} (%)`}
                value={round(transform().scale[axis] * 100, 3)}
                step={5}
                minValue={MIN_SCALE * 100}
                maxValue={MAX_SCALE * 100}
                onCommit={(value) => props.actions.setScale(axis, value / 100, uniform())}
              />
            )}
          </For>
        </div>
        <Checkbox checked={uniform()} onChange={setUniform}>Uniform</Checkbox>
      </fieldset>

      <Show when={faces().length > 0}>
        <fieldset class={styles.group}>
          <legend class={styles.legend}>Lay flat</legend>
          <ul class={styles.faces}>
            <For each={faces()}>
              {(face, index) => (
                <li>
                  <Button variant="ghost" size="sm" onClick={() => props.actions.layFlatOn(index())}>
                    Face {index() + 1} · {formatFaceArea(face.areaMm2)} mm²
                  </Button>
                </li>
              )}
            </For>
          </ul>
        </fieldset>
      </Show>

      <div class={styles.plate} data-field="plate">
        <Select
          label="Plate"
          options={props.plates.map((plate) => plate.plateKey)}
          value={props.plateKey}
          optionLabel={(key) => {
            const at = props.plates.findIndex((plate) => plate.plateKey === key);
            return at < 0 ? key : plateLabel(props.plates[at], at);
          }}
          onChange={(key) => { if (key !== props.plateKey) props.actions.moveToPlate(key); }}
          disabled={props.plates.length < 2}
        />
        <Show when={props.plates.length < 2}>
          <p class={styles.hint}>Add a plate to move objects between plates.</p>
        </Show>
      </div>

      <div class={styles.actions}>
        <Button variant="secondary" size="sm" aria-keyshortcuts="Control+D" onClick={() => props.actions.duplicate()}>
          Duplicate
        </Button>
        <Button variant="secondary" size="sm" aria-keyshortcuts="Delete" onClick={() => props.actions.remove()}>
          Delete
        </Button>
      </div>
    </section>
  );
}
