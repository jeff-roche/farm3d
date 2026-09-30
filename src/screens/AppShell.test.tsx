import { fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MonitorPrinterView, MonitorRosterView } from "../monitor/monitor-store";
import { resetAttentionStoreMock } from "../attention/attention-store-mock";
import { resetDiagnosticsStoreMock, setDiagnosticsStoreState } from "../diagnostics/diagnostics-store-mock";
import { webAboutInfo } from "../diagnostics/web-fixtures";
import { backupStoreMock, resetBackupStoreMock, setBackupStoreState } from "../backup/backup-store-mock";
import { AppShell } from "./AppShell";

vi.mock("../attention/attention-store", async () => (await import("../attention/attention-store-mock")).attentionStoreMock);
vi.mock("../backup/backup-store", async () => (await import("../backup/backup-store-mock")).backupStoreMock);
vi.mock("../diagnostics/about-store", async () =>
  (await import("../diagnostics/diagnostics-store-mock")).aboutStoreMock);

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
  resetAttentionStoreMock();
  resetDiagnosticsStoreMock();
  resetBackupStoreMock();
});

function renderShell() {
  return render(() => (
    <AppShell
      active="monitor"
      onSelect={vi.fn()}
      title="Monitor"
      printerRoster={roster()}
      operationalRosters={[]}
      adapterHealth={{ severity: "resolved", label: "All adapters connected" }}
    >
      <p>Workspace</p>
    </AppShell>
  ));
}

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
    await fireEvent.click(totalRoster);
    expect(await screen.findByText("Bay One")).toBeInTheDocument();
    expect(screen.getByRole("status", { name: "All adapters connected" })).toBeInTheDocument();
    expect(screen.getByText("Last live event 2m ago")).toBeInTheDocument();
    expect(screen.queryByText(/job/i)).not.toBeInTheDocument();
    // The Attention trigger is always present, unlike the active-Job count.
    expect(screen.getByRole("button", { name: "Attention: nothing needs action" })).toBeInTheDocument();

    const shell = container.querySelector("div");
    expect(Array.from(shell?.children ?? []).map((child) => child.nodeName)).toEqual([
      "HEADER", "NAV", "MAIN", "FOOTER",
    ]);
  });

  it("keeps the header's Tab order through its roster chips", async () => {
    render(() => (
      <AppShell
        active="monitor"
        onSelect={() => {}}
        title="Monitor"
        printerRoster={roster()}
        operationalRosters={[roster({ key: "ready", label: "Ready", count: 1 })]}
        adapterHealth={{ severity: "resolved", label: "All adapters connected" }}
      >
        <p>Workspace</p>
      </AppShell>
    ));
    const total = screen.getByRole("button", { name: "3 Printers" });
    const ready = screen.getByRole("button", { name: "1 Ready" });

    total.focus();
    await fireEvent.focus(total);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(document.activeElement).toBe(total);
    expect(total).toHaveAttribute("aria-expanded", "false");

    await fireEvent.click(total);
    const list = await screen.findByRole("dialog", { hidden: true });
    await waitFor(() => expect(list).toHaveFocus());
    await fireEvent.keyDown(list, { key: "Tab" });
    await waitFor(() => expect(ready).toHaveFocus());

    await fireEvent.click(ready);
    const readyList = await screen.findByRole("dialog", { hidden: true });
    await waitFor(() => expect(readyList).toHaveFocus());
    await fireEvent.keyDown(readyList, { key: "Tab", shiftKey: true });
    await waitFor(() => expect(ready).toHaveFocus());
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

  it("shows the active Job count as a focusable roster that names each Job's Printer and state", async () => {
    const onSelect = vi.fn();
    render(() => (
      <AppShell
        active="monitor"
        onSelect={onSelect}
        title="Monitor"
        printerRoster={roster()}
        operationalRosters={[]}
        adapterHealth={{ severity: "resolved", label: "All adapters connected" }}
        activeJobs={[{ id: "job-1", name: "Bay One", detail: "Bracket", stateLabel: "Printing" }]}
        queueAttentionCount={2}
        attentionActionableCount={3}
      >
        <p>Workspace</p>
      </AppShell>
    ));
    const jobs = screen.getByRole("button", { name: "1 Active Jobs" });
    await fireEvent.click(jobs);
    expect(await screen.findByText("Printing")).toBeInTheDocument();
    expect(screen.getByText("Bay One")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Queue (2 need attention)" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Monitor (3 need attention)" })).toBeInTheDocument();
  });

  it("shows the version from About in the status bar, not a hard-coded one", async () => {
    setDiagnosticsStoreState({ about: { ...webAboutInfo(), appVersion: "7.8.9" } });
    render(() => (
      <AppShell
        active="monitor"
        onSelect={vi.fn()}
        title="Monitor"
        printerRoster={roster()}
        operationalRosters={[]}
        adapterHealth={{ severity: "resolved", label: "All adapters connected" }}
      >
        <p>Workspace</p>
      </AppShell>
    ));

    expect(await screen.findByText("farm3d 7.8.9")).toBeInTheDocument();
    expect(screen.queryByText("farm3d 0.1.0")).not.toBeInTheDocument();
  });

  it("reads restore_status on mount and shows no banner when nothing finished", () => {
    renderShell();
    expect(backupStoreMock.loadRestoreStatus).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("button", { name: "Dismiss" })).not.toBeInTheDocument();
  });

  it("shows a finished restore's outcome until the operator dismisses it", async () => {
    setBackupStoreState({
      restoreStatus: {
        state: "done", journalId: "rst-1", kind: "restore", finishedAt: "2026-09-30T12:00:00Z",
        safetyBackupId: "sfb-1",
      },
    });
    renderShell();
    expect(screen.getByText("The restore finished. You can restore from safety backup sfb-1.").parentElement)
      .toHaveAttribute("role", "status");
    await fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(backupStoreMock.acknowledgeRestoreStatus).toHaveBeenCalledTimes(1);
  });

  it("announces a failed reset as an alert", () => {
    setBackupStoreState({
      restoreStatus: {
        state: "failed", journalId: "rsf-1", kind: "reset", finishedAt: "2026-09-30T12:00:00Z",
        code: "RESTORE_FAILED", failedStep: null, safetyBackupId: null,
      },
    });
    renderShell();
    expect(screen.getByRole("alert")).toHaveTextContent("The reset did not finish, and your previous data was kept.");
  });
});
