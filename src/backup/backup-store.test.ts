import { beforeEach, describe, expect, it, vi } from "vitest";
import { webRestorePreview } from "./web-fixtures";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

const ok = (data: unknown) => ({ contractVersion: 1, data });
const failure = (code: string) => ({ contractVersion: 1, code, message: code, recovery: [], retryable: false });
const preview = webRestorePreview();
const uuid = /^[0-9a-f-]{36}$/;

async function load() {
  return import("./backup-store");
}

beforeEach(() => {
  vi.resetModules();
  tauriMock.isTauri.mockReset().mockReturnValue(true);
  tauriMock.invoke.mockReset();
});

describe("restore flow", () => {
  it("runs idle, choosing, previewing, confirming, restarting", async () => {
    const store = await load();
    expect(store.backup.restore.phase()).toBe("idle");
    let phaseDuringDialog = "";
    tauriMock.invoke.mockImplementationOnce(async () => {
      phaseDuringDialog = store.backup.restore.phase();
      return ok({ status: "previewed", preview });
    });
    await store.startRestore({ kind: "file" });
    expect(phaseDuringDialog).toBe("choosing");
    expect(store.backup.restore.phase()).toBe("previewing");
    expect(store.backup.restore.preview()?.stagingId).toBe("stg-web-1");
    expect(tauriMock.invoke).toHaveBeenCalledWith("preview_restore", { contractVersion: 1, source: { kind: "file" } });

    store.beginRestoreConfirm();
    expect(store.backup.restore.phase()).toBe("confirming");
    tauriMock.invoke.mockResolvedValueOnce(ok({ status: "restarting", safetyBackupId: "bkp-1" }));
    await store.applyRestore("restore");
    expect(store.backup.restore.phase()).toBe("restarting");
    expect(store.backup.restore.safetyBackupId()).toBe("bkp-1");
    const [name, args] = tauriMock.invoke.mock.calls[1];
    expect(name).toBe("apply_restore");
    expect(args).toMatchObject({ stagingId: "stg-web-1", confirmation: "restore" });
    expect(args.operationId).toMatch(uuid);
  });

  it("goes to cancelled when the dialog is closed, then idle on dismiss", async () => {
    const store = await load();
    tauriMock.invoke.mockResolvedValueOnce(ok({ status: "cancelled" }));
    await store.startRestore({ kind: "file" });
    expect(store.backup.restore.phase()).toBe("cancelled");
    expect(store.backup.restore.preview()).toBeNull();
    await store.dismissRestore();
    expect(store.backup.restore.phase()).toBe("idle");
  });

  it("goes to failed with the error, and dismiss discards the staged preview", async () => {
    const store = await load();
    tauriMock.invoke.mockResolvedValueOnce(ok({ status: "previewed", preview }));
    await store.startRestore({ kind: "safetyBackup", backupId: "bkp-1" });
    store.beginRestoreConfirm();
    tauriMock.invoke.mockRejectedValueOnce(failure("RESTORE_BLOCKED"));
    await store.applyRestore("restore");
    expect(store.backup.restore.phase()).toBe("failed");
    expect(store.backup.restore.error()?.code).toBe("RESTORE_BLOCKED");
    tauriMock.invoke.mockResolvedValueOnce(ok({ discarded: true }));
    await store.dismissRestore();
    expect(tauriMock.invoke).toHaveBeenLastCalledWith("discard_restore_preview", { contractVersion: 1, stagingId: "stg-web-1" });
    expect(store.backup.restore.phase()).toBe("idle");
    expect(store.backup.restore.preview()).toBeNull();
  });

  it("fails when the preview command fails", async () => {
    const store = await load();
    tauriMock.invoke.mockRejectedValueOnce(failure("BACKUP_INVALID"));
    await store.startRestore({ kind: "file" });
    expect(store.backup.restore.phase()).toBe("failed");
    expect(store.backup.restore.error()?.code).toBe("BACKUP_INVALID");
  });

  it("can back out of confirming to previewing", async () => {
    const store = await load();
    tauriMock.invoke.mockResolvedValueOnce(ok({ status: "previewed", preview }));
    await store.startRestore({ kind: "file" });
    store.beginRestoreConfirm();
    store.cancelRestoreConfirm();
    expect(store.backup.restore.phase()).toBe("previewing");
  });

  it("discards the preview on leaving the panel, but not while restarting", async () => {
    const store = await load();
    tauriMock.invoke.mockResolvedValueOnce(ok({ status: "previewed", preview }));
    await store.startRestore({ kind: "file" });
    tauriMock.invoke.mockResolvedValueOnce(ok({ discarded: true }));
    await store.leaveRestorePanel();
    expect(tauriMock.invoke).toHaveBeenLastCalledWith("discard_restore_preview", { contractVersion: 1, stagingId: "stg-web-1" });
    expect(store.backup.restore.phase()).toBe("idle");

    tauriMock.invoke.mockResolvedValueOnce(ok({ status: "previewed", preview }));
    await store.startRestore({ kind: "file" });
    store.beginRestoreConfirm();
    tauriMock.invoke.mockResolvedValueOnce(ok({ status: "restarting", safetyBackupId: "b" }));
    await store.applyRestore("restore");
    tauriMock.invoke.mockClear();
    await store.leaveRestorePanel();
    expect(tauriMock.invoke).not.toHaveBeenCalled();
    expect(store.backup.restore.phase()).toBe("restarting");
  });

  it("reuses the operationId on one transport retry", async () => {
    const store = await load();
    tauriMock.invoke.mockResolvedValueOnce(ok({ status: "previewed", preview }));
    await store.startRestore({ kind: "file" });
    store.beginRestoreConfirm();
    tauriMock.invoke
      .mockRejectedValueOnce(new Error("transport"))
      .mockResolvedValueOnce(ok({ status: "restarting", safetyBackupId: "b" }));
    await store.applyRestore("restore");
    const applies = tauriMock.invoke.mock.calls.filter(([name]) => name === "apply_restore");
    expect(applies).toHaveLength(2);
    expect(applies[0][1].operationId).toBe(applies[1][1].operationId);
  });
});

