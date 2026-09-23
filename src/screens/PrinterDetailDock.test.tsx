import { Show } from "solid-js";
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

const archivePrinter = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
const unarchivePrinter = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
const lifecycleEligibility = vi.hoisted(() => vi.fn());
const printersMock = vi.hoisted(() => vi.fn().mockReturnValue([]));
vi.mock("../printers/printer-store", () => ({
  archivePrinter,
  unarchivePrinter,
  lifecycleEligibility,
  printers: printersMock,
}));

vi.mock("./DeletePrinterDialog", () => ({
  DeletePrinterDialog: (props: { open: boolean; printerName: string; onDeleted?: () => void }) => (
    <Show when={props.open}>
      <div>
        <p>Delete dialog open for {props.printerName}</p>
        <button onClick={() => props.onDeleted?.()}>Confirm delete (stub)</button>
      </div>
    </Show>
  ),
}));

const ACTIVE_ELIGIBILITY = {
  canArchive: true,
  canUnarchive: false,
  canDelete: false,
  blockers: [
    { action: "delete" as const, code: "NOT_ARCHIVED" as const, message: "Archive this Printer before deleting it." },
    { action: "unarchive" as const, code: "NOT_ARCHIVED" as const, message: "This Printer is not archived." },
  ],
};

const ARCHIVED_ELIGIBILITY = {
  canArchive: false,
  canUnarchive: true,
  canDelete: true,
  blockers: [
    { action: "archive" as const, code: "ALREADY_ARCHIVED" as const, message: "This Printer is already archived." },
  ],
};

function makePrinter(overrides: Partial<ResolvedPrinter> = {}): ResolvedPrinter {
  return {
    id: "prn-1", revision: 1, name: "North Bay", notes: "", overrides: {},
    catalogRef: { vendor: "Bambu Lab", model: "X1 Carbon", variant: "X1 Carbon 0.4", modelId: "x1", printerVariant: "0.4" },
    catalogStatus: "ok", modelLabel: "X1 Carbon", variantLabel: "X1 Carbon 0.4", overriddenFields: [], inherited: {},
    profileDrift: [], unknownOverrideKeys: [], startSafety: "confirmBedClear", setupGaps: [], createdAt: "", updatedAt: "",
    profile: { bedShape: { kind: "rectangular", widthMm: 256, depthMm: 0, originXMm: 0, originYMm: 0 }, printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "", nozzleDiameterMm: [0.4], nozzleType: "brass", gcodeFlavor: "klipper", hasAuxiliaryFan: false, supportsAirFiltration: false, supportsMultiFilament: false, suggestedHostType: null },
    runtimeStatus: {
      connectionState: "online", telemetry: { hostActivity: "idle", nozzleTempC: 210, nozzleTargetC: 210 },
      operationalState: "ready", readiness: { state: "ready", reason: null }, freshness: "fresh", cacheWarnings: [], updatedAt: "2026-09-18T12:00:00Z",
    },
    ...overrides,
  };
}

const printer = makePrinter();

