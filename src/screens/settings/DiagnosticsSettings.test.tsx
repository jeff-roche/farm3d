import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { diagnosticsStoreMock, loadWebDiagnosticsFixture, resetDiagnosticsStoreMock } from "../../diagnostics/diagnostics-store-mock";
import { DiagnosticsSettings } from "./DiagnosticsSettings";

vi.mock("../../diagnostics/diagnostics-store", async () => (await import("../../diagnostics/diagnostics-store-mock")).diagnosticsStoreMock);
vi.mock("../../settings/settings-store", () => ({ settings: () => ({ revision: 7 }), loadSettings: vi.fn(async () => ({ revision: 7 })) }));

beforeEach(() => {
  resetDiagnosticsStoreMock();
  loadWebDiagnosticsFixture();
});
afterEach(cleanup);

describe("diagnostics bundle", () => {
  it("shows each section with its description and size, and what is never included", async () => {
    render(() => <DiagnosticsSettings />);
    const logs = await screen.findByRole("checkbox", { name: /Logs/ });
    expect((logs as HTMLInputElement).checked).toBe(true);
    expect(screen.getByText(/farm3d's log files/)).toBeTruthy();
    expect(screen.getByText("256 KB")).toBeTruthy();
    expect(screen.getByText(/Never included: credentials/)).toBeTruthy();
  });

  it("exports the chosen sections and reports the file name", async () => {
    diagnosticsStoreMock.exportDiagnostics.mockResolvedValueOnce({ status: "exported", exportedAt: "now", fileName: "diag.zip", bytes: 4096, sections: ["about"] } as never);
    render(() => <DiagnosticsSettings />);
    fireEvent.click(await screen.findByRole("checkbox", { name: /Logs/ }));
    fireEvent.click(screen.getByRole("button", { name: "Export diagnostics…" }));
    await waitFor(() => expect(diagnosticsStoreMock.exportDiagnostics).toHaveBeenCalledWith(["about", "health", "storage", "configuration", "recentProblems"]));
    expect(await screen.findByText(/Exported diag\.zip/)).toBeTruthy();
  });

  it("names the section when redaction fails and says nothing was written", async () => {
    diagnosticsStoreMock.exportDiagnostics.mockRejectedValueOnce({
      contractVersion: 1, code: "DIAGNOSTICS_REDACTION_FAILED", message: "Redaction failed.", recovery: [], retryable: false, details: { section: "logs" },
    });
    render(() => <DiagnosticsSettings />);
    fireEvent.click(await screen.findByRole("button", { name: "Export diagnostics…" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Logs");
    expect(alert.textContent).toMatch(/nothing was written/i);
  });

  it("disables Export when no section is chosen", async () => {
    render(() => <DiagnosticsSettings />);
    for (const box of await screen.findAllByRole("checkbox")) fireEvent.click(box);
    expect((screen.getByRole("button", { name: "Export diagnostics…" }) as HTMLButtonElement).disabled).toBe(true);
  });
});

describe("reset", () => {
  it("shows three tiers each with its classes and effects", async () => {
    render(() => <DiagnosticsSettings />);
    for (const name of ["Reset settings", "Reset camera images", "Reset the Farm"]) {
      expect(await screen.findByRole("region", { name })).toBeTruthy();
    }
    const media = screen.getByRole("region", { name: "Reset camera images" });
    expect(within(media).getByText("Unpinned camera images")).toBeTruthy();
    expect(within(media).getByText("Pinned camera images")).toBeTruthy();
  });

  it("states that scope All also prunes pinned evidence", async () => {
    render(() => <DiagnosticsSettings />);
    const media = await screen.findByRole("region", { name: "Reset camera images" });
    expect(within(media).queryByText(/also removes pinned/i)).toBeNull();
    fireEvent.click(within(media).getByLabelText(/All camera images/));
    expect(within(media).getByText(/also removes pinned/i)).toBeTruthy();
  });

  it("resets settings with the phrase and the current revision", async () => {
    diagnosticsStoreMock.resetFarm.mockResolvedValueOnce({ tier: "settings", settings: {} });
    render(() => <DiagnosticsSettings />);
    const region = await screen.findByRole("region", { name: "Reset settings" });
    fireEvent.click(within(region).getByRole("button", { name: "Reset settings…" }));
    fireEvent.input(await screen.findByLabelText('Type "reset settings" to confirm'), { target: { value: "reset settings" } });
    fireEvent.click(screen.getByRole("button", { name: "Reset settings" }));
    await waitFor(() => expect(diagnosticsStoreMock.resetFarm).toHaveBeenCalledWith({ tier: "settings", expectedRevision: 7 }, "reset settings"));
  });

  it("resets camera images with the chosen scope", async () => {
    diagnosticsStoreMock.resetFarm.mockResolvedValueOnce({ tier: "cameraMedia", prunedCount: 9, freedBytes: 3_000_000 });
    render(() => <DiagnosticsSettings />);
    const region = await screen.findByRole("region", { name: "Reset camera images" });
    fireEvent.click(within(region).getByLabelText(/All camera images/));
    fireEvent.click(within(region).getByRole("button", { name: "Reset camera images…" }));
    fireEvent.input(await screen.findByLabelText('Type "reset media" to confirm'), { target: { value: "reset media" } });
    fireEvent.click(screen.getByRole("button", { name: "Reset camera images" }));
    await waitFor(() => expect(diagnosticsStoreMock.resetFarm).toHaveBeenCalledWith({ tier: "cameraMedia", scope: "all" }, "reset media"));
    expect(await screen.findByText(/Removed 9 camera images/)).toBeTruthy();
  });

  it("tier (c) writes a safety backup by default and keeps the old ones", async () => {
    diagnosticsStoreMock.resetFarm.mockResolvedValueOnce({ tier: "farm", status: "restarting", safetyBackupId: "b1" });
    render(() => <DiagnosticsSettings />);
    const region = await screen.findByRole("region", { name: "Reset the Farm" });
    expect((within(region).getByRole("checkbox", { name: "Write a safety backup first" }) as HTMLInputElement).checked).toBe(true);
    expect((within(region).getByRole("checkbox", { name: "Also delete safety backups" }) as HTMLInputElement).checked).toBe(false);
    expect(within(region).getByText(/active/i)).toBeTruthy();
    fireEvent.click(within(region).getByRole("button", { name: "Reset the Farm…" }));
    fireEvent.input(await screen.findByLabelText('Type "reset farm" to confirm'), { target: { value: "reset farm" } });
    fireEvent.click(screen.getByRole("button", { name: "Reset and restart" }));
    await waitFor(() => expect(diagnosticsStoreMock.resetFarm).toHaveBeenCalledWith({ tier: "farm", safetyBackup: true, deleteSafetyBackups: false }, "reset farm"));
    expect((await screen.findByRole("status")).textContent).toContain("Restarting farm3d");
  });

  it("shows a failed reset in the dialog", async () => {
    diagnosticsStoreMock.resetFarm.mockRejectedValueOnce({ contractVersion: 1, code: "PERSISTENCE_UNAVAILABLE", message: "Reset was refused.", recovery: [], retryable: false });
    render(() => <DiagnosticsSettings />);
    const region = await screen.findByRole("region", { name: "Reset settings" });
    fireEvent.click(within(region).getByRole("button", { name: "Reset settings…" }));
    fireEvent.input(await screen.findByLabelText('Type "reset settings" to confirm'), { target: { value: "reset settings" } });
    fireEvent.click(screen.getByRole("button", { name: "Reset settings" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Reset was refused.");
  });
});
