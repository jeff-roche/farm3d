import { Show } from "solid-js";
import styles from "./BuildPlate.module.css";

export interface BuildPlateProps {
  modelName: string;
  /** e.g. "40 × 40 × 5 mm"; `null` when the Model's extents aren't known. */
  approximateSize: string | null;
}

/**
 * A static placeholder: a perspective floor grid with the selected Model's
 * name and approximate size. P5 replaces it with a rendered viewport, whose
 * state stays inside the viewport component.
 */
export function BuildPlate(props: BuildPlateProps) {
  return (
    <figure class={styles.stage} aria-label="Build plate">
      <div class={styles.plane} aria-hidden="true" />
      <figcaption class={styles.caption}>
        <span class={styles.name}>{props.modelName}</span>
        <Show when={props.approximateSize} fallback={<span>Size unknown</span>}>
          {(size) => <span>About {size()}</span>}
        </Show>
      </figcaption>
    </figure>
  );
}
