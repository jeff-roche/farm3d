import { Show } from "solid-js";
import { Button, Select } from "../design-system";
import type { Measurement } from "../slicing/bounds";
import type { Vec3 } from "../slicing/transforms";
import styles from "./MeasurePanel.module.css";

export interface MeasurePanelProps {
  /** Surface points picked in the viewport: none, one, or two. */
  points: Vec3[];
  onClear: () => void;
  /** The selected object's name, if any. */
  selectedName: string | undefined;
  /** The other objects on the plate, to measure the selected one to. */
  others: { key: string; name: string }[];
  otherKey: string | undefined;
  onOtherChange: (key: string) => void;
  between: Measurement | undefined;
}

const mm = (value: number) => `${value.toFixed(1)} mm`;

/** D19 **Measure**: two points picked on the objects, and, as the keyboard
 *  alternative, the distance between the selected object and another
 *  (centre to centre, and the gap). */
export function MeasurePanel(props: MeasurePanelProps) {
  const picked = () => {
    if (props.points.length < 2) return undefined;
    const [a, b] = props.points;
    return { distance: Math.hypot(b[0] - a[0], b[1] - a[1], b[2] - a[2]), delta: [b[0] - a[0], b[1] - a[1], b[2] - a[2]] };
  };

  return (
    <section class={styles.panel} aria-label="Measure">
      <p class={styles.line} role="status">
        <Show
          when={picked()}
          fallback={props.points.length === 1
            ? "First point picked. Click a second point."
            : "Click two points on the objects to measure between them."}
        >
          {(result) => (
            <>
              Distance: <strong>{mm(result().distance)}</strong>
              {" "}(X {mm(Math.abs(result().delta[0]))}, Y {mm(Math.abs(result().delta[1]))}, Z {mm(Math.abs(result().delta[2]))})
            </>
          )}
        </Show>
      </p>
      <Show when={props.points.length > 0}>
        <Button variant="ghost" size="sm" onClick={props.onClear}>Clear points</Button>
      </Show>
      <h4 class={styles.heading}>Distance between selected objects</h4>
      <Show
        when={props.selectedName && props.others.length > 0}
        fallback={<p class={styles.hint}>Select an object on a plate with at least two objects.</p>}
      >
        <Select
          label={`From ${props.selectedName} to`}
          options={props.others.map((other) => other.key)}
          value={props.otherKey}
          placeholder="Choose an object"
          optionLabel={(key) => props.others.find((other) => other.key === key)?.name ?? key}
          onChange={props.onOtherChange}
        />
        <Show when={props.between}>
          {(between) => (
            <p class={styles.line} role="status">
              Centre to centre: <strong>{mm(between().centreDistanceMm)}</strong>. Gap: <strong>{mm(between().gapMm)}</strong>.
            </p>
          )}
        </Show>
      </Show>
    </section>
  );
}
