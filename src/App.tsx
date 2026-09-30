// src/App.tsx
import { createSignal, lazy, Match, onCleanup, onMount, Show, Suspense, Switch } from "solid-js";
import { createMonitorStore, type MonitorShellView, type MonitorStore } from "./monitor/monitor-store";
import { AppShell } from "./screens/AppShell";
import { SlicerSettingsHost } from "./screens/SlicerSettingsHost";
import type { ScreenId } from "./screens/ActivityBar";
import { PrinterDashboard } from "./screens/PrinterDashboard";
import { LibraryWorkspace } from "./screens/LibraryWorkspace";
import { ensureInventoryLoaded, spoolState } from "./spools/spool-store";
import { desktopAvailable } from "./ipc/client";
import {
  cancelSelection,
  library,
  onSelectionDropped,
  pickFiles,
  reportLibraryError,
  startLibrary,
} from "./library/library-store";
import type { ImportSelectionSummary } from "./library/types";
import { startSlicing } from "./slicing/slicing-store";
import { startHostOperations } from "./host-ops/host-operations-store";
import { syncCapabilities } from "./host-ops/capabilities-store";
import { queue, startQueue } from "./queue/queue-store";
import { jobStateLabel } from "./queue/presentation";
import { attention, requestAttentionCenterOpen, startAttention } from "./attention/attention-store";
import type { NavigateRequest } from "./generated/contracts/domain/NavigateRequest";
import type { PrinterRosterEntry } from "./design-system";
import {
  dismissPrinterArchiveNotice,
  dismissPrinterStoreError,
  loadDuplicateHostArchives,
  loadPrinters,
  printerArchiveNotice,
  printers,
  printerStoreCommandError,
  printerStoreError,
  printerStoreRetryable,
  printerStoreStatus,
  printerStatusSyncState,
  startStatusListener,
} from "./printers/printer-store";
import { Button } from "./design-system";
import { HostOperationAlert } from "./screens/HostOperationAlert";
import { loadSettings, updateSettings } from "./settings/settings-store";
import styles from "./App.module.css";
import {
  navigation,
  parseNavigationTarget,
  serializeNavigationTarget,
  type NavigationDestination,
} from "./navigation/navigation-store";
import { settingsCategorySlugs } from "./screens/settings/categories";

// The Spools screen and its dialogs load on first visit, keeping them out of
// the main chunk.
const SpoolInventory = lazy(() => import("./screens/SpoolInventory").then((m) => ({ default: m.SpoolInventory })));
const QueueScreen = lazy(() => import("./screens/QueueScreen").then((m) => ({ default: m.QueueScreen })));
// The Settings workspace loads on first visit; its categories are chunks of their own.
const SettingsWorkspace = lazy(() => import("./screens/settings/SettingsWorkspace").then((m) => ({ default: m.SettingsWorkspace })));

const SCREEN_TITLE: Record<NavigationDestination, string> = {
  monitor: "Monitor",
  queue: "Queue",
  library: "Library",
  spools: "Spools",
  settings: "Settings",
};

const EMPTY_SHELL: MonitorShellView = {
  printerRoster: { key: "all", label: "Printers", count: 0, printers: [], remainingCount: 0 },
  operationalRosters: [],
  adapterHealth: { severity: "info", label: "Waiting for adapter status" },
};

const FARM_VISITED_KEY = "farm3d:monitor-farm-visited";

function isFirstFarmVisit(): boolean {
  return window.localStorage.getItem(FARM_VISITED_KEY) !== "true";
}

function markFarmVisited(): void {
  window.localStorage.setItem(FARM_VISITED_KEY, "true");
}

