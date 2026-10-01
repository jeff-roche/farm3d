import { createSignal } from "solid-js";
import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getThemeMode, initTheme, setThemeMode } from "../../design-system";
import { resetDiagnosticsStoreMock, loadWebDiagnosticsFixture } from "../../diagnostics/diagnostics-store-mock";
import { SETTINGS_CATEGORIES } from "./categories";
import { SettingsWorkspace } from "./SettingsWorkspace";

const stores = vi.hoisted(() => ({
  exportSettings: vi.fn(),
  importSettings: vi.fn(),
  exportPrinters: vi.fn(),
  importPrinters: vi.fn(),
  settingsValue: {
    revision: 1,
    themeMode: "farm3d-light",
    monitorSection: "printerModel",
    monitorDensity: "comfortable",
    notifications: { fatal: true, confirmation: true, completion: true, reconciliation: false, connectivity: false, inventory: false },
    snapshotRetention: { retentionDays: 30, diskCapMb: 2048 },
    updatedAt: "now",
  },
}));

vi.mock("../../settings/settings-store", () => ({
  exportSettings: stores.exportSettings,
  importSettings: stores.importSettings,
  settings: () => stores.settingsValue,
  loadSettings: vi.fn(async () => stores.settingsValue),
  updateSettings: vi.fn(async () => {}),
}));
vi.mock("../../printers/printer-store", () => ({
  exportPrinters: stores.exportPrinters,
  importPrinters: stores.importPrinters,
}));
vi.mock("../../diagnostics/diagnostics-store", async () =>
  (await import("../../diagnostics/diagnostics-store-mock")).diagnosticsStoreMock);
vi.mock("../../backup/backup-store", async () => (await import("../../backup/backup-store-mock")).backupStoreMock);
vi.mock("../../slicing/slicing-store", async () => (await import("../../slicing/slicing-store-mock")).slicingStoreMock);

beforeEach(async () => {
  await initTheme();
  setThemeMode("farm3d-light");
  loadWebDiagnosticsFixture();
  stores.exportSettings.mockReset().mockResolvedValue({ status: "exported", exportedAt: "now", recordCount: 1 });
  stores.importSettings.mockReset().mockResolvedValue({ status: "applied", settings: stores.settingsValue, warnings: [] });
  stores.exportPrinters.mockReset().mockResolvedValue({ status: "exported", exportedAt: "now", recordCount: 2 });
  stores.importPrinters.mockReset().mockResolvedValue({ status: "applied", printers: [], createdCount: 1, updatedCount: 0, deletedCount: 0, warnings: [] });
});

afterEach(() => {
  cleanup();
  resetDiagnosticsStoreMock();
  document.body.innerHTML = "";
});

function renderWorkspace(category?: string, extra: { onPrintersImported?: () => void } = {}) {
  const onCategoryChange = vi.fn();
  const view = render(() => (
    <SettingsWorkspace category={category} onCategoryChange={onCategoryChange} {...extra} />
  ));
  return { onCategoryChange, ...view };
}

