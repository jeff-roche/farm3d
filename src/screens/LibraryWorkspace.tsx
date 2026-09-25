import {
  createEffect,
  createMemo,
  createSignal,
  lazy,
  Match,
  on,
  onCleanup,
  onMount,
  Show,
  Suspense,
  Switch,
} from "solid-js";
import { Dialog as KDialog } from "@kobalte/core/dialog";
import { IconLayoutGrid, IconList } from "@tabler/icons-solidjs";
import { Button, FileDropSurface, SegmentedControl, Select, TextField } from "../design-system";
import { desktopAvailable } from "../ipc/client";
import {
  checkSources,
  dismissLibraryError,
  library,
  refreshLibrary,
  reportLibraryError,
} from "../library/library-store";
import { modelsFor, SAVED_VIEW_IDS, searchModels, sortModels, type LibraryView } from "../library/saved-views";
import type { ImportSelectionSummary, ModelRecord, ProjectRecord } from "../library/types";
import { navigation, type NavigationTarget } from "../navigation/navigation-store";
import { formatBytes, viewLabel } from "./library-presentation";
import { LibrarySidebar } from "./LibrarySidebar";
import { ModelDetailsPanel } from "./ModelDetailsPanel";
import { ModelGrid } from "./ModelGrid";
import { ModelList } from "./ModelList";
import styles from "./LibraryWorkspace.module.css";

// The dialogs load on first use, keeping them out of the main chunk.
const ImportDialog = lazy(() => import("./ImportDialog").then((m) => ({ default: m.ImportDialog })));
const LocateSourceDialog = lazy(() => import("./LocateSourceDialog").then((m) => ({ default: m.LocateSourceDialog })));
const ConvertToManagedDialog = lazy(() =>
  import("./LocateSourceDialog").then((m) => ({ default: m.ConvertToManagedDialog })));
const DeleteModelDialog = lazy(() => import("./DeleteModelDialog").then((m) => ({ default: m.DeleteModelDialog })));
const CreateProjectDialog = lazy(() => import("./ProjectDialogs").then((m) => ({ default: m.CreateProjectDialog })));
const RenameProjectDialog = lazy(() => import("./ProjectDialogs").then((m) => ({ default: m.RenameProjectDialog })));
const DeleteProjectDialog = lazy(() => import("./ProjectDialogs").then((m) => ({ default: m.DeleteProjectDialog })));
// The Preparation workspace (viewport, tools) loads on first **Prepare…**.
const PreparationMode = lazy(() => import("./PreparationMode").then((m) => ({ default: m.PreparationMode })));

export interface LibraryWorkspaceProps {
  /** App's navigation, so a selection is checked against every available
   *  id and lands in the URL fragment. */
  navigate: (target: NavigationTarget) => void;
  /** Starts an import: App opens the native picker and, for a selection,
   *  passes it back as `importSelection`. */
  onImport?: () => void;
  /** The selection the import dialog is working on (picked or dropped);
   *  `null` or absent while no import is open. */
  importSelection?: ImportSelectionSummary | null;
  /** The import dialog closed; App clears `importSelection`. */
  onImportClose?: () => void;
  /** A file drag is over the window (App's webview drag listener). */
  dropActive?: boolean;
  /** A drop was refused because an import is already open. */
  dropRefused?: boolean;
}

type LayoutMode = "grid" | "list";
type SortOrder = "name" | "recent";

/** The one workspace dialog open at a time, by the id it acts on. */
type WorkspaceDialog =
  | { kind: "locate" | "convert" | "deleteModel"; modelId: string }
  | { kind: "createProject" }
  | { kind: "renameProject" | "deleteProject"; projectId: string };

const MODE_KEY = "farm3d:library-mode";
const VIEW_KEY = "farm3d:library-view";
const ALL_MODELS: LibraryView = { kind: "view", id: "all" };
/** Below this window width the details panel becomes an overlay (the
 *  umbrella's dock rule: three panes at 1440 × 900, an overlay at 1024). */
const INLINE_DETAILS_MIN_WIDTH = 1280;
const SORT_LABELS: Record<SortOrder, string> = { name: "Name", recent: "Recently added" };
const WEB_IMPORT_REASON = "Importing Models needs the desktop app.";
const WEB_CHECK_REASON = "Checking sources needs the desktop app.";
const MINUTE_MS = 60_000;

