import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { createSignal, type JSX } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const appState = vi.hoisted(() => ({
  printers: [] as Record<string, unknown>[],
  loadSettings: vi.fn(),
  updateSettings: vi.fn(),
  loadPrinters: vi.fn(),
  startStatusListener: vi.fn(),
  removePrinter: vi.fn(),
  importPrinters: vi.fn(),
  exportPrinters: vi.fn(),
  loadDuplicateHostArchives: vi.fn(),
  dismissPrinterArchiveNotice: vi.fn(),
}));

const [syncState, setSyncState] = createSignal("syncing");
const [archiveNotice, setArchiveNotice] = createSignal<string | null>(null);
const [storeCommandError, setStoreCommandError] = createSignal<Record<string, unknown> | null>(null);

vi.mock("./settings/settings-store", () => ({
  loadSettings: appState.loadSettings,
  updateSettings: appState.updateSettings,
}));

vi.mock("./printers/printer-store", () => ({
  dismissPrinterArchiveNotice: appState.dismissPrinterArchiveNotice,
  dismissPrinterStoreError: vi.fn(),
  loadDuplicateHostArchives: appState.loadDuplicateHostArchives,
  printerArchiveNotice: archiveNotice,
  exportPrinters: appState.exportPrinters,
  importPrinters: appState.importPrinters,
  loadPrinters: appState.loadPrinters,
  printers: () => appState.printers,
  printerStoreError: () => (storeCommandError()?.message as string | undefined) ?? null,
  printerStoreCommandError: storeCommandError,
  printerStoreRetryable: () => false,
  printerStoreStatus: () => "ready",
  removePrinter: appState.removePrinter,
  startStatusListener: appState.startStatusListener,
  printerStatusSyncState: syncState,
}));

vi.mock("./printers/printer-catalog", () => ({
  listCatalogModels: vi.fn().mockResolvedValue([]),
  listCatalogVariants: vi.fn().mockResolvedValue([]),
  previewProfile: vi.fn().mockResolvedValue(null),
}));

vi.mock("./design-system", () => ({
  Button: (props: { children: JSX.Element; onClick?: () => void }) => <button onClick={props.onClick}>{props.children}</button>,
}));

vi.mock("./screens/AppShell", () => ({
  AppShell: (props: {
    title: string;
    printerRoster: { count: number };
    lowSpoolCount?: number;
    children: JSX.Element;
  }) => (
    <div>
      <h1>{props.title}</h1>
      <output aria-label="Printer count">{props.printerRoster.count}</output>
      <output aria-label="Low Spools">{props.lowSpoolCount}</output>
      {props.children}
    </div>
  ),
}));

vi.mock("./screens/LibraryWorkspace", () => ({
  LibraryWorkspace: (props: {
    onImport?: () => void;
    onImportClose?: () => void;
    importSelection?: { selectionId: string } | null;
    dropActive?: boolean;
    dropRefused?: boolean;
  }) => (
    <div>
      <output aria-label="Drop refused">{String(props.dropRefused ?? false)}</output>
      <p>Library workspace</p>
      <output aria-label="Import selection">{props.importSelection?.selectionId ?? "none"}</output>
      <output aria-label="Drop active">{String(props.dropActive ?? false)}</output>
      <button onClick={() => props.onImport?.()}>Import…</button>
      <button onClick={() => props.onImportClose?.()}>Close import</button>
    </div>
  ),
}));

const desktop = vi.hoisted(() => ({ available: true }));
vi.mock("./ipc/client", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./ipc/client")>()),
  desktopAvailable: () => desktop.available,
}));

type DragPayload = { type: "enter" | "over" | "drop" | "leave" };
const webview = vi.hoisted(() => ({
  handler: undefined as undefined | ((event: { payload: DragPayload }) => void),
  unlisten: vi.fn(),
  onDragDropEvent: vi.fn(),
}));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: webview.onDragDropEvent }),
}));

/** `vi.resetModules` gives each test a fresh navigation store, so read the
 *  one this test's App imported. */
async function importAppAndNavigation() {
  const { default: App } = await import("./App");
  const { navigation } = await import("./navigation/navigation-store");
  return { App, navigation };
}