function App() {
  const [monitorStore, setMonitorStore] = createSignal<MonitorStore>();
  const [statusStartupError, setStatusStartupError] = createSignal<string | null>(null);
  const [isFirstRun, setIsFirstRun] = createSignal(false);
  let retryStartup: (() => void) | undefined;
  const active = () => navigation.target().destination;
  const shellActive = () => {
    const destination = active();
    return (destination === "queue" || destination === "library" || destination === "spools" || destination === "settings" ? destination : "monitor") satisfies ScreenId;
  };
  const shell = () => monitorStore()?.shell() ?? EMPTY_SHELL;
  // Until the Library's first load settles, a Library selection is pending,
  // not unavailable: counting it as available keeps the "no longer
  // available" banner away, and the reconcile after `startLibrary`
  // resolves decides for real.
  const libraryPending = () => library.status() === "idle" || library.status() === "loading";
  const queuePending = () => queue.status() === "idle" || queue.status() === "loading";
  const attentionPending = () => attention.status() === "idle" || attention.status() === "loading";
  // Every Queue Entry and Job the store holds: a Queue selection names
  // either, resolved by prefix (spec "Navigation").
  const queueIds = () => [...queue.entries(), ...queue.history()]
    .flatMap((entry) => (entry.jobId ? [entry.id, entry.jobId] : [entry.id]));
  // Every Attention Event and Incident the store holds, for `monitor`'s
  // `attention`/`incident` selections (deep links, `farm3d-navigate-v1`).
  // An Incident stays a valid `monitor/incident` target after it closes
  // (Controller ruling, fix round 1): an Event's detail links its
  // Incident even once closed, and D8 keeps an archived Printer's
  // Incidents openable, so `openIncidents()` alone isn't enough -- every
  // loaded Event's own `incidentId` (open or resolved) counts too.
  const attentionEvents = () => [...attention.open(), ...attention.resolved()];
  const attentionIds = () => [
    ...attentionEvents().map((event) => event.id),
    ...attention.openIncidents().map((incident) => incident.id),
    ...attentionEvents().flatMap((event) => (event.incidentId ? [event.incidentId] : [])),
  ];
  const navigationContext = (target: Parameters<typeof navigation.navigate>[0]) => ({
    availableDestinations: ["monitor", "queue", "library", "spools", "settings"] as NavigationDestination[],
    availableIds: [
      ...settingsCategorySlugs(),
      ...printers().map((printer) => printer.id),
      ...spoolState.spools.map((spool) => spool.id),
      ...library.projects().map((project) => project.id),
      ...library.models().map((model) => model.id),
      ...queueIds(),
      ...attentionIds(),
      ...(target.destination === "library" && target.selection && libraryPending() ? [target.selection.id] : []),
      ...(target.destination === "queue" && target.selection && queuePending() ? [target.selection.id] : []),
      ...(
        target.destination === "monitor"
        && target.selection
        && (target.selection.kind === "attention" || target.selection.kind === "incident")
        && attentionPending()
          ? [target.selection.id]
          : []
      ),
    ],
  });
  const reconcileNavigation = () => {
    navigation.navigate(navigation.target(), navigationContext(navigation.target()));
    const target = navigation.target();
    monitorStore()?.setSelectedPrinterId(
      navigation.availability() === "available" && target.destination === "monitor" && target.selection?.kind === "printer"
        ? target.selection.id
        : null,
    );
  };
  const navigate = (target: Parameters<typeof navigation.navigate>[0]) => {
    navigation.navigate(target, navigationContext(target));
    reconcileNavigation();
    window.location.hash = serializeNavigationTarget(target).slice(1);
  };
  const setActive = (destination: ScreenId) => navigate({ version: 1, destination });

  // The Queue badge: entries whose verdict is Blocked or Awaiting operator,
  // plus open Reconciliation Requirements (spec "Frontend architecture").
  const queueAttentionCount = () =>
    queue.entries().filter((entry) => {
      const verdict = entry.state === "queued" ? queue.eligibility(entry.id)?.verdict : undefined;
      return verdict === "blocked" || verdict === "awaitingOperator";
    }).length + queue.requirements().length;
  // An open `assigned` entry's Job is active until it ends (D2/D3).
  const activeJobs = (): PrinterRosterEntry[] => queue.entries().flatMap((entry) => {
    const job = entry.state === "assigned" ? queue.jobFor(entry.id) : undefined;
    return job ? [{ id: job.id, name: job.printerSnapshot.name, detail: entry.display.modelName, stateLabel: jobStateLabel(job.state) }] : [];
  });

  // The import dialog's selection, from the picker or a window drop (D7).
  const [importSelection, setImportSelection] = createSignal<ImportSelectionSummary | null>(null);
  // A file drag is over the window: only the hover highlight. Rust observes
  // the drop itself and announces it as `library.selection.dropped`.
  const [dropActive, setDropActive] = createSignal(false);
  // A drop that arrived while an import was open; the open dialog says so.
  const [dropRefused, setDropRefused] = createSignal(false);
  const openImport = (selection: ImportSelectionSummary | null) => {
    setDropRefused(false);
    setImportSelection(selection);
  };
  const startImport = () => {
    void pickFiles("import").then((selection) => {
      if (selection) openImport(selection);
    }).catch(reportLibraryError);
  };
  const openDroppedSelection = (selection: ImportSelectionSummary) => {
    if (importSelection()) {
      // One import at a time: a drop onto an open import dialog is refused,
      // its staging released, and the dialog says why.
      void cancelSelection(selection.selectionId).catch(reportLibraryError);
      setDropRefused(true);
      return;
    }
    navigate({ version: 1, destination: "library" });
    openImport(selection);
  };

  onMount(() => {
    const stopDropped = onSelectionDropped(openDroppedSelection);
    let stopDrag: (() => void) | undefined;
    let dragDisposed = false;
    if (desktopAvailable()) {
      // Loaded on demand: the webview API pulls in the whole window module,
      // which web mode never needs.
      void import("@tauri-apps/api/webview").then(({ getCurrentWebview }) => getCurrentWebview().onDragDropEvent((event) => {
        setDropActive(event.payload.type === "enter" || event.payload.type === "over");
      })).then((unlisten) => {
        if (dragDisposed) unlisten();
        else stopDrag = unlisten;
      }).catch(() => {
        // Without the listener there is no drag highlight; dropping still
        // works, since Rust observes the drop itself.
      });
    }
    onCleanup(() => {
      dragDisposed = true;
      stopDrag?.();
      stopDropped();
    });
  });

  // `farm3d-navigate-v1` (spec D7/"Events"): a separate, unsequenced event
  // (its own Tauri channel, not `farm3d-event-v1`) a notification click
  // emits. Routed through `navigate` like any other selection; opens the
  // Attention center (Task 13's `AttentionTrigger`, via
  // `requestAttentionCenterOpen`'s seam) when `openAttentionCenter` is true.
  onMount(() => {
    if (!desktopAvailable()) return;
    let disposed = false;
    let stopNavigate: (() => void) | undefined;
    void import("@tauri-apps/api/event").then(({ listen }) => listen<NavigateRequest>("farm3d-navigate-v1", (event) => {
      navigate(event.payload.target);
      if (event.payload.openAttentionCenter) requestAttentionCenterOpen();
    })).then((unlisten) => {
      if (disposed) unlisten();
      else stopNavigate = unlisten;
    }).catch(() => {
      // Without the listener, a notification click just raises the window
      // (Rust's own activation step) without navigating.
    });
    onCleanup(() => {
      disposed = true;
      stopNavigate?.();
    });
  });

  onMount(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let disposeLibrary: (() => void) | undefined;
    let disposeSlicing: (() => void) | undefined;
    let disposeHostOperations: (() => void) | undefined;
    let disposeQueue: (() => void) | undefined;
    let disposeAttention: (() => void) | undefined;
    let stopCapabilitySync: (() => void) | undefined;
    let startupGeneration = 0;
    const start = () => {
      const generation = ++startupGeneration;
      setStatusStartupError(null);
      unlisten?.();
      unlisten = undefined;
      void (async () => {
        let settings;
        try {
          settings = await loadSettings();
          await loadPrinters();
          if (printerStoreStatus() === "error") throw new Error("Printer loading failed");
        } catch {
          if (!disposed && generation === startupGeneration) {
            setStatusStartupError("Monitor startup could not be completed.");
          }
          return;
        }
        if (disposed || generation !== startupGeneration) return;
        void loadDuplicateHostArchives();
        setIsFirstRun(isFirstFarmVisit() && printers().length === 0);
        markFarmVisited();
        setMonitorStore(createMonitorStore({
          printers,
          initialSection: settings.monitorSection,
          initialDensity: settings.monitorDensity,
          persistPreferences: (next) => updateSettings(next),
        }));
        reconcileNavigation();
        // The low-Spool badge and a cold-launch Spool deep link both need
        // the inventory; re-check navigation once it has loaded, since a
        // Spool id isn't known until then.
        void ensureInventoryLoaded().then(() => {
          if (!disposed && generation === startupGeneration) reconcileNavigation();
        });
        // Same for a Library deep link. `startLibrary` is idempotent, so a
        // startup retry replaces the previous listener; a start that a
        // retry or unmount has overtaken disposes its own.
        void startLibrary().then((dispose) => {
          if (disposed || generation !== startupGeneration) {
            dispose();
            return;
          }
          disposeLibrary = dispose;
          reconcileNavigation();
        });
        // Slicing follows the Library (the spec's startup order), with
        // the same retry and unmount handling; Host Operations follow
        // slicing (P6).
        void startSlicing().then((dispose) => {
          if (disposed || generation !== startupGeneration) {
            dispose();
            return;
          }
          disposeSlicing = dispose;
          return startHostOperations().then((disposeOps) => {
            if (disposed || generation !== startupGeneration) {
              disposeOps();
              return;
            }
            disposeHostOperations = disposeOps;
            // The Queue follows Host Operations (P7); Attention follows
            // the Queue (P8).
            return startQueue().then((disposeQueueStream) => {
              if (disposed || generation !== startupGeneration) {
                disposeQueueStream();
                return;
              }
              disposeQueue = disposeQueueStream;
              reconcileNavigation();
              return startAttention().then((disposeAttentionStream) => {
                if (disposed || generation !== startupGeneration) {
                  disposeAttentionStream();
                  return;
                }
                disposeAttention = disposeAttentionStream;
                reconcileNavigation();
              });
            });
          });
        });
        // Capabilities have no event: refetch a Printer's whenever its
        // status changes (spec "Events").
        stopCapabilitySync?.();
        stopCapabilitySync = syncCapabilities(printers);
        try {
          const dispose = await startStatusListener();
          if (disposed || generation !== startupGeneration) dispose();
          else unlisten = dispose;
        } catch {
          if (!disposed && generation === startupGeneration) {
            setStatusStartupError("Live Printer status could not be started.");
          }
        }
      })();
    };
    retryStartup = start;
    const applyFragment = () => {
      const target = parseNavigationTarget(window.location.hash);
      if (target) navigate(target);
    };
    applyFragment();
    window.addEventListener("hashchange", applyFragment);
    // Printers must be known before the listener's backfill arrives; status
    // events for unknown ids are deliberately ignored by the reconciliation store.
    start();
    onCleanup(() => {
      disposed = true;
      retryStartup = undefined;
      unlisten?.();
      disposeLibrary?.();
      disposeSlicing?.();
      disposeHostOperations?.();
      disposeQueue?.();
      disposeAttention?.();
      stopCapabilitySync?.();
      window.removeEventListener("hashchange", applyFragment);
    });
  });

  return (
    <AppShell
      active={shellActive()}
      onSelect={setActive}
      title={SCREEN_TITLE[active()]}
      printerRoster={shell().printerRoster}
      operationalRosters={shell().operationalRosters}
      adapterHealth={shell().adapterHealth}
      lastLiveEventAt={shell().lastLiveEventAt}
      attentionSpoolCount={spoolState.spools.filter((spool) => spool.facets.low || spool.facets.reconciliation).length}
      queueAttentionCount={queueAttentionCount()}
      attentionActionableCount={attention.actionableCount()}
      activeJobs={activeJobs()}
    >
      <Show when={printerStoreCommandError()?.code === "HOST_OPERATION_PENDING" ? printerStoreCommandError() : undefined}>
        {(error) => (
          <div class={styles.errorBanner}>
            <HostOperationAlert error={error()} fallback={error().message}>
              <Button variant="ghost" onClick={dismissPrinterStoreError}>
                Dismiss
              </Button>
            </HostOperationAlert>
          </div>
        )}
      </Show>
      <Show when={printerStoreCommandError()?.code !== "HOST_OPERATION_PENDING" && printerStoreError()}>
        {(message) => (
          <div class={styles.errorBanner} role="alert">
            <p class={styles.errorMessage}>{message()}</p>
            <Show when={printerStoreRetryable()}>
              <Button variant="ghost" onClick={() => retryStartup?.()}>
                Retry startup
              </Button>
            </Show>
            <Button variant="ghost" onClick={dismissPrinterStoreError}>
              Dismiss
            </Button>
          </div>
        )}
      </Show>
      <Show when={statusStartupError()}>
        {(message) => (
          <div class={styles.errorBanner} role="alert">
            <p class={styles.errorMessage}>{message()}</p>
            <Button variant="ghost" onClick={() => {
              retryStartup?.();
            }}>
              Retry startup
            </Button>
            <Button variant="ghost" onClick={() => setStatusStartupError(null)}>
              Dismiss
            </Button>
          </div>
        )}
      </Show>
      <Show when={printerArchiveNotice()}>
        {(message) => (
          <div class={styles.noticeBanner} role="status">
            <p class={styles.noticeMessage}>{message()}</p>
            <Button variant="ghost" onClick={dismissPrinterArchiveNotice}>
              Dismiss
            </Button>
          </div>
        )}
      </Show>
      <Show when={navigation.availability() === "destinationUnavailable"}>
        <div class={styles.errorBanner} role="status">
          <p class={styles.errorMessage}>{SCREEN_TITLE[active()]} is not available in this version.</p>
          <Button variant="ghost" onClick={() => setActive("monitor")}>Open Monitor</Button>
        </div>
      </Show>
      <Show when={navigation.availability() === "selectionUnavailable"}>
        <div class={styles.errorBanner} role="status">
          <p class={styles.errorMessage}>The requested item is no longer available.</p>
        </div>
      </Show>
      <Show
        when={navigation.availability() !== "destinationUnavailable" && active() === "monitor"}
        fallback={
          <Switch>
            <Match when={active() === "library"}>
              <LibraryWorkspace
                navigate={navigate}
                onImport={startImport}
                importSelection={importSelection()}
                onImportClose={() => openImport(null)}
                dropActive={dropActive()}
                dropRefused={dropRefused()}
              />
            </Match>
            <Match when={active() === "queue"}>
              <Suspense fallback={<p class={styles.loading} role="status">Loading the Queue…</p>}>
                <QueueScreen />
              </Suspense>
            </Match>
            <Match when={active() === "settings"}>
              <Suspense fallback={<p class={styles.loading} role="status">Loading Settings…</p>}>
                <SettingsWorkspace
                  category={(() => {
                    const selection = navigation.target().selection;
                    return selection?.kind === "settingsCategory" ? selection.id : undefined;
                  })()}
                  onCategoryChange={(slug) => navigate({
                    version: 1,
                    destination: "settings",
                    selection: { kind: "settingsCategory", id: slug },
                  })}
                  monitor={monitorStore()}
                  onOpenQueue={() => navigate({ version: 1, destination: "queue" })}
                  onPrintersImported={() => {
                    setIsFirstRun(false);
                    reconcileNavigation();
                  }}
                />
              </Suspense>
            </Match>
            <Match when={active() === "spools"}>
              <Suspense fallback={<p class={styles.loading} role="status">Loading Spools…</p>}>
                <SpoolInventory />
              </Suspense>
            </Match>
          </Switch>
        }
      >
        <Show when={monitorStore()}>
          {(store) => (
            <PrinterDashboard
              store={store()}
              loading={printerStoreStatus() === "loading"}
              isFirstRun={isFirstRun()}
              syncState={printerStatusSyncState()}
              onSelectionChange={(id) => navigate({
                version: 1,
                destination: "monitor",
                ...(id ? { selection: { kind: "printer", id } } : {}),
              })}
              attentionEventId={(() => {
                const selection = navigation.target().selection;
                return selection?.kind === "attention" ? selection.id : null;
              })()}
              onAttentionEventClose={() => navigate({ version: 1, destination: "monitor" })}
              incidentId={(() => {
                const selection = navigation.target().selection;
                return selection?.kind === "incident" ? selection.id : null;
              })()}
              onIncidentClose={() => navigate({ version: 1, destination: "monitor" })}
              existingPrinters={printers()}
              onPrinterCreated={() => setIsFirstRun(false)}
              // The Setup tab's guarded Archive -> Delete... flow already
              // called `removePrinter` itself (spec D7's typed-name confirm)
              // before this fires -- this only reconciles navigation and
              // first-run state the way P1's direct removal did.
              onRemovePrinter={() => {
                setIsFirstRun(false);
                reconcileNavigation();
              }}
            />
          )}
        </Show>
      </Show>
      {/* Portalled: it renders over the whole app, whatever the screen. */}
      <SlicerSettingsHost />
    </AppShell>
  );
}

export default App;
