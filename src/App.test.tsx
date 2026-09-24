import { cleanup, render, screen, waitFor } from "@solidjs/testing-library";
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
  printerStoreError: () => null,
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
  LibraryWorkspace: () => <div>Library workspace</div>,
}));

/** `vi.resetModules` gives each test a fresh navigation store, so read the
 *  one this test's App imported. */
async function importAppAndNavigation() {
  const { default: App } = await import("./App");
  const { navigation } = await import("./navigation/navigation-store");
  return { App, navigation };
}

const libraryStore = vi.hoisted(() => ({
  startLibrary: vi.fn(),
  dispose: vi.fn(),
  load: undefined as undefined | (() => void),
  reset: undefined as undefined | (() => void),
}));
vi.mock("./library/library-store", async () => {
  const { createStore } = await import("solid-js/store");
  const [state, setState] = createStore({ projects: [] as { id: string }[], models: [] as { id: string }[] });
  libraryStore.reset = () => setState({ projects: [], models: [] });
  libraryStore.load = () => setState({ projects: [{ id: "prj-brackets" }], models: [{ id: "mdl-bracket" }] });
  return {
    library: { projects: () => state.projects, models: () => state.models },
    startLibrary: libraryStore.startLibrary,
  };
});

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

beforeEach(() => {
  vi.resetModules();
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
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("App", () => {
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
    expect(navigation.availability()).toBe("selectionUnavailable");
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

  it("shows the no-longer-available banner for an unknown Model id", async () => {
    window.location.hash = "#nav=v1/library/model/mdl-gone";
    const { App, navigation } = await importAppAndNavigation();
    render(() => <App />);

    await waitFor(() => expect(libraryStore.startLibrary).toHaveBeenCalled());
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
