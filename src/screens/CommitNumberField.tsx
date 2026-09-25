import { createSignal, Show } from "solid-js";
import { NumberField } from "../design-system";

export interface CommitNumberFieldProps {
  label: string;
  value: number;
  /** Called with a finite number that differs from `value`. */
  onCommit: (value: number) => void;
  step?: number;
  minValue?: number;
  maxValue?: number;
  suffix?: string;
  disabled?: boolean;
  class?: string;
}

/** A `NumberField` that commits a finished number rather than every
 *  keystroke: on Enter, on leaving the field, and on each spin (its
 *  buttons, or the arrow and page keys). Escape, or leaving an empty or
 *  unchanged field, puts the current value back. So typing "120" never
 *  moves an object to 1 and then 12 on the way. */
export function CommitNumberField(props: CommitNumberFieldProps) {
  let latest = props.value;
  let wrapper: HTMLDivElement | undefined;
  // Remounting the field is how it shows `value` again after an edit that
  // changed nothing (Kobalte keeps its typed text until its value prop
  // changes).
  const [generation, setGeneration] = createSignal(0);

  const restore = (refocus: boolean) => {
    latest = props.value;
    setGeneration((n) => n + 1);
    if (refocus) wrapper?.querySelector("input")?.focus();
  };

  const commit = (settle: boolean, refocus = false) => {
    const before = props.value;
    const typed = latest;
    if (Number.isFinite(typed) && typed !== before) props.onCommit(typed);
    if (!settle) return;
    // If nothing took (an empty field, or a value that clamps back to the
    // current one), show the current value again.
    queueMicrotask(() => {
      if (props.value === before && typed !== before) restore(refocus);
    });
  };

  return (
    <div
      ref={wrapper}
      class={props.class}
      onFocusOut={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) commit(true);
      }}
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          commit(true, true);
        } else if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          restore(true);
        } else if (["ArrowUp", "ArrowDown", "PageUp", "PageDown"].includes(event.key)) {
          queueMicrotask(() => commit(false));
        }
      }}
      onClick={(event) => {
        if ((event.target as Element).closest("button")) queueMicrotask(() => commit(false));
      }}
    >
      <Show when={generation() + 1} keyed>
        {(_generation) => (
          <NumberField
            label={props.label}
            value={props.value}
            onChange={(value) => { latest = value; }}
            step={props.step}
            minValue={props.minValue}
            maxValue={props.maxValue}
            suffix={props.suffix}
            disabled={props.disabled}
          />
        )}
      </Show>
    </div>
  );
}
