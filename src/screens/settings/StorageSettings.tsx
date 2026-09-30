import { createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { AlertDialog, Button, DataTable, RadioGroup } from "../../design-system";
import {
  applyRestore, backup, beginRestoreConfirm, cancelRestoreConfirm, createBackup, deleteBackup, dismissRestore,
  leaveRestorePanel, loadBackups, loadInventory, startRestore,
} from "../../backup/backup-store";
import { backupOriginLabel } from "../../backup/presentation";
import type { BackupMediaChoice, BackupSummary } from "../../backup/types";
import { clearStorage, diagnostics, loadStorageUsage } from "../../diagnostics/diagnostics-store";
import { formatBytes, STORAGE_CLEANUP_CLASS, storageCleanupLabel, storageClassLabel } from "../../diagnostics/presentation";
import type { StorageCleanupTarget } from "../../diagnostics/types";
import type { StorageClassUsage } from "../../generated/contracts/domain/StorageClassUsage";
import { OperationStatus } from "./OperationStatus";
import { RestorePreviewPanel } from "./RestorePreviewPanel";
import styles from "./Settings.module.css";
import portability from "./Portability.module.css";

export interface StorageSettingsProps {
  /** Opens the Queue, for a restore blocked by active work. */
  onOpenQueue?: () => void;
}

const MEDIA_LABEL: Record<BackupMediaChoice, string> = {
  none: "No camera images",
  pinned: "Pinned camera images",
  all: "All camera images",
};

const plural = (count: number, one: string, many: string): string => `${count} ${count === 1 ? one : many}`;
const messageOf = (error: unknown): string => (error instanceof Error ? error.message : (error as { message?: string })?.message ?? String(error));

/** Storage and backup category: usage and cleanup, Create backup, Restore
 *  from file, and the safety backups. */
export function StorageSettings(props: StorageSettingsProps) {
  const [cleaning, setCleaning] = createSignal<StorageCleanupTarget | null>(null);
  const [cleanup, setCleanup] = createSignal<{ message: string | null; error: boolean }>({ message: null, error: false });
  const [media, setMedia] = createSignal<BackupMediaChoice>("none");
  const [creating, setCreating] = createSignal(false);
  const [created, setCreated] = createSignal<{ message: string | null; error: boolean }>({ message: null, error: false });
  const [deleting, setDeleting] = createSignal<BackupSummary | null>(null);
  const [listError, setListError] = createSignal<string | null>(null);
  const [applying, setApplying] = createSignal(false);

  onMount(() => {
    void loadStorageUsage().catch(() => {});
    void loadInventory().catch(() => {});
    void loadBackups().catch(() => {});
  });
  onCleanup(() => void leaveRestorePanel());

  const cleanupTargets = Object.fromEntries(
    Object.entries(STORAGE_CLEANUP_CLASS).map(([target, cls]) => [cls, target as StorageCleanupTarget]),
  );

  async function runCleanup(target: StorageCleanupTarget) {
    setCleaning(target);
    setCleanup({ message: null, error: false });
    try {
      const outcome = await clearStorage(target);
      setCleanup({
        message: `Removed ${plural(outcome.removedCount, "item", "items")}, freed ${formatBytes(outcome.freedBytes)}.`,
        error: false,
      });
    } catch (error) {
      setCleanup({ message: messageOf(error), error: true });
    }
    setCleaning(null);
  }

  const mediaTotals = (choice: BackupMediaChoice) => backup.inventory()?.media.find((entry) => entry.choice === choice);
  const estimate = createMemo(() => {
    const inventory = backup.inventory();
    if (!inventory) return null;
    return inventory.databaseBytes + inventory.content.bytes + (mediaTotals(media())?.bytes ?? 0);
  });
  const mediaOptions = () =>
    (["none", "pinned", "all"] as const).map((choice) => {
      const totals = mediaTotals(choice);
      const detail = choice === "none" || !totals ? "" : ` (${plural(totals.count, "image", "images")}, ${formatBytes(totals.bytes)})`;
      return { value: choice, label: `${MEDIA_LABEL[choice]}${detail}` };
    });

  async function runCreate() {
    setCreating(true);
    setCreated({ message: null, error: false });
    try {
      const outcome = await createBackup(media());
      if (outcome.status === "exported") {
        const missing = outcome.mediaNotInBackup > 0
          ? ` ${plural(outcome.mediaNotInBackup, "camera image was", "camera images were")} not included.`
          : "";
        setCreated({ message: `Exported ${outcome.fileName} (${formatBytes(outcome.bytes)}).${missing}`, error: false });
      } else if (outcome.status === "cancelled") {
        setCreated({ message: "Backup cancelled. Nothing was written.", error: false });
      } else {
        setCreated({ message: "Creating a backup needs the desktop app.", error: false });
      }
    } catch (error) {
      setCreated({ message: messageOf(error), error: true });
    }
    setCreating(false);
  }

  async function confirmDelete() {
    const target = deleting();
    if (!target) return;
    setDeleting(null);
    setListError(null);
    try {
      await deleteBackup(target.backupId);
    } catch (error) {
      setListError(messageOf(error));
    }
  }

  async function confirmRestore(confirmation: string) {
    setApplying(true);
    await applyRestore(confirmation);
    setApplying(false);
  }

  const phase = () => backup.restore.phase();
  const busy = () => phase() !== "idle";

  return (
    <div class={styles.category}>
      <h3 class={styles.heading}>Storage and backup</h3>

      <section class={styles.section} aria-label="Storage usage">
        <span class={styles.sectionTitle}>Storage usage</span>
        <Show when={diagnostics.usage()} fallback={<p class={styles.status}>Measuring…</p>}>
          {(usage) => (
            <>
              <DataTable<StorageClassUsage>
                label="Storage usage"
                rows={usage().classes}
                rowId={(row) => row.class}
                columns={[
                  { id: "class", header: "Kind", cell: (row) => storageClassLabel(row.class) },
                  { id: "count", header: "Items", align: "end", cell: (row) => String(row.count) },
                  { id: "bytes", header: "Size", align: "end", cell: (row) => formatBytes(row.bytes) },
                  {
                    id: "cleanup", header: "Cleanup",
                    cell: (row) => {
                      const target = cleanupTargets[row.class];
                      return target ? (
                        <Button size="sm" disabled={cleaning() !== null} onClick={() => void runCleanup(target)}>
                          {storageCleanupLabel(target)}
                        </Button>
                      ) : null;
                    },
                  },
                ]}
              />
              <p class={styles.note}>Total {formatBytes(usage().totalBytes)}, measured {usage().measuredAt}.</p>
            </>
          )}
        </Show>
        <Show when={cleaning() !== null}><p class={styles.note}>Cleaning up…</p></Show>
        <OperationStatus message={cleanup().message} error={cleanup().error} />
      </section>

      <section class={styles.section} aria-label="Create backup">
        <span class={styles.sectionTitle}>Create backup</span>
        <p class={styles.note}>
          A backup holds your Printers, Spools, Models, Jobs, and settings. It never holds credentials, so Printers need
          their credentials entered again after a restore.
        </p>
        <RadioGroup label="Camera images" options={mediaOptions()} value={media()} onChange={(value) => setMedia(value as BackupMediaChoice)} />
        <Show when={estimate() !== null}>
          <p class={styles.note}>Estimated size: {formatBytes(estimate() ?? 0)}</p>
        </Show>
        <div class={styles.actions}>
          <Button variant="primary" disabled={creating()} onClick={() => void runCreate()}>Create backup…</Button>
        </div>
        <OperationStatus message={created().message} error={created().error} />
      </section>

      <section class={styles.section} aria-label="Restore">
        <span class={styles.sectionTitle}>Restore</span>
        <p class={styles.note}>Restore replaces this Farm with a backup. You see what changes before anything does.</p>
        <div class={styles.actions}>
          <Button disabled={busy()} onClick={() => void startRestore({ kind: "file" })}>Restore from file…</Button>
        </div>
        <Show when={phase() === "choosing"}><p class={styles.note}>Waiting for the file to open…</p></Show>
        <Show when={phase() === "cancelled"}>
          <p class={styles.note}>No file was chosen.</p>
          <div class={styles.actions}><Button size="sm" onClick={() => void dismissRestore()}>Dismiss</Button></div>
        </Show>
        <Show when={phase() === "failed"}>
          <p class={styles.error} role="alert">{backup.restore.error()?.message ?? "The restore failed."}</p>
          <div class={styles.actions}><Button size="sm" onClick={() => void dismissRestore()}>Dismiss</Button></div>
        </Show>
        <Show when={phase() === "restarting"}>
          <p class={styles.status} role="status">Restarting farm3d to finish the restore…</p>
        </Show>
        <Show when={(phase() === "previewing" || phase() === "confirming") ? backup.restore.preview() : null}>
          {(preview) => (
            <RestorePreviewPanel
              preview={preview()}
              confirming={phase() === "confirming"}
              pending={applying()}
              onRestore={beginRestoreConfirm}
              onCancel={() => void dismissRestore()}
              onConfirm={(confirmation) => void confirmRestore(confirmation)}
              onCancelConfirm={cancelRestoreConfirm}
              onOpenQueue={() => props.onOpenQueue?.()}
            />
          )}
        </Show>
      </section>

      <section class={styles.section} aria-label="Safety backups">
        <span class={styles.sectionTitle}>Safety backups</span>
        <p class={styles.note}>farm3d writes one before a restore or a reset.</p>
        <Show when={backup.backups().length > 0} fallback={<p class={styles.note}>There are no safety backups.</p>}>
          <ul class={portability.rows} aria-label="Safety backups">
            <For each={backup.backups()}>
              {(entry) => (
                <li class={portability.row}>
                  <span class={portability.grow}>
                    {backupOriginLabel(entry.origin)}
                    <span class={portability.meta}>
                      {" "}{entry.valid ? `${entry.createdAt ?? ""}, ${formatBytes(entry.bytes)}` : `Unreadable file, ${formatBytes(entry.bytes)}`}
                    </span>
                  </span>
                  <Show when={entry.valid}>
                    <Button
                      size="sm"
                      aria-label={`Restore ${backupOriginLabel(entry.origin)} from ${entry.createdAt ?? entry.backupId}`}
                      disabled={busy()}
                      onClick={() => void startRestore({ kind: "safetyBackup", backupId: entry.backupId })}
                    >
                      Restore…
                    </Button>
                  </Show>
                  <Button
                    size="sm"
                    variant="ghost"
                    aria-label={`Delete ${backupOriginLabel(entry.origin)} from ${entry.createdAt ?? entry.backupId}`}
                    onClick={() => setDeleting(entry)}
                  >
                    Delete
                  </Button>
                </li>
              )}
            </For>
          </ul>
        </Show>
        <OperationStatus message={listError()} error />
      </section>

      <AlertDialog
        open={deleting() !== null}
        onOpenChange={(open) => { if (!open) setDeleting(null); }}
        title="Delete this safety backup?"
        description="It can't be recovered once deleted."
      >
        <div class={styles.actions}>
          <Button variant="secondary" onClick={() => setDeleting(null)}>Keep</Button>
          <Button variant="danger" onClick={() => void confirmDelete()}>Delete backup</Button>
        </div>
      </AlertDialog>
    </div>
  );
}
