import { createSignal, Match, onCleanup, onMount, Show, Switch } from "solid-js";
import { Button } from "../design-system";
import { isCommandError } from "../ipc/client";
import type { ModelRecord } from "../library/types";
import type { CatalogRef } from "../printers/types";
import { createPreparation, slicing } from "../slicing/slicing-store";
import type { SliceTarget } from "../slicing/types";
import { PreparationPanel } from "./PreparationPanel";
import { createPreparationSession } from "./preparation-session";
import { PreparationWorkspace } from "./PreparationWorkspace";
import { TargetProfileDialog } from "./TargetProfileDialog";
import styles from "./PreparationMode.module.css";

export interface PreparationModeProps {
  /** An STL or 3MF Model. */
  model: ModelRecord;
  onBack: () => void;
  /** A finished slice's **Open the Slice Revision**. */
  onOpenRevision?: (sliceRevisionId: string) => void;
}

type Opening =
  | { kind: "opening" }
  | { kind: "open" }
  /** `needsTarget`: there's no Printer to default to, so a profile is asked for. */
  | { kind: "failed"; message: string; needsTarget: boolean };

/** D19 **Prepare…**: loads the Model's Preparation (creating it the first
 *  time) and shows the workspace for it. Loaded lazily from the Library,
 *  with the viewport and the tools. */
export function PreparationMode(props: PreparationModeProps) {
  const [opening, setOpening] = createSignal<Opening>({ kind: "opening" });
  // Leaving before the Preparation opens drops the result.
  let disposed = false;
  onCleanup(() => { disposed = true; });
  const open = (target?: SliceTarget) => {
    setOpening({ kind: "opening" });
    createPreparation(props.model.id, target).then(
      () => { if (!disposed) setOpening({ kind: "open" }); },
      (error: unknown) => {
        if (disposed) return;
        setOpening({
          kind: "failed",
          message: isCommandError(error) ? error.message : "The Preparation could not be opened.",
          needsTarget: isCommandError(error) && error.code === "VALIDATION" && error.details?.fieldPath === "target",
        });
      },
    );
  };
  onMount(() => open());

  // D20: with no Printer to default to, any catalog profile will do.
  const [choosingProfile, setChoosingProfile] = createSignal(false);
  const openForProfile = (catalogRef: CatalogRef) => {
    setChoosingProfile(false);
    open({ kind: "profile", catalogRef });
  };

  const unavailable = (message: string, needsTarget = false) => (
    <div class={styles.message}>
      <p role="alert">{message}</p>
      <div class={styles.actions}>
        <Show when={needsTarget}>
          <Button variant="primary" onClick={() => setChoosingProfile(true)}>Choose a printer profile…</Button>
          <TargetProfileDialog open={choosingProfile()} onOpenChange={setChoosingProfile} onPick={openForProfile} />
        </Show>
        <Button variant="secondary" onClick={props.onBack}>Back to Library</Button>
      </div>
    </div>
  );

  return (
    <Switch>
      <Match when={opening().kind === "opening"}>
        <p class={styles.message} role="status">Opening the Preparation…</p>
      </Match>
      <Match when={opening().kind === "failed" && opening()}>
        {(failed) => {
          const { message, needsTarget } = failed() as { message: string; needsTarget: boolean };
          return unavailable(message, needsTarget);
        }}
      </Match>
      <Match when={opening().kind === "open"}>
        <Show
          when={slicing.preparation(props.model.id)?.id}
          keyed
          fallback={unavailable("This Preparation was deleted.")}
        >
          {(_id) => {
            const session = createPreparationSession(() => props.model);
            return (
              <PreparationWorkspace
                session={session}
                onBack={props.onBack}
                dock={<PreparationPanel session={session} onOpenRevision={props.onOpenRevision} />}
              />
            );
          }}
        </Show>
      </Match>
    </Switch>
  );
}
