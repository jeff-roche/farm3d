import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ModelGrid } from "./ModelGrid";
import { buildWebLibraryFixture } from "../library/web-fixtures";
import { modelsFor, type LibraryView } from "../library/saved-views";
import { libraryStoreMock, resetLibraryStoreMock } from "../library/library-store-mock";

vi.mock("../library/library-store", async () => (await import("../library/library-store-mock")).libraryStoreMock);

const NOW = new Date("2026-09-24T12:00:00Z");
const fixture = buildWebLibraryFixture(NOW);
const BRACKETS = "prj-web-brackets";
const CALIBRATION = "prj-web-calibration";

beforeEach(resetLibraryStoreMock);
afterEach(cleanup);

function renderGrid(view: LibraryView = { kind: "view", id: "all" }, selectedId: string | null = null) {
  const props = {
    models: modelsFor(view, fixture.models, NOW),
    projects: fixture.projects,
    view,
    selectedId,
    onSelect: vi.fn(),
    onAddToProject: vi.fn(),
  };
  render(() => <ModelGrid {...props} />);
  return props;
}

function card(name: string): HTMLElement {
  return screen.getByRole("button", { name: new RegExp(`^${name}`) });
}

describe("ModelGrid", () => {
  it("shows the thumbnail once loadThumbnail resolves, and the format icon with text otherwise", async () => {
    libraryStoreMock.loadThumbnail.mockImplementation(async (id: string) => `data:image/png;base64,${id}`);
    renderGrid();

    await waitFor(() => expect(card("Enclosure lid").querySelector("img")).not.toBeNull());
    const image = card("Enclosure lid").querySelector("img")!;
    expect(image).toHaveAttribute("alt", "");
    expect(image).toHaveAttribute("src", "data:image/png;base64,msr-web-enclosure-1");
    expect(libraryStoreMock.loadThumbnail).not.toHaveBeenCalledWith("msr-web-bracket-1");

    const bracket = card("Corner bracket");
    expect(bracket.querySelector("img")).toBeNull();
    expect(within(bracket).getByText("STL", { selector: "[data-format-icon] *" })).toBeInTheDocument();
  });

  it("marks a missing linked source with a SeverityMarker", () => {
    renderGrid();
    expect(within(card("Cable clip")).getByRole("status", { name: "Source missing" })).toBeInTheDocument();
    expect(within(card("Corner bracket")).queryByRole("status", { name: /Source/ })).toBeNull();
  });

  it("cards are native buttons: a click selects, and aria-pressed marks the selected card", async () => {
    const props = renderGrid({ kind: "view", id: "all" }, "mdl-web-knob");
    expect(card("Corner bracket").tagName).toBe("BUTTON");
    expect(card("Spare knob")).toHaveAttribute("aria-pressed", "true");
    expect(card("Corner bracket")).toHaveAttribute("aria-pressed", "false");

    await fireEvent.click(card("Corner bracket"));
    expect(props.onSelect).toHaveBeenCalledWith("mdl-web-bracket");
  });

  it("shows a Model in every Project it belongs to", () => {
    renderGrid({ kind: "project", id: BRACKETS });
    expect(card("Corner bracket")).toBeInTheDocument();
    cleanup();
    renderGrid({ kind: "project", id: CALIBRATION });
    expect(card("Corner bracket")).toBeInTheDocument();
  });

  it("offers Add to Project…, and no Remove entry outside a Project view", async () => {
    const props = renderGrid();
    await fireEvent.pointerDown(screen.getByLabelText("Actions for Corner bracket"), { pointerType: "mouse", button: 0 });
    const add = await screen.findByText("Add to Project…");
    expect(screen.queryByText(/^Remove from/)).toBeNull();
    await fireEvent.pointerUp(add, { pointerType: "mouse", button: 0 });
    expect(props.onAddToProject).toHaveBeenCalledWith("mdl-web-bracket");
  });

  it("inside a Project view, Remove from <Project> removes only that membership", async () => {
    renderGrid({ kind: "project", id: CALIBRATION });
    await fireEvent.pointerDown(screen.getByLabelText("Actions for Corner bracket"), { pointerType: "mouse", button: 0 });
    await fireEvent.pointerUp(await screen.findByText("Remove from Calibration"), { pointerType: "mouse", button: 0 });
    expect(libraryStoreMock.setModelProjects).toHaveBeenCalledWith("mdl-web-bracket", { add: [], remove: [CALIBRATION] });
  });

  it("routes a failed removal to the Library banner", async () => {
    const failure = new Error("nope");
    libraryStoreMock.setModelProjects.mockRejectedValueOnce(failure);
    renderGrid({ kind: "project", id: BRACKETS });
    await fireEvent.pointerDown(screen.getByLabelText("Actions for Corner bracket"), { pointerType: "mouse", button: 0 });
    await fireEvent.pointerUp(await screen.findByText("Remove from Brackets"), { pointerType: "mouse", button: 0 });
    await waitFor(() => expect(libraryStoreMock.reportLibraryError).toHaveBeenCalledWith(failure));
  });
});
