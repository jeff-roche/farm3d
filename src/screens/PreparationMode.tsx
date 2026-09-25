import { createSignal, Match, onCleanup, onMount, Show, Switch } from "solid-js";
import { Button } from "../design-system";
import { isCommandError } from "../ipc/client";
import type { ModelRecord } from "../library/types";
import { createPreparation, slicing } from "../slicing/slicing-store";
import { createPreparationSession } from "./preparation-session";
import { PreparationWorkspace } from "./PreparationWorkspace";
import styles from "./PreparationMode.module.css";

export interface PreparationModeProps {
  /** An STL or 3MF Model. */
  model: ModelRecord;
  onBack: () => void;
}

type Opening = { kind: "opening" } | { kind: "open" } | { kind: "failed"; message: string };

/** D19 **Prepare…**: loads the Model's Preparation (creating it the first
 *  time) and shows the workspace for it. Loaded lazily from the Library,
 *  with the viewport and the tools. */
export function PreparationMode(props: PreparationModeProps) {
  const [opening, setOpening] = createSignal<Opening>({ kind: "opening" });
  // Leaving before the Preparation opens drops the result.
  let disposed = false;
  onCleanup(() => { disposed = true; });
  onMount(() => {
    createPreparation(props.model.id).then(
      () => { if (!disposed) setOpening({ kind: "open" }); },
      (error: unknown) => {
        if (disposed) return;
        setOpening({
          kind: "failed",
          message: isCommandError(error) ? error.message : "The Preparation could not be opened.",
        });
      },
    );
  });

  const unavailable = (message: string) => (
    <div class={styles.message}>
      <p role="alert">{message}</p>
      <Button variant="secondary" onClick={props.onBack}>Back to Library</Button>
    </div>
  );

  return (
    <Switch>
      <Match when={opening().kind === "opening"}>
        <p class={styles.message} role="status">Opening the Preparation…</p>
      </Match>
      <Match when={opening().kind === "failed" && opening()}>
        {(failed) => unavailable((failed() as { message: string }).message)}
      </Match>
      <Match when={opening().kind === "open"}>
        <Show
          when={slicing.preparation(props.model.id)?.id}
          keyed
          fallback={unavailable("This Preparation was deleted.")}
        >
          {(_id) => {
            const session = createPreparationSession(() => props.model);
            return <PreparationWorkspace session={session} onBack={props.onBack} />;
          }}
        </Show>
      </Match>
    </Switch>
  );
}
