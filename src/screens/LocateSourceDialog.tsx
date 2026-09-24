import { createSignal, createUniqueId, onCleanup, Show } from "solid-js";
import { Button, Dialog } from "../design-system";
import { desktopAvailable, isCommandError } from "../ipc/client";
import { cancelSelection, convertToManaged, locateSource, pickFiles } from "../library/library-store";
import type { ModelRecord } from "../library/types";
import styles from "./LocateSourceDialog.module.css";

export interface RecoveryDialogProps {
  /** A linked Model. */
  model: ModelRecord;
  /** Closed: cancelled, or done. The Model's record settles in the store. */
  onClose: () => void;
}

const LOCATE_UNAVAILABLE = "Locating a source file needs the desktop app.";
const CONVERT_UNAVAILABLE = "Converting to managed storage needs the desktop app.";

/** The last part of a path, whichever separator it uses. */
function fileName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

function failureMessage(error: unknown): string {
  return isCommandError(error) ? error.message : "The source could not be located.";
}

/** The file a Locate selection holds (a Locate picks exactly one). */
interface Located {
  selectionId: string;
  fileIndex: number;
  fileName: string;
}

/** D16's **Locate source…**: pick the file where it is now, then relink.
 *  When its bytes differ from the current revision, the backend refuses
 *  with `SOURCE_CONTENT_DIFFERS` until the user agrees to store it as a new
 *  revision. Every failure shows here, inline. */
export function LocateSourceDialog(props: RecoveryDialogProps) {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  // A selection the backend refused as different: kept for the consent
  // retry, and cancelled if the user walks away instead.
  const [differs, setDiffers] = createSignal<Located | null>(null);
  const reasonId = createUniqueId();
  const linkedPath = () => props.model.link?.path ?? "";
  const missing = () => props.model.link?.state === "missing";

  // The dialog may be gone when a cancel settles, and an unused selection
  // expires on its own anyway, so a failed cancel needs no message.
  const release = (selectionId: string) => void cancelSelection(selectionId).catch(() => {});
  const discardPending = () => {
    const pending = differs();
    setDiffers(null);
    if (pending) release(pending.selectionId);
  };
  // No closing mid-request. The dialog can still unmount under a request
  // (its Model deleted elsewhere); a late result then only releases its
  // selection and touches nothing else.
  let disposed = false;
  onCleanup(() => {
    disposed = true;
    discardPending();
  });
  const close = () => {
    if (busy()) return;
    discardPending();
    props.onClose();
  };

  const locate = async (located: Located, acceptDifferentContent: boolean) => {
    try {
      await locateSource(props.model.id, located.selectionId, located.fileIndex, acceptDifferentContent);
      if (disposed) return;
      // The backend used the selection, so there's nothing left to cancel.
      setDiffers(null);
      props.onClose();
    } catch (failure) {
      if (disposed) {
        release(located.selectionId);
        return;
      }
      if (isCommandError(failure) && failure.code === "SOURCE_CONTENT_DIFFERS") {
        setDiffers(located);
      } else {
        // A refused locate leaves its selection behind; this one is done.
        setDiffers(null);
        release(located.selectionId);
        setError(failureMessage(failure));
      }
    }
  };

  const chooseFile = async () => {
    if (busy()) return;
    setBusy(true);
    setError(null);
    discardPending();
    try {
      const selection = await pickFiles("locate");
      if (selection && disposed) {
        release(selection.selectionId);
        return;
      }
      const file = selection?.files[0];
      if (!selection || !file) return;
      await locate({ selectionId: selection.selectionId, fileIndex: file.fileIndex, fileName: file.fileName }, false);
    } catch (failure) {
      if (!disposed) setError(failureMessage(failure));
    } finally {
      if (!disposed) setBusy(false);
    }
  };

  const relink = async () => {
    const located = differs();
    if (!located || busy()) return;
    setBusy(true);
    setError(null);
    try {
      await locate(located, true);
    } finally {
      if (!disposed) setBusy(false);
    }
  };

  return (
    <Dialog
      title="Locate source"
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
    >
      <div class={styles.body}>
        <p class={styles.text}>
          {missing()
            ? `farm3d can't find “${fileName(linkedPath())}”.`
            : `farm3d can't use “${fileName(linkedPath())}” right now.`}{" "}
          It was linked at:
        </p>
        <code class={styles.path}>{linkedPath()}</code>
        <p class={styles.note}>Choose the file where it is now. The revisions already stored are kept.</p>
        <Show when={differs()}>
          {(located) => (
            <div class={styles.differs}>
              <p class={styles.text}>This file is different from the last imported version.</p>
              <p class={styles.note}>
                Relink to “{located().fileName}” and store its contents as a new revision, or choose another file.
              </p>
            </div>
          )}
        </Show>
        <Show when={error()}>{(message) => <p class={styles.error} role="alert">{message()}</p>}</Show>
        <Show when={!desktopAvailable()}>
          <p id={reasonId} class={styles.note}>{LOCATE_UNAVAILABLE}</p>
        </Show>
        <div class={styles.actions}>
          <Button variant="ghost" disabled={busy()} onClick={close}>Cancel</Button>
          <Button
            variant={differs() ? "secondary" : "primary"}
            disabled={busy() || !desktopAvailable()}
            aria-describedby={desktopAvailable() ? undefined : reasonId}
            onClick={() => void chooseFile()}
          >
            {differs() ? "Choose another file…" : "Choose file…"}
          </Button>
          <Show when={differs()}>
            <Button variant="primary" disabled={busy()} onClick={() => void relink()}>
              Relink and import as a new revision
            </Button>
          </Show>
        </div>
      </div>
    </Dialog>
  );
}

/** D5's **Convert to managed**, confirmed first: farm3d stops following the
 *  file. Every revision already holds its bytes, so nothing is copied and
 *  it works whatever the source state. */
export function ConvertToManagedDialog(props: RecoveryDialogProps) {
  // Read once: converting clears the link, and the dialog must not lose
  // the name while it closes.
  const linkedName = fileName(props.model.link?.path ?? "");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const reasonId = createUniqueId();
  const requestClose = () => {
    if (!busy()) props.onClose();
  };

  const convert = async () => {
    if (busy()) return;
    setBusy(true);
    setError(null);
    try {
      await convertToManaged(props.model.id);
      props.onClose();
    } catch (failure) {
      setError(isCommandError(failure) ? failure.message : "The Model could not be converted.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title="Convert to managed"
      open
      onOpenChange={(open) => {
        if (!open) requestClose();
      }}
    >
      <div class={styles.body}>
        <p class={styles.text}>
          farm3d will stop following {linkedName}. Your existing revisions are kept.
        </p>
        <Show when={error()}>{(message) => <p class={styles.error} role="alert">{message()}</p>}</Show>
        <Show when={!desktopAvailable()}>
          <p id={reasonId} class={styles.note}>{CONVERT_UNAVAILABLE}</p>
        </Show>
        <div class={styles.actions}>
          <Button variant="ghost" disabled={busy()} onClick={requestClose}>Cancel</Button>
          <Button
            variant="primary"
            disabled={busy() || !desktopAvailable()}
            aria-describedby={desktopAvailable() ? undefined : reasonId}
            onClick={() => void convert()}
          >
            Convert to managed
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
