import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }));

describe("IPC client", () => {
  beforeEach(() => invoke.mockReset());

  it("adds the contract version and unwraps successful data", async () => {
    invoke.mockResolvedValue({ contractVersion: 1, data: [] });
    const { command } = await import("./client");

    await expect(command("list_catalog_variants", { vendor: "Prusa", model: "MK4" })).resolves.toEqual([]);
    expect(invoke).toHaveBeenCalledWith("list_catalog_variants", {
      contractVersion: 1,
      vendor: "Prusa",
      model: "MK4",
    });
  });

  it("keeps raw-byte commands out of the JSON command map", async () => {
    const { command } = await import("./client");
    // `tsc` (just build) fails if this ever type-checks again.
    // @ts-expect-error get_revision_mesh answers with raw bytes (BinaryCommandMap).
    const call = () => command("get_revision_mesh", { revisionId: "msr-1", objectKey: 1 });
    expect(call).toBeTypeOf("function");
  });

  it("invokes a raw-byte command and resolves with its ArrayBuffer", async () => {
    const bytes = new Uint8Array([0x46, 0x33, 0x44, 0x4d]).buffer;
    invoke.mockResolvedValue(bytes);
    const { binaryCommand } = await import("./client");

    await expect(binaryCommand("get_revision_mesh", { revisionId: "msr-1", objectKey: 1 })).resolves.toBe(bytes);
    expect(invoke).toHaveBeenCalledWith("get_revision_mesh", { contractVersion: 1, revisionId: "msr-1", objectKey: 1 });
  });

  it("copies a typed-array view or a plain byte array into an ArrayBuffer", async () => {
    const { binaryCommand } = await import("./client");
    const backing = new Uint8Array([9, 1, 2, 3, 9]);
    invoke.mockResolvedValueOnce(backing.subarray(1, 4)).mockResolvedValueOnce([4, 5]);

    const fromView = await binaryCommand("get_revision_mesh", { revisionId: "msr-1", objectKey: 1 });
    expect([...new Uint8Array(fromView)]).toEqual([1, 2, 3]);
    const fromArray = await binaryCommand("get_revision_mesh", { revisionId: "msr-1", objectKey: 1 });
    expect([...new Uint8Array(fromArray)]).toEqual([4, 5]);
  });

  it("rejects a raw-byte command with the backend's CommandError, and a non-byte answer as incompatible", async () => {
    const notFound = {
      contractVersion: 1,
      code: "NOT_FOUND",
      message: "msr-1/9",
      recovery: [],
      retryable: false,
    };
    invoke.mockRejectedValueOnce(notFound).mockResolvedValueOnce({ contractVersion: 1, data: [] });
    const { binaryCommand } = await import("./client");

    await expect(binaryCommand("get_revision_mesh", { revisionId: "msr-1", objectKey: 9 })).rejects.toBe(notFound);
    await expect(binaryCommand("get_revision_mesh", { revisionId: "msr-1", objectKey: 1 })).rejects.toMatchObject({
      code: "INCOMPATIBLE_CONTRACT_VERSION",
    });
  });

  it("keeps JSON commands out of the raw-byte map", async () => {
    const { binaryCommand } = await import("./client");
    // @ts-expect-error list_slicing answers with the JSON envelope (CommandMap).
    const call = () => binaryCommand("list_slicing", {});
    expect(call).toBeTypeOf("function");
  });

  it("recognizes a structured command rejection", async () => {
    const failure = {
      contractVersion: 1,
      code: "CONFLICT",
      message: "State changed",
      recovery: ["RELOAD"],
      retryable: false,
    };
    const { isCommandError } = await import("./client");

    expect(isCommandError(failure)).toBe(true);
    if (isCommandError(failure)) {
      expect(failure.code).toBe("CONFLICT");
      expect(failure.recovery).toEqual(["RELOAD"]);
    }
  });

  it("rejects incompatible successful envelopes", async () => {
    invoke.mockResolvedValue({ contractVersion: 2, data: null });
    const { command } = await import("./client");

    await expect(command("catalog_info")).rejects.toMatchObject({
      code: "INCOMPATIBLE_CONTRACT_VERSION",
      contractVersion: 1,
    });
  });

  it("preserves valid structured errors and replaces incompatible rejected envelopes", async () => {
    const { command } = await import("./client");
    const conflict = {
      contractVersion: 1,
      code: "CONFLICT",
      message: "State changed",
      recovery: ["RELOAD"],
      retryable: false,
    };
    invoke.mockRejectedValueOnce(conflict).mockRejectedValueOnce({
      ...conflict,
      contractVersion: 2,
    });

    await expect(command("catalog_info")).rejects.toBe(conflict);
    await expect(command("catalog_info")).rejects.toMatchObject({
      contractVersion: 1,
      code: "INCOMPATIBLE_CONTRACT_VERSION",
      details: { supportedVersion: 1, receivedVersion: 2 },
    });
  });

  it("seeds a debug reservation only in development builds", async () => {
    invoke.mockResolvedValue({ contractVersion: 1, data: {} });
    const { debugSeedReservation } = await import("./client");

    await debugSeedReservation("spl-1", 200_000);
    expect(invoke).toHaveBeenCalledWith("debug_seed_reservation", {
      contractVersion: 1,
      spoolId: "spl-1",
      amountMg: 200_000,
    });

    invoke.mockReset();
    vi.stubEnv("DEV", false);
    try {
      await expect(debugSeedReservation("spl-1", 200_000)).rejects.toThrow("development");
      expect(invoke).not.toHaveBeenCalled();
    } finally {
      vi.unstubAllEnvs();
    }
  });
});
