import { createSignal, For, Show } from "solid-js";
import { Button, Dialog } from "../design-system";
import { isCommandError } from "../ipc/client";
import { deleteModel } from "../library/library-store";
import type { ModelRecord } from "../library/types";
import styles from "./DeleteModelDialog.module.css";

export interface DeleteModelDialogProps {
  model: ModelRecord;
  onClose: () => void;
  /** The Model is gone; the caller closes the dialog and clears the
   *  selection. */
  onDeleted: () => void;
}

interface DeleteFailure {
  message: string;
  /** `LIFECYCLE_BLOCKED`'s reasons, one per blocker. */
  blockers: string[];
}

function blockerMessages(details: Record<string, unknown> | undefined): string[] {
  const blockers = details?.blockers;
  if (!Array.isArray(blockers)) return [];
  return blockers.flatMap((blocker) =>
    typeof blocker === "object" && blocker !== null && typeof (blocker as { message?: unknown }).message === "string"
      ? [(blocker as { message: string }).message]
      : [],
  );
}

/** D18: deleting a Model is permanent -- its revisions go too -- but the
 *  linked or imported file on disk is never touched. A later phase can
 *  block the delete (a Slice Revision or Queue Entry uses the Model); the
 *  blockers are listed here. */
export function DeleteModelDialog(props: DeleteModelDialogProps) {
  const [busy, setBusy] = createSignal(false);
  const [failure, setFailure] = createSignal<DeleteFailure | null>(null);

  const confirm = async () => {
    if (busy()) return;
    setBusy(true);
    setFailure(null);
    // Read before awaiting: once the Model leaves the store, the caller may
    // already have unmounted this dialog, and `props.model` with it.
    const { id, name } = props.model;
    try {
      await deleteModel(id);
      props.onDeleted();
    } catch (error) {
      if (isCommandError(error) && error.code === "LIFECYCLE_BLOCKED") {
        setFailure({ message: `${name} can't be deleted yet.`, blockers: blockerMessages(error.details) });
      } else {
        setFailure({ message: isCommandError(error) ? error.message : "The Model could not be deleted.", blockers: [] });
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title="Delete Model"
      open
      onOpenChange={(open) => {
        if (!open) props.onClose();
      }}
    >
      <div class={styles.body}>
        <p class={styles.text}>
          Delete {props.model.name}? Its imported revisions are deleted. The original file on disk is not.
        </p>
        <Show when={failure()}>
          {(current) => (
            <div class={styles.error} role="alert">
              <p>{current().message}</p>
              <Show when={current().blockers.length > 0}>
                <ul class={styles.blockers}>
                  <For each={current().blockers}>{(blocker) => <li>{blocker}</li>}</For>
                </ul>
              </Show>
            </div>
          )}
        </Show>
        <div class={styles.actions}>
          <Button variant="ghost" onClick={props.onClose}>Cancel</Button>
          <Button variant="danger" disabled={busy()} onClick={() => void confirm()}>Delete</Button>
        </div>
      </div>
    </Dialog>
  );
}
