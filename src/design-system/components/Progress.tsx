import { Progress as KProgress } from "@kobalte/core/progress";
import styles from "./Progress.module.css";

export interface ProgressProps {
  label?: string;
  value?: number;
  minValue?: number;
  maxValue?: number;
  indeterminate?: boolean;
  showValue?: boolean;
  /** The value as text, for the visible value and `aria-valuetext`
   *  (Kobalte's default is the percentage). */
  valueLabel?: string;
  class?: string;
}

export function Progress(props: ProgressProps) {
  return (
    <KProgress
      class={[styles.root, props.class].filter(Boolean).join(" ")}
      value={props.value}
      minValue={props.minValue}
      maxValue={props.maxValue}
      indeterminate={props.indeterminate}
      getValueLabel={props.valueLabel === undefined ? undefined : () => props.valueLabel!}
    >
      {(props.label || props.showValue) && (
        <div class={styles.labelRow}>
          {props.label && <KProgress.Label class={styles.label}>{props.label}</KProgress.Label>}
          {props.showValue && (
            <KProgress.ValueLabel class={styles.valueLabel} />
          )}
        </div>
      )}
      <KProgress.Track class={styles.track}>
        <KProgress.Fill class={styles.fill} />
      </KProgress.Track>
    </KProgress>
  );
}
