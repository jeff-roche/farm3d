import { Show } from "solid-js";
import styles from "./Settings.module.css";

/** A one-line result under an export or import button: polite for an
 *  outcome, an alert for a failure. */
export function OperationStatus(props: { message: string | null; error?: boolean }) {
  return (
    <Show when={props.message}>
      {(message) => (
        <p class={props.error ? styles.error : styles.status} role={props.error ? "alert" : "status"}>
          {message()}
        </p>
      )}
    </Show>
  );
}

/** Runs an export or import and turns its outcome into a status line. */
export async function runOutcome<T extends { status: string }>(
  operation: () => Promise<T | undefined>,
  text: (outcome: T) => string | null,
  set: (state: { message: string | null; error: boolean }) => void,
): Promise<T | undefined> {
  set({ message: null, error: false });
  try {
    const outcome = await operation();
    if (outcome) {
      if (outcome.status === "unsupported") set({ message: "This needs the desktop app.", error: false });
      else set({ message: text(outcome), error: false });
    }
    return outcome;
  } catch (error) {
    set({ message: error instanceof Error ? error.message : String(error), error: true });
    return undefined;
  }
}
