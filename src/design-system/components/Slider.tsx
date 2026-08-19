import { Slider as KSlider } from "@kobalte/core/slider";
import styles from "./Slider.module.css";

export interface SliderProps {
  label?: string;
  value?: number;
  defaultValue?: number;
  onChange?: (value: number) => void;
  minValue?: number;
  maxValue?: number;
  step?: number;
  disabled?: boolean;
  showValue?: boolean;
  class?: string;
}

export function Slider(props: SliderProps) {
  return (
    <KSlider
      class={[styles.root, props.class].filter(Boolean).join(" ")}
      value={props.value !== undefined ? [props.value] : undefined}
      defaultValue={props.defaultValue !== undefined ? [props.defaultValue] : undefined}
      onChange={(value) => props.onChange?.(value[0])}
      minValue={props.minValue}
      maxValue={props.maxValue}
      step={props.step}
      disabled={props.disabled}
    >
      {(props.label || props.showValue) && (
        <div class={styles.labelRow}>
          {props.label && <KSlider.Label class={styles.label}>{props.label}</KSlider.Label>}
          {props.showValue && <KSlider.ValueLabel class={styles.valueLabel} />}
        </div>
      )}
      <KSlider.Track class={styles.track}>
        <KSlider.Fill class={styles.fill} />
        <KSlider.Thumb class={styles.thumb}>
          <KSlider.Input />
        </KSlider.Thumb>
      </KSlider.Track>
    </KSlider>
  );
}