describe("PrinterDetailDock", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
    lifecycleEligibility.mockReset().mockResolvedValue(ACTIVE_ELIGIBILITY);
    printersMock.mockReturnValue([]);
  });

  it("uses a modal dialog in overlay mode, supports keyboard tabs, and closes on Escape", async () => {
    const onClose = vi.fn();
    render(() => <PrinterDetailDock printer={printer} mode="overlay" onClose={onClose} />);

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
    render(() => <PrinterDetailDock printer={printer} mode="inline" onClose={vi.fn()} />);

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("complementary", { name: "North Bay" })).toBeInTheDocument();
  });

  it("renders nothing for an unknown selection", () => {
    const { container } = render(() => <PrinterDetailDock mode="inline" onClose={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows Archive (not Delete) for an active Printer, calls archivePrinter, and lists the delete blocker text", async () => {
    render(() => <PrinterDetailDock printer={printer} mode="inline" onClose={vi.fn()} />);
    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));

    const archiveButton = await screen.findByRole("button", { name: "Archive" });
    await waitFor(() => expect(archiveButton).not.toBeDisabled());
    expect(screen.queryByRole("button", { name: "Delete…" })).not.toBeInTheDocument();
    expect(screen.getByText("Archive this Printer before deleting it.")).toBeInTheDocument();

    fireEvent.click(archiveButton);
    await waitFor(() => expect(archivePrinter).toHaveBeenCalledWith("prn-1"));
  });

  it("shows Unarchive and Delete… for an archived Printer, and Delete… opens DeletePrinterDialog", async () => {
    lifecycleEligibility.mockResolvedValue(ARCHIVED_ELIGIBILITY);
    const archived = makePrinter({ archivedAt: "2026-09-22T00:00:00.000Z" });
    render(() => <PrinterDetailDock printer={archived} mode="inline" onClose={vi.fn()} />);
    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));

    expect(screen.queryByRole("button", { name: "Archive" })).not.toBeInTheDocument();
    const unarchiveButton = await screen.findByRole("button", { name: "Unarchive" });
    await waitFor(() => expect(unarchiveButton).not.toBeDisabled());
    const deleteButton = screen.getByRole("button", { name: "Delete…" });

    expect(screen.queryByText("Delete dialog open for North Bay")).not.toBeInTheDocument();
    fireEvent.click(deleteButton);
    expect(screen.getByText("Delete dialog open for North Bay")).toBeInTheDocument();
  });

  it("clicking Unarchive calls unarchivePrinter", async () => {
    lifecycleEligibility.mockResolvedValue(ARCHIVED_ELIGIBILITY);
    const archived = makePrinter({ archivedAt: "2026-09-22T00:00:00.000Z" });
    render(() => <PrinterDetailDock printer={archived} mode="inline" onClose={vi.fn()} />);
    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));

    const unarchiveButton = await screen.findByRole("button", { name: "Unarchive" });
    await waitFor(() => expect(unarchiveButton).not.toBeDisabled());
    fireEvent.click(unarchiveButton);

    await waitFor(() => expect(unarchivePrinter).toHaveBeenCalledWith("prn-1"));
  });

  it("shows the conflicting Printer's name inline when unarchive fails with DUPLICATE_HOST", async () => {
    lifecycleEligibility.mockResolvedValue(ARCHIVED_ELIGIBILITY);
    printersMock.mockReturnValue([{ id: "prn-2", name: "South Bay" }]);
    unarchivePrinter.mockRejectedValueOnce({
      contractVersion: 1,
      code: "DUPLICATE_HOST",
      message: "Another Printer already uses this host.",
      recovery: [],
      retryable: false,
      details: { conflictingPrinterId: "prn-2" },
    });
    const archived = makePrinter({ archivedAt: "2026-09-22T00:00:00.000Z" });
    render(() => <PrinterDetailDock printer={archived} mode="inline" onClose={vi.fn()} />);
    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));

    const unarchiveButton = await screen.findByRole("button", { name: "Unarchive" });
    await waitFor(() => expect(unarchiveButton).not.toBeDisabled());
    fireEvent.click(unarchiveButton);

    expect(await screen.findByText(/South Bay/)).toBeInTheDocument();
  });

  it("closes the dock and notifies onDeleted after DeletePrinterDialog reports a completed delete", async () => {
    lifecycleEligibility.mockResolvedValue(ARCHIVED_ELIGIBILITY);
    const onClose = vi.fn();
    const onDeleted = vi.fn();
    const archived = makePrinter({ archivedAt: "2026-09-22T00:00:00.000Z" });
    render(() => <PrinterDetailDock printer={archived} mode="inline" onClose={onClose} onDeleted={onDeleted} />);
    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));

    fireEvent.click(await screen.findByRole("button", { name: "Delete…" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm delete (stub)" }));

    expect(onDeleted).toHaveBeenCalledWith("prn-1");
    expect(onClose).toHaveBeenCalledOnce();
  });
});