describe("restore status banner", () => {
  const done = { state: "done", journalId: "jrn-1", kind: "restore", finishedAt: "2026-09-29T10:00:00Z", safetyBackupId: "bkp-1" };

  it("is read once at startup and acknowledged", async () => {
    tauriMock.invoke.mockResolvedValueOnce(ok(done));
    const store = await load();
    await store.loadRestoreStatus();
    await store.loadRestoreStatus();
    expect(tauriMock.invoke).toHaveBeenCalledTimes(1);
    expect(store.backup.restoreBanner()?.state).toBe("done");
    tauriMock.invoke.mockResolvedValueOnce(ok({ state: "none" }));
    await store.acknowledgeRestoreStatus();
    expect(tauriMock.invoke).toHaveBeenLastCalledWith("acknowledge_restore_status", { contractVersion: 1, journalId: "jrn-1" });
    expect(store.backup.restoreBanner()).toBeNull();
  });

  it("shows nothing for state none", async () => {
    tauriMock.invoke.mockResolvedValueOnce(ok({ state: "none" }));
    const store = await load();
    await store.loadRestoreStatus();
    expect(store.backup.restoreBanner()).toBeNull();
  });

  it("keeps the banner when the acknowledgement fails", async () => {
    tauriMock.invoke.mockResolvedValueOnce(ok(done));
    const store = await load();
    await store.loadRestoreStatus();
    tauriMock.invoke.mockRejectedValueOnce(failure("NOT_FOUND"));
    await expect(store.acknowledgeRestoreStatus()).rejects.toMatchObject({ code: "NOT_FOUND" });
    expect(store.backup.restoreBanner()?.state).toBe("done");
  });
});

describe("backup commands", () => {
  it("createBackup sends media with an operationId and keeps only the file name", async () => {
    const store = await load();
    tauriMock.invoke.mockResolvedValueOnce(ok({
      status: "exported", exportedAt: "2026-09-29T10:00:00Z", fileName: "farm.farm3d-backup", bytes: 10, media: "pinned",
      mediaNotInBackup: 0, mediaMissingFile: 1, path: "/home/someone/farm.farm3d-backup",
    }));
    const outcome = await store.createBackup("pinned");
    expect(tauriMock.invoke.mock.calls[0][1]).toMatchObject({ media: "pinned" });
    expect(tauriMock.invoke.mock.calls[0][1].operationId).toMatch(uuid);
    expect(JSON.stringify(outcome)).not.toContain("/home/");
    expect(JSON.stringify(store.backup.lastExport())).not.toContain("/home/");
    expect(store.backup.lastExport()?.fileName).toBe("farm.farm3d-backup");
  });

  it("createBackup passes cancelled through and records nothing", async () => {
    const store = await load();
    tauriMock.invoke.mockResolvedValueOnce(ok({ status: "cancelled" }));
    expect(await store.createBackup("none")).toEqual({ status: "cancelled" });
    expect(store.backup.lastExport()).toBeNull();
  });

  it("lists backups, loads the inventory, and deletes one", async () => {
    const { webBackupList, webBackupInventory } = await import("./web-fixtures");
    const store = await load();
    tauriMock.invoke.mockResolvedValueOnce(ok(webBackupList()));
    await store.loadBackups();
    expect(store.backup.backups()).toHaveLength(3);
    tauriMock.invoke.mockResolvedValueOnce(ok(webBackupInventory()));
    await store.loadInventory();
    expect(store.backup.inventory()?.credentialRefCount).toBe(2);
    tauriMock.invoke.mockResolvedValueOnce(ok({ backupId: "bkp-web-broken", deleted: true }));
    await store.deleteBackup("bkp-web-broken");
    expect(store.backup.backups().map((b) => b.backupId)).not.toContain("bkp-web-broken");
  });
});

describe("web mode (D18)", () => {
  beforeEach(() => tauriMock.isTauri.mockReturnValue(false));

  it("returns fixtures for reads without IPC", async () => {
    const store = await load();
    await store.loadInventory();
    await store.loadBackups();
    await store.loadRestoreStatus();
    expect(store.backup.inventory()).not.toBeNull();
    expect(store.backup.backups().length).toBeGreaterThan(0);
    expect(store.backup.restoreBanner()).toBeNull();
    expect(tauriMock.invoke).not.toHaveBeenCalled();
  });

  it("answers the dialog-owning wrappers with unsupported, without IPC", async () => {
    const store = await load();
    expect(await store.createBackup("none")).toEqual({ status: "unsupported", reason: "desktopRequired" });
    expect(await store.startRestore({ kind: "file" })).toEqual({ status: "unsupported", reason: "desktopRequired" });
    expect(store.backup.restore.phase()).toBe("idle");
    expect(tauriMock.invoke).not.toHaveBeenCalled();
  });

  it("throws needsDesktopError for the other writes", async () => {
    const store = await load();
    await expect(store.deleteBackup("x")).rejects.toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
    await expect(store.applyRestore("restore")).rejects.toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
    await expect(store.startRestore({ kind: "safetyBackup", backupId: "x" })).resolves.toBeDefined();
    expect(store.backup.restore.phase()).toBe("failed");
    expect(tauriMock.invoke).not.toHaveBeenCalled();
  });
});
