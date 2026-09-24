import { cleanup, fireEvent, render, screen, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ModelList } from "./ModelList";
import { buildWebLibraryFixture } from "../library/web-fixtures";
import { sortModels, type LibraryView } from "../library/saved-views";
import { model, project } from "../library/test-records";
import { libraryStoreMock, resetLibraryStoreMock } from "../library/library-store-mock";
import type { ModelRecord, ProjectRecord } from "../library/types";

vi.mock("../library/library-store", async () => (await import("../library/library-store-mock")).libraryStoreMock);

const NOW = new Date("2026-09-24T12:00:00Z");
const fixture = buildWebLibraryFixture(NOW);

beforeEach(resetLibraryStoreMock);
afterEach(cleanup);

function renderList(options: {
  models?: ModelRecord[];
  projects?: ProjectRecord[];
  view?: LibraryView;
  selectedId?: string | null;
} = {}) {
  const props = {
    models: options.models ?? sortModels(fixture.models, "name"),
    projects: options.projects ?? fixture.projects,
    view: options.view ?? ({ kind: "view", id: "all" } as const),
    selectedId: options.selectedId ?? null,
    onSelect: vi.fn(),
    onAddToProject: vi.fn(),
  };
  render(() => <ModelList {...props} />);
  return props;
}

describe("ModelList", () => {
  it("shows the Name, Projects, Format, Storage, Source, Revisions and Added columns", () => {
    renderList();
    const headers = screen.getAllByRole("columnheader").map((header) => header.textContent);
    expect(headers.slice(0, 7)).toEqual(["Name", "Projects", "Format", "Storage", "Source", "Revisions", "Added"]);
  });

  it("renders a row per Model with its storage, source state, and revision count", () => {
    renderList();
    const row = screen.getByRole("row", { name: /Cable clip/ });
    expect(within(row).getByText("Calibration")).toBeInTheDocument();
    expect(within(row).getByText("STL")).toBeInTheDocument();
    expect(within(row).getByText("Linked")).toBeInTheDocument();
    expect(within(row).getByRole("status", { name: "Source missing" })).toBeInTheDocument();
    expect(within(row).getByText("2")).toBeInTheDocument();
  });

  it("joins Project names and counts the rest past two as +N", () => {
    const projects = [
      project({ id: "p-a", name: "Alpha" }),
      project({ id: "p-b", name: "Beta" }),
      project({ id: "p-c", name: "Gamma" }),
    ];
    renderList({ projects, models: [model({ id: "m-1", name: "Widget", projectIds: ["p-a", "p-b", "p-c"] })] });
    const row = screen.getByRole("row", { name: /Widget/ });
    expect(within(row).getByText("Alpha, Beta +1")).toBeInTheDocument();
  });

  it("selects rows from the keyboard through DataTable", async () => {
    const props = renderList();
    const grid = screen.getByRole("grid", { name: "Models" });
    await fireEvent.keyDown(grid, { key: "ArrowDown" });
    expect(props.onSelect).toHaveBeenCalledWith("mdl-web-clip");
  });

  it("offers the same row menu as the grid", async () => {
    renderList({ view: { kind: "project", id: "prj-web-brackets" } });
    await fireEvent.pointerDown(screen.getByLabelText("Actions for Enclosure lid"), { pointerType: "mouse", button: 0 });
    expect(await screen.findByText("Add to Project…")).toBeInTheDocument();
    await fireEvent.pointerUp(screen.getByText("Remove from Brackets"), { pointerType: "mouse", button: 0 });
    expect(libraryStoreMock.setModelProjects).toHaveBeenCalledWith("mdl-web-enclosure", { add: [], remove: ["prj-web-brackets"] });
  });
});