type Summary = { selectionId: string; purpose: "import"; files: { fileIndex: number; fileName: string; sizeBytes: number | null }[] };
const libraryStore = vi.hoisted(() => ({
  startLibrary: vi.fn(),
  dispose: vi.fn(),
  pickFiles: vi.fn(),
  cancelSelection: vi.fn(),
  reportLibraryError: vi.fn(),
  dropped: undefined as undefined | ((summary: Summary) => void),
  stopDropped: vi.fn(),
  onSelectionDropped: vi.fn(),
  load: undefined as undefined | (() => void),
  reset: undefined as undefined | (() => void),
}));
vi.mock("./library/library-store", async () => {
  const { createStore } = await import("solid-js/store");
  const [state, setState] = createStore({
    projects: [] as { id: string }[],
    models: [] as { id: string }[],
    status: "idle" as "idle" | "loading" | "ready",
  });
  libraryStore.reset = () => setState({ projects: [], models: [], status: "idle" });
  libraryStore.load = () => setState({ projects: [{ id: "prj-brackets" }], models: [{ id: "mdl-bracket" }], status: "ready" });
  return {
    library: { projects: () => state.projects, models: () => state.models, status: () => state.status },
    startLibrary: libraryStore.startLibrary,
    pickFiles: libraryStore.pickFiles,
    cancelSelection: libraryStore.cancelSelection,
    reportLibraryError: libraryStore.reportLibraryError,
    onSelectionDropped: libraryStore.onSelectionDropped,
  };
});

vi.mock("./slicing/slicing-store", async () => (await import("./slicing/slicing-store-mock")).slicingStoreMock);
vi.mock("./host-ops/host-operations-store", async () =>
  (await import("./host-ops/host-operations-store-mock")).hostOperationsStoreMock);
vi.mock("./host-ops/capabilities-store", async () =>
  (await import("./host-ops/capabilities-store-mock")).capabilitiesStoreMock);

vi.mock("./screens/SlicerSettingsDialog", () => ({
  SlicerSettingsDialog: (props: { open: boolean; onOpenChange: (open: boolean) => void }) => (
    <div role="dialog" aria-label="Slicer">
      <button onClick={() => props.onOpenChange(false)}>Close Slicer</button>
    </div>
  ),
}));

vi.mock("./screens/SpoolInventory", () => ({
  SpoolInventory: () => <div>Spools</div>,
}));

const inventory = vi.hoisted(() => ({
  onLoad: undefined as undefined | (() => void),
  reset: undefined as undefined | (() => void),
  ensureInventoryLoaded: vi.fn(),
}));
vi.mock("./spools/spool-store", async () => {
  const { createStore } = await import("solid-js/store");
  const [spoolState, setSpoolState] = createStore({ spools: [] as { id: string; facets: { low: boolean } }[] });
  inventory.reset = () => setSpoolState("spools", []);
  inventory.onLoad = () => setSpoolState("spools", [
    { id: "spl-low", facets: { low: true } },
    { id: "spl-ok", facets: { low: false } },
  ]);
  return { spoolState, ensureInventoryLoaded: inventory.ensureInventoryLoaded };
});

vi.mock("./screens/PrinterDashboard", () => ({
  PrinterDashboard: (props: {
    store: { hasPrinters: () => boolean; selectedPrinterId: () => string | null };
    isFirstRun?: boolean;
    syncState?: string;
    onImport?: () => void;
    onExport?: () => void;
  }) => (
    <div>
      <p>{props.store.hasPrinters() ? "Persisted Printers are visible" : props.isFirstRun ? "First run" : "Returning empty Farm"}</p>
      <p>Sync state: {props.syncState}</p>
      <output aria-label="Selected Printer">{props.store.selectedPrinterId() ?? "none"}</output>
      <button onClick={props.onImport}>Import Printers</button>
      <button onClick={props.onExport}>Export Printers</button>
    </div>
  ),
}));

