import { createMemo, createSignal, For, onMount, Show } from "solid-js";
import { Button, Checkbox } from "../../design-system";
import { diagnostics, exportDiagnostics, loadDiagnosticsPreview } from "../../diagnostics/diagnostics-store";
import {
  DIAGNOSTICS_NEVER_INCLUDED, diagnosticsSectionDescription, diagnosticsSectionLabel, exportSummaryText, formatBytes,
} from "../../diagnostics/presentation";
import type { DiagnosticsSection } from "../../diagnostics/types";
import { isCommandError } from "../../ipc/client";
import { OperationStatus } from "./OperationStatus";
import { ResetPanel } from "./ResetPanel";
import styles from "./Settings.module.css";
import portability from "./Portability.module.css";

function exportErrorText(error: unknown): string {
  if (isCommandError(error) && error.code === "DIAGNOSTICS_REDACTION_FAILED") {
    const section = error.details?.section;
    const where = typeof section === "string" ? ` in ${diagnosticsSectionLabel(section as DiagnosticsSection)}` : "";
    return `Redaction failed${where}, so nothing was written.`;
  }
  return error instanceof Error ? error.message : (error as { message?: string })?.message ?? String(error);
}

/** Diagnostics category: the diagnostics bundle, then Reset. */
export function DiagnosticsSettings() {
  const [excluded, setExcluded] = createSignal<ReadonlySet<DiagnosticsSection>>(new Set());
  const [busy, setBusy] = createSignal(false);
  const [result, setResult] = createSignal<{ message: string | null; error: boolean }>({ message: null, error: false });

  onMount(() => {
    void loadDiagnosticsPreview().catch(() => {});
  });

  const chosen = createMemo(() => (diagnostics.preview()?.sections ?? []).map((entry) => entry.section).filter((section) => !excluded().has(section)));

  function toggle(section: DiagnosticsSection, on: boolean) {
    setExcluded((current) => {
      const next = new Set(current);
      if (on) next.delete(section);
      else next.add(section);
      return next;
    });
  }

  async function run() {
    setBusy(true);
    setResult({ message: null, error: false });
    try {
      const outcome = await exportDiagnostics(chosen());
      setResult({ message: exportSummaryText(outcome), error: false });
    } catch (error) {
      setResult({ message: exportErrorText(error), error: true });
    }
    setBusy(false);
  }

  return (
    <div class={styles.category}>
      <h3 class={styles.heading}>Diagnostics</h3>
      <section class={styles.section} aria-label="Diagnostics bundle">
        <span class={styles.sectionTitle}>Diagnostics bundle</span>
        <p class={styles.note}>A zip file to attach to a bug report. Choose what goes in.</p>
        <Show when={diagnostics.preview()} fallback={<p class={styles.status}>Loading…</p>}>
          {(preview) => (
            <ul class={portability.rows}>
              <For each={preview().sections}>
                {(entry) => (
                  <li class={portability.row}>
                    <span class={portability.grow}>
                      <Checkbox checked={!excluded().has(entry.section)} onChange={(on) => toggle(entry.section, on)}>
                        {diagnosticsSectionLabel(entry.section)}
                      </Checkbox>
                      <span class={portability.meta}>{diagnosticsSectionDescription(entry.section)}</span>
                    </span>
                    <span class={`${portability.meta} ${portability.numeric}`}>{formatBytes(entry.estimatedBytes)}</span>
                  </li>
                )}
              </For>
            </ul>
          )}
        </Show>
        <p class={styles.note}>{DIAGNOSTICS_NEVER_INCLUDED}</p>
        <div class={styles.actions}>
          <Button variant="primary" disabled={busy() || chosen().length === 0} onClick={() => void run()}>
            Export diagnostics…
          </Button>
        </div>
        <OperationStatus message={result().message} error={result().error} />
      </section>
      <section class={styles.section} aria-label="Reset">
        <ResetPanel />
      </section>
    </div>
  );
}
