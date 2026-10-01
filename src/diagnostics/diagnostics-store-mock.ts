/** A stand-in for `diagnostics-store` in screen tests. Test-only:
 *
 *    vi.mock("../diagnostics/diagnostics-store", async () =>
 *      (await import("../diagnostics/diagnostics-store-mock")).diagnosticsStoreMock);
 *
 *  Reads are a real Solid store (`setDiagnosticsStoreState`); actions are spies. */
import { createStore, reconcile } from "solid-js/store";
import { vi } from "vitest";
import { webAboutInfo, webDiagnosticsPreview, webResetPreview, webStorageUsage } from "./web-fixtures";
import type { ExportedDiagnostics, ResetSummary } from "./diagnostics-store";
import type { AboutInfo, DiagnosticsPreview, ResetPreview, ResetTier, StorageUsage } from "./types";

interface MockDiagnosticsState {
  preview: DiagnosticsPreview | null;
  usage: StorageUsage | null;
  about: AboutInfo | null;
  lastExport: ExportedDiagnostics | null;
  lastReset: ResetSummary | null;
  resetPreviews: Partial<Record<ResetTier, ResetPreview>>;
}

const initialState = (): MockDiagnosticsState => ({
  preview: null, usage: null, about: null, lastExport: null, lastReset: null, resetPreviews: {},
});
const [state, setState] = createStore<MockDiagnosticsState>(initialState());

export const diagnosticsStoreMock = {
  diagnostics: {
    preview: () => state.preview,
    usage: () => state.usage,
    about: () => state.about,
    lastExport: () => state.lastExport,
    lastReset: () => state.lastReset,
    resetPreview: (tier: ResetTier) => state.resetPreviews[tier],
  },
  loadDiagnosticsPreview: vi.fn(async () => state.preview ?? webDiagnosticsPreview()),
  loadStorageUsage: vi.fn(async () => state.usage ?? webStorageUsage()),
  loadAbout: vi.fn(async () => state.about ?? webAboutInfo()),
  loadResetPreview: vi.fn(async (tier: ResetTier) => state.resetPreviews[tier] ?? webResetPreview(tier)),
  exportDiagnostics: vi.fn(async () => ({ status: "cancelled" as const })),
  clearStorage: vi.fn(),
  resetFarm: vi.fn(),
  resetDiagnosticsStore: vi.fn(),
};

/** The shell's `about-store`, backed by the same state so tests set `about` once. */
export const aboutStoreMock = { about: () => state.about, loadAbout: diagnosticsStoreMock.loadAbout };

export function setDiagnosticsStoreState(patch: Partial<MockDiagnosticsState>): void {
  setState(patch);
}

export function loadWebDiagnosticsFixture(): void {
  setState({
    preview: webDiagnosticsPreview(), usage: webStorageUsage(), about: webAboutInfo(),
    resetPreviews: { settings: webResetPreview("settings"), cameraMedia: webResetPreview("cameraMedia"), farm: webResetPreview("farm") },
  });
}

export function resetDiagnosticsStoreMock(): void {
  setState(reconcile(initialState()));
  for (const action of Object.values(diagnosticsStoreMock)) {
    if (typeof action === "function" && "mockClear" in action) action.mockClear();
  }
}