const PRINTER = {
  id: "prn-1",
  revision: 1,
  name: "North Bay",
  notes: "",
  overrides: {},
  catalogRef: {
    vendor: "Bambu Lab", model: "X1 Carbon", variant: "X1 Carbon 0.4", modelId: "x1", printerVariant: "0.4",
  },
  catalogStatus: "ok",
  modelLabel: "X1 Carbon",
  variantLabel: "X1 Carbon 0.4",
  profile: {
    bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
    printableHeightMm: 256,
    bedExcludeAreas: [],
    defaultBedType: "",
    nozzleDiameterMm: [0.4],
    nozzleType: "brass",
    gcodeFlavor: "klipper",
    hasAuxiliaryFan: false,
    supportsAirFiltration: false,
    supportsMultiFilament: false,
    suggestedHostType: null,
  },
  overriddenFields: [],
  inherited: {},
  profileDrift: [],
  unknownOverrideKeys: [],
  createdAt: "",
  updatedAt: "",
};

const SETTINGS = {
  revision: 1,
  themeMode: "system" as const,
  monitorSection: "printerModel" as const,
  monitorDensity: "comfortable" as const,
  updatedAt: "",
};

beforeEach(async () => {
  vi.resetModules();
  // The mocked slicing store outlives `vi.resetModules`; start each test
  // with its default spies.
  vi.mocked(await import("./slicing/slicing-store")).startSlicing.mockReset();
  setStoreCommandError(null);
  vi.mocked(await import("./host-ops/host-operations-store")).startHostOperations.mockReset().mockResolvedValue(() => {});
  vi.mocked(await import("./host-ops/capabilities-store")).syncCapabilities.mockReset().mockReturnValue(() => {});
  window.localStorage.clear();
  appState.printers = [];
  appState.loadSettings.mockReset().mockResolvedValue(SETTINGS);
  appState.updateSettings.mockReset().mockResolvedValue(undefined);
  appState.loadPrinters.mockReset().mockImplementation(async () => {
    appState.printers = [PRINTER];
  });
  appState.startStatusListener.mockReset().mockResolvedValue(() => {});
  appState.removePrinter.mockReset();
  appState.importPrinters.mockReset().mockResolvedValue({ status: "applied" });
  appState.exportPrinters.mockReset().mockResolvedValue({ status: "exported" });
  appState.loadDuplicateHostArchives.mockReset().mockResolvedValue(undefined);
  appState.dismissPrinterArchiveNotice.mockReset().mockImplementation(() => setArchiveNotice(null));
  setArchiveNotice(null);
  setSyncState("syncing");
  window.location.hash = "";
  inventory.reset?.();
  inventory.ensureInventoryLoaded.mockReset().mockImplementation(async () => inventory.onLoad?.());
  libraryStore.reset?.();
  libraryStore.dispose.mockReset();
  libraryStore.startLibrary.mockReset().mockImplementation(async () => {
    libraryStore.load?.();
    return libraryStore.dispose;
  });
  libraryStore.pickFiles.mockReset().mockResolvedValue(null);
  libraryStore.cancelSelection.mockReset().mockResolvedValue(undefined);
  libraryStore.reportLibraryError.mockReset();
  libraryStore.stopDropped.mockReset();
  libraryStore.dropped = undefined;
  libraryStore.onSelectionDropped.mockReset().mockImplementation((handler: (summary: Summary) => void) => {
    libraryStore.dropped = handler;
    return libraryStore.stopDropped;
  });
  desktop.available = true;
  webview.handler = undefined;
  webview.unlisten.mockReset();
  webview.onDragDropEvent.mockReset().mockImplementation(async (handler: (event: { payload: DragPayload }) => void) => {
    webview.handler = handler;
    return webview.unlisten;
  });
});

