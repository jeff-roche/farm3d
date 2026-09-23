import { createResource, createSignal, Show } from "solid-js";
import { Button } from "../design-system";
import { buildMismatches, ConnectionFields, toSubmission, type ConnectionDraft } from "./ConnectionFields";
import {
  clearConnection,
  credentialStoreInfo,
  reportError,
  setConnection,
  testConnection,
} from "../printers/printer-store";
import type { ResolvedPrinter } from "../printers/types";
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

  async function onSave() {
    try {
      await setConnection(props.printer.id, toSubmission(draft(), "edit"));
    } catch (e) {
      // `setConnection` now rejects (Ruling R2) so a caller that renders the
      // failure inline can offer "Save anyway" (`acceptUnverified`). This
      // panel doesn't do that yet (Task 11), so it falls back to the same
      // store error banner `setConnection` used to populate itself.
      reportError(e);
    } finally {
      setDraft((d) => ({ ...d, credential: "" }));
    }
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
