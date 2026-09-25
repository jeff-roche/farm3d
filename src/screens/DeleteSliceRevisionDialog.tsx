import { createSignal, For, Show } from "solid-js";
import { Button, Dialog } from "../design-system";
import { isCommandError } from "../ipc/client";
import { revisionTitle } from "../slicing/revision-presentation";
import { deleteSliceRevision } from "../slicing/slicing-store";
import type { SliceRevisionSummary } from "../slicing/types";
import { blockerMessages } from "./DeleteModelDialog";
import styles from "./DeleteSliceRevisionDialog.module.css";

export interface DeleteSliceRevisionDialogProps {
  revision: SliceRevisionSummary;
  onClose: () => void;
  /** The revision is gone; the caller closes the dialog and leaves the
   *  review. */
  onDeleted: (sliceRevisionId: string) => void;
}

interface DeleteFailure {
  message: string;
  blockers: string[];
}

/** D14, open question 2: a Slice Revision is never edited, but it can be
 *  deleted while nothing depends on it. The backend refuses with
 *  `LIFECYCLE_BLOCKED` otherwise, and the blockers are listed here. */
export function DeleteSliceRevisionDialog(props: DeleteSliceRevisionDialogProps) {
  const [busy, setBusy] = createSignal(false);
  const [failure, setFailure] = createSignal<DeleteFailure | null>(null);
  const title = () => revisionTitle(props.revision);
  const requestClose = () => {
    if (!busy()) props.onClose();
  };

  const confirm = async () => {
    if (busy()) return;
    setBusy(true);
    setFailure(null);
    const { id } = props.revision;
    const name = title();
    try {
      await deleteSliceRevision(id);
      props.onDeleted(id);
    } catch (error) {
      if (isCommandError(error) && error.code === "LIFECYCLE_BLOCKED") {
        setFailure({ message: `${name} can't be deleted yet.`, blockers: blockerMessages(error.details) });
      } else {
        setFailure({ message: isCommandError(error) ? error.message : "The Slice Revision could not be deleted.", blockers: [] });
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title="Delete Slice Revision"
      open
      onOpenChange={(open) => {
        if (!open) requestClose();
      }}
    >
      <div class={styles.body}>
        <p class={styles.text}>
          {props.revision.kind === "farm3d"
            ? `Delete ${title()}? Its G-code, presets and log are removed from farm3d. The Model is not changed.`
            : `Delete ${title()}? The facts you confirmed are removed. The G-code stays with the Model.`}
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
          <Button variant="ghost" disabled={busy()} onClick={requestClose}>Cancel</Button>
          <Button variant="danger" disabled={busy()} onClick={() => void confirm()}>Delete</Button>
        </div>
      </div>
    </Dialog>
  );
}
