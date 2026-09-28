import { fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NotificationSettingsDialog } from "./NotificationSettingsDialog";
import type { Settings } from "../settings/settings-store";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

const settingsSignal = vi.hoisted(() => vi.fn());
const loadSettings = vi.hoisted(() => vi.fn());
const updateSettings = vi.hoisted(() => vi.fn());
vi.mock("../settings/settings-store", () => ({
  settings: settingsSignal,
  loadSettings,
  updateSettings,
}));

function settingsFixture(overrides: Partial<Settings> = {}): Settings {
  return {
    revision: 1,
    themeMode: "system",
    monitorSection: "printerModel",
    monitorDensity: "comfortable",
    notifications: { fatal: true, confirmation: true, completion: true, reconciliation: false, connectivity: false, inventory: false },
    snapshotRetention: { retentionDays: 30, diskCapMb: 2048 },
    updatedAt: "now",
    ...overrides,
  };
}

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
  tauriMock.isTauri.mockReturnValue(false);
  settingsSignal.mockReturnValue(null);
});

describe("NotificationSettingsDialog", () => {
  it("seeds the six class switches and retention fields from settings", async () => {
    settingsSignal.mockReturnValue(settingsFixture({
      notifications: { fatal: true, confirmation: false, completion: true, reconciliation: true, connectivity: false, inventory: true },
      snapshotRetention: { retentionDays: 7, diskCapMb: 512 },
    }));

    render(() => <NotificationSettingsDialog open onOpenChange={vi.fn()} />);

    expect(await screen.findByRole("switch", { name: "Fatal failures" })).toBeChecked();
    expect(screen.getByRole("switch", { name: "Material reconciliation" })).toBeChecked();
    expect(screen.getByRole("switch", { name: "Start confirmations" })).not.toBeChecked();
    await waitFor(() => expect(screen.getByLabelText("Retention (days)")).toHaveValue("7"));
    expect(screen.getByLabelText("Disk cap (MiB)")).toHaveValue("512");
  });

  it("switches the class toggles independently", async () => {
    settingsSignal.mockReturnValue(settingsFixture());
    render(() => <NotificationSettingsDialog open onOpenChange={vi.fn()} />);
    await screen.findByRole("switch", { name: "Fatal failures" });

    fireEvent.click(screen.getByRole("switch", { name: "Connectivity (offline, connection errors, host-cancelled)" }));

    expect(screen.getByRole("switch", { name: "Connectivity (offline, connection errors, host-cancelled)" })).toBeChecked();
    expect(screen.getByRole("switch", { name: "Fatal failures" })).toBeChecked();
  });

  it("shows 'Unavailable: no notification service' when the notifier is unavailable", async () => {
    settingsSignal.mockReturnValue(settingsFixture());
    tauriMock.isTauri.mockReturnValue(true);
    tauriMock.invoke.mockResolvedValueOnce({
      contractVersion: 1,
      data: { state: "unavailable", reason: "noSessionBus" },
    });

    render(() => <NotificationSettingsDialog open onOpenChange={vi.fn()} />);

    expect(await screen.findByText("Unavailable: no notification service")).toBeInTheDocument();
  });

  it("shows the server name when available", async () => {
    settingsSignal.mockReturnValue(settingsFixture());
    tauriMock.isTauri.mockReturnValue(true);
    tauriMock.invoke.mockResolvedValueOnce({
      contractVersion: 1,
      data: { state: "available", serverName: "Plasma", serverVendor: "KDE", serverVersion: "6.7.5", specVersion: "1.2", actions: true, bodyMarkup: true },
    });

    render(() => <NotificationSettingsDialog open onOpenChange={vi.fn()} />);

    expect(await screen.findByText("Available: Plasma")).toBeInTheDocument();
  });

  it("Send test notification calls send_test_notification and reports success", async () => {
    settingsSignal.mockReturnValue(settingsFixture());
    tauriMock.isTauri.mockReturnValue(true);
    tauriMock.invoke.mockImplementation((command: string) => {
      if (command === "notification_status") return Promise.resolve({ contractVersion: 1, data: { state: "unsupported" } });
      if (command === "send_test_notification") return Promise.resolve({ contractVersion: 1, data: { sent: true } });
      return Promise.reject(new Error(`unexpected command ${command}`));
    });

    render(() => <NotificationSettingsDialog open onOpenChange={vi.fn()} />);
    await screen.findByText("Not supported on this platform");

    fireEvent.click(screen.getByRole("button", { name: "Send test notification" }));

    expect(await screen.findByText("Test notification sent.")).toBeInTheDocument();
    expect(tauriMock.invoke).toHaveBeenCalledWith("send_test_notification", { contractVersion: 1 });
  });

  it("Save calls updateSettings with the six classes and retention values", async () => {
    settingsSignal.mockReturnValue(settingsFixture());
    updateSettings.mockResolvedValueOnce(undefined);
    render(() => <NotificationSettingsDialog open onOpenChange={vi.fn()} />);
    await screen.findByRole("switch", { name: "Fatal failures" });

    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(updateSettings).toHaveBeenCalledWith({
      notifications: { fatal: true, confirmation: true, completion: true, reconciliation: false, connectivity: false, inventory: false },
      snapshotRetention: { retentionDays: 30, diskCapMb: 2048 },
    }));
  });

  it("a revision conflict reloads settings and explains, rather than losing the edit silently", async () => {
    settingsSignal.mockReturnValue(settingsFixture());
    updateSettings.mockRejectedValueOnce({
      contractVersion: 1,
      code: "CONFLICT",
      message: "This changed since you opened it.",
      recovery: ["RELOAD"],
      retryable: false,
    });
    loadSettings.mockImplementationOnce(async () => {
      const reloaded = settingsFixture({ snapshotRetention: { retentionDays: 60, diskCapMb: 4096 } });
      settingsSignal.mockReturnValue(reloaded);
      return reloaded;
    });

    render(() => <NotificationSettingsDialog open onOpenChange={vi.fn()} />);
    await screen.findByRole("switch", { name: "Fatal failures" });

    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByText(/changed elsewhere since this dialog opened/)).toBeInTheDocument();
    expect(loadSettings).toHaveBeenCalled();
    await waitFor(() => expect(screen.getByLabelText("Retention (days)")).toHaveValue("60"));
  });
});
