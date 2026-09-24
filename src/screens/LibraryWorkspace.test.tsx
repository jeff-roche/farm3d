import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LibraryWorkspace } from "./LibraryWorkspace";
import { buildWebLibraryFixture } from "../library/web-fixtures";
import { libraryStoreMock, resetLibraryStoreMock, setLibraryState } from "../library/library-store-mock";
import { navigation, type NavigationTarget } from "../navigation/navigation-store";
import type { ImportSelectionSummary } from "../library/types";

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
    await fireEvent.click(within(projects).getByRole("button", { name: /Brackets/ }).querySelector("[aria-label='Remove']")!);

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
