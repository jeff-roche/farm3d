import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { backupStoreMock, loadWebBackupFixture, resetBackupStoreMock, setBackupStoreState } from "../../backup/backup-store-mock";
import { loadWebDiagnosticsFixture, resetDiagnosticsStoreMock, diagnosticsStoreMock } from "../../diagnostics/diagnostics-store-mock";
import { webStorageUsage } from "../../diagnostics/web-fixtures";
import { webRestorePreview } from "../../backup/web-fixtures";
import { StorageSettings } from "./StorageSettings";

vi.mock("../../backup/backup-store", async () => (await import("../../backup/backup-store-mock")).backupStoreMock);
vi.mock("../../diagnostics/diagnostics-store", async () => (await import("../../diagnostics/diagnostics-store-mock")).diagnosticsStoreMock);

beforeEach(() => {
  resetBackupStoreMock();
  resetDiagnosticsStoreMock();
  loadWebBackupFixture();
  loadWebDiagnosticsFixture();
});
afterEach(cleanup);

describe("storage usage and cleanup", () => {
  it("lists every class with its size and total", async () => {
    render(() => <StorageSettings />);
    const table = await screen.findByRole("grid", { name: "Storage usage" });
    expect(within(table).getByText("Database")).toBeTruthy();
    expect(within(table).getByText("Slicer profile cache")).toBeTruthy();
    expect(screen.getByText(/Total/)).toBeTruthy();
  });

  it("shows a pending state and then the result of a cleanup", async () => {
    let finish: (value: unknown) => void = () => {};
    diagnosticsStoreMock.clearStorage.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    render(() => <StorageSettings />);
    const button = await screen.findByRole("button", { name: "Remove unreferenced content" });
    fireEvent.click(button);
    expect(diagnosticsStoreMock.clearStorage).toHaveBeenCalledWith("unreferencedContent");
    await waitFor(() => expect((screen.getByRole("button", { name: "Remove unreferenced content" }) as HTMLButtonElement).disabled).toBe(true));
    finish({ target: "unreferencedContent", removedCount: 2, freedBytes: 122_880, usage: webStorageUsage() });
    expect(await screen.findByText(/Removed 2 items, freed 120 KB/)).toBeTruthy();
  });

  it("shows a cleanup failure as an alert", async () => {
    diagnosticsStoreMock.clearStorage.mockRejectedValueOnce(new Error("Storage is busy"));
    render(() => <StorageSettings />);
    fireEvent.click(await screen.findByRole("button", { name: "Remove rotated logs" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Storage is busy");
  });
});

describe("create backup", () => {
  it("shows the estimated size for each media choice and creates with the choice", async () => {
    backupStoreMock.createBackup.mockResolvedValueOnce({
      status: "exported", exportedAt: "now", fileName: "farm.farm3d-backup", bytes: 2048, media: "pinned", mediaNotInBackup: 0, mediaMissingFile: 0,
    } as never);
    render(() => <StorageSettings />);
    const pinned = await screen.findByLabelText(/Pinned camera images/);
    expect(screen.getByText(/Pinned camera images \(2 images/)).toBeTruthy();
    fireEvent.click(pinned);
    expect(screen.getByText(/Estimated size: 11 MB/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Create backup…" }));
    expect(backupStoreMock.createBackup).toHaveBeenCalledWith("pinned");
    expect(await screen.findByText(/Exported farm\.farm3d-backup/)).toBeTruthy();
  });

  it("says what a backup never holds", async () => {
    render(() => <StorageSettings />);
    expect(await screen.findByText(/never holds credentials/i)).toBeTruthy();
  });

  it("explains an unsupported create in web mode", async () => {
    backupStoreMock.createBackup.mockResolvedValueOnce({ status: "unsupported", reason: "desktopRequired" } as never);
    render(() => <StorageSettings />);
    fireEvent.click(await screen.findByRole("button", { name: "Create backup…" }));
    expect(await screen.findByText(/needs the desktop app/i)).toBeTruthy();
  });
});

describe("safety backups", () => {
  it("lists them with restore and delete, and only delete for an invalid one", async () => {
    render(() => <StorageSettings />);
    const list = await screen.findByRole("list", { name: "Safety backups" });
    const items = within(list).getAllByRole("listitem");
    expect(items).toHaveLength(3);
    expect(within(items[0]).getByRole("button", { name: /Restore/ })).toBeTruthy();
    expect(within(items[2]).queryByRole("button", { name: /Restore/ })).toBeNull();
    expect(within(items[2]).getByRole("button", { name: /Delete/ })).toBeTruthy();
    fireEvent.click(within(items[0]).getByRole("button", { name: /Restore/ }));
    expect(backupStoreMock.startRestore).toHaveBeenCalledWith({ kind: "safetyBackup", backupId: "bkp-web-before-restore" });
  });

  it("deletes after confirmation", async () => {
    render(() => <StorageSettings />);
    const list = await screen.findByRole("list", { name: "Safety backups" });
    fireEvent.click(within(within(list).getAllByRole("listitem")[1]).getByRole("button", { name: /Delete/ }));
    expect(backupStoreMock.deleteBackup).not.toHaveBeenCalled();
    fireEvent.click(await screen.findByRole("button", { name: "Delete backup" }));
    await waitFor(() => expect(backupStoreMock.deleteBackup).toHaveBeenCalledWith("bkp-web-before-reset"));
  });
});

describe("restore", () => {
  it("starts a restore from a file", async () => {
    render(() => <StorageSettings />);
    fireEvent.click(await screen.findByRole("button", { name: "Restore from file…" }));
    expect(backupStoreMock.startRestore).toHaveBeenCalledWith({ kind: "file" });
  });

  it("shows the preview panel while previewing and leaves the panel on unmount", async () => {
    setBackupStoreState({ phase: "previewing", preview: webRestorePreview() });
    const view = render(() => <StorageSettings />);
    expect(await screen.findByRole("region", { name: "Restore preview" })).toBeTruthy();
    view.unmount();
    expect(backupStoreMock.leaveRestorePanel).toHaveBeenCalled();
  });

  it("announces the restart", async () => {
    setBackupStoreState({ phase: "restarting", preview: webRestorePreview() });
    render(() => <StorageSettings />);
    expect((await screen.findByRole("status")).textContent).toContain("Restarting farm3d to finish the restore");
  });

  it("shows a failed restore and dismisses it", async () => {
    setBackupStoreState({ phase: "failed", error: { contractVersion: 1, code: "BACKUP_INVALID", message: "That file is not a backup.", recovery: [], retryable: false } });
    render(() => <StorageSettings />);
    expect((await screen.findByRole("alert")).textContent).toContain("That file is not a backup.");
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(backupStoreMock.dismissRestore).toHaveBeenCalled();
  });
});
