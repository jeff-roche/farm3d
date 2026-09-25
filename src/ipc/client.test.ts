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
