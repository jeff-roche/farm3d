import { onMount, Show } from "solid-js";
import { diagnostics, loadAbout } from "../../diagnostics/diagnostics-store";
import styles from "./Settings.module.css";

/** About category: what this build is (`about_farm3d`). */
export function AboutSettings() {
  onMount(() => {
    void loadAbout().catch(() => {});
  });

  return (
    <div class={styles.category}>
      <h3 class={styles.heading}>About</h3>
      <Show when={diagnostics.about()} fallback={<p class={styles.status} role="status">Loading…</p>}>
        {(about) => (
          <dl class={styles.definitions}>
            <dt>Version</dt>
            <dd>farm3d {about().appVersion}</dd>
            <dt>Platform</dt>
            <dd>{about().platform.os} ({about().platform.arch})</dd>
            <dt>Database schema</dt>
            <dd>{about().schemaVersion}</dd>
            <dt>Backup format</dt>
            <dd>{about().backupFormatVersion}</dd>
            <dt>Printer catalog</dt>
            <dd>{about().catalog.sourceTag} ({about().catalog.generatedAt})</dd>
            <dt>OrcaSlicer</dt>
            <dd>{`${about().slicer.configured ? "Configured" : "Not configured"}${about().slicer.version ? `, version ${about().slicer.version}` : ""}`}</dd>
          </dl>
        )}
      </Show>
    </div>
  );
}
