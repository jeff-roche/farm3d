import { createResource, createSignal, Show } from "solid-js";
import { Button } from "../design-system";
import {
  buildMismatches,
  ConnectionFields,
  connectionDraftChanged,
  supportedKind,
  toSubmission,
  type ConnectionDraft,
} from "./ConnectionFields";
import { isCommandError } from "../ipc/client";
import {
  clearConnection,
  credentialStoreInfo,
  reportError,
  setConnection,
  testConnection,
} from "../printers/printer-store";
import type { ConnectionSubmission, ResolvedPrinter } from "../printers/types";
import { HostOperationAlert } from "./HostOperationAlert";
import { RemoveCredentialsDialog } from "./RemoveCredentialsDialog";
import styles from "./PrinterConnectionPanel.module.css";

export { buildMismatches };

const DEFAULT_PORTS: Record<string, number> = { moonraker: 7125, octoprint: 80 };

/** The error codes a replacement probe (D8) can fail with -- the only
 *  failures "Save anyway" (`acceptUnverified`) can actually get past. */
const PROBE_ERROR_CODES = new Set([
  "PRINTER_UNREACHABLE",
  "TIMEOUT",
  "AUTHENTICATION_FAILED",
  "PROTOCOL_ERROR",
]);

export interface PrinterConnectionPanelProps {
  printer: ResolvedPrinter;
}

export function PrinterConnectionPanel(props: PrinterConnectionPanelProps) {
  const existing = () => props.printer.connection;
  const initialKind = existing()?.kind ?? supportedKind(props.printer.profile.suggestedHostType) ?? "moonraker";
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
  const [removingCredentials, setRemovingCredentials] = createSignal(false);
  const [saveError, setSaveError] = createSignal<string | null>(null);
  // `CONNECTION_IN_USE` (spec D7: a Host Operation on this Printer is
  // unresolved) from Save or Disconnect, shown with its link to the Job tab.
  const [inUse, setInUse] = createSignal<unknown>(null);
  // The exact submission a failed Save was attempted with, so "Save anyway"
  // (D8's `acceptUnverified`) resubmits it unchanged -- `draft().credential`
  // is blanked right after the failed attempt below (never re-echoing a
  // typed secret), so re-deriving the submission from the draft at that
  // point would silently drop it.
  // Only set for a probe failure; any other Save error has nothing
  // `acceptUnverified` could get past.
  const [failedSubmission, setFailedSubmission] = createSignal<ConnectionSubmission | null>(null);

  function clearSaveFailure() {
    setSaveError(null);
    setFailedSubmission(null);
    setInUse(null);
  }

  // A failed Save describes the exact submission it was attempted with; a
  // materially edited draft makes both the error and "Save anyway" stale.
  function onDraftChange(next: ConnectionDraft) {
    if (connectionDraftChanged(draft(), next)) clearSaveFailure();
    setDraft(next);
  }

  async function attemptSave(submission: ConnectionSubmission, acceptUnverified?: boolean) {
    try {
      if (acceptUnverified !== undefined) {
        await setConnection(props.printer.id, submission, acceptUnverified);
      } else {
        await setConnection(props.printer.id, submission);
      }
      clearSaveFailure();
    } catch (e) {
      if (isCommandError(e) && e.code === "CONNECTION_IN_USE") {
        clearSaveFailure();
        setInUse(e);
        return;
      }
      // `setConnection` rejects (Ruling R2, superseded by spec D8) so a
      // replacement probe failure can be shown inline next to a "Save
      // anyway" affordance, instead of only reaching the store's error
      // banner.
      setSaveError(isCommandError(e) ? e.message : "The Connection could not be saved.");
      setFailedSubmission(isCommandError(e) && PROBE_ERROR_CODES.has(e.code) ? submission : null);
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

  async function onDisconnect() {
    clearSaveFailure();
    try {
      await clearConnection(props.printer.id);
    } catch (e) {
      if (isCommandError(e) && e.code === "CONNECTION_IN_USE") setInUse(e);
      else reportError(e);
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
        onChange={onDraftChange}
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
            <Show when={failedSubmission()}>
              <Button variant="secondary" onClick={() => void onSaveAnyway()}>
                Save anyway
              </Button>
            </Show>
          </div>
        )}
      </Show>

      <Show when={inUse()}>
        {(error) => (
          <HostOperationAlert
            error={error()}
            fallback="Finish or abandon the pending printer operation before changing this Connection."
            printerId={props.printer.id}
          />
        )}
      </Show>

      <div class={styles.actions}>
        <Button variant="primary" onClick={() => void onSave()}>
          Save
        </Button>
        <Show when={existing()?.credentialRef}>
          <Button variant="secondary" onClick={() => setRemovingCredentials(true)}>
            Remove credentials
          </Button>
        </Show>
        <Show when={existing()}>
          <Button variant="danger" onClick={() => void onDisconnect()}>
            Disconnect
          </Button>
        </Show>
      </div>

      <RemoveCredentialsDialog
        open={removingCredentials()}
        onOpenChange={setRemovingCredentials}
        printer={props.printer}
      />
    </div>
  );
}