function checkedLabel(checkedAt: number, now: number): string {
  const minutes = Math.floor((now - checkedAt) / MINUTE_MS);
  if (minutes < 1) return "Checked just now";
  return minutes === 1 ? "Checked 1 minute ago" : `Checked ${minutes} minutes ago`;
}

// The grid/list mode and the active view are display state, remembered per
// viewer. Storage can be unavailable (private windows, blocked site data),
// so every read and write is guarded and the defaults always work.
function readStored(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeStored(key: string, value: string): void {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // Not remembered this time; nothing else depends on it.
  }
}

function storedMode(): LayoutMode {
  return readStored(MODE_KEY) === "list" ? "list" : "grid";
}

function storedView(): LibraryView {
  try {
    const parsed = JSON.parse(readStored(VIEW_KEY) ?? "null") as Partial<LibraryView> | null;
    if (parsed?.kind === "view" && (SAVED_VIEW_IDS as readonly string[]).includes(String(parsed.id))) {
      return parsed as LibraryView;
    }
    if (parsed?.kind === "project" && typeof parsed.id === "string") return parsed as LibraryView;
  } catch {
    // Unreadable: fall back to All Models.
  }
  return ALL_MODELS;
}

/** D19's workspace: toolbar, then the sidebar, the Model grid or list, and
 *  the details panel. Reads `library-store` and the navigation store
 *  directly; a Model selection lives in the navigation target so it can be
 *  deep-linked, while the view, mode, search and sort are display state. */
