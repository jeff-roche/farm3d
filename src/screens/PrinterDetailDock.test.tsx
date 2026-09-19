import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ResolvedPrinter } from "../printers/types";
import { PrinterDetailDock } from "./PrinterDetailDock";

vi.mock("./PrinterProfilePanel", () => ({
  PrinterProfilePanel: () => <div>Profile setup</div>,
}));

vi.mock("./PrinterConnectionPanel", () => ({
  PrinterConnectionPanel: () => <div>Connection setup</div>,
}));

vi.mock("./PrinterSetupPanel", () => ({
  PrinterSetupPanel: () => <div>Identity setup</div>,
}));

const printer: ResolvedPrinter = {
  id: "prn-1", revision: 1, name: "North Bay", notes: "", overrides: {},
  catalogRef: { vendor: "Bambu Lab", model: "X1 Carbon", variant: "X1 Carbon 0.4", modelId: "x1", printerVariant: "0.4" },
  catalogStatus: "ok", modelLabel: "X1 Carbon", variantLabel: "X1 Carbon 0.4", overriddenFields: [], inherited: {},
  profileDrift: [], unknownOverrideKeys: [], createdAt: "", updatedAt: "",
  profile: { bedShape: { kind: "rectangular", widthMm: 256, depthMm: 0, originXMm: 0, originYMm: 0 }, printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "", nozzleDiameterMm: [0.4], nozzleType: "brass", gcodeFlavor: "klipper", hasAuxiliaryFan: false, supportsAirFiltration: false, supportsMultiFilament: false, suggestedHostType: null },
  runtimeStatus: {
    connectionState: "online", telemetry: { hostActivity: "idle", nozzleTempC: 210, nozzleTargetC: 210 },
    operationalState: "ready", readiness: { state: "ready", reason: null }, freshness: "fresh", cacheWarnings: [], updatedAt: "2026-09-18T12:00:00Z",
  },
};

describe("PrinterDetailDock", () => {
  afterEach(cleanup);

  it("uses a modal dialog in overlay mode, supports keyboard tabs, and closes on Escape", async () => {
    const onClose = vi.fn();
    render(() => <PrinterDetailDock printer={printer} mode="overlay" onClose={onClose} onRemove={vi.fn()} />);

    expect(screen.getByRole("dialog", { name: "North Bay" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Status" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Setup" })).toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: /Job|Camera/ })).not.toBeInTheDocument();

    const status = screen.getByRole("tab", { name: "Status" });
    status.focus();
    await fireEvent.keyDown(status, { key: "ArrowRight" });
    expect(screen.getByRole("tab", { name: "Setup" })).toHaveFocus();
    expect(screen.getByText("Profile setup")).toBeInTheDocument();

    await fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("renders complementary inline content without dialog semantics", () => {
    render(() => <PrinterDetailDock printer={printer} mode="inline" onClose={vi.fn()} onRemove={vi.fn()} />);

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("complementary", { name: "North Bay" })).toBeInTheDocument();
  });

  it("keeps removal under Setup and identifies the selected Printer", async () => {
    const onRemove = vi.fn();
    render(() => <PrinterDetailDock printer={printer} mode="inline" onClose={vi.fn()} onRemove={onRemove} />);

    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));
    await fireEvent.click(screen.getByRole("button", { name: "Remove Printer" }));

    expect(onRemove).toHaveBeenCalledWith("prn-1");
  });

  it("renders nothing for an unknown selection", () => {
    const { container } = render(() => <PrinterDetailDock mode="inline" onClose={vi.fn()} onRemove={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });
});
