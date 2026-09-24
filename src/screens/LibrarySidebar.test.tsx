import { cleanup, fireEvent, render, screen, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { LibrarySidebar } from "./LibrarySidebar";
import { buildWebLibraryFixture } from "../library/web-fixtures";

const NOW = new Date("2026-09-24T12:00:00Z");
const fixture = buildWebLibraryFixture(NOW);

afterEach(cleanup);

function renderSidebar(overrides: Partial<Parameters<typeof LibrarySidebar>[0]> = {}) {
  const props = {
    view: { kind: "view", id: "all" } as const,
    // Deliberately not alphabetical: the sidebar sorts them.
    projects: [...fixture.projects].reverse(),
    models: fixture.models,
    now: NOW,
    onSelectView: vi.fn(),
    onRenameProject: vi.fn(),
    onDeleteProject: vi.fn(),
    ...overrides,
  };
  render(() => <LibrarySidebar {...props} />);
  return props;
}

describe("LibrarySidebar", () => {
  it("lists the five views, then the Projects alphabetically, each with a count", () => {
    renderSidebar();
    const nav = screen.getByRole("navigation", { name: "Library" });
    const entries = within(nav)
      .getAllByRole("button")
      .filter((button) => button.dataset.entry !== undefined)
      .map((button) => button.textContent);
    expect(entries).toEqual([
      "All Models5",
      "Unfiled1",
      "Recently added2",
      "Needs attention1",
      "Pre-sliced G-code1",
      "Brackets3",
      "Calibration3",
    ]);
  });

  it("marks only the active item with aria-current=page", () => {
    renderSidebar({ view: { kind: "project", id: "prj-web-calibration" } });
    const current = screen
      .getAllByRole("button")
      .filter((button) => button.getAttribute("aria-current") === "page");
    expect(current).toHaveLength(1);
    expect(current[0]).toHaveTextContent("Calibration");
  });

  it("selects a saved view or a Project", async () => {
    const props = renderSidebar();
    await fireEvent.click(screen.getByRole("button", { name: /^Unfiled/ }));
    expect(props.onSelectView).toHaveBeenLastCalledWith({ kind: "view", id: "unfiled" });
    await fireEvent.click(screen.getByRole("button", { name: /^Brackets/ }));
    expect(props.onSelectView).toHaveBeenLastCalledWith({ kind: "project", id: "prj-web-brackets" });
  });

  it("offers enabled Rename… and Delete… in each Project row's menu", async () => {
    const props = renderSidebar();
    await fireEvent.pointerDown(screen.getByLabelText("Actions for Brackets"), { pointerType: "mouse", button: 0 });
    expect(await screen.findByText("Rename…")).toBeInTheDocument();
    for (const item of ["Rename…", "Delete…"]) {
      expect(screen.getByText(item).closest("[role='menuitem']")).not.toHaveAttribute("aria-disabled", "true");
    }
    await fireEvent.pointerUp(screen.getByText("Rename…"), { pointerType: "mouse", button: 0 });
    expect(props.onRenameProject).toHaveBeenCalledWith("prj-web-brackets");

    await fireEvent.pointerDown(screen.getByLabelText("Actions for Calibration"), { pointerType: "mouse", button: 0 });
    await fireEvent.pointerUp(await screen.findByText("Delete…"), { pointerType: "mouse", button: 0 });
    expect(props.onDeleteProject).toHaveBeenCalledWith("prj-web-calibration");
  });
});
