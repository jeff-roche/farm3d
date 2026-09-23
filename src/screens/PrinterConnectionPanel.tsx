import { createResource, createSignal, Show } from "solid-js";
import { Button } from "../design-system";
import { buildMismatches, ConnectionFields, toSubmission, type ConnectionDraft } from "./ConnectionFields";
import { isCommandError } from "../ipc/client";
import {
  clearConnection,
  credentialStoreInfo,
  setConnection,
  testConnection,
} from "../printers/printer-store";
import type { ConnectionSubmission, ResolvedPrinter } from "../printers/types";
import styles from "./PrinterConnectionPanel.module.css";

export { buildMismatches };

const DEFAULT_PORTS: Record<string, number> = { moonraker: 7125, octoprint: 80 };

export interface PrinterConnectionPanelProps {
  printer: ResolvedPrinter;
}

export function PrinterConnectionPanel(props: PrinterConnectionPanelProps) {
  const existing = () => props.printer.connection;
  const initialKind = existing()?.kind ?? props.printer.profile.suggestedHostType ?? "moonraker";
  const [draft, setDraft] = createSignal<ConnectionDraft>({
    kind: initialKind,
    host: existing()?.host ?? "",
    port: existing()?.port ?? DEFAULT_PORTS[initialKind] ?? 7125,
    useTls: existing()?.useTls ?? false,
    // Never seeded from storage — a stored secret is never echoed back. Empty
    // therefore means "leave it alone", which is why `toSubmission` sends
    // `undefined` rather than `""` for a blank credential.
    credential: "",
  });

  const [store] = createResource(credentialStoreInfo);
  const [saveError, setSaveError] = createSignal<string | null>(null);
  // The exact submission a failed Save was attempted with, so "Save anyway"
  // (D8's `acceptUnverified`) resubmits it unchanged -- `draft().credential`
  // is blanked right after the failed attempt below (never re-echoing a
  // typed secret), so re-deriving the submission from the draft at that
  // point would silently drop it.
  const [failedSubmission, setFailedSubmission] = createSignal<ConnectionSubmission | null>(null);

  async function attemptSave(submission: ConnectionSubmission, acceptUnverified?: boolean) {
    try {
      if (acceptUnverified !== undefined) {
        await setConnection(props.printer.id, submission, acceptUnverified);
      } else {
        await setConnection(props.printer.id, submission);
      }
      setSaveError(null);
      setFailedSubmission(null);
    } catch (e) {
      // `setConnection` rejects (Ruling R2, superseded by spec D8) so a
      // replacement probe failure can be shown inline next to a "Save
      // anyway" affordance, instead of only reaching the store's error
      // banner.
      setSaveError(isCommandError(e) ? e.message : "The Connection could not be saved.");
      setFailedSubmission(submission);
    }
  }

  async function onSave() {
    const submission = toSubmission(draft(), "edit");
    try {
      await attemptSave(submission);
    } finally {
      setDraft((d) => ({ ...d, credential: "" }));
    }
  }

  async function onSaveAnyway() {
    const submission = failedSubmission();
    if (!submission) return;
    await attemptSave(submission, true);
  }

  return (
    <div class={styles.panel}>
      <ConnectionFields
        value={draft()}
        onChange={setDraft}
        onTest={() => testConnection(props.printer.id, toSubmission(draft(), "edit"))}
        profile={props.printer.profile}
        credentialHint={existing()?.credentialRef ? "Stored — leave blank to keep" : undefined}
      />

      <Show when={store()}>
        {(info) => (
          <p class={styles.note}>
            Credentials stored in:{" "}
            {info().kind === "keychain" ? "OS keychain" : "credentials.json"}
            <Show when={info().reasonCode}>
              {(reason) => <span class={styles.warn}> — no OS keychain available ({reason()})</span>}
            </Show>
          </p>
        )}
      </Show>

      <Show when={saveError()}>
        {(message) => (
          <div class={styles.saveError}>
            <p class={styles.error} role="alert">{message()}</p>
            <Button variant="secondary" onClick={() => void onSaveAnyway()}>
              Save anyway
            </Button>
          </div>
        )}
      </Show>

      <div class={styles.actions}>
        <Button variant="primary" onClick={() => void onSave()}>
          Save
        </Button>
        <Show when={existing()}>
          <Button variant="danger" onClick={() => void clearConnection(props.printer.id)}>
            Disconnect
          </Button>
        </Show>
      </div>
    </div>
  );
}
