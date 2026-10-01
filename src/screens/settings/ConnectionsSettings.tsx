import { createSignal, onMount, Show } from "solid-js";
import { Button } from "../../design-system";
import { diagnostics, loadAbout } from "../../diagnostics/diagnostics-store";
import { exportPrinters, importPrinters } from "../../printers/printer-store";
import { OperationStatus, runOutcome } from "./OperationStatus";
import styles from "./Settings.module.css";

export interface ConnectionsSettingsProps {
  /** Called after an import changed the Printers, so the shell can settle
   *  first-run state and re-check the current selection. */
  onPrintersImported?: () => void;
}

/** Connections category: the Printers file's export and import, and which
 *  tier holds Printer credentials. */
export function ConnectionsSettings(props: ConnectionsSettingsProps) {
  const [result, setResult] = createSignal<{ message: string | null; error: boolean }>({ message: null, error: false });
  const [busy, setBusy] = createSignal(false);

  onMount(() => {
    void loadAbout().catch(() => {});
  });

  async function run(operation: () => Promise<{ status: string } | undefined>, applied: string, done: string) {
    setBusy(true);
    const outcome = await runOutcome(operation, (o) => (o.status === applied ? done : null), setResult);
    setBusy(false);
    if (outcome?.status === "applied") props.onPrintersImported?.();
  }

  const credentialStore = () => {
    const store = diagnostics.about()?.credentialStore;
    if (!store) return "Checking…";
    const name = store.kind === "keychain" ? "The system keychain" : "A protected file in farm3d's data folder";
    return store.available ? name : `${name} (unavailable)`;
  };

  return (
    <div class={styles.category}>
      <h3 class={styles.heading}>Connections</h3>
      <section class={styles.section} aria-label="Printers file">
        <span class={styles.sectionTitle}>Printers file</span>
        <p class={styles.note}>Export or import your Printers, without their credentials, as a file.</p>
        <div class={styles.actions}>
          <Button variant="secondary" disabled={busy()} onClick={() => void run(exportPrinters, "exported", "Printers exported.")}>
            Export Printers…
          </Button>
          <Button variant="secondary" disabled={busy()} onClick={() => void run(importPrinters, "applied", "Printers imported.")}>
            Import Printers…
          </Button>
        </div>
        <OperationStatus message={result().message} error={result().error} />
      </section>
      <section class={styles.section} aria-label="Credential store">
        <span class={styles.sectionTitle}>Credential store</span>
        <p class={styles.status}>{credentialStore()}</p>
        <Show when={diagnostics.about()?.credentialStore.kind === "fallbackFile"}>
          <p class={styles.note}>No system keychain was available, so Printer credentials are kept in a protected file instead.</p>
        </Show>
      </section>
    </div>
  );
}
