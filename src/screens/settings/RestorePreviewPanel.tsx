import { For, Show } from "solid-js";
import { Button, DataTable, TypedConfirmDialog } from "../../design-system";
import { groupConflicts, previewSourceText, restoreBlockerLabel, restoreNoticeText } from "../../backup/presentation";
import type { RestorePreview } from "../../backup/types";
import type { RestoreCount } from "../../generated/contracts/domain/RestoreCount";
import styles from "./Settings.module.css";
import portability from "./Portability.module.css";

export const RESTORE_PHRASE = "restore";

export interface RestorePreviewPanelProps {
  preview: RestorePreview;
  /** The store's `confirming` phase: the typed confirmation is open. */
  confirming: boolean;
  /** Restore was sent and is waiting on the backend. */
  pending?: boolean;
  onRestore: () => void;
  onCancel: () => void;
  onConfirm: (confirmation: string) => void;
  onCancelConfirm: () => void;
  onOpenQueue: () => void;
}

/** What restoring the staged backup would change. Rust decides every
 *  conflict, notice, and blocker; this presents them. */
export function RestorePreviewPanel(props: RestorePreviewPanelProps) {
  const conflicts = () => groupConflicts(props.preview.conflicts);
  const blocked = () => props.preview.blockers.length > 0 || props.preview.blockerTotal > 0;
  const moreBlockers = () => Math.max(0, props.preview.blockerTotal - props.preview.blockers.length);

  return (
    <section class={portability.panel} aria-label="Restore preview">
      <span class={styles.sectionTitle}>Restore preview</span>
      <dl class={styles.definitions}>
        <dt>Backup</dt>
        <dd>{previewSourceText(props.preview)}</dd>
        <dt>Made</dt>
        <dd>{props.preview.backup.createdAt} by farm3d {props.preview.backup.appVersion} on {props.preview.backup.platform.os} ({props.preview.backup.platform.arch})</dd>
        <dt>Camera images</dt>
        <dd>{props.preview.backup.media === "none" ? "None" : props.preview.backup.media === "pinned" ? "Pinned images only" : "All images"}</dd>
        <dt>Expires</dt>
        <dd>{props.preview.expiresAt}</dd>
      </dl>

      <DataTable<RestoreCount>
        label="Row counts"
        rows={props.preview.counts}
        rowId={(row) => row.table}
        columns={[
          { id: "table", header: "Table", cell: (row) => row.table },
          { id: "local", header: "This Farm", align: "end", cell: (row) => String(row.local) },
          { id: "backup", header: "Backup", align: "end", cell: (row) => String(row.backup) },
        ]}
      />

      <Show
        when={conflicts().length > 0}
        fallback={<p class={styles.note}>Nothing in this Farm conflicts with the backup.</p>}
      >
        <div class={portability.stack}>
          <For each={conflicts()}>
            {(group) => (
              <div>
                <h4 class={portability.subheading}>{group.label} ({group.total})</h4>
                <For each={group.domains}>
                  {(domain) => (
                    <div>
                      <h5 class={portability.meta}>{domain.label}: {domain.total}</h5>
                      <ul class={`${portability.rows} ${portability.scroll}`} aria-label={`${group.label}: ${domain.label}`}>
                        <For each={domain.items}>{(item) => <li>{item.label}</li>}</For>
                        <Show when={domain.moreText}>{(text) => <li class={portability.meta}>{text()}</li>}</Show>
                      </ul>
                    </div>
                  )}
                </For>
              </div>
            )}
          </For>
        </div>
      </Show>

      <Show when={props.preview.notices.length > 0}>
        <div>
          <h4 class={portability.subheading}>Before you restore</h4>
          <ul class={portability.rows}>
            <For each={props.preview.notices}>{(notice) => <li>{restoreNoticeText(notice)}</li>}</For>
          </ul>
        </div>
      </Show>

      <Show when={blocked()}>
        <div role="group" aria-label="Blockers">
          <h4 class={portability.subheading}>Restore is blocked until this finishes</h4>
          <ul class={portability.rows}>
            <For each={props.preview.blockers}>
              {(blocker) => (
                <li class={portability.row}>
                  <span>{restoreBlockerLabel(blocker)}</span>
                  <span class={portability.meta}>{blocker.id}</span>
                </li>
              )}
            </For>
            <Show when={moreBlockers() > 0}><li class={portability.meta}>and {moreBlockers()} more</li></Show>
          </ul>
          <Button variant="secondary" size="sm" onClick={props.onOpenQueue}>Open the Queue</Button>
        </div>
      </Show>

      <div class={styles.actions}>
        <Button variant="danger" disabled={blocked() || props.pending} onClick={props.onRestore}>Restore</Button>
        <Button variant="secondary" disabled={props.pending} onClick={props.onCancel}>Cancel</Button>
      </div>

      <TypedConfirmDialog
        open={props.confirming}
        onOpenChange={(open) => { if (!open) props.onCancelConfirm(); }}
        title="Restore this backup?"
        consequences={[
          "A safety backup of this Farm is written first, so you can undo the restore.",
          "farm3d will restart to install the backup.",
          "Everything marked above as only on this Farm, or changed, is replaced by the backup's version.",
        ]}
        phrase={RESTORE_PHRASE}
        confirmLabel="Restore and restart"
        pending={props.pending}
        onConfirm={() => props.onConfirm(RESTORE_PHRASE)}
      />
    </section>
  );
}
