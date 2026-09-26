import { createSignal, Show } from "solid-js";
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

vi.mock("./PrinterJobPanel", () => ({
  PrinterJobPanel: (props: { printer: { name: string } }) => <div>Job panel for {props.printer.name}</div>,
}));

vi.mock("./CapabilityList", () => ({
  CapabilityList: (props: { printerId: string }) => <div>Capabilities of {props.printerId}</div>,
}));

vi.mock("./MaterialSlotsEditor", () => ({
  MATERIAL_SLOTS_ANCHOR_ID: "printer-setup-material-slots",
  MaterialSlotsSection: () => (
    <section id="printer-setup-material-slots"><h3 tabIndex={-1}>Material Slots</h3></section>
  ),
}));

vi.mock("./ArchivePrinterDialog", () => ({
  ArchivePrinterDialog: (props: { open: boolean; loadedSpools: unknown[] }) => (
    <Show when={props.open}><p>Archive dialog for {props.loadedSpools.length} Spool(s)</p></Show>
  ),
}));

vi.mock("../spools/spool-store", () => ({
  get spoolState() {
    return { spools: [], loaded: true };
  },
  ensureInventoryLoaded: () => Promise.resolve(),
}));

const reportError = vi.hoisted(() => vi.fn());
const archivePrinter = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
const unarchivePrinter = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
const lifecycleEligibility = vi.hoisted(() => vi.fn());
const printersMock = vi.hoisted(() => vi.fn().mockReturnValue([]));
vi.mock("../printers/printer-store", () => ({
  archivePrinter,
  unarchivePrinter,
  lifecycleEligibility,
  printers: printersMock,
  reportError,
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
  loadedSpools: [],
};

const ARCHIVED_ELIGIBILITY = {
  canArchive: false,
  canUnarchive: true,
  canDelete: true,
  blockers: [
    { action: "archive" as const, code: "ALREADY_ARCHIVED" as const, message: "This Printer is already archived." },
  ],
  loadedSpools: [],
};

function makePrinter(overrides: Partial<ResolvedPrinter> = {}): ResolvedPrinter {
  return {
    id: "prn-1", revision: 1, name: "North Bay", notes: "", overrides: {},
    catalogRef: { vendor: "Bambu Lab", model: "X1 Carbon", variant: "X1 Carbon 0.4", modelId: "x1", printerVariant: "0.4" },
    catalogStatus: "ok", modelLabel: "X1 Carbon", variantLabel: "X1 Carbon 0.4", overriddenFields: [], inherited: {},
    profileDrift: [], unknownOverrideKeys: [], startSafety: "confirmBedClear", materialSlots: [{ id: "slt-main", position: 0, name: "Main" }], setupGaps: [], createdAt: "", updatedAt: "",
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
    expect(screen.getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["Status", "Setup", "Job"]);
    expect(screen.queryByRole("tab", { name: /Camera/ })).not.toBeInTheDocument();

    const status = screen.getByRole("tab", { name: "Status" });
    status.focus();
    await fireEvent.keyDown(status, { key: "ArrowRight" });
    expect(screen.getByRole("tab", { name: "Setup" })).toHaveFocus();
    expect(screen.getByText("Profile setup")).toBeInTheDocument();

    await fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("has a Job tab with the Printer's Job panel, and lists capabilities in Setup", async () => {
    render(() => <PrinterDetailDock printer={printer} mode="inline" onClose={vi.fn()} />);
    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));
    expect(screen.getByText("Capabilities of prn-1")).toBeInTheDocument();
    await fireEvent.click(screen.getByRole("tab", { name: "Job" }));
    expect(screen.getByText("Job panel for North Bay")).toBeInTheDocument();
  });

  it("opens the Job tab on an OPEN_PRINTER_JOB request for this Printer, and ignores one for another", async () => {
    const { openPrinterJob, printerJobRequest } = await import("../host-ops/open-printer-job");
    render(() => <PrinterDetailDock printer={printer} mode="inline" onClose={vi.fn()} />);
    expect(screen.getByRole("tab", { name: "Status" })).toHaveAttribute("aria-selected", "true");

    openPrinterJob("prn-other");
    await Promise.resolve();
    expect(screen.getByRole("tab", { name: "Status" })).toHaveAttribute("aria-selected", "true");
    expect(printerJobRequest()).toEqual({ printerId: "prn-other" });

    openPrinterJob("prn-1");
    await waitFor(() => expect(screen.getByRole("tab", { name: "Job" })).toHaveAttribute("aria-selected", "true"));
    expect(printerJobRequest()).toBeUndefined();
  });

  it("an unresolved Host Operation's Archive blocker links to the Job tab", async () => {
    lifecycleEligibility.mockResolvedValue({
      ...ACTIVE_ELIGIBILITY,
      canArchive: false,
      blockers: [
        ...ACTIVE_ELIGIBILITY.blockers,
        { action: "archive", code: "HOST_OPERATION_UNRESOLVED", message: "Finish or abandon the pending printer operation before archiving." },
      ],
    });
    render(() => <PrinterDetailDock printer={printer} mode="inline" onClose={vi.fn()} />);
    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));
    expect(await screen.findByText("Finish or abandon the pending printer operation before archiving.")).toBeInTheDocument();
    await fireEvent.click(screen.getByRole("button", { name: "Open the Job tab" }));
    await waitFor(() => expect(screen.getByRole("tab", { name: "Job" })).toHaveAttribute("aria-selected", "true"));
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
    // Unarchive isn't offered for an active Printer, so its blocker is noise.
    expect(screen.queryByText("This Printer is not archived.")).not.toBeInTheDocument();

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
    expect(screen.queryByText("This Printer is already archived.")).not.toBeInTheDocument();

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

  it("shows an inline error with Retry when lifecycleEligibility fails, and recovers on Retry", async () => {
    lifecycleEligibility.mockReset()
      .mockRejectedValueOnce({
        contractVersion: 1,
        code: "PERSISTENCE_UNAVAILABLE",
        message: "printers.json is read-only",
        recovery: ["RETRY"],
        retryable: true,
      })
      .mockResolvedValueOnce(ACTIVE_ELIGIBILITY);
    render(() => <PrinterDetailDock printer={printer} mode="inline" onClose={vi.fn()} />);
    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));

    expect(await screen.findByText(/Couldn't check what you can do with this Printer\./)).toBeInTheDocument();
    expect(screen.getByText(/printers\.json is read-only/)).toBeInTheDocument();
    const archiveButton = screen.getByRole("button", { name: "Archive" });
    expect(archiveButton).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "Retry" }));

    await waitFor(() => expect(screen.getByRole("button", { name: "Archive" })).not.toBeDisabled());
    expect(
      screen.queryByText(/Couldn't check what you can do with this Printer\./),
    ).not.toBeInTheDocument();
    expect(lifecycleEligibility).toHaveBeenCalledTimes(2);
  });

  it("clears a stale eligibility error when the selected Printer changes", async () => {
    lifecycleEligibility.mockReset().mockRejectedValueOnce({
      contractVersion: 1,
      code: "PERSISTENCE_UNAVAILABLE",
      message: "printers.json is read-only",
      recovery: ["RETRY"],
      retryable: true,
    }).mockResolvedValue(ACTIVE_ELIGIBILITY);
    const [selected, setSelected] = createSignal(printer);
    render(() => <Show when={selected()}>{(current) => <PrinterDetailDock printer={current()} mode="inline" onClose={vi.fn()} />}</Show>);
    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));
    expect(await screen.findByText(/Couldn't check what you can do with this Printer\./)).toBeInTheDocument();

    setSelected(makePrinter({ id: "prn-2" }));

    await waitFor(() => expect(
      screen.queryByText(/Couldn't check what you can do with this Printer\./),
    ).not.toBeInTheDocument());
  });

  it("guards Archive against double-fire while pending, then reflects the post-archive eligibility once the Printer record updates (Archive → Unarchive/Delete…)", async () => {
    lifecycleEligibility.mockReset()
      .mockResolvedValueOnce(ACTIVE_ELIGIBILITY) // on open
      .mockResolvedValueOnce(ACTIVE_ELIGIBILITY) // Archive re-checks before acting
      .mockResolvedValueOnce(ARCHIVED_ELIGIBILITY);
    let resolveArchive: (() => void) | undefined;
    archivePrinter.mockImplementation(() => new Promise<void>((resolve) => { resolveArchive = resolve; }));

    const [current, setCurrent] = createSignal(printer);
    render(() => <PrinterDetailDock printer={current()} mode="inline" onClose={vi.fn()} />);
    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));

    const archiveButton = await screen.findByRole("button", { name: "Archive" });
    await waitFor(() => expect(archiveButton).not.toBeDisabled());

    fireEvent.click(archiveButton);
    await waitFor(() => expect(archiveButton).toBeDisabled());
    fireEvent.click(archiveButton); // disabled, and onArchive's own in-flight guard: must not fire again

    expect(archivePrinter).toHaveBeenCalledTimes(1);

    // Mirrors what the real store push does once archivePrinter succeeds --
    // PrinterDashboard re-renders this dock from the freshly archived
    // record. The (id, archivedAt)-keyed effect (not an explicit call from
    // onArchive, per Fix round 2) is what refetches eligibility here.
    setCurrent({ ...printer, archivedAt: "2026-09-22T00:00:00.000Z" });
    resolveArchive?.();

    await waitFor(() => expect(screen.queryByRole("button", { name: "Archive" })).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByRole("button", { name: "Unarchive" })).not.toBeDisabled());
    expect(screen.getByRole("button", { name: "Delete…" })).toBeInTheDocument();
  });

  it("refetches eligibility when the same Printer's archivedAt changes through a non-dock push (e.g. importPrinters()), and the buttons reflect it", async () => {
    lifecycleEligibility.mockReset()
      .mockResolvedValueOnce(ACTIVE_ELIGIBILITY)
      .mockResolvedValueOnce(ARCHIVED_ELIGIBILITY);
    const [current, setCurrent] = createSignal(printer);
    render(() => <PrinterDetailDock printer={current()} mode="inline" onClose={vi.fn()} />);
    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));

    const archiveButton = await screen.findByRole("button", { name: "Archive" });
    await waitFor(() => expect(archiveButton).not.toBeDisabled());
    expect(lifecycleEligibility).toHaveBeenCalledTimes(1);

    // Not a dock action -- archivePrinter/unarchivePrinter are never
    // called. Simulates e.g. importPrinters() replacing the whole Printer
    // list while this dock is open.
    setCurrent({ ...printer, archivedAt: "2026-09-22T00:00:00.000Z" });

    await waitFor(() => expect(lifecycleEligibility).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Archive" })).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByRole("button", { name: "Unarchive" })).not.toBeDisabled());
    expect(screen.getByRole("button", { name: "Delete…" })).toBeInTheDocument();
    expect(archivePrinter).not.toHaveBeenCalled();
    expect(unarchivePrinter).not.toHaveBeenCalled();
  });

  it("does not refetch eligibility for a same-id push that leaves archivedAt unchanged", async () => {
    lifecycleEligibility.mockReset().mockResolvedValue(ACTIVE_ELIGIBILITY);
    const [current, setCurrent] = createSignal(printer);
    render(() => <PrinterDetailDock printer={current()} mode="inline" onClose={vi.fn()} />);
    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));

    await screen.findByRole("button", { name: "Archive" });
    await waitFor(() => expect(lifecycleEligibility).toHaveBeenCalledTimes(1));

    // A new object reference for the *same* Printer with the *same*
    // archivedAt (e.g. a rename pushed from elsewhere) -- must not refetch.
    setCurrent({ ...printer, name: "North Bay (renamed)" });
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(lifecycleEligibility).toHaveBeenCalledTimes(1);
  });
  it("shows the Material Slots section on the Setup tab", async () => {
    render(() => <PrinterDetailDock printer={printer} mode="inline" onClose={vi.fn()} />);
    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));
    expect(screen.getByRole("heading", { name: "Material Slots" })).toBeInTheDocument();
  });

  it("opens the Setup tab at Material Slots when asked to (batch Equip)", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout"] });
    try {
      render(() => (
        <PrinterDetailDock printer={printer} mode="inline" onClose={vi.fn()} focusRequest={{ printerId: "prn-1", section: "materialSlots" }} />
      ));
      expect(screen.getByRole("tab", { name: "Setup" })).toHaveAttribute("aria-selected", "true");
      vi.runAllTimers();
      expect(document.activeElement).toBe(screen.getAllByRole("heading", { name: "Material Slots" }).find((h) => h.tagName === "H3" && h.closest("#printer-setup-material-slots")));
    } finally {
      vi.useRealTimers();
    }
  });

  it("with loaded Spools, Archive re-checks eligibility and opens the disposition dialog instead of archiving", async () => {
    const loaded = { id: "spl-7", spoolNumber: 7 };
    lifecycleEligibility.mockReset().mockResolvedValue({
      ...ACTIVE_ELIGIBILITY,
      canArchive: false,
      blockers: [...ACTIVE_ELIGIBILITY.blockers, { action: "archive", code: "SPOOLS_LOADED", message: "Unload every Spool before archiving this Printer." }],
      loadedSpools: [loaded],
    });
    render(() => <PrinterDetailDock printer={printer} mode="inline" onClose={vi.fn()} />);
    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));

    const archiveButton = await screen.findByRole("button", { name: "Archive" });
    await waitFor(() => expect(archiveButton).not.toBeDisabled());
    fireEvent.click(archiveButton);

    expect(await screen.findByText("Archive dialog for 1 Spool(s)")).toBeInTheDocument();
    expect(lifecycleEligibility).toHaveBeenCalledTimes(2);
    expect(archivePrinter).not.toHaveBeenCalled();
  });

  it("keeps Archive disabled when a blocker other than loaded Spools applies", async () => {
    lifecycleEligibility.mockReset().mockResolvedValue({
      ...ACTIVE_ELIGIBILITY,
      canArchive: false,
      blockers: [
        { action: "archive", code: "SPOOLS_LOADED", message: "Unload every Spool before archiving this Printer." },
        { action: "archive", code: "SPOOL_RESERVED", message: "A loaded Spool is reserved for a Job." },
      ],
      loadedSpools: [{ id: "spl-7", spoolNumber: 7 }],
    });
    render(() => <PrinterDetailDock printer={printer} mode="inline" onClose={vi.fn()} />);
    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));

    expect(await screen.findByText("A loaded Spool is reserved for a Job.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Archive" })).toBeDisabled();
  });

  it("explains a pending printer operation and keeps Archive disabled, even with loaded Spools (P6 D7)", async () => {
    lifecycleEligibility.mockReset().mockResolvedValue({
      ...ACTIVE_ELIGIBILITY,
      canArchive: false,
      blockers: [
        ...ACTIVE_ELIGIBILITY.blockers,
        { action: "archive", code: "SPOOLS_LOADED", message: "Unload every Spool before archiving this Printer." },
        { action: "archive", code: "HOST_OPERATION_UNRESOLVED", message: "Finish or abandon the pending printer operation before archiving." },
        { action: "delete", code: "HOST_OPERATION_UNRESOLVED", message: "Finish or abandon the pending printer operation before deleting." },
      ],
      loadedSpools: [{ id: "spl-7", spoolNumber: 7 }],
    });
    render(() => <PrinterDetailDock printer={printer} mode="inline" onClose={vi.fn()} />);
    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));

    expect(await screen.findByText("Finish or abandon the pending printer operation before archiving.")).toBeInTheDocument();
    expect(screen.getByText("Finish or abandon the pending printer operation before deleting.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Archive" })).toBeDisabled();
  });

  it("routes a failed plain archive to the store banner", async () => {
    const failure = { contractVersion: 1, code: "PERSISTENCE_UNAVAILABLE", message: "read-only", recovery: [], retryable: true };
    archivePrinter.mockRejectedValueOnce(failure);
    render(() => <PrinterDetailDock printer={printer} mode="inline" onClose={vi.fn()} />);
    await fireEvent.click(screen.getByRole("tab", { name: "Setup" }));
    const archiveButton = await screen.findByRole("button", { name: "Archive" });
    await waitFor(() => expect(archiveButton).not.toBeDisabled());

    fireEvent.click(archiveButton);

    await waitFor(() => expect(reportError).toHaveBeenCalledWith(failure));
  });
});
