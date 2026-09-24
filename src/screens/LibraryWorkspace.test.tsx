import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LibraryWorkspace } from "./LibraryWorkspace";
import { buildWebLibraryFixture } from "../library/web-fixtures";
import { libraryStoreMock, resetLibraryStoreMock, setLibraryState } from "../library/library-store-mock";
import { navigation, type NavigationTarget } from "../navigation/navigation-store";
import type { ImportSelectionSummary, ModelRecord } from "../library/types";

vi.mock("../library/library-store", async () => (await import("../library/library-store-mock")).libraryStoreMock);

const desktop = vi.hoisted(() => ({ available: false }));
vi.mock("../ipc/client", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../ipc/client")>()),
  desktopAvailable: () => desktop.available,
}));

const fixture = buildWebLibraryFixture(new Date());
const MODE_KEY = "farm3d:library-mode";
const VIEW_KEY = "farm3d:library-view";

function navigate(target: NavigationTarget) {
  navigation.navigate(target, {
    availableDestinations: ["monitor", "library", "spools"],
    availableIds: [...libraryStoreMock.library.projects().map((p) => p.id), ...libraryStoreMock.library.models().map((m) => m.id)],
  });
}

function setWindowWidth(width: number) {
  Object.defineProperty(window, "innerWidth", { configurable: true, writable: true, value: width });
  window.dispatchEvent(new Event("resize"));
}

