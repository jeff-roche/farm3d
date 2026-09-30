import { createSignal } from "solid-js";
import { Button, Select } from "../../design-system";
import type { MonitorStore } from "../../monitor/monitor-store";
import { exportSettings, importSettings, settings, updateSettings } from "../../settings/settings-store";
import type { MonitorDensity } from "../../generated/contracts/domain/MonitorDensity";
import type { MonitorSection } from "../../generated/contracts/domain/MonitorSection";
import { OperationStatus, runOutcome } from "./OperationStatus";
import styles from "./Settings.module.css";

const SECTIONS: readonly { value: MonitorSection; label: string }[] = [
  { value: "printerModel", label: "Printer model" },
  { value: "location", label: "Location" },
  { value: "operationalState", label: "Operational state" },
  { value: "none", label: "No section" },
];

const DENSITIES: readonly { value: MonitorDensity; label: string }[] = [
  { value: "comfortable", label: "Comfortable" },
  { value: "compact", label: "Compact" },
];

export interface GeneralSettingsProps {
  /** The Monitor's store, when it is running: its section and density are
   *  the live values, and it persists them. Without it the settings file is
   *  edited directly. */
  monitor?: MonitorStore;
}

/** General category: the Monitor's section and density, and the settings
 *  file's export and import. */
export function GeneralSettings(props: GeneralSettingsProps) {
  const [result, setResult] = createSignal<{ message: string | null; error: boolean }>({ message: null, error: false });
  const [busy, setBusy] = createSignal(false);
  const [preferenceError, setPreferenceError] = createSignal<string | null>(null);

  const section = () => props.monitor?.section() ?? settings()?.monitorSection ?? "printerModel";
  const density = () => props.monitor?.density() ?? settings()?.monitorDensity ?? "comfortable";

  function setPreference(patch: { monitorSection?: MonitorSection; monitorDensity?: MonitorDensity }) {
    setPreferenceError(null);
    if (props.monitor) {
      if (patch.monitorSection) props.monitor.setSection(patch.monitorSection);
      if (patch.monitorDensity) props.monitor.setDensity(patch.monitorDensity);
      return;
    }
    void updateSettings(patch).catch(() => setPreferenceError("This preference could not be saved."));
  }

  async function run(operation: () => Promise<{ status: string } | undefined>, text: (status: string) => string | null) {
    setBusy(true);
    await runOutcome(operation, (outcome) => text(outcome.status), setResult);
    setBusy(false);
  }

  return (
    <div class={styles.category}>
      <h3 class={styles.heading}>General</h3>
      <section class={styles.section} aria-label="Monitor">
        <span class={styles.sectionTitle}>Monitor</span>
        <div class={styles.preferences}>
          <Select
            label="Monitor section"
            options={[...SECTIONS]}
            value={SECTIONS.find((option) => option.value === section()) ?? SECTIONS[0]}
            optionValue={(option) => option.value}
            optionLabel={(option) => option.label}
            onChange={(option) => setPreference({ monitorSection: option.value })}
          />
          <Select
            label="Monitor density"
            options={[...DENSITIES]}
            value={DENSITIES.find((option) => option.value === density()) ?? DENSITIES[0]}
            optionValue={(option) => option.value}
            optionLabel={(option) => option.label}
            onChange={(option) => setPreference({ monitorDensity: option.value })}
          />
        </div>
        <OperationStatus message={preferenceError() ?? props.monitor?.preferenceError() ?? null} error />
      </section>
      <section class={styles.section} aria-label="Settings file">
        <span class={styles.sectionTitle}>Settings file</span>
        <p class={styles.note}>Export or import the theme, Monitor, notification, and retention settings as a file.</p>
        <div class={styles.actions}>
          <Button variant="secondary" disabled={busy()} onClick={() => void run(exportSettings, (s) => (s === "exported" ? "Settings exported." : null))}>
            Export settings…
          </Button>
          <Button variant="secondary" disabled={busy()} onClick={() => void run(importSettings, (s) => (s === "applied" ? "Settings imported." : null))}>
            Import settings…
          </Button>
        </div>
        <OperationStatus message={result().message} error={result().error} />
      </section>
    </div>
  );
}