describe("SettingsWorkspace", () => {
  it("lists the categories as vertical tabs, in order", () => {
    renderWorkspace();

    const tabs = screen.getAllByRole("tab");
    expect(tabs.map((tab) => tab.textContent)).toEqual(SETTINGS_CATEGORIES.map((category) => category.label));
    expect(screen.getByRole("tablist")).toHaveAttribute("aria-orientation", "vertical");
  });

  it("shows General by default and for an unknown slug", async () => {
    renderWorkspace(undefined);
    expect(await screen.findByRole("heading", { name: "General" })).toBeInTheDocument();
    cleanup();

    renderWorkspace("nonsense");
    expect(await screen.findByRole("heading", { name: "General" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "General" })).toHaveAttribute("aria-selected", "true");
  });

  it.each([
    ["appearance", "Appearance"],
    ["slicing", "Slicer"],
    ["notifications", "Notifications and retention"],
    ["storage", "Storage and backup"],
    ["connections", "Connections"],
    ["diagnostics", "Diagnostics"],
    ["about", "About"],
  ])("shows the %s category for its slug", async (slug, heading) => {
    renderWorkspace(slug);

    if (slug === "slicing") {
      expect(await screen.findByText("farm3d hasn't checked for OrcaSlicer yet.")).toBeInTheDocument();
    } else {
      expect(await screen.findByRole("heading", { name: heading })).toBeInTheDocument();
    }
  });

  it("reports the slug of a clicked category", async () => {
    const { onCategoryChange } = renderWorkspace("general");
    await screen.findByRole("heading", { name: "General" });

    await fireEvent.click(screen.getByRole("tab", { name: "About" }));

    expect(onCategoryChange).toHaveBeenCalledWith("about");
  });

  it("restores the committed theme when a category with a pending preview is left", async () => {
    const [category, setCategory] = createSignal("appearance");
    render(() => <SettingsWorkspace category={category()} onCategoryChange={setCategory} />);
    await fireEvent.click(await screen.findByText("Dark"));
    expect(document.documentElement.dataset.themeName).toBe("farm3d-dark");

    await fireEvent.click(screen.getByRole("tab", { name: "General" }));

    await waitFor(() => expect(document.documentElement.dataset.themeName).toBe("farm3d-light"));
    expect(getThemeMode()).toBe("farm3d-light");
  });

  it("restores the committed theme when the workspace goes away with a pending preview", async () => {
    const view = renderWorkspace("appearance");
    await fireEvent.click(await screen.findByText("Dark"));
    expect(document.documentElement.dataset.themeName).toBe("farm3d-dark");

    view.unmount();

    expect(document.documentElement.dataset.themeName).toBe("farm3d-light");
  });

  describe("Settings import and export (General)", () => {
    it("exports and imports the settings file", async () => {
      renderWorkspace("general");

      await fireEvent.click(await screen.findByRole("button", { name: "Export settings…" }));
      await waitFor(() => expect(stores.exportSettings).toHaveBeenCalledOnce());
      expect(await screen.findByText("Settings exported.")).toBeInTheDocument();

      await fireEvent.click(screen.getByRole("button", { name: "Import settings…" }));
      await waitFor(() => expect(stores.importSettings).toHaveBeenCalledOnce());
      expect(await screen.findByText("Settings imported.")).toBeInTheDocument();
    });

    it("says a failure, and that the desktop is needed in web mode", async () => {
      stores.exportSettings.mockRejectedValue(new Error("disk full"));
      stores.importSettings.mockResolvedValue({ status: "unsupported", reason: "desktopRequired" });
      renderWorkspace("general");

      await fireEvent.click(await screen.findByRole("button", { name: "Export settings…" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("disk full");

      await fireEvent.click(screen.getByRole("button", { name: "Import settings…" }));
      expect(await screen.findByRole("status")).toHaveTextContent("This needs the desktop app.");
    });
  });

  describe("Printers import and export (Connections)", () => {
    it("exports Printers", async () => {
      renderWorkspace("connections");

      await fireEvent.click(await screen.findByRole("button", { name: "Export Printers…" }));

      await waitFor(() => expect(stores.exportPrinters).toHaveBeenCalledOnce());
      expect(await screen.findByText("Printers exported.")).toBeInTheDocument();
    });

    it("imports Printers and tells the shell only when the import applied", async () => {
      const onPrintersImported = vi.fn();
      renderWorkspace("connections", { onPrintersImported });

      await fireEvent.click(await screen.findByRole("button", { name: "Import Printers…" }));
      await waitFor(() => expect(onPrintersImported).toHaveBeenCalledOnce());
      expect(await screen.findByText("Printers imported.")).toBeInTheDocument();

      stores.importPrinters.mockResolvedValue({ status: "cancelled" });
      await fireEvent.click(screen.getByRole("button", { name: "Import Printers…" }));
      await waitFor(() => expect(stores.importPrinters).toHaveBeenCalledTimes(2));
      expect(onPrintersImported).toHaveBeenCalledOnce();
    });

    it("names the credential store tier", async () => {
      renderWorkspace("connections");

      expect(await screen.findByText("The system keychain")).toBeInTheDocument();
    });
  });

  it("About shows the build from about_farm3d", async () => {
    renderWorkspace("about");

    expect(await screen.findByText("farm3d 0.9.0")).toBeInTheDocument();
    expect(screen.getByText("linux (x86_64)")).toBeInTheDocument();
  });
});
