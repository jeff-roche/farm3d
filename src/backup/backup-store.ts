/** The backup and restore data layer (spec "Frontend architecture").
 *  Rust owns every conflict, blocker, and notice; this holds them for
 *  presentation. The restore flow is a state machine:
 *
 *    idle -> choosing -> previewing -> confirming -> restarting
 *
 *  with `failed` and `cancelled` branches that `dismissRestore` returns to
 *  `idle`. The staged preview is discarded on dismissal and on leaving the
 *  panel. The frontend never passes a path and no result holds one (D18).
 *  Web mode: reads are fixtures, the dialog-owning wrappers answer
 *  `unsupported`, other writes throw `needsDesktopError`. */
import { createStore, reconcile } from "solid-js/store";
import { command, desktopAvailable, isCommandError, needsDesktopError, retryOnTransportFailure } from "../ipc/client";
import { commandError } from "../ipc/local-errors";
import type { CommandError } from "../generated/contracts/command/CommandError";
import type {
  BackupInventory,
  BackupMediaChoice,
  BackupSummary,
  CreateBackupOutcome,
  PreviewRestoreOutcome,
  RestorePreview,
  RestoreSource,
  RestoreStatus,
} from "./types";

export type RestorePhase = "idle" | "choosing" | "previewing" | "confirming" | "restarting" | "failed" | "cancelled";

/** What the store keeps of an exported backup: never a path. */
export interface ExportedBackup {
  exportedAt: string;
  fileName: string;
  bytes: number;
  media: BackupMediaChoice;
  mediaNotInBackup: number;
  mediaMissingFile: number;
}

interface BackupState {
  inventory: BackupInventory | null;
  backups: BackupSummary[];
  lastExport: ExportedBackup | null;
  phase: RestorePhase;
  preview: RestorePreview | null;
  error: CommandError | null;
  safetyBackupId: string | null;
  restoreStatus: RestoreStatus;
}

const initialState = (): BackupState => ({
  inventory: null, backups: [], lastExport: null, phase: "idle", preview: null, error: null,
  safetyBackupId: null, restoreStatus: { state: "none" },
});

const [state, setState] = createStore<BackupState>(initialState());
let restoreStatusLoad: Promise<void> | null = null;
let applyOperationId: string | null = null;

export const backup = {
  inventory: (): BackupInventory | null => state.inventory,
  backups: (): BackupSummary[] => state.backups,
  lastExport: (): ExportedBackup | null => state.lastExport,
  restore: {
    phase: (): RestorePhase => state.phase,
    preview: (): RestorePreview | null => state.preview,
    error: (): CommandError | null => state.error,
    safetyBackupId: (): string | null => state.safetyBackupId,
  },
  /** The finished restore/reset to tell the operator about, until acknowledged. */
  restoreBanner: (): Exclude<RestoreStatus, { state: "none" }> | null =>
    state.restoreStatus.state === "none" ? null : state.restoreStatus,
};

const UNSUPPORTED = { status: "unsupported", reason: "desktopRequired" } as const;

async function write<T>(action: string, send: (operationId: string) => Promise<T>, operationId: string = crypto.randomUUID()): Promise<T> {
  if (!desktopAvailable()) throw needsDesktopError(action);
  return retryOnTransportFailure(() => send(operationId));
}

// --- Reads ------------------------------------------------------------------

export async function loadInventory(): Promise<BackupInventory> {
  const inventory = desktopAvailable()
    ? await retryOnTransportFailure(() => command("backup_inventory"))
    : await import("./web-fixtures").then(({ webBackupInventory }) => webBackupInventory());
  setState("inventory", reconcile(inventory));
  return inventory;
}

export async function loadBackups(): Promise<BackupSummary[]> {
  const backups = desktopAvailable()
    ? await retryOnTransportFailure(() => command("list_backups"))
    : await import("./web-fixtures").then(({ webBackupList }) => webBackupList());
  setState("backups", reconcile(backups, { key: "backupId" }));
  return backups;
}

// --- Writes -----------------------------------------------------------------

/** `create_backup`: the backend owns the save dialog. Cancelled records
 *  nothing; an export keeps its file name only. */
export async function createBackup(media: BackupMediaChoice): Promise<CreateBackupOutcome> {
  if (!desktopAvailable()) return { ...UNSUPPORTED };
  const outcome = await write("Creating a backup", (operationId) => command("create_backup", { operationId, media }));
  if (outcome.status === "exported") {
    const kept: ExportedBackup = {
      exportedAt: outcome.exportedAt, fileName: outcome.fileName, bytes: outcome.bytes, media: outcome.media,
      mediaNotInBackup: outcome.mediaNotInBackup, mediaMissingFile: outcome.mediaMissingFile,
    };
    setState("lastExport", reconcile(kept));
    return { status: "exported", ...kept };
  }
  return outcome.status === "cancelled" ? { status: "cancelled" } : { ...UNSUPPORTED };
}