function summary(selectionId: string): Summary {
  return { selectionId, purpose: "import", files: [{ fileIndex: 0, fileName: "cube.stl", sizeBytes: 684 }] };
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("App", () => {
  it("mounts the Slicer settings once, at app level, whenever the opener asks, on any screen", async () => {
    const { App } = await importAppAndNavigation();
    const opener = await import("./slicing/slicer-settings-opener");
    render(() => <App />);
    expect(screen.queryByRole("dialog", { name: "Slicer" })).toBeNull();

    opener.openSlicerSettings();
    expect(await screen.findByRole("dialog", { name: "Slicer" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Close Slicer" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Slicer" })).toBeNull());
    expect(opener.slicerSettingsOpen()).toBe(false);
  });

  it("loads settings and durable Printers before listener/backfill startup, retaining Printer content while status syncs", async () => {
    const callOrder: string[] = [];
    appState.loadSettings.mockImplementation(async () => {
      callOrder.push("loadSettings");
      return SETTINGS;
    });
    appState.loadPrinters.mockImplementation(async () => {
      callOrder.push("loadPrinters");
      appState.printers = [PRINTER];
    });
    appState.startStatusListener.mockImplementation(async () => {
      callOrder.push("listen");
      callOrder.push("backfill");
      return () => {};
    });
    const { default: App } = await import("./App");

    render(() => <App />);

    await waitFor(() => expect(screen.getByText("Persisted Printers are visible")).toBeInTheDocument());
    expect(callOrder).toEqual(["loadSettings", "loadPrinters", "listen", "backfill"]);
    expect(screen.getByText("Sync state: syncing")).toBeInTheDocument();
  });

  it("shows first-run only on an unvisited empty Farm and distinguishes a returning empty Farm", async () => {
    appState.loadPrinters.mockResolvedValue(undefined);
    const { default: App } = await import("./App");

    const firstVisit = render(() => <App />);
    expect(await screen.findByText("First run")).toBeInTheDocument();
    firstVisit.unmount();

    render(() => <App />);
    expect(await screen.findByText("Returning empty Farm")).toBeInTheDocument();
  });

  it("selects a valid Printer deep link after durable Printers load", async () => {
    window.location.hash = "#nav=v1/monitor/printer/prn-1";
    const { default: App } = await import("./App");

    render(() => <App />);

    expect(await screen.findByRole("status", { name: "Selected Printer" })).toHaveTextContent("prn-1");
  });

  it("keeps Monitor open and closes detail for an unknown deep-linked Printer", async () => {
    window.location.hash = "#nav=v1/monitor/printer/missing";
    const { default: App } = await import("./App");

    render(() => <App />);

    await waitFor(() => expect(screen.getAllByText("The requested item is no longer available.").length).toBeGreaterThan(0));
    await waitFor(() => expect(screen.getByLabelText("Selected Printer")).toHaveTextContent("none"));
    expect(screen.getByText("Monitor")).toBeInTheDocument();
  });

  it("loads the Spool inventory at startup, so the low-Spool badge counts without visiting Spools", async () => {
    const { default: App } = await import("./App");
    render(() => <App />);

    await waitFor(() => expect(screen.getByLabelText("Low Spools")).toHaveTextContent("1"));
    expect(inventory.ensureInventoryLoaded).toHaveBeenCalledTimes(1);
  });

  it("resolves a cold-launch Spool deep link once the inventory has loaded", async () => {
    window.location.hash = "#nav=v1/spools/spool/spl-low";
    const { default: App } = await import("./App");
    render(() => <App />);

    await waitFor(() => expect(inventory.ensureInventoryLoaded).toHaveBeenCalled());
    await waitFor(() => expect(screen.queryAllByText("The requested item is no longer available.")).toHaveLength(0));
    expect(screen.getByRole("heading", { name: "Spools" })).toBeInTheDocument();
  });

  it("starts the Library after Printers load and disposes it on unmount", async () => {
    const callOrder: string[] = [];
    appState.loadPrinters.mockImplementation(async () => {
      callOrder.push("loadPrinters");
      appState.printers = [PRINTER];
    });
    libraryStore.startLibrary.mockImplementation(async () => {
      callOrder.push("startLibrary");
      return libraryStore.dispose;
    });
    const { default: App } = await import("./App");
    const { unmount } = render(() => <App />);

    await waitFor(() => expect(libraryStore.startLibrary).toHaveBeenCalledOnce());
    expect(callOrder).toEqual(["loadPrinters", "startLibrary"]);
    unmount();
    expect(libraryStore.dispose).toHaveBeenCalledOnce();
  });

  it("starts slicing right after the Library and disposes it on unmount", async () => {
    const callOrder: string[] = [];
    libraryStore.startLibrary.mockImplementation(async () => {
      callOrder.push("startLibrary");
      return libraryStore.dispose;
    });
    // The mocked store, as App sees it (mocks outlive `vi.resetModules`).
    const slicingStoreMock = vi.mocked(await import("./slicing/slicing-store"));
    const disposeSlicing = vi.fn();
    slicingStoreMock.startSlicing.mockImplementation(async () => {
      callOrder.push("startSlicing");
      return disposeSlicing;
    });
    const { default: App } = await import("./App");
    const { unmount } = render(() => <App />);

    await waitFor(() => expect(slicingStoreMock.startSlicing).toHaveBeenCalledOnce());
    expect(callOrder).toEqual(["startLibrary", "startSlicing"]);
    // Let the start settle, so unmount has its disposer.
    await Promise.resolve();
    unmount();
    expect(disposeSlicing).toHaveBeenCalledOnce();
  });

  it("starts Host Operations after slicing, keeps capabilities synced with the Printers, and disposes both on unmount", async () => {
    const callOrder: string[] = [];
    const slicingStoreMock = vi.mocked(await import("./slicing/slicing-store"));
    slicingStoreMock.startSlicing.mockImplementation(async () => {
      callOrder.push("startSlicing");
      return () => {};
    });
    const hostOps = vi.mocked(await import("./host-ops/host-operations-store"));
    const capabilitiesStore = vi.mocked(await import("./host-ops/capabilities-store"));
    const disposeHostOps = vi.fn();
    const stopSync = vi.fn();
    hostOps.startHostOperations.mockImplementation(async () => {
      callOrder.push("startHostOperations");
      return disposeHostOps;
    });
    capabilitiesStore.syncCapabilities.mockImplementation(() => stopSync);
    const { default: App } = await import("./App");
    const { unmount } = render(() => <App />);

    await waitFor(() => expect(hostOps.startHostOperations).toHaveBeenCalledOnce());
    expect(callOrder).toEqual(["startSlicing", "startHostOperations"]);
    expect(capabilitiesStore.syncCapabilities).toHaveBeenCalledOnce();
    const list = capabilitiesStore.syncCapabilities.mock.calls[0][0];
    expect(list().map((printer) => printer.id)).toEqual([PRINTER.id]);
    await Promise.resolve();
    unmount();
    expect(disposeHostOps).toHaveBeenCalledOnce();
    expect(stopSync).toHaveBeenCalledOnce();
  });

  it("shows an import's HOST_OPERATION_PENDING in the banner with a link to each named Printer's Job tab", async () => {
    appState.loadPrinters.mockImplementation(async () => {
      appState.printers = [PRINTER, { ...PRINTER, id: "prn-2", name: "Second bay" }];
    });
    const { default: App } = await import("./App");
    const { printerJobRequest } = await import("./host-ops/open-printer-job");
    render(() => <App />);
    // The mocked Printer list isn't reactive: raise the error once it has loaded.
    await waitFor(() => expect(appState.printers).toHaveLength(2));
    setStoreCommandError({
      contractVersion: 1, code: "HOST_OPERATION_PENDING", recovery: ["OPEN_PRINTER_JOB"], retryable: false,
      message: "This printer has a pending operation. Finish or abandon it first.",
      details: { printerIds: [PRINTER.id, "prn-2"], hostOperationIds: ["hop-1", "hop-2"] },
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("This printer has a pending operation.");
    fireEvent.click(await screen.findByRole("button", { name: "Open Second bay's Job tab" }));
    expect(printerJobRequest()).toEqual({ printerId: "prn-2" });
    expect(screen.getByRole("button", { name: `Open ${PRINTER.name as string}'s Job tab` })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Dismiss" })).toBeInTheDocument();
  });

  it("disposes a slicing start that finishes after unmount", async () => {
    // The mocked store, as App sees it (mocks outlive `vi.resetModules`).
    const slicingStoreMock = vi.mocked(await import("./slicing/slicing-store"));
    const disposeSlicing = vi.fn();
    let finish: (() => void) | undefined;
    slicingStoreMock.startSlicing.mockImplementation(() => new Promise((resolve) => {
      finish = () => resolve(disposeSlicing);
    }));
    const { default: App } = await import("./App");
    const { unmount } = render(() => <App />);

    await waitFor(() => expect(slicingStoreMock.startSlicing).toHaveBeenCalled());
    unmount();
    finish?.();
    await waitFor(() => expect(disposeSlicing).toHaveBeenCalledOnce());
  });

  it("lists the Library as an available destination", async () => {
    window.location.hash = "#nav=v1/library";
    const { default: App } = await import("./App");
    render(() => <App />);

    expect(await screen.findByRole("heading", { name: "Library" })).toBeInTheDocument();
    await waitFor(() => expect(libraryStore.startLibrary).toHaveBeenCalled());
    expect(screen.queryByText("Library is not available in this version.")).toBeNull();
  });

  it("resolves a cold-launch Model deep link once the Library has loaded", async () => {
    window.location.hash = "#nav=v1/library/model/mdl-bracket";
    let finishLoad: (() => void) | undefined;
    libraryStore.startLibrary.mockImplementation(() => new Promise((resolve) => {
      finishLoad = () => {
        libraryStore.load?.();
        resolve(libraryStore.dispose);
      };
    }));
    const { App, navigation } = await importAppAndNavigation();
    // Let the hash assignment's own hashchange land before mounting, so
    // only the post-load reconcile can resolve the selection.
    await new Promise((resolve) => setTimeout(resolve, 0));
    render(() => <App />);

    await waitFor(() => expect(libraryStore.startLibrary).toHaveBeenCalled());
    // Pending, not unavailable, while the Library loads: no banner.
    expect(screen.queryAllByText("The requested item is no longer available.")).toHaveLength(0);
    finishLoad?.();
    await waitFor(() => expect(navigation.availability()).toBe("available"));
    expect(navigation.target().selection).toEqual({ kind: "model", id: "mdl-bracket" });
    expect(screen.queryAllByText("The requested item is no longer available.")).toHaveLength(0);
    expect(screen.getByText("Library workspace")).toBeInTheDocument();
  });

  it("resolves a cold-launch Project deep link once the Library has loaded", async () => {
    window.location.hash = "#nav=v1/library/project/prj-brackets";
    const { App, navigation } = await importAppAndNavigation();
    render(() => <App />);

    await waitFor(() => expect(libraryStore.startLibrary).toHaveBeenCalled());
    await waitFor(() => expect(navigation.availability()).toBe("available"));
    expect(navigation.target().selection).toEqual({ kind: "project", id: "prj-brackets" });
  });

  it("shows the no-longer-available banner for an unknown Model id, once the Library has loaded", async () => {
    window.location.hash = "#nav=v1/library/model/mdl-gone";
    let finishLoad: (() => void) | undefined;
    libraryStore.startLibrary.mockImplementation(() => new Promise((resolve) => {
      finishLoad = () => {
        libraryStore.load?.();
        resolve(libraryStore.dispose);
      };
    }));
    const { App, navigation } = await importAppAndNavigation();
    await new Promise((resolve) => setTimeout(resolve, 0));
    render(() => <App />);

    await waitFor(() => expect(libraryStore.startLibrary).toHaveBeenCalled());
    expect(screen.queryAllByText("The requested item is no longer available.")).toHaveLength(0);
    finishLoad?.();
    await waitFor(() => expect(screen.getAllByText("The requested item is no longer available.").length).toBeGreaterThan(0));
    expect(navigation.availability()).toBe("selectionUnavailable");
  });

  it("shows listener startup failure as a recoverable banner", async () => {
    appState.startStatusListener.mockRejectedValue(new Error("listener failed"));
    const { default: App } = await import("./App");

    render(() => <App />);

    expect(await screen.findByRole("alert")).toHaveTextContent("Live Printer status could not be started.");
    expect(screen.getByRole("button", { name: "Retry startup" })).toBeInTheDocument();
  });

  it("reads duplicate-host archives after Printers load and shows a dismissible notice", async () => {
    appState.loadDuplicateHostArchives.mockImplementation(async () => {
      expect(appState.loadPrinters).toHaveBeenCalled();
      setArchiveNotice("Archived during the upgrade because it shares a host with another Printer: Voron B.");
    });
    const { default: App } = await import("./App");

    render(() => <App />);

    const notice = await screen.findByText(/shares a host with another Printer: Voron B\./);
    expect(notice.closest("[role=status]")).not.toBeNull();
    screen.getByRole("button", { name: "Dismiss" }).click();
    expect(appState.dismissPrinterArchiveNotice).toHaveBeenCalledOnce();
    await waitFor(() => expect(screen.queryByText(/shares a host/)).not.toBeInTheDocument());
  });

  it("updates Monitor sync state after listener reconciliation", async () => {
    const { default: App } = await import("./App");

    render(() => <App />);

    await screen.findByText("Sync state: syncing");
    setSyncState("current");
    await waitFor(() => expect(screen.getByText("Sync state: current")).toBeInTheDocument());
    setSyncState("uncertain");
    await waitFor(() => expect(screen.getByText("Sync state: uncertain")).toBeInTheDocument());
  });

  it("passes import/export callbacks and clears an imported-away deep-link selection", async () => {
    window.location.hash = "#nav=v1/monitor/printer/prn-1";
    appState.importPrinters.mockImplementation(async () => {
      appState.printers = [];
      return { status: "applied" };
    });
    const { default: App } = await import("./App");

    render(() => <App />);

    await waitFor(() => expect(screen.getByLabelText("Selected Printer")).toHaveTextContent("prn-1"));
    await screen.getByRole("button", { name: "Import Printers" }).click();
    await waitFor(() => expect(appState.importPrinters).toHaveBeenCalledOnce());
    await waitFor(() => expect(screen.getByLabelText("Selected Printer")).toHaveTextContent("none"));
    await screen.getByRole("button", { name: "Export Printers" }).click();
    expect(appState.exportPrinters).toHaveBeenCalledOnce();
  });

  it("retries the full startup sequence after a settings failure", async () => {
    const callOrder: string[] = [];
    appState.loadSettings
      .mockRejectedValueOnce(new Error("settings failed"))
      .mockImplementation(async () => {
        callOrder.push("loadSettings");
        return SETTINGS;
      });
    appState.loadPrinters.mockImplementation(async () => {
      callOrder.push("loadPrinters");
      appState.printers = [PRINTER];
    });
    appState.startStatusListener.mockImplementation(async () => {
      callOrder.push("listen");
      return () => {};
    });
    const { default: App } = await import("./App");

    render(() => <App />);

    await screen.findByRole("alert");
    await screen.getByRole("button", { name: "Retry startup" }).click();
    await waitFor(() => expect(callOrder).toEqual(["loadSettings", "loadPrinters", "listen"]));
  });

  it("retries the full startup sequence after a Printer-load failure", async () => {
    const callOrder: string[] = [];
    appState.loadSettings.mockImplementation(async () => {
      callOrder.push("loadSettings");
      return SETTINGS;
    });
    appState.loadPrinters
      .mockRejectedValueOnce(new Error("Printers failed"))
      .mockImplementation(async () => {
        callOrder.push("loadPrinters");
        appState.printers = [PRINTER];
      });
    appState.startStatusListener.mockImplementation(async () => {
      callOrder.push("listen");
      return () => {};
    });
    const { default: App } = await import("./App");

    render(() => <App />);

    await screen.findByRole("alert");
    await screen.getByRole("button", { name: "Retry startup" }).click();
    await waitFor(() => expect(callOrder).toEqual(["loadSettings", "loadSettings", "loadPrinters", "listen"]));
  });

  describe("importing Models", () => {
    it("Import… opens the picker, and only a selection opens the import dialog", async () => {
      window.location.hash = "#nav=v1/library";
      const { default: App } = await import("./App");
      render(() => <App />);
      const importButton = await screen.findByRole("button", { name: "Import…" });

      await fireEvent.click(importButton);
      expect(libraryStore.pickFiles).toHaveBeenCalledWith("import");
      await Promise.resolve();
      expect(screen.getByRole("status", { name: "Import selection" })).toHaveTextContent("none");

      libraryStore.pickFiles.mockResolvedValueOnce(summary("sel-picked"));
      await fireEvent.click(importButton);
      await waitFor(() => expect(screen.getByRole("status", { name: "Import selection" })).toHaveTextContent("sel-picked"));

      await fireEvent.click(screen.getByRole("button", { name: "Close import" }));
      expect(screen.getByRole("status", { name: "Import selection" })).toHaveTextContent("none");
    });

    it("reports a picker failure to the Library banner", async () => {
      window.location.hash = "#nav=v1/library";
      const failure = { contractVersion: 1, code: "INTERNAL", message: "The file picker failed.", recovery: [], retryable: false };
      libraryStore.pickFiles.mockRejectedValue(failure);
      const { default: App } = await import("./App");
      render(() => <App />);

      await fireEvent.click(await screen.findByRole("button", { name: "Import…" }));
      await waitFor(() => expect(libraryStore.reportLibraryError).toHaveBeenCalledWith(failure));
    });

    it("a dropped selection navigates to the Library and opens the import dialog", async () => {
      const { default: App } = await import("./App");
      render(() => <App />);
      await screen.findByText("Persisted Printers are visible");
      await waitFor(() => expect(libraryStore.dropped).toBeDefined());

      libraryStore.dropped!(summary("sel-dropped"));
      expect(await screen.findByRole("heading", { name: "Library" })).toBeInTheDocument();
      expect(screen.getByRole("status", { name: "Import selection" })).toHaveTextContent("sel-dropped");
    });

    it("cancels a second drop while an import is already open", async () => {
      window.location.hash = "#nav=v1/library";
      const { default: App } = await import("./App");
      render(() => <App />);
      await waitFor(() => expect(libraryStore.dropped).toBeDefined());

      libraryStore.dropped!(summary("sel-first"));
      libraryStore.dropped!(summary("sel-second"));
      expect(screen.getByRole("status", { name: "Import selection" })).toHaveTextContent("sel-first");
      expect(libraryStore.cancelSelection).toHaveBeenCalledWith("sel-second");
      // The open dialog says why, until it closes.
      expect(screen.getByRole("status", { name: "Drop refused" })).toHaveTextContent("true");
      await fireEvent.click(screen.getByRole("button", { name: "Close import" }));
      expect(screen.getByRole("status", { name: "Drop refused" })).toHaveTextContent("false");
    });

    it("drag enter and leave toggle the drop highlight", async () => {
      window.location.hash = "#nav=v1/library";
      const { default: App } = await import("./App");
      const { unmount } = render(() => <App />);
      await waitFor(() => expect(webview.onDragDropEvent).toHaveBeenCalledOnce());
      const active = await screen.findByRole("status", { name: "Drop active" });

      webview.handler!({ payload: { type: "enter" } });
      expect(active).toHaveTextContent("true");
      webview.handler!({ payload: { type: "over" } });
      expect(active).toHaveTextContent("true");
      webview.handler!({ payload: { type: "leave" } });
      expect(active).toHaveTextContent("false");
      webview.handler!({ payload: { type: "enter" } });
      webview.handler!({ payload: { type: "drop" } });
      expect(active).toHaveTextContent("false");

      unmount();
      expect(webview.unlisten).toHaveBeenCalledOnce();
      expect(libraryStore.stopDropped).toHaveBeenCalledOnce();
    });

    it("registers no drag listener in web mode", async () => {
      desktop.available = false;
      window.location.hash = "#nav=v1/library";
      const { default: App } = await import("./App");
      render(() => <App />);
      await screen.findByText("Library workspace");
      expect(webview.onDragDropEvent).not.toHaveBeenCalled();
    });
  });

  it("disposes a listener that resolves after App unmounts", async () => {
    let resolveListener: ((dispose: () => void) => void) | undefined;
    appState.startStatusListener.mockImplementation(() => new Promise((resolve) => {
      resolveListener = resolve;
    }));
    const { default: App } = await import("./App");

    const { unmount } = render(() => <App />);
    await waitFor(() => expect(appState.startStatusListener).toHaveBeenCalledOnce());
    unmount();
    const dispose = vi.fn();
    resolveListener?.(dispose);

    await waitFor(() => expect(dispose).toHaveBeenCalledOnce());
  });
});
