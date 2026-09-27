import { describe, expect, it } from "vitest";
import { capabilitiesStoreMock, resetCapabilitiesStoreMock, setPrinterCapabilitiesForTest } from "./capabilities-store-mock";
import { printerCapabilities } from "./test-records";

describe("capabilitiesStoreMock", () => {
  it("mirrors every export of the real store's API", async () => {
    const real = await import("./capabilities-store");
    expect(Object.keys(capabilitiesStoreMock).sort()).toEqual(Object.keys(real).sort());
    expect(Object.keys(capabilitiesStoreMock.capabilities).sort()).toEqual(Object.keys(real.capabilities).sort());
  });

  it("serves what a test sets, until reset", () => {
    setPrinterCapabilitiesForTest(printerCapabilities({ printerId: "prn-9" }));
    expect(capabilitiesStoreMock.capabilities.forPrinter("prn-9")?.printerId).toBe("prn-9");
    resetCapabilitiesStoreMock();
    expect(capabilitiesStoreMock.capabilities.forPrinter("prn-9")).toBeUndefined();
  });
});
