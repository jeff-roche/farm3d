import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MonitorPrinterView, MonitorRosterView } from "../monitor/monitor-store";
import { AppShell } from "./AppShell";

function printer(overrides: Partial<MonitorPrinterView> = {}): MonitorPrinterView {
  return {
    id: "printer-1",
    name: "Bay One",
    vendor: "Bambu Lab",
    model: "X1 Carbon",
    modelLabel: "X1 Carbon",
    archived: false,
    catalogStatus: "ok",
    operationalLabel: "Ready",
    severity: "resolved",
    hostActivity: "idle",
    statusSummary: "Host activity: Idle",
    hasMissingReadings: true,
    readings: {},
    accessibleSummary: "Bay One; ready; idle",
    operationalState: "ready",
    ...overrides,
  };
}

function roster(overrides: Partial<MonitorRosterView> = {}): MonitorRosterView {
  return {
    key: "all",
    label: "Printers",
    count: 3,
    printers: [printer()],
    remainingCount: 0,
    ...overrides,
  };
}

afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
});

describe("AppShell", () => {
  it("renders structured Printer rosters, health, last live-event age, and nav-main-footer source order", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-18T12:02:00Z"));
    const onSelect = vi.fn();
    const { container } = render(() => (
      <AppShell
        active="monitor"
        onSelect={onSelect}
        title="Monitor"
        printerRoster={roster()}
        operationalRosters={[roster({ key: "ready", label: "Ready", count: 1 })]}
        adapterHealth={{ severity: "resolved", label: "All adapters connected" }}
        lastLiveEventAt="2026-09-18T12:00:00Z"
      >
        <p>Workspace</p>
      </AppShell>
    ));

    const totalRoster = screen.getByRole("button", { name: "3 Printers" });
    await fireEvent.focus(totalRoster);
    expect(await screen.findByText("Bay One")).toBeInTheDocument();
    expect(screen.getByRole("status", { name: "All adapters connected" })).toBeInTheDocument();
    expect(screen.getByText("Last live event 2m ago")).toBeInTheDocument();
    expect(screen.queryByText(/job|attention/i)).not.toBeInTheDocument();

    const shell = container.querySelector("div");
    expect(Array.from(shell?.children ?? []).map((child) => child.nodeName)).toEqual([
      "HEADER", "NAV", "MAIN", "FOOTER",
    ]);
  });

  it("refreshes the relative age on a low-frequency timer and clears it on unmount", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-18T12:00:30Z"));
    const clearIntervalSpy = vi.spyOn(globalThis, "clearInterval");
    const { unmount } = render(() => (
      <AppShell
        active="monitor"
        onSelect={() => {}}
        title="Monitor"
        printerRoster={roster()}
        operationalRosters={[]}
        adapterHealth={{ severity: "info", label: "Checking adapters" }}
        lastLiveEventAt="2026-09-18T12:00:00Z"
      >
        <p>Workspace</p>
      </AppShell>
    ));

    expect(screen.getByText("Last live event just now")).toBeInTheDocument();
    vi.advanceTimersByTime(60_000);
    expect(screen.getByText("Last live event 1m ago")).toBeInTheDocument();

    unmount();
    expect(clearIntervalSpy).toHaveBeenCalled();
  });
});
