/** `restore_status` and its banner, on their own so the shell can show a
 *  finished restore or reset without pulling the rest of the backup store
 *  into the main chunk (spec "Restore status banner"). */
import { createStore, reconcile } from "solid-js/store";
import { command, desktopAvailable, retryOnTransportFailure } from "../ipc/client";
import type { RestoreStatus } from "./types";

export type FinishedRestoreStatus = Exclude<RestoreStatus, { state: "none" }>;

const [state, setState] = createStore<{ restoreStatus: RestoreStatus }>({ restoreStatus: { state: "none" } });
let restoreStatusLoad: Promise<void> | null = null;

/** The finished restore/reset to tell the operator about, until acknowledged. */
export const restoreBanner = (): FinishedRestoreStatus | null =>
  state.restoreStatus.state === "none" ? null : state.restoreStatus;

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

/** Acknowledges the shown status. A failure rejects and keeps the banner. */
export async function acknowledgeRestoreStatus(): Promise<void> {
  const status = state.restoreStatus;
  if (status.state === "none") return;
  const next = await retryOnTransportFailure(() => command("acknowledge_restore_status", { journalId: status.journalId }));
  setState("restoreStatus", next);
}

/** The banner's one-line outcome. Never names a path. */
export function restoreStatusText(status: FinishedRestoreStatus): string {
  const what = status.kind === "restore" ? "restore" : "reset";
  const safety = status.safetyBackupId ? ` You can restore from safety backup ${status.safetyBackupId}.` : "";
  return status.state === "done"
    ? `The ${what} finished.${safety}`
    : `The ${what} did not finish, and your previous data was kept.${safety}`;
}

/** Test seam. */
export function resetRestoreStatusStore(): void {
  setState(reconcile({ restoreStatus: { state: "none" } }));
  restoreStatusLoad = null;
}