export async function deleteBackup(backupId: string): Promise<void> {
  await write("Deleting a backup", (operationId) => command("delete_backup", { operationId, backupId }));
  setState("backups", (list) => list.filter((entry) => entry.backupId !== backupId));
}

// --- Restore flow -----------------------------------------------------------

function fail(error: unknown): void {
  const normalized = isCommandError(error)
    ? error
    : commandError("PERSISTENCE_UNAVAILABLE", "The restore failed.");
  setState({ phase: "failed", error: normalized });
}

/** What `startRestore` reports: Rust's outcome, or how the flow ended
 *  otherwise. It always matches the phase the flow finished in. */
export type StartRestoreResult = PreviewRestoreOutcome | { status: "failed" } | { status: "abandoned" };

/** idle -> choosing (the backend's open dialog, or a safety-backup read)
 *  -> previewing, or `cancelled`/`failed`. Starting again discards a staged
 *  preview first; a result that lands after the operator left the panel
 *  (the phase is no longer `choosing`) is discarded, not shown. */
export async function startRestore(source: RestoreSource): Promise<StartRestoreResult> {
  if (source.kind === "file" && !desktopAvailable()) return { ...UNSUPPORTED };
  await discardPreview();
  setState({ phase: "choosing", preview: null, error: null, safetyBackupId: null });
  try {
    if (!desktopAvailable()) throw needsDesktopError("Restoring a backup");
    const outcome = await retryOnTransportFailure(() => command("preview_restore", { source }));
    if (state.phase !== "choosing") {
      if (outcome.status === "previewed" && desktopAvailable()) {
        try {
          await command("discard_restore_preview", { stagingId: outcome.preview.stagingId });
        } catch {
          // The staging expires on its own (D6).
        }
      }
      return { status: "abandoned" };
    }
    if (outcome.status === "previewed") {
      setState({ phase: "previewing", preview: outcome.preview });
    } else if (outcome.status === "cancelled") {
      setState({ phase: "cancelled" });
    } else {
      setState({ phase: "idle" });
    }
    return outcome;
  } catch (error) {
    if (state.phase !== "choosing") return { status: "abandoned" };
    fail(error);
    return { status: "failed" };
  }
}

/** previewing -> confirming (the operator pressed Restore). */
export function beginRestoreConfirm(): void {
  if (state.phase === "previewing") {
    applyOperationId = crypto.randomUUID();
    setState("phase", "confirming");
  }
}

/** confirming -> previewing (the operator closed the confirm dialog). */
export function cancelRestoreConfirm(): void {
  if (state.phase === "confirming") setState("phase", "previewing");
}

/** confirming -> restarting. The `confirmation` is checked exactly by Rust. */
export async function applyRestore(confirmation: string): Promise<void> {
  if (!desktopAvailable()) throw needsDesktopError("Restoring a backup");
  const preview = state.preview;
  if (state.phase !== "confirming" || !preview) {
    throw commandError("VALIDATION", "There is no restore preview to apply.");
  }
  const operationId = applyOperationId ?? crypto.randomUUID();
  try {
    const outcome = await write(
      "Restoring a backup",
      (id) => command("apply_restore", { operationId: id, stagingId: preview.stagingId, confirmation }),
      operationId,
    );
    setState({ phase: "restarting", safetyBackupId: outcome.safetyBackupId });
  } catch (error) {
    fail(error);
  }
}

async function discardPreview(): Promise<void> {
  const preview = state.preview;
  setState("preview", null);
  if (!preview || !desktopAvailable()) return;
  try {
    await command("discard_restore_preview", { stagingId: preview.stagingId });
  } catch {
    // The staging expires on its own (D6); nothing for the operator to do.
  }
}

/** failed / cancelled -> idle, discarding any staged preview. */
export async function dismissRestore(): Promise<void> {
  if (state.phase === "restarting") return;
  await discardPreview();
  setState({ phase: "idle", error: null });
}

/** Leaving the panel discards the preview and resets the flow, except while
 *  farm3d is restarting (the restore is already committed). */
export async function leaveRestorePanel(): Promise<void> {
  if (state.phase === "restarting") return;
  await discardPreview();
  setState({ phase: "idle", error: null });
}

// --- Restore status banner --------------------------------------------------

/** Reads `restore_status` once (startup); later calls reuse the first read. */
export function loadRestoreStatus(): Promise<void> {
  restoreStatusLoad ??= (async () => {
    if (!desktopAvailable()) return;
    try {
      setState("restoreStatus", await retryOnTransportFailure(() => command("restore_status")));
    } catch {
      restoreStatusLoad = null; // a failed read may be retried
    }
  })();
  return restoreStatusLoad;
}

export async function acknowledgeRestoreStatus(): Promise<void> {
  const status = state.restoreStatus;
  if (status.state === "none") return;
  const next = await retryOnTransportFailure(() => command("acknowledge_restore_status", { journalId: status.journalId }));
  setState("restoreStatus", next);
}

/** Test seam. */
export function resetBackupStore(): void {
  setState(reconcile(initialState()));
  restoreStatusLoad = null;
  applyOperationId = null;
}
