import { afterEach, describe, expect, it } from "vitest";
import { clearPrinterJobRequest, openPrinterJob, printerJobRequest } from "./open-printer-job";

afterEach(() => {
  clearPrinterJobRequest();
  window.location.hash = "";
});

function navigateTo(hash: string): void {
  window.location.hash = hash;
  window.dispatchEvent(new HashChangeEvent("hashchange"));
}

describe("openPrinterJob", () => {
  it("navigates to the Printer in Monitor and raises a request for its Job tab", () => {
    openPrinterJob("prn-1");
    expect(window.location.hash).toBe("#nav=v1/monitor/printer/prn-1");
    expect(printerJobRequest()).toEqual({ printerId: "prn-1" });
  });

  it("keeps the request through its own navigation, and drops it on any later one", () => {
    openPrinterJob("prn-1");
    window.dispatchEvent(new HashChangeEvent("hashchange"));
    expect(printerJobRequest()).toEqual({ printerId: "prn-1" });
    navigateTo("#nav=v1/library");
    expect(printerJobRequest()).toBeUndefined();
  });
});
