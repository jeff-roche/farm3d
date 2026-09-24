import { createEffect, createMemo, createSignal, on, onCleanup, onMount, Show } from "solid-js";
import { Dialog as KDialog } from "@kobalte/core/dialog";
import { IconLayoutGrid, IconList } from "@tabler/icons-solidjs";
import { Button, FileDropSurface, SegmentedControl, Select, TextField } from "../design-system";
import { desktopAvailable } from "../ipc/client";
import {
  convertToManaged,
  dismissLibraryError,
  library,
  refreshLibrary,
  reportLibraryError,
} from "../library/library-store";
import { modelsFor, SAVED_VIEW_IDS, searchModels, sortModels, type LibraryView } from "../library/saved-views";
import { navigation, type NavigationTarget } from "../navigation/navigation-store";
import { formatBytes, viewLabel } from "./library-presentation";
import { LibrarySidebar } from "./LibrarySidebar";
import { ModelDetailsPanel } from "./ModelDetailsPanel";
import { ModelGrid } from "./ModelGrid";
import { ModelList } from "./ModelList";
import styles from "./LibraryWorkspace.module.css";

export interface LibraryWorkspaceProps {
  /** App's navigation, so a selection is checked against every available
   *  id and lands in the URL fragment. */
  navigate: (target: NavigationTarget) => void;
  /** Opens the import flow. */
  onImport?: () => void;
}

type LayoutMode = "grid" | "list";
type SortOrder = "name" | "recent";

const MODE_KEY = "farm3d:library-mode";
const VIEW_KEY = "farm3d:library-view";
const ALL_MODELS: LibraryView = { kind: "view", id: "all" };
/** Below this window width the details panel becomes an overlay (the
 *  umbrella's dock rule: three panes at 1440 × 900, an overlay at 1024). */
const INLINE_DETAILS_MIN_WIDTH = 1280;
const SORT_LABELS: Record<SortOrder, string> = { name: "Name", recent: "Recently added" };
const WEB_IMPORT_REASON = "Importing Models needs the desktop app.";

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
  const now = () => new Date();

  onMount(() => {
    const onResize = () => setNarrow(window.innerWidth < INLINE_DETAILS_MIN_WIDTH);
    window.addEventListener("resize", onResize);
    onCleanup(() => window.removeEventListener("resize", onResize));
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
  // switches to All Models. Re-checked when the Library loads, so a cold
  // launch lands on the right view.
  createEffect(on([() => libraryTarget()?.selection, () => library.projects(), selectedModel], ([selection]) => {
    if (selection?.kind === "project") {
      if (library.projects().some((project) => project.id === selection.id)) {
        setView({ kind: "project", id: selection.id });
      }
      return;
    }
    const model = selectedModel();
    if (model && !modelsFor(activeView(), [model], now()).length) setView(ALL_MODELS);
  }));

  const inView = createMemo(() => modelsFor(activeView(), library.models(), now()));
  const shown = createMemo(() => sortModels(searchModels(inView(), library.projects(), search()), sort()));

  const selectView = (next: LibraryView) => {
    setView(next);
    props.navigate(next.kind === "project"
      ? { version: 1, destination: "library", selection: { kind: "project", id: next.id } }
      : { version: 1, destination: "library" });
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

  const details = () => {
    const model = selectedModel();
    return model ? (
      <ModelDetailsPanel
        model={model}
        projects={library.projects()}
        onConvertToManaged={(id) => void convertToManaged(id).catch(reportLibraryError)}
        focusRequest={focusRequest()}
        onFocusHandled={() => setFocusRequest(0)}
      />
    ) : (
      <p class={styles.detailsEmpty}>Select a Model to see its details.</p>
    );
  };

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
        <Show when={storedCopies()}>{(text) => <span class={styles.status}>{text()}</span>}</Show>
        <Show when={narrow()}>
          <Button variant="ghost" aria-expanded={detailsOpen()} onClick={() => setDetailsOpen((open) => !open)}>
            Details
          </Button>
        </Show>
      </div>
      <div class={styles.body}>
        <LibrarySidebar
          view={activeView()}
          projects={library.projects()}
          models={library.models()}
          now={now()}
          onSelectView={selectView}
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
                    active={false}
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
      </div>
    </div>
  );
}
