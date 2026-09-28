import { createSignal } from "solid-js";
import { fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AlertDefaultsSection, alertDefaultsSummary, DEFAULT_ALERT_DEFAULTS } from "./AlertDefaultsSection";
import type { AlertDefaults } from "../attention/types";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
});

function DraftHarness(props: { initial: AlertDefaults }) {
  const [value, setValue] = createSignal(props.initial);
  return <AlertDefaultsSection mode="draft" value={value()} onChange={setValue} />;
}

describe("AlertDefaultsSection — draft mode", () => {
  it("renders with D12's own defaults", () => {
    render(() => <DraftHarness initial={{ ...DEFAULT_ALERT_DEFAULTS }} />);

    expect(screen.getByRole("radio", { name: "5 minutes" })).toBeChecked();
    expect(screen.getByRole("radio", { name: "Follow the notification settings" })).toBeChecked();
    expect(screen.getByRole("switch", { name: "Capture a snapshot when an Incident opens" })).toBeChecked();
    expect(screen.getByRole("switch", { name: "Capture a snapshot when a Job completes" })).toBeChecked();
  });

  it("switching the offline option updates the draft (via onChange)", () => {
    render(() => <DraftHarness initial={{ ...DEFAULT_ALERT_DEFAULTS }} />);

    fireEvent.click(screen.getByRole("radio", { name: "Off" }));

    expect(screen.getByRole("radio", { name: "Off" })).toBeChecked();
    expect(screen.getByRole("radio", { name: "5 minutes" })).not.toBeChecked();
  });

  it("shows no Save button (nothing to persist until the wizard's own Save)", () => {
    render(() => <DraftHarness initial={{ ...DEFAULT_ALERT_DEFAULTS }} />);

    expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
  });
});

describe("AlertDefaultsSection — printer mode", () => {
  it("loads via get_printer_alert_defaults and saves via set_printer_alert_defaults", async () => {
    tauriMock.isTauri.mockReturnValue(true);
    tauriMock.invoke.mockImplementation((command: string) => {
      if (command === "get_printer_alert_defaults") {
        return Promise.resolve({
          contractVersion: 1,
          data: { printerId: "prn-1", revision: 1, alertDefaults: { offlineAfterMinutes: 15, notifications: "muted", snapshotOnIncident: false, snapshotOnCompletion: true }, updatedAt: "now" },
        });
      }
      if (command === "set_printer_alert_defaults") {
        return Promise.resolve({
          contractVersion: 1,
          data: { printerId: "prn-1", revision: 2, alertDefaults: { offlineAfterMinutes: 1, notifications: "follow", snapshotOnIncident: false, snapshotOnCompletion: true }, updatedAt: "later" },
        });
      }
      return Promise.reject(new Error(`unexpected command ${command}`));
    });

    render(() => <AlertDefaultsSection mode="printer" printerId="prn-1" />);

    await waitFor(() => expect(screen.getByRole("radio", { name: "15 minutes" })).toBeChecked());
    expect(screen.getByRole("radio", { name: "Muted" })).toBeChecked();

    fireEvent.click(screen.getByRole("radio", { name: "1 minute" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(tauriMock.invoke).toHaveBeenCalledWith("set_printer_alert_defaults", expect.objectContaining({
      printerId: "prn-1",
      alertDefaults: { offlineAfterMinutes: 1, notifications: "muted", snapshotOnIncident: false, snapshotOnCompletion: true },
    })));
    await waitFor(() => expect(screen.getByRole("radio", { name: "1 minute" })).toBeChecked());
  });
});

describe("alertDefaultsSummary", () => {
  it("summarizes the offline grace and notification mode", () => {
    expect(alertDefaultsSummary(DEFAULT_ALERT_DEFAULTS)).toBe("offline after 5 min, notifications follow settings");
    expect(alertDefaultsSummary({ offlineAfterMinutes: null, notifications: "muted", snapshotOnIncident: true, snapshotOnCompletion: true })).toBe(
      "offline alert off, notifications muted",
    );
  });
});
