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
});
