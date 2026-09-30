/** A stand-in for `backup-store` in screen tests. Test-only:
 *
 *    vi.mock("../backup/backup-store", async () =>
 *      (await import("../backup/backup-store-mock")).backupStoreMock);
 *
 *  The read side is a real Solid store (`setBackupStoreState`); the actions
 *  are spies. */
import { createStore, reconcile } from "solid-js/store";
import { vi } from "vitest";
import { webBackupInventory, webBackupList } from "./web-fixtures";
import type { BackupInventory, BackupSummary, RestorePreview, RestoreStatus } from "./types";
import type { ExportedBackup, RestorePhase } from "./backup-store";
import type { CommandError } from "../generated/contracts/command/CommandError";

interface MockBackupState {
  inventory: BackupInventory | null;
  backups: BackupSummary[];
  lastExport: ExportedBackup | null;
  phase: RestorePhase;
  preview: RestorePreview | null;
  error: CommandError | null;
  safetyBackupId: string | null;
  restoreStatus: RestoreStatus;
}

const initialState = (): MockBackupState => ({
  inventory: null, backups: [], lastExport: null, phase: "idle", preview: null, error: null,
  safetyBackupId: null, restoreStatus: { state: "none" },
});
const [state, setState] = createStore<MockBackupState>(initialState());

export const backupStoreMock = {
  backup: {
    inventory: () => state.inventory,
    backups: () => state.backups,
    lastExport: () => state.lastExport,
    restore: {
      phase: () => state.phase,
      preview: () => state.preview,
      error: () => state.error,
      safetyBackupId: () => state.safetyBackupId,
    },
    restoreBanner: () => (state.restoreStatus.state === "none" ? null : state.restoreStatus),
  },
  loadInventory: vi.fn(async () => state.inventory ?? webBackupInventory()),
  loadBackups: vi.fn(async () => state.backups),
  createBackup: vi.fn(async () => ({ status: "cancelled" as const })),
  deleteBackup: vi.fn(async (_backupId: string): Promise<void> => {}),
  startRestore: vi.fn(async () => ({ status: "cancelled" as const })),
  beginRestoreConfirm: vi.fn(),
  cancelRestoreConfirm: vi.fn(),
  applyRestore: vi.fn(async (_confirmation: string): Promise<void> => {}),
  dismissRestore: vi.fn(async (): Promise<void> => {}),
  leaveRestorePanel: vi.fn(async (): Promise<void> => {}),
  loadRestoreStatus: vi.fn(async (): Promise<void> => {}),
  acknowledgeRestoreStatus: vi.fn(async (): Promise<void> => {}),
  resetBackupStore: vi.fn(),
};

/** A stand-in for `restore-status-store`, over the same state. Spread it
 *  over the real module so `restoreStatusText` stays real:
 *
 *    vi.mock("../backup/restore-status-store", async (importActual) =>
 *      ({ ...(await importActual()), ...(await import("../backup/backup-store-mock")).restoreStatusStoreMock }));
 */
export const restoreStatusStoreMock = {
  restoreBanner: backupStoreMock.backup.restoreBanner,
  loadRestoreStatus: backupStoreMock.loadRestoreStatus,
  acknowledgeRestoreStatus: backupStoreMock.acknowledgeRestoreStatus,
};

export function setBackupStoreState(patch: Partial<MockBackupState>): void {
  const { backups, ...rest } = patch;
  if (backups) setState("backups", reconcile(backups));
  setState(rest);
}

export function loadWebBackupFixture(): void {
  setState({ inventory: webBackupInventory(), backups: webBackupList() });
}

export function resetBackupStoreMock(): void {
  setState(reconcile(initialState()));
  for (const action of Object.values(backupStoreMock)) {
    if (typeof action === "function" && "mockClear" in action) action.mockClear();
  }
}
