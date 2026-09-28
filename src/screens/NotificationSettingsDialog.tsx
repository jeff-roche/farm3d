import { createEffect, createSignal, For, on, Show } from "solid-js";
import { Button, Dialog, NumberField, Switch } from "../design-system";
import { command, desktopAvailable, isCommandError, needsDesktopError } from "../ipc/client";
import { notificationClassLabel, NOTIFICATION_CLASS_ORDER } from "../attention/presentation";
import type { NotificationClassSettings, NotifierStatus } from "../attention/types";
import { loadSettings, settings, updateSettings } from "../settings/settings-store";
import styles from "./NotificationSettingsDialog.module.css";

const DEFAULT_CLASSES: NotificationClassSettings = {
  fatal: true,
  confirmation: true,
  completion: true,
  reconciliation: false,
  connectivity: false,
  inventory: false,
};

export interface NotificationSettingsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** `SettingsMenu`'s "Notifications and retention…" (spec "Frontend
 *  architecture" → "Settings"): the six class switches, the retention days
 *  and disk cap, the notifier status text, and "Send test notification".
 *  A revision conflict on Save reloads and explains rather than clobbering
 *  whatever changed elsewhere (P8 leaves a Settings workspace/destination
 *  to P9, so this dialog is the only surface for these fields). */
export function NotificationSettingsDialog(props: NotificationSettingsDialogProps) {
  // Seeded synchronously at construction, not only from the `open` effect
  // below: `NumberField`'s Kobalte root also runs its own mount-time
  // effect, and a same-tick external update racing it can lose (the
  // control keeps its stale initial value even though this component's
  // own signal is already correct). Reading already-loaded settings before
  // the first render sidesteps that race entirely for the common case
  // (the app always loads settings before this dialog can be opened); the
  // `open` effect still re-seeds on every later open, and covers the
  // fallback where settings weren't loaded yet.
  const initial = settings();
  const [classes, setClasses] = createSignal<NotificationClassSettings>(
    initial ? initial.notifications : { ...DEFAULT_CLASSES },
  );
  const [retentionDays, setRetentionDays] = createSignal(initial ? initial.snapshotRetention.retentionDays : 30);
  const [diskCapMb, setDiskCapMb] = createSignal(initial ? initial.snapshotRetention.diskCapMb : 2048);
  const [status, setStatus] = createSignal<NotifierStatus | null>(null);
  const [statusError, setStatusError] = createSignal<unknown>(null);
  const [saving, setSaving] = createSignal(false);
  const [saveError, setSaveError] = createSignal<string | null>(null);
  const [testPending, setTestPending] = createSignal(false);
  const [testError, setTestError] = createSignal<unknown>(null);
  const [testSent, setTestSent] = createSignal(false);

  function seedFromSettings(): void {
    const current = settings();
    if (!current) return;
    setClasses(current.notifications);
    setRetentionDays(current.snapshotRetention.retentionDays);
    setDiskCapMb(current.snapshotRetention.diskCapMb);
  }

  async function loadStatus(): Promise<void> {
    setStatusError(null);
    if (!desktopAvailable()) {
      setStatus(null);
      setStatusError(needsDesktopError("Checking notification status"));
      return;
    }
    try {
      setStatus(await command("notification_status"));
    } catch (e) {
      setStatus(null);
      setStatusError(e);
    }
  }

  createEffect(
    on(
      () => props.open,
      (open) => {
        if (!open) return;
        setSaveError(null);
        setTestError(null);
        setTestSent(false);
        if (settings()) seedFromSettings();
        else void loadSettings().then(seedFromSettings).catch(() => {});
        void loadStatus();
      },
    ),
  );

  function toggleClass(key: keyof NotificationClassSettings, checked: boolean): void {
    setClasses((prev) => ({ ...prev, [key]: checked }));
  }

  async function onSave(): Promise<void> {
    setSaving(true);
    setSaveError(null);
    try {
      await updateSettings({
        notifications: classes(),
        snapshotRetention: { retentionDays: retentionDays(), diskCapMb: diskCapMb() },
      });
    } catch (e) {
      if (isCommandError(e) && e.code === "CONFLICT") {
        await loadSettings().catch(() => {});
        seedFromSettings();
        setSaveError(
          "These settings changed elsewhere since this dialog opened. They've been reloaded with the current values — check them and try again.",
        );
      } else {
        setSaveError(isCommandError(e) ? e.message : "These settings could not be saved.");
      }
    } finally {
      setSaving(false);
    }
  }

  async function onSendTest(): Promise<void> {
    setTestPending(true);
    setTestError(null);
    setTestSent(false);
    try {
      if (!desktopAvailable()) throw needsDesktopError("Sending a test notification");
      await command("send_test_notification");
      setTestSent(true);
    } catch (e) {
      setTestError(e);
    } finally {
      setTestPending(false);
    }
  }

  function statusLabel(): string {
    const err = statusError();
    if (err) return isCommandError(err) ? err.message : "Couldn't check notification status.";
    const current = status();
    if (!current) return "Checking…";
    if (current.state === "available") return `Available: ${current.serverName}`;
    if (current.state === "unavailable") return "Unavailable: no notification service";
    return "Not supported on this platform";
  }

  return (
    <Dialog title="Notifications and retention" open={props.open} onOpenChange={props.onOpenChange}>
      <div class={styles.body}>
        <section class={styles.section} aria-label="Notification classes">
          <span class={styles.sectionTitle}>Notify for</span>
          <For each={NOTIFICATION_CLASS_ORDER}>
            {(key) => (
              <Switch checked={classes()[key]} onChange={(checked) => toggleClass(key, checked)}>
                {notificationClassLabel(key)}
              </Switch>
            )}
          </For>
        </section>

        <section class={styles.section} aria-label="Snapshot retention">
          <span class={styles.sectionTitle}>Snapshot retention</span>
          <NumberField label="Retention (days)" value={retentionDays()} onChange={setRetentionDays} minValue={1} maxValue={365} />
          <NumberField label="Disk cap (MiB)" value={diskCapMb()} onChange={setDiskCapMb} minValue={100} maxValue={102400} />
        </section>

        <section class={styles.section} aria-label="Desktop notifications">
          <span class={styles.sectionTitle}>Desktop notifications</span>
          <p class={styles.status} role="status">{statusLabel()}</p>
          <Button
            class={styles.sendTestButton}
            variant="secondary"
            size="sm"
            disabled={testPending()}
            onClick={() => void onSendTest()}
          >
            {testPending() ? "Sending…" : "Send test notification"}
          </Button>
          <Show when={testSent()}>
            <p class={styles.note}>Test notification sent.</p>
          </Show>
          <Show when={testError()}>
            {(held) => {
              const e = held();
              return (
                <p class={styles.error} role="alert">
                  {isCommandError(e) ? e.message : "The test notification could not be sent."}
                </p>
              );
            }}
          </Show>
        </section>

        <Show when={saveError()}>
          {(message) => (
            <p class={styles.error} role="alert">
              {message()}
            </p>
          )}
        </Show>

        <div class={styles.actions}>
          <Button variant="secondary" onClick={() => props.onOpenChange(false)}>
            Close
          </Button>
          <Button variant="primary" disabled={saving()} onClick={() => void onSave()}>
            {saving() ? "Saving…" : "Save"}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
