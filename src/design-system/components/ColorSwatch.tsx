import { Show } from "solid-js";
import styles from "./ColorSwatch.module.css";

export interface ColorSwatchProps {
  /** A CSS hex color (e.g. "#2f7a3c"), or `null` for "no color known". This
   *  is the one allowed inline color in the design system — it's data (the
   *  filament's actual color), not a theme choice. */
  hex: string | null;
  /** Accessible name, e.g. the color/material name. `ColorSwatch` never
   *  stands alone — always render the name as visible text next to it too. */
  name: string;
  size?: "sm" | "md";
}

/** A small token-bordered square showing a filament color. A `null` `hex`
 *  renders a diagonal "no color known" pattern instead of a flat fill —
 *  there's never a themed color guess standing in for real data. */
export function ColorSwatch(props: ColorSwatchProps) {
  const size = () => props.size ?? "md";

  return (
    <Show
      when={props.hex !== null}
      fallback={
        <span
          class={[styles.swatch, styles[size()], styles.none].join(" ")}
          role="img"
          aria-label={props.name}
          data-color="none"
        />
      }
    >
      <span
        class={[styles.swatch, styles[size()]].join(" ")}
        role="img"
        aria-label={props.name}
        style={{ "background-color": props.hex ?? undefined }}
      />
    </Show>
  );
}
