import { IconAlertTriangle, IconCircleCheck } from "@tabler/icons-solidjs";
import { For, Show } from "solid-js";
import styles from "./Stepper.module.css";

export interface StepperStep {
  id: string;
  label: string;
  state?: "complete" | "error";
}

export interface StepperProps {
  steps: StepperStep[];
  current: string;
  onSelect?: (id: string) => void;
  "aria-label"?: string;
}

function StepMarker(props: { index: number; state?: "complete" | "error" }) {
  return (
    <span class={[styles.marker, props.state ? styles[props.state] : ""].filter(Boolean).join(" ")}>
      <Show
        when={props.state === "complete"}
        fallback={
          <Show when={props.state === "error"} fallback={<span>{props.index}</span>}>
            <IconAlertTriangle aria-hidden="true" size={14} />
          </Show>
        }
      >
        <IconCircleCheck aria-hidden="true" size={14} />
      </Show>
    </span>
  );
}

/** A linear step indicator for multi-step flows (e.g. batch printer setup).
 *  Renders an `<ol>`; the current step carries `aria-current="step"`.
 *  Only `complete` steps are focusable/clickable (when `onSelect` is
 *  supplied) — a user can jump back to a step they already finished, but
 *  not ahead to one that isn't reachable yet, and not onto an `error`
 *  step (it needs to be revisited through the flow, not skipped to). */
export function Stepper(props: StepperProps) {
  return (
    <ol class={styles.list} aria-label={props["aria-label"]}>
      <For each={props.steps}>
        {(step, index) => {
          const isCurrent = () => step.id === props.current;
          const clickable = () => step.state === "complete" && props.onSelect !== undefined;

          return (
            <li
              class={styles.item}
              data-state={step.state}
              aria-current={isCurrent() ? "step" : undefined}
            >
              <Show
                when={clickable()}
                fallback={
                  <span class={styles.step}>
                    <StepMarker index={index() + 1} state={step.state} />
                    <span class={styles.label}>{step.label}</span>
                  </span>
                }
              >
                <button
                  type="button"
                  class={styles.stepButton}
                  onClick={() => props.onSelect?.(step.id)}
                >
                  <StepMarker index={index() + 1} state={step.state} />
                  <span class={styles.label}>{step.label}</span>
                </button>
              </Show>
            </li>
          );
        }}
      </For>
    </ol>
  );
}