beforeEach(() => {
  resetLibraryStoreMock();
  libraryStoreMock.loadRevisions.mockImplementation(async (modelId: string) => fixture.revisions[modelId] ?? []);
  setLibraryState({ projects: fixture.projects, models: fixture.models });
  window.localStorage.clear();
  desktop.available = false;
  setWindowWidth(1440);
  navigate({ version: 1, destination: "library" });
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderWorkspace(props: Partial<Parameters<typeof LibraryWorkspace>[0]> = {}) {
  const onImport = vi.fn();
  const onImportClose = vi.fn();
  render(() => <LibraryWorkspace navigate={navigate} onImport={onImport} onImportClose={onImportClose} {...props} />);
  return { onImport, onImportClose };
}

const SELECTION: ImportSelectionSummary = {
  selectionId: "sel-1",
  purpose: "import",
  files: [{ fileIndex: 0, fileName: "hook.stl", sizeBytes: 684 }],
};

function sidebarEntry(name: RegExp): HTMLElement {
  return within(screen.getByRole("navigation", { name: "Library" })).getByRole("button", { name });
}

describe("LibraryWorkspace", () => {
  it("switches between grid and list and remembers the choice", async () => {
    renderWorkspace();
    expect(screen.getByRole("list", { name: "Models" })).toBeInTheDocument();
    expect(screen.queryByRole("grid", { name: "Models" })).toBeNull();

    await fireEvent.click(screen.getByText("List"));
    expect(screen.getByRole("grid", { name: "Models" })).toBeInTheDocument();
    expect(window.localStorage.getItem(MODE_KEY)).toBe("list");

    cleanup();
    renderWorkspace();
    expect(screen.getByRole("grid", { name: "Models" })).toBeInTheDocument();
  });

  it("still works when localStorage throws", async () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("denied");
    });
    renderWorkspace();
    expect(screen.getByRole("list", { name: "Models" })).toBeInTheDocument();
    await fireEvent.click(screen.getByText("List"));
    expect(screen.getByRole("grid", { name: "Models" })).toBeInTheDocument();
  });

  it("filters by search", async () => {
    renderWorkspace();
    await fireEvent.input(screen.getByRole("searchbox", { name: "Search Models" }), { target: { value: "clip" } });
    const models = screen.getByRole("list", { name: "Models" });
    expect(within(models).getAllByRole("listitem")).toHaveLength(1);
    expect(within(models).getByText("Cable clip")).toBeInTheDocument();
  });

  it("shows Import a Model with the drop surface when the Library is empty", () => {
    setLibraryState({ projects: [], models: [] });
    renderWorkspace();
    expect(screen.getByText("Import a Model")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Import Models" })).toBeInTheDocument();
  });

  it("disables the drop surface in web mode, saying why", () => {
    setLibraryState({ projects: [], models: [] });
    renderWorkspace();
    expect(screen.getByRole("button", { name: "Choose files…" })).toBeDisabled();
    // The toolbar's Import… says so too; this is the surface's own copy.
    const surface = screen.getByRole("region", { name: "Import Models" });
    expect(within(surface).getByText("Importing Models needs the desktop app.")).toBeInTheDocument();
  });

  it("enables the drop surface on the desktop", async () => {
    desktop.available = true;
    setLibraryState({ projects: [], models: [] });
    const { onImport } = renderWorkspace();
    const choose = screen.getByRole("button", { name: "Choose files…" });
    expect(choose).toBeEnabled();
    await fireEvent.click(choose);
    expect(onImport).toHaveBeenCalledOnce();
  });

  it("the toolbar's Import… starts an import on the desktop", async () => {
    desktop.available = true;
    const { onImport } = renderWorkspace();
    await fireEvent.click(screen.getByRole("button", { name: "Import…" }));
    expect(onImport).toHaveBeenCalledOnce();
  });

  it("disables the toolbar's Import… in web mode, saying why", () => {
    renderWorkspace();
    const importButton = screen.getByRole("button", { name: "Import…" });
    expect(importButton).toBeDisabled();
    const reason = screen.getByText("Importing Models needs the desktop app.");
    expect(importButton.getAttribute("aria-describedby")).toBe(reason.id);
  });

  it("highlights the drop surface while a file drag is over the window", () => {
    desktop.available = true;
    setLibraryState({ projects: [], models: [] });
    renderWorkspace({ dropActive: true });
    expect(screen.getByRole("region", { name: "Import Models" })).toHaveAttribute("data-active");
  });

  it("opens the import dialog for a selection, defaulting its rows to the viewed Project", async () => {
    desktop.available = true;
    libraryStoreMock.inspectSelection.mockImplementation(async (selectionId: string) => ({
      selectionId,
      items: [{
        status: "ready", fileIndex: 0, fileName: "hook.stl", format: "stl", sizeBytes: 684, sha256: "e".repeat(64),
        summary: { format: "stl", triangleCount: 12, boundsMm: { min: [0, 0, 0], max: [1, 1, 1] }, unitsAssumed: true },
        unsupported: [], warnings: [], duplicates: [],
      }],
    }));
    window.localStorage.setItem(VIEW_KEY, JSON.stringify({ kind: "project", id: "prj-web-calibration" }));
    renderWorkspace({ importSelection: SELECTION });

    const dialog = await screen.findByRole("dialog", { name: "Import Models" });
    expect(libraryStoreMock.inspectSelection).toHaveBeenCalledWith("sel-1");
    expect(await within(dialog).findByRole("button", { name: "Remove Calibration" })).toBeInTheDocument();
  });

  it("Done closes the import and selects the first imported Model", async () => {
    desktop.available = true;
    libraryStoreMock.inspectSelection.mockImplementation(async (selectionId: string) => ({
      selectionId,
      items: [{
        status: "ready", fileIndex: 0, fileName: "hook.stl", format: "stl", sizeBytes: 684, sha256: "e".repeat(64),
        summary: { format: "stl", triangleCount: 12, boundsMm: { min: [0, 0, 0], max: [1, 1, 1] }, unitsAssumed: true },
        unsupported: [], warnings: [], duplicates: [],
      }],
    }));
    libraryStoreMock.importModels.mockImplementation(async () => ({
      items: [{ fileIndex: 0, outcome: "imported", model: fixture.models[1]!, errors: [], warnings: [] }],
    }));
    const { onImportClose } = renderWorkspace({ importSelection: SELECTION });

    await fireEvent.click(await screen.findByRole("button", { name: "Import" }));
    await fireEvent.click(await screen.findByRole("button", { name: "Done" }));
    expect(onImportClose).toHaveBeenCalled();
    const target = navigation.target();
    expect(target.destination === "library" && target.selection).toEqual({ kind: "model", id: fixture.models[1]!.id });
  });

  it("shows a filtered-empty view with Show all Models", async () => {
    setLibraryState({ models: fixture.models.filter((m) => m.projectIds.length > 0) });
    renderWorkspace();
    await fireEvent.click(sidebarEntry(/^Unfiled/));
    expect(screen.getByText("No Models in Unfiled")).toBeInTheDocument();
    await fireEvent.click(screen.getByRole("button", { name: "Show all Models" }));
    expect(sidebarEntry(/^All Models/)).toHaveAttribute("aria-current", "page");
    expect(within(screen.getByRole("list", { name: "Models" })).getAllByRole("listitem")).toHaveLength(4);
  });

  it("warns when the Library may be out of date and offers Refresh", async () => {
    setLibraryState({ syncState: "uncertain" });
    renderWorkspace();
    expect(screen.getByText("Library may be out of date")).toBeInTheDocument();
    await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(libraryStoreMock.refreshLibrary).toHaveBeenCalledOnce();
  });

  it("shows a reported Library error with Dismiss", async () => {
    setLibraryState({ error: "This Model no longer exists." });
    renderWorkspace();
    expect(screen.getByRole("alert")).toHaveTextContent("This Model no longer exists.");
    await fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(libraryStoreMock.dismissLibraryError).toHaveBeenCalledOnce();
  });

  it("selecting a card navigates to that Model and shows its details", async () => {
    renderWorkspace();
    await fireEvent.click(screen.getByRole("button", { name: /^Spare knob/ }));
    expect(navigation.target()).toEqual({ version: 1, destination: "library", selection: { kind: "model", id: "mdl-web-knob" } });
    const details = screen.getByRole("complementary", { name: "Model details" });
    expect(within(details).getByRole("textbox", { name: "Name" })).toHaveValue("Spare knob");
  });

  it("a card's Add to Project… selects the Model and focuses the details panel's picker", async () => {
    renderWorkspace();
    await fireEvent.pointerDown(screen.getByLabelText("Actions for Spare knob"), { pointerType: "mouse", button: 0 });
    await fireEvent.pointerUp(await screen.findByText("Add to Project…"), { pointerType: "mouse", button: 0 });
    expect(navigation.target().selection).toEqual({ kind: "model", id: "mdl-web-knob" });
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Add to Project…" })).toHaveFocus());
  });

  it("keeps the same details panel, draft and history when the selected record changes", async () => {
    navigate({ version: 1, destination: "library", selection: { kind: "model", id: "mdl-web-bracket" } });
    renderWorkspace();
    await waitFor(() => expect(libraryStoreMock.loadRevisions).toHaveBeenCalledTimes(1));
    const name = screen.getByRole("textbox", { name: "Name" });
    await fireEvent.input(name, { target: { value: "Half" } });

    setLibraryState({
      models: fixture.models.map((m) => (m.id === "mdl-web-bracket"
        ? { ...m, revision: m.revision + 1, projectIds: ["prj-web-brackets"] }
        : m)),
    });

    expect(screen.getByRole("textbox", { name: "Name" })).toBe(name);
    expect(name).toHaveValue("Half");
    expect(libraryStoreMock.loadRevisions).toHaveBeenCalledTimes(1);
  });

  it("removing the viewed Project from the selected Model keeps the view", async () => {
    libraryStoreMock.setModelProjects.mockImplementationOnce(async (id, change) => {
      setLibraryState({
        models: libraryStoreMock.library.models().map((m) => (m.id === id
          ? { ...m, revision: m.revision + 1, projectIds: m.projectIds.filter((p) => !change.remove.includes(p)) }
          : m)),
      });
    });
    renderWorkspace();
    await fireEvent.click(sidebarEntry(/^Brackets/));
    await fireEvent.click(screen.getByRole("button", { name: /^Enclosure lid/ }));
    const projects = within(screen.getByRole("complementary", { name: "Model details" })).getByRole("group", { name: "Projects" });
    await fireEvent.click(within(projects).getByRole("button", { name: "Remove Brackets" }));

    await waitFor(() => expect(libraryStoreMock.library.models().find((m) => m.id === "mdl-web-enclosure")?.projectIds).toEqual([]));
    expect(sidebarEntry(/^Brackets/)).toHaveAttribute("aria-current", "page");
  });

  it("a Project deep link opens that Project's view", () => {
    navigate({ version: 1, destination: "library", selection: { kind: "project", id: "prj-web-calibration" } });
    renderWorkspace();
    expect(sidebarEntry(/^Calibration/)).toHaveAttribute("aria-current", "page");
  });

  it("a Model deep link keeps the current view when it contains the Model", () => {
    window.localStorage.setItem(VIEW_KEY, JSON.stringify({ kind: "project", id: "prj-web-brackets" }));
    navigate({ version: 1, destination: "library", selection: { kind: "model", id: "mdl-web-enclosure" } });
    renderWorkspace();
    expect(sidebarEntry(/^Brackets/)).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("button", { name: /^Enclosure lid/ })).toHaveAttribute("aria-pressed", "true");
  });

  it("a Model deep link switches to All Models when the view doesn't contain it", () => {
    window.localStorage.setItem(VIEW_KEY, JSON.stringify({ kind: "project", id: "prj-web-brackets" }));
    navigate({ version: 1, destination: "library", selection: { kind: "model", id: "mdl-web-knob" } });
    renderWorkspace();
    expect(sidebarEntry(/^All Models/)).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("button", { name: /^Spare knob/ })).toHaveAttribute("aria-pressed", "true");
  });

  it("renders none of the old placeholder Models or the Slice, Dispatch, and Target printer controls", () => {
    navigate({ version: 1, destination: "library", selection: { kind: "model", id: "mdl-web-bracket" } });
    renderWorkspace();
    expect(screen.queryByText("Benchy_v3.gcode")).toBeNull();
    expect(screen.queryByText("mount_bracket.stl")).toBeNull();
    expect(screen.queryByRole("button", { name: /Slice|Dispatch/ })).toBeNull();
    expect(screen.queryByText(/Target printer/i)).toBeNull();
    expect(screen.getByRole("complementary", { name: "Model details" })).toBeInTheDocument();
  });

  it("at 1024 wide, the details panel becomes an overlay toggled by Details", async () => {
    navigate({ version: 1, destination: "library", selection: { kind: "model", id: "mdl-web-bracket" } });
    renderWorkspace();
    expect(screen.queryByRole("button", { name: "Details" })).toBeNull();
    expect(screen.getByRole("complementary", { name: "Model details" })).toBeInTheDocument();

    setWindowWidth(1024);
    const toggle = await screen.findByRole("button", { name: "Details" });
    expect(screen.queryByRole("complementary", { name: "Model details" })).toBeNull();
    expect(screen.queryByRole("dialog", { name: "Model details" })).toBeNull();

    await fireEvent.click(toggle);
    const dialog = await screen.findByRole("dialog", { name: "Model details" });
    expect(within(dialog).getByRole("textbox", { name: "Name" })).toHaveValue("Corner bracket");

    await fireEvent.click(within(dialog).getByRole("button", { name: "Close details" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Model details" })).toBeNull());
  });
});

function details(): HTMLElement {
  return screen.getByRole("complementary", { name: "Model details" });
}

function selectModel(id: string) {
  navigate({ version: 1, destination: "library", selection: { kind: "model", id } });
}

function updateModel(id: string, change: (model: ModelRecord) => ModelRecord) {
  setLibraryState({ models: libraryStoreMock.library.models().map((m) => (m.id === id ? change(m) : m)) });
}

async function openProjectMenu(name: string, item: string) {
  await fireEvent.pointerDown(screen.getByLabelText(`Actions for ${name}`), { pointerType: "mouse", button: 0 });
  await fireEvent.pointerUp(await screen.findByText(item), { pointerType: "mouse", button: 0 });
}

describe("LibraryWorkspace recovery", () => {
  it("Locate source… opens the dialog; a successful locate closes it and the panel shows the source OK", async () => {
    desktop.available = true;
    libraryStoreMock.pickFiles.mockResolvedValue({
      selectionId: "sel-locate", purpose: "locate", files: [{ fileIndex: 0, fileName: "cable-clip.stl", sizeBytes: 684 }],
    });
    libraryStoreMock.locateSource.mockImplementation(async (modelId) => {
      updateModel(modelId, (m) => ({ ...m, revision: m.revision + 1, link: { ...m.link!, state: "ok" } }));
      return libraryStoreMock.library.models().find((m) => m.id === modelId)!;
    });
    selectModel("mdl-web-clip");
    renderWorkspace();
    expect(within(details()).getByRole("status", { name: "Source missing" })).toBeInTheDocument();

    await fireEvent.click(within(details()).getByRole("button", { name: "Locate source…" }));
    const dialog = await screen.findByRole("dialog", { name: "Locate source" });
    await fireEvent.click(within(dialog).getByRole("button", { name: "Choose file…" }));

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Locate source" })).toBeNull());
    expect(libraryStoreMock.locateSource).toHaveBeenCalledWith("mdl-web-clip", "sel-locate", 0, false);
    expect(within(details()).getByRole("status", { name: "Source OK" })).toBeInTheDocument();
  });

  it("Convert to managed asks first, and converts only on confirm", async () => {
    desktop.available = true;
    selectModel("mdl-web-clip");
    renderWorkspace();
    await fireEvent.click(within(details()).getByRole("button", { name: "Convert to managed" }));
    const dialog = await screen.findByRole("dialog", { name: "Convert to managed" });
    expect(dialog).toHaveTextContent("farm3d will stop following cable-clip.stl. Your existing revisions are kept.");
    expect(libraryStoreMock.convertToManaged).not.toHaveBeenCalled();

    await fireEvent.click(within(dialog).getByRole("button", { name: "Convert to managed" }));
    expect(libraryStoreMock.convertToManaged).toHaveBeenCalledWith("mdl-web-clip");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Convert to managed" })).toBeNull());
  });
});

describe("LibraryWorkspace Projects and deletion", () => {
  it("New Project creates a Project and opens its view", async () => {
    libraryStoreMock.createProject.mockImplementation(async (name: string) => {
      const created = { id: "prj-new", revision: 1, name, modelCount: 0, createdAt: "", updatedAt: "" };
      setLibraryState({ projects: [...libraryStoreMock.library.projects(), created] });
      return created;
    });
    renderWorkspace();
    await fireEvent.click(screen.getByRole("button", { name: "New Project" }));
    const dialog = await screen.findByRole("dialog", { name: "New Project" });
    await fireEvent.input(within(dialog).getByRole("textbox", { name: "Name" }), { target: { value: "Knobs" } });
    await fireEvent.click(within(dialog).getByRole("button", { name: "Create Project" }));

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "New Project" })).toBeNull());
    expect(sidebarEntry(/^Knobs/)).toHaveAttribute("aria-current", "page");
  });

  it("a Project's Rename… opens the rename dialog for it", async () => {
    renderWorkspace();
    await openProjectMenu("Calibration", "Rename…");
    const dialog = await screen.findByRole("dialog", { name: "Rename Project" });
    expect(within(dialog).getByRole("textbox", { name: "Name" })).toHaveValue("Calibration");
    await fireEvent.input(within(dialog).getByRole("textbox", { name: "Name" }), { target: { value: "Tuning" } });
    await fireEvent.click(within(dialog).getByRole("button", { name: "Rename" }));
    expect(libraryStoreMock.renameProject).toHaveBeenCalledWith("prj-web-calibration", "Tuning");
  });

  it("deleting the viewed Project keeps every Model and switches to All Models", async () => {
    libraryStoreMock.deleteProject.mockImplementation(async (id: string) => {
      setLibraryState({
        projects: libraryStoreMock.library.projects().filter((p) => p.id !== id),
        models: libraryStoreMock.library.models().map((m) => ({ ...m, projectIds: m.projectIds.filter((p) => p !== id) })),
      });
    });
    renderWorkspace();
    await fireEvent.click(sidebarEntry(/^Brackets/));
    await openProjectMenu("Brackets", "Delete…");
    const dialog = await screen.findByRole("dialog", { name: "Delete Project" });
    expect(dialog).toHaveTextContent("Delete Brackets? Its 3 Models stay in the Library. 1 of them will become Unfiled.");
    await fireEvent.click(within(dialog).getByRole("button", { name: "Delete Project" }));

    await waitFor(() => expect(navigation.target()).toEqual({ version: 1, destination: "library" }));
    expect(sidebarEntry(/^All Models/)).toHaveAttribute("aria-current", "page");
    expect(screen.queryByRole("dialog", { name: "Delete Project" })).toBeNull();
    expect(libraryStoreMock.library.models()).toHaveLength(fixture.models.length);
    expect(libraryStoreMock.deleteModel).not.toHaveBeenCalled();
    expect(within(screen.getByRole("list", { name: "Models" })).getAllByRole("listitem")).toHaveLength(fixture.models.length);
  });

  it("deleting another Project keeps the current view", async () => {
    libraryStoreMock.deleteProject.mockImplementation(async (id: string) => {
      setLibraryState({ projects: libraryStoreMock.library.projects().filter((p) => p.id !== id) });
    });
    renderWorkspace();
    await fireEvent.click(sidebarEntry(/^Brackets/));
    await openProjectMenu("Calibration", "Delete…");
    await fireEvent.click(within(await screen.findByRole("dialog", { name: "Delete Project" })).getByRole("button", { name: "Delete Project" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Delete Project" })).toBeNull());
    expect(sidebarEntry(/^Brackets/)).toHaveAttribute("aria-current", "page");
  });

  it("Delete… on a Model deletes it after confirmation and clears the selection", async () => {
    libraryStoreMock.deleteModel.mockImplementation(async (id: string) => {
      setLibraryState({ models: libraryStoreMock.library.models().filter((m) => m.id !== id) });
    });
    selectModel("mdl-web-knob");
    renderWorkspace();
    await fireEvent.click(within(details()).getByRole("button", { name: "Delete…" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete Model" });
    expect(dialog).toHaveTextContent("Delete Spare knob? Its imported revisions are deleted. The original file on disk is not.");
    await fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Delete Model" })).toBeNull());
    expect(libraryStoreMock.deleteModel).toHaveBeenCalledWith("mdl-web-knob");
    expect(navigation.target()).toEqual({ version: 1, destination: "library" });
    expect(within(details()).getByText("Select a Model to see its details.")).toBeInTheDocument();
  });
});

describe("LibraryWorkspace source checks", () => {
  it("checks linked sources on mount and when the window regains focus", async () => {
    desktop.available = true;
    renderWorkspace();
    expect(libraryStoreMock.checkSources).toHaveBeenCalledTimes(1);
    expect(libraryStoreMock.checkSources).toHaveBeenLastCalledWith();
    window.dispatchEvent(new Event("focus"));
    expect(libraryStoreMock.checkSources).toHaveBeenCalledTimes(2);
    expect(libraryStoreMock.checkSources).toHaveBeenLastCalledWith();
  });

  it("Check sources forces a check and says when it ran", async () => {
    desktop.available = true;
    renderWorkspace();
    expect(screen.queryByText("Checked just now")).toBeNull();
    await fireEvent.click(screen.getByRole("button", { name: "Check sources" }));
    expect(libraryStoreMock.checkSources).toHaveBeenLastCalledWith(undefined, { force: true });
    expect(await screen.findByText("Checked just now")).toBeInTheDocument();
  });

  it("sends a failed check to the Library banner", async () => {
    desktop.available = true;
    const failure = { contractVersion: 1, code: "INTERNAL", message: "Something went wrong.", recovery: [], retryable: false };
    libraryStoreMock.checkSources.mockRejectedValue(failure);
    renderWorkspace();
    await fireEvent.click(screen.getByRole("button", { name: "Check sources" }));
    await waitFor(() => expect(libraryStoreMock.reportLibraryError).toHaveBeenCalledWith(failure));
    expect(screen.queryByText("Checked just now")).toBeNull();
  });

  it("never checks in web mode, where Check sources is unavailable", async () => {
    renderWorkspace();
    window.dispatchEvent(new Event("focus"));
    expect(libraryStoreMock.checkSources).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Check sources" })).toBeDisabled();
  });
});
