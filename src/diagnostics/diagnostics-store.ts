/** The diagnostics, storage, reset, and About data layer (spec "Frontend
 *  architecture"). No result ever holds a path: the store keeps `fileName`
 *  at most, and copies only the fields it knows from each outcome (D18).
 *  Web mode: reads are fixtures, `export_diagnostics` answers
 *  `unsupported`, other writes throw `needsDesktopError`. */
import { createStore, reconcile } from "solid-js/store";
import { command, desktopAvailable, needsDesktopError, retryOnTransportFailure } from "../ipc/client";
import type {
  AboutInfo,
  ClearStorageOutcome,
  DiagnosticsPreview,
  DiagnosticsSection,
  ExportDiagnosticsOutcome,
  ResetPreview,
  ResetRequest,
  ResetResult,
  ResetTier,
  StorageCleanupTarget,
  StorageUsage,
} from "./types";

/** What the store keeps of an export: a name, never a path. */
export interface ExportedDiagnostics {
  exportedAt: string;
  fileName: string;
  bytes: number;
  sections: DiagnosticsSection[];
}

/** What the store keeps of a reset. */
export type ResetSummary =
  | { tier: "settings" }
  | { tier: "cameraMedia"; prunedCount: number; freedBytes: number }
  | { tier: "farm"; restarting: true; safetyBackupId: string | null };

interface DiagnosticsState {
  preview: DiagnosticsPreview | null;
  usage: StorageUsage | null;
  about: AboutInfo | null;
  lastExport: ExportedDiagnostics | null;
  lastReset: ResetSummary | null;
  resetPreviews: Partial<Record<ResetTier, ResetPreview>>;
}

const [state, setState] = createStore<DiagnosticsState>({
  preview: null, usage: null, about: null, lastExport: null, lastReset: null, resetPreviews: {},
});

export const diagnostics = {
  preview: (): DiagnosticsPreview | null => state.preview,
  usage: (): StorageUsage | null => state.usage,
  about: (): AboutInfo | null => state.about,
  lastExport: (): ExportedDiagnostics | null => state.lastExport,
  lastReset: (): ResetSummary | null => state.lastReset,
  resetPreview: (tier: ResetTier): ResetPreview | undefined => state.resetPreviews[tier],
};

const UNSUPPORTED = { status: "unsupported", reason: "desktopRequired" } as const;

async function write<T>(action: string, send: (operationId: string) => Promise<T>): Promise<T> {
  if (!desktopAvailable()) throw needsDesktopError(action);
  const operationId = crypto.randomUUID();
  return retryOnTransportFailure(() => send(operationId));
}

async function read<T>(desktop: () => Promise<T>, web: () => Promise<T>): Promise<T> {
  return desktopAvailable() ? retryOnTransportFailure(desktop) : web();
}

export async function loadDiagnosticsPreview(): Promise<DiagnosticsPreview> {
  const preview = await read(
    () => command("diagnostics_preview"),
    () => import("./web-fixtures").then(({ webDiagnosticsPreview }) => webDiagnosticsPreview()),
  );
  setState("preview", reconcile(preview));
  return preview;
}

export async function loadStorageUsage(): Promise<StorageUsage> {
  const usage = await read(
    () => command("storage_usage"),
    () => import("./web-fixtures").then(({ webStorageUsage }) => webStorageUsage()),
  );
  setState("usage", reconcile(usage));
  return usage;
}

export async function loadAbout(): Promise<AboutInfo> {
  const about = await read(
    () => command("about_farm3d"),
    () => import("./web-fixtures").then(({ webAboutInfo }) => webAboutInfo()),
  );
  setState("about", reconcile(about));
  return about;
}

export async function loadResetPreview(tier: ResetTier): Promise<ResetPreview> {
  const preview = await read(
    () => command("reset_preview", { tier }),
    () => import("./web-fixtures").then(({ webResetPreview }) => webResetPreview(tier)),
  );
  setState("resetPreviews", tier, reconcile(preview));
  return preview;
}

/** `export_diagnostics`: the backend owns the save dialog. A redaction
 *  failure rejects with `DIAGNOSTICS_REDACTION_FAILED` and its `section`. */
export async function exportDiagnostics(sections: DiagnosticsSection[]): Promise<ExportDiagnosticsOutcome> {
  if (!desktopAvailable()) return { ...UNSUPPORTED };
  const outcome = await write("Exporting diagnostics", (operationId) => command("export_diagnostics", { operationId, sections }));
  if (outcome.status === "exported") {
    const kept: ExportedDiagnostics = {
      exportedAt: outcome.exportedAt, fileName: outcome.fileName, bytes: outcome.bytes, sections: [...outcome.sections],
    };
    setState("lastExport", reconcile(kept));
    return { status: "exported", ...kept };
  }
  return outcome.status === "cancelled" ? { status: "cancelled" } : { ...UNSUPPORTED };
}

export async function clearStorage(target: StorageCleanupTarget): Promise<ClearStorageOutcome> {
  const outcome = await write("Clearing stored data", (operationId) => command("clear_storage", { operationId, target }));
  setState("usage", reconcile(outcome.usage));
  return outcome;
}

/** `reset_farm`. `confirmation` is checked exactly by Rust. */
export async function resetFarm(request: ResetRequest, confirmation: string): Promise<ResetResult> {
  const result = await write("Resetting", (operationId) => command("reset_farm", { operationId, request, confirmation }));
  switch (result.tier) {
    case "settings":
      setState("lastReset", reconcile({ tier: "settings" }));
      return { tier: "settings", settings: result.settings };
    case "cameraMedia":
      setState("lastReset", reconcile({ tier: "cameraMedia", prunedCount: result.prunedCount, freedBytes: result.freedBytes }));
      return { tier: "cameraMedia", prunedCount: result.prunedCount, freedBytes: result.freedBytes };
    case "farm":
      setState("lastReset", reconcile({ tier: "farm", restarting: true, safetyBackupId: result.safetyBackupId }));
      return { tier: "farm", status: result.status, safetyBackupId: result.safetyBackupId };
  }
}

/** Test seam. */
export function resetDiagnosticsStore(): void {
  setState({ preview: null, usage: null, about: null, lastExport: null, lastReset: null, resetPreviews: {} });
}
