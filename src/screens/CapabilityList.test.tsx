import { cleanup, render, screen, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  capabilitiesStoreMock,
  resetCapabilitiesStoreMock,
  setPrinterCapabilitiesForTest,
} from "../host-ops/capabilities-store-mock";
import { printerCapabilities } from "../host-ops/test-records";
import { CapabilityList } from "./CapabilityList";

vi.mock("../host-ops/capabilities-store", async () =>
  (await import("../host-ops/capabilities-store-mock")).capabilitiesStoreMock);

beforeEach(() => resetCapabilitiesStoreMock());
afterEach(cleanup);

describe("CapabilityList", () => {
  it("lists every capability with its label, and the evidence tier or the reason", async () => {
    const base = printerCapabilities();
    setPrinterCapabilitiesForTest({
      ...base,
      capabilities: {
        ...base.capabilities,
        pause: { status: "unsupported", reason: "host", detail: "This printer has no pause and resume support." },
        camera: { status: "unsupported", reason: "notVerified", detail: "Not verified for this Connection type yet." },
        hostState: { status: "supported", evidence: { source: "hardware-runs/x/manifest.json", tier: "readOnlyHardware", verifiedHostVersions: [] } },
      },
    });
    render(() => <CapabilityList printerId="prn-1" />);

    const list = screen.getByRole("list", { name: "Capabilities" });
    const items = within(list).getAllByRole("listitem");
    expect(items).toHaveLength(8);
    const row = (name: string) => items.find((item) => within(item).queryByText(name))!;

    expect(row("Upload")).toHaveTextContent("Supported");
    expect(row("Upload")).toHaveTextContent("Simulator");
    expect(row("Read print state")).toHaveTextContent("Read-only hardware");
    expect(row("Pause")).toHaveTextContent("Not available on this printer");
    expect(row("Pause")).toHaveTextContent("This printer has no pause and resume support.");
    expect(row("Camera")).toHaveTextContent("Not verified yet");
    expect(row("Camera")).toHaveTextContent("Not verified for this Connection type yet.");
    expect(row("Camera")).not.toHaveTextContent("Simulator");
  });

  it("names the adapter for an adapter-reason capability", () => {
    const base = printerCapabilities({ adapterKind: "octoprint" });
    setPrinterCapabilitiesForTest({
      ...base,
      capabilities: { ...base.capabilities, upload: { status: "unsupported", reason: "adapter", detail: "farm3d can't use this Connection type." } },
    });
    render(() => <CapabilityList printerId="prn-1" />);
    expect(screen.getByText("Not supported by OctoPrint")).toBeInTheDocument();
  });

  it("loads the capabilities when none are held, and shows a load failure inline with Try again", async () => {
    capabilitiesStoreMock.loadCapabilities.mockRejectedValueOnce({
      contractVersion: 1, code: "PERSISTENCE_UNAVAILABLE", message: "Storage is busy.", recovery: ["RETRY"], retryable: true,
    });
    render(() => <CapabilityList printerId="prn-1" />);
    expect(capabilitiesStoreMock.loadCapabilities).toHaveBeenCalledWith("prn-1");
    expect(await screen.findByRole("alert")).toHaveTextContent("Storage is busy.");
    expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
  });
});
