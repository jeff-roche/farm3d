import { createEffect, createSignal, on, Show } from "solid-js";
import { Button, RadioGroup, Switch } from "../design-system";
import { command, desktopAvailable, isCommandError, retryOnTransportFailure } from "../ipc/client";
import type { AlertDefaults, NotificationMode } from "../attention/types";
import styles from "./AlertDefaultsSection.module.css";

/** D12: no `printer_alert_defaults` row means these (mirrors
 *  `AlertDefaults::default` on the Rust side). */
export const DEFAULT_ALERT_DEFAULTS: AlertDefaults = {
  offlineAfterMinutes: 5,
  notifications: "follow",
  snapshotOnIncident: true,
  snapshotOnCompletion: true,
};

const OFFLINE_OPTIONS = [
  { value: "off", label: "Off" },
  { value: "1", label: "1 minute" },
  { value: "5", label: "5 minutes" },
  { value: "15", label: "15 minutes" },
];

const NOTIFICATION_MODE_OPTIONS = [
  { value: "follow", label: "Follow the notification settings" },
  { value: "muted", label: "Muted" },
];

function offlineOptionValue(minutes: 1 | 5 | 15 | null): string {
  return minutes === null ? "off" : String(minutes);
}

function parseOfflineOption(value: string): 1 | 5 | 15 | null {
  return value === "off" ? null : (Number(value) as 1 | 5 | 15);
}

/** Review's one-line summary (spec "Setup": "Review summarizes both"). */
export function alertDefaultsSummary(defaults: AlertDefaults): string {
  const offline = defaults.offlineAfterMinutes === null
    ? "offline alert off"
    : `offline after ${defaults.offlineAfterMinutes} min`;
  const notifications = defaults.notifications === "muted" ? "notifications muted" : "notifications follow settings";
  return `${offline}, ${notifications}`;
}

export type AlertDefaultsSectionProps =
  | {
      mode: "draft";
      /** The wizard's own local draft (submitted with `createPrinter` at
       *  Save). Renders with D12's own defaults until touched. */
      value: AlertDefaults;
      onChange: (value: AlertDefaults) => void;
    }
  | {
      mode: "printer";
      /** An existing Printer (the dock's Setup tab): loads its row via
       *  `get_printer_alert_defaults`, and Save goes through
       *  `set_printer_alert_defaults`. */
      printerId: string;
    };

/** The Printer wizard's Operate step and the dock's Setup tab share this
 *  editor (spec "Frontend architecture" → "Setup"): the offline grace, the
 *  notification mode, and the two capture toggles, all D12 alert defaults. */
export function AlertDefaultsSection(props: AlertDefaultsSectionProps) {
  const [printerValue, setPrinterValue] = createSignal<AlertDefaults>({ ...DEFAULT_ALERT_DEFAULTS });
  const [loading, setLoading] = createSignal(false);
  const [loadError, setLoadError] = createSignal<unknown>(null);
  const [saving, setSaving] = createSignal(false);
  const [saveError, setSaveError] = createSignal<unknown>(null);

  const value = (): AlertDefaults => (props.mode === "draft" ? props.value : printerValue());
  function setValue(next: AlertDefaults): void {
    if (props.mode === "draft") props.onChange(next);
    else setPrinterValue(next);
  }

  async function load(printerId: string): Promise<void> {
    setLoading(true);
    setLoadError(null);
    try {
      if (!desktopAvailable()) {
        setPrinterValue({ ...DEFAULT_ALERT_DEFAULTS });
        return;
      }
      const result = await command("get_printer_alert_defaults", { printerId });
      setPrinterValue(result.alertDefaults);
    } catch (e) {
      setLoadError(e);
    } finally {
      setLoading(false);
    }
  }

  createEffect(
    on(
      () => (props.mode === "printer" ? props.printerId : null),
      (printerId) => {
        if (printerId) void load(printerId);
      },
    ),
  );

  async function onSave(): Promise<void> {
    if (props.mode !== "printer") return;
    const printerId = props.printerId;
    setSaving(true);
    setSaveError(null);
    try {
      const result = await retryOnTransportFailure(() =>
        command("set_printer_alert_defaults", { operationId: crypto.randomUUID(), printerId, alertDefaults: value() }),
      );
      setPrinterValue(result.alertDefaults);
    } catch (e) {
      setSaveError(e);
    } finally {
      setSaving(false);
    }
  }

  return (
    <div class={styles.section}>
      <h3 class={styles.title}>Alerts</h3>
      <Show when={props.mode === "printer" && loading()}>
        <p class={styles.note}>Loading…</p>
      </Show>
      <Show when={props.mode === "printer" && loadError()}>
        {(held) => {
          const e = held();
          return (
            <p class={styles.error} role="alert">
              {isCommandError(e) ? e.message : "The alert defaults couldn't be loaded."}
            </p>
          );
        }}
      </Show>

      <RadioGroup
        label="Offline alert"
        options={OFFLINE_OPTIONS}
        value={offlineOptionValue(value().offlineAfterMinutes)}
        onChange={(v) => setValue({ ...value(), offlineAfterMinutes: parseOfflineOption(v) })}
      />

      <RadioGroup
        label="Notifications"
        options={NOTIFICATION_MODE_OPTIONS}
        value={value().notifications}
        onChange={(v) => setValue({ ...value(), notifications: v as NotificationMode })}
      />

      <Switch
        checked={value().snapshotOnIncident}
        onChange={(checked) => setValue({ ...value(), snapshotOnIncident: checked })}
      >
        Capture a snapshot when an Incident opens
      </Switch>
      <Switch
        checked={value().snapshotOnCompletion}
        onChange={(checked) => setValue({ ...value(), snapshotOnCompletion: checked })}
      >
        Capture a snapshot when a Job completes
      </Switch>

      <Show when={props.mode === "printer"}>
        <div class={styles.actions}>
          <Button variant="primary" disabled={saving()} onClick={() => void onSave()}>
            {saving() ? "Saving…" : "Save"}
          </Button>
          <Show when={saveError()}>
            {(held) => {
              const e = held();
              return (
                <p class={styles.error} role="alert">
                  {isCommandError(e) ? e.message : "The alert defaults couldn't be saved."}
                </p>
              );
            }}
          </Show>
        </div>
      </Show>
    </div>
  );
}