export function LibraryWorkspace(props: LibraryWorkspaceProps) {
  const [mode, setModeSignal] = createSignal<LayoutMode>(storedMode());
  const [view, setViewSignal] = createSignal<LibraryView>(storedView());
  const [search, setSearch] = createSignal("");
  const [sort, setSort] = createSignal<SortOrder>("name");
  const [narrow, setNarrow] = createSignal(window.innerWidth < INLINE_DETAILS_MIN_WIDTH);
  const [detailsOpen, setDetailsOpen] = createSignal(false);
  const [focusRequest, setFocusRequest] = createSignal(0);
  const [dialog, setDialog] = createSignal<WorkspaceDialog | null>(null);
  const [checking, setChecking] = createSignal(false);
  const [checkedAt, setCheckedAt] = createSignal<number | null>(null);
  const [clock, setClock] = createSignal(Date.now());
  // D19: the Model being prepared. Workspace state, not deep-linked; the
  // navigation target stays the Model's.
  const [preparingId, setPreparingId] = createSignal<string | null>(null);
  const now = () => new Date();

  // D15 trigger 3: check linked sources when the Library becomes visible
  // and when the window regains focus. The store throttles these to one per
  // 30 s. Web mode has no files to check.
  const checkInBackground = () => {
    if (desktopAvailable()) void checkSources().catch(reportLibraryError);
  };
  const checkNow = async () => {
    if (checking()) return;
    setChecking(true);
    try {
      await checkSources(undefined, { force: true });
      setCheckedAt(Date.now());
      setClock(Date.now());
    } catch (error) {
      reportLibraryError(error);
    } finally {
      setChecking(false);
    }
  };

  onMount(() => {
    const onResize = () => setNarrow(window.innerWidth < INLINE_DETAILS_MIN_WIDTH);
    window.addEventListener("resize", onResize);
    window.addEventListener("focus", checkInBackground);
    // Keeps "Checked N minutes ago" current.
    const ticker = window.setInterval(() => setClock(Date.now()), MINUTE_MS / 2);
    onCleanup(() => {
      window.removeEventListener("resize", onResize);
      window.removeEventListener("focus", checkInBackground);
      window.clearInterval(ticker);
    });
    checkInBackground();
  });

  const setMode = (next: LayoutMode) => {
    setModeSignal(next);
    writeStored(MODE_KEY, next);
  };
  const setView = (next: LibraryView) => {
    setViewSignal(next);
    writeStored(VIEW_KEY, JSON.stringify(next));
  };

  // A remembered Project view whose Project has since been deleted falls
  // back to All Models.
  const activeView = createMemo<LibraryView>(() => {
    const current = view();
    if (current.kind === "project" && library.status() === "ready"
      && !library.projects().some((project) => project.id === current.id)) {
      return ALL_MODELS;
    }
    return current;
  });

  const libraryTarget = () => {
    const target = navigation.target();
    return target.destination === "library" ? target : null;
  };
  const selectedModel = createMemo(() => {
    const selection = libraryTarget()?.selection;
    if (selection?.kind !== "model") return undefined;
    return library.models().find((model) => model.id === selection.id);
  });

  // Deep links (D19): a Project target opens that Project's view; a Model
  // target keeps the current view if it contains the Model and otherwise
  // switches to All Models. Runs when the target changes and once the
  // Library first loads (a cold launch), never on an ordinary edit -- so
  // removing the viewed Project from the selected Model keeps the view.
  const loaded = createMemo(() => library.status() === "ready");
  createEffect(on([() => libraryTarget()?.selection, loaded], ([selection, isLoaded]) => {
    if (!isLoaded) return;
    if (selection?.kind === "project") {
      if (library.projects().some((project) => project.id === selection.id)) {
        setView({ kind: "project", id: selection.id });
      }
      return;
    }
    const model = selectedModel();
    if (model && !modelsFor(activeView(), [model], now()).length) setView(ALL_MODELS);
  }));

  // Preparing ends when the selection moves to another Model (or none).
  const preparing = createMemo(() => {
    const model = selectedModel();
    return model && model.id === preparingId() && model.format !== "gcode" ? model : undefined;
  });
  createEffect(on(() => selectedModel()?.id, (id) => {
    if (id !== preparingId()) setPreparingId(null);
  }, { defer: true }));

  const inView = createMemo(() => modelsFor(activeView(), library.models(), now()));
  const shown = createMemo(() => sortModels(searchModels(inView(), library.projects(), search()), sort()));

  const selectView = (next: LibraryView) => {
    setView(next);
    props.navigate(next.kind === "project"
      ? { version: 1, destination: "library", selection: { kind: "project", id: next.id } }
      : { version: 1, destination: "library" });
  };
  const closeDialog = () => setDialog(null);
  const dialogModel = (kind: "locate" | "convert" | "deleteModel"): ModelRecord | undefined => {
    const current = dialog();
    if (current?.kind !== kind) return undefined;
    return library.models().find((model) => model.id === current.modelId);
  };
  const dialogProject = (kind: "renameProject" | "deleteProject"): ProjectRecord | undefined => {
    const current = dialog();
    if (current?.kind !== kind) return undefined;
    return library.projects().find((project) => project.id === current.projectId);
  };
  const projectCreated = (project: ProjectRecord) => {
    closeDialog();
    selectView({ kind: "project", id: project.id });
  };
  // D18: the Models stay. Only the view goes, if it was this Project's.
  const projectDeleted = (projectId: string) => {
    const current = dialog();
    if (current?.kind === "deleteProject" && current.projectId === projectId) closeDialog();
    const viewed = view();
    if (viewed.kind === "project" && viewed.id === projectId) selectView(ALL_MODELS);
  };
  const modelDeleted = (modelId: string) => {
    const current = dialog();
    if (current?.kind === "deleteModel" && current.modelId === modelId) closeDialog();
    // Clears the selection if it was this Model.
    const selection = libraryTarget()?.selection;
    if (selection?.kind === "model" && selection.id === modelId) selectView(activeView());
  };
  const selectModel = (modelId: string) => {
    props.navigate({ version: 1, destination: "library", selection: { kind: "model", id: modelId } });
  };
  const addToProject = (modelId: string) => {
    selectModel(modelId);
    if (narrow()) setDetailsOpen(true);
    setFocusRequest((request) => request + 1);
  };
  const showAllModels = () => {
    setSearch("");
    selectView(ALL_MODELS);
  };

  // Keyed on the id: each store update swaps in a new record object, and
  // remounting the panel for it would discard a name draft, drop focus,
  // and refetch the history. `shownModel` holds the last record so the
  // panel never reads `undefined` while it unmounts.
  const shownModel = createMemo<ModelRecord | undefined>((previous) => selectedModel() ?? previous);
  const details = () => (
    <Show
      when={selectedModel()?.id}
      keyed
      fallback={<p class={styles.detailsEmpty}>Select a Model to see its details.</p>}
    >
      {(_id) => (
        <ModelDetailsPanel
          model={shownModel()!}
          projects={library.projects()}
          onLocateSource={(modelId) => setDialog({ kind: "locate", modelId })}
          onConvertToManaged={(modelId) => setDialog({ kind: "convert", modelId })}
          onDelete={(modelId) => setDialog({ kind: "deleteModel", modelId })}
          onPrepare={(modelId) => {
            setDetailsOpen(false);
            setPreparingId(modelId);
          }}
          focusRequest={focusRequest()}
          onFocusHandled={() => setFocusRequest(0)}
        />
      )}
    </Show>
  );

  // New import rows start in the viewed Project, or Unfiled in a saved view.
  const importProjectIds = () => {
    const current = activeView();
    return current.kind === "project" ? [current.id] : [];
  };
  const finishImport = (modelId: string | undefined) => {
    props.onImportClose?.();
    if (modelId) selectModel(modelId);
  };
  const importReasonId = "library-import-unavailable";
  const checkReasonId = "library-check-unavailable";

  const storedCopies = () => {
    const info = library.contentInfo();
    return info ? `Stored copies: ${formatBytes(info.totalBytes)}` : null;
  };

  return (
    <div class={styles.workspace}>
      <Show when={library.error()}>
        {(message) => (
          <div class={styles.errorBanner} role="alert">
            <p class={styles.bannerMessage}>{message()}</p>
            <Button variant="ghost" onClick={dismissLibraryError}>Dismiss</Button>
          </div>
        )}
      </Show>
      <Show when={library.syncState() === "uncertain"}>
        <div class={styles.staleBanner} role="status">
          <p class={styles.bannerMessage}>Library may be out of date</p>
          <Button variant="ghost" onClick={refreshLibrary}>Refresh</Button>
        </div>
      </Show>
      <div class={styles.toolbar}>
        <Button
          variant="primary"
          disabled={!desktopAvailable()}
          aria-describedby={desktopAvailable() ? undefined : importReasonId}
          onClick={() => props.onImport?.()}
        >
          Import…
        </Button>
        <Button variant="secondary" onClick={() => setDialog({ kind: "createProject" })}>New Project</Button>
        <TextField
          type="search"
          aria-label="Search Models"
          placeholder="Search Models"
          value={search()}
          onChange={setSearch}
        />
        <Select
          label="Sort"
          options={["name", "recent"] as SortOrder[]}
          value={sort()}
          onChange={setSort}
          optionLabel={(order) => SORT_LABELS[order]}
        />
        <SegmentedControl
          label="Layout"
          value={mode()}
          onChange={setMode}
          options={[
            { value: "grid", label: "Grid", icon: <IconLayoutGrid size={14} /> },
            { value: "list", label: "List", icon: <IconList size={14} /> },
          ]}
        />
        <span class={styles.spacer} />
        <Show when={!desktopAvailable()}>
          <span id={importReasonId} class={styles.status}>{WEB_IMPORT_REASON}</span>
        </Show>
        <Show when={storedCopies()}>{(text) => <span class={styles.status}>{text()}</span>}</Show>
        <Show when={checkedAt()}>
          {(at) => <span class={styles.status} role="status">{checkedLabel(at(), clock())}</span>}
        </Show>
        <Show when={!desktopAvailable()}>
          <span id={checkReasonId} class={styles.status}>{WEB_CHECK_REASON}</span>
        </Show>
        <Button
          variant="secondary"
          disabled={checking() || !desktopAvailable()}
          aria-describedby={desktopAvailable() ? undefined : checkReasonId}
          onClick={() => void checkNow()}
        >
          Check sources
        </Button>
        <Show when={narrow() && !preparing()}>
          <Button variant="ghost" aria-expanded={detailsOpen()} onClick={() => setDetailsOpen((open) => !open)}>
            Details
          </Button>
        </Show>
      </div>
      <div class={styles.body}>
        <Show
          when={preparing()?.id}
          keyed
          fallback={
            <>
            <LibrarySidebar
              view={activeView()}
              projects={library.projects()}
              models={library.models()}
              now={now()}
              onSelectView={selectView}
              onRenameProject={(projectId) => setDialog({ kind: "renameProject", projectId })}
              onDeleteProject={(projectId) => setDialog({ kind: "deleteProject", projectId })}
            />
            <div class={styles.content}>
              <Show
                when={library.status() !== "loading" || library.models().length > 0}
                fallback={<p class={styles.notice} role="status">Loading the Library…</p>}
              >
                <Show
                  when={library.models().length > 0}
                  fallback={
                    <div class={styles.empty}>
                      <h2 class={styles.emptyTitle}>Import a Model</h2>
                      <p class={styles.notice}>STL, 3MF, and pre-sliced G-code files are kept in the Library.</p>
                      <FileDropSurface
                        label="Import Models"
                        hint="Drop files here, or choose them."
                        active={props.dropActive ?? false}
                        disabled={!desktopAvailable()}
                        disabledReason={WEB_IMPORT_REASON}
                        onChoose={() => props.onImport?.()}
                      />
                    </div>
                  }
                >
                  <Show
                    when={shown().length > 0}
                    fallback={
                      <div class={styles.empty}>
                        <p class={styles.notice}>
                          {search().trim() && inView().length > 0
                            ? `No Models in ${viewLabel(activeView(), library.projects())} match “${search().trim()}”`
                            : `No Models in ${viewLabel(activeView(), library.projects())}`}
                        </p>
                        <Button variant="secondary" onClick={showAllModels}>Show all Models</Button>
                      </div>
                    }
                  >
                    <Show
                      when={mode() === "list"}
                      fallback={
                        <ModelGrid
                          models={shown()}
                          projects={library.projects()}
                          view={activeView()}
                          selectedId={selectedModel()?.id ?? null}
                          onSelect={selectModel}
                          onAddToProject={addToProject}
                        />
                      }
                    >
                      <ModelList
                        models={shown()}
                        projects={library.projects()}
                        view={activeView()}
                        selectedId={selectedModel()?.id ?? null}
                        onSelect={selectModel}
                        onAddToProject={addToProject}
                      />
                    </Show>
                  </Show>
                </Show>
              </Show>
            </div>
            <Show
              when={narrow()}
              fallback={<aside class={styles.details} aria-label="Model details">{details()}</aside>}
            >
              <KDialog open={detailsOpen()} onOpenChange={setDetailsOpen}>
                <KDialog.Portal>
                  <KDialog.Overlay class={styles.overlay} />
                  <KDialog.Content
                    class={styles.overlayContent}
                    aria-label="Model details"
                    onOpenAutoFocus={(event) => {
                      // Add to Project… moves focus to its own picker instead.
                      if (focusRequest()) event.preventDefault();
                    }}
                  >
                    <div class={styles.overlayHeader}>
                      <Button variant="ghost" size="sm" onClick={() => setDetailsOpen(false)}>Close details</Button>
                    </div>
                    {details()}
                  </KDialog.Content>
                </KDialog.Portal>
              </KDialog>
            </Show>
            </>
          }
        >
          {(_id) => (
            <div class={styles.preparing}>
              <Suspense fallback={<p class={styles.notice} role="status">Loading the Preparation workspace…</p>}>
                <PreparationMode model={preparing()!} onBack={() => setPreparingId(null)} />
              </Suspense>
            </div>
          )}
        </Show>
      </div>
      <Suspense>
        <Show when={props.importSelection}>
          {(selection) => (
            <ImportDialog
              selection={selection()}
              defaultProjectIds={importProjectIds()}
              onClose={() => props.onImportClose?.()}
              onDone={finishImport}
              onChooseAgain={() => props.onImport?.()}
              dropRefused={props.dropRefused ?? false}
            />
          )}
        </Show>
        <Switch>
          <Match when={dialogModel("locate")}>
            {(model) => <LocateSourceDialog model={model()} onClose={closeDialog} />}
          </Match>
          <Match when={dialogModel("convert")}>
            {(model) => <ConvertToManagedDialog model={model()} onClose={closeDialog} />}
          </Match>
          <Match when={dialogModel("deleteModel")}>
            {(model) => <DeleteModelDialog model={model()} onClose={closeDialog} onDeleted={modelDeleted} />}
          </Match>
          <Match when={dialog()?.kind === "createProject"}>
            <CreateProjectDialog onClose={closeDialog} onCreated={projectCreated} />
          </Match>
          <Match when={dialogProject("renameProject")}>
            {(project) => <RenameProjectDialog project={project()} onClose={closeDialog} />}
          </Match>
          <Match when={dialogProject("deleteProject")}>
            {(project) => <DeleteProjectDialog project={project()} onClose={closeDialog} onDeleted={projectDeleted} />}
          </Match>
        </Switch>
      </Suspense>
    </div>
  );
}
