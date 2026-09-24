import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CreateProjectDialog, DeleteProjectDialog, RenameProjectDialog } from "./ProjectDialogs";
import { buildWebLibraryFixture } from "../library/web-fixtures";
import { libraryStoreMock, resetLibraryStoreMock, setLibraryState } from "../library/library-store-mock";
import { model, project } from "../library/test-records";

vi.mock("../library/library-store", async () => (await import("../library/library-store-mock")).libraryStoreMock);

const fixture = buildWebLibraryFixture(new Date("2026-09-24T12:00:00Z"));
const BRACKETS = fixture.projects.find((p) => p.name === "Brackets")!;

const VALIDATION = {
  contractVersion: 1, code: "VALIDATION", message: "A Project with this name already exists.",
  recovery: ["EDIT_FIELDS"], retryable: false, details: { fieldPath: "name" },
};

beforeEach(() => {
  resetLibraryStoreMock();
  setLibraryState({ projects: fixture.projects, models: fixture.models });
});
afterEach(cleanup);

describe("CreateProjectDialog", () => {
  it("creates the named Project", async () => {
    const onCreated = vi.fn();
    const onClose = vi.fn();
    render(() => <CreateProjectDialog onClose={onClose} onCreated={onCreated} />);
    await fireEvent.input(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Knobs" } });
    await fireEvent.click(screen.getByRole("button", { name: "Create Project" }));
    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(expect.objectContaining({ id: "prj-new", name: "Knobs" })));
    expect(libraryStoreMock.createProject).toHaveBeenCalledWith("Knobs");
  });

  it("shows VALIDATION inline on the name field", async () => {
    libraryStoreMock.createProject.mockRejectedValueOnce(VALIDATION);
    const onCreated = vi.fn();
    render(() => <CreateProjectDialog onClose={vi.fn()} onCreated={onCreated} />);
    const name = screen.getByRole("textbox", { name: "Name" });
    await fireEvent.input(name, { target: { value: "brackets" } });
    await fireEvent.click(screen.getByRole("button", { name: "Create Project" }));
    const message = await screen.findByText("A Project with this name already exists.");
    expect(name).toHaveAttribute("aria-invalid", "true");
    expect(name.getAttribute("aria-describedby")?.split(" ")).toContain(message.id);
    expect(onCreated).not.toHaveBeenCalled();
    expect(libraryStoreMock.reportLibraryError).not.toHaveBeenCalled();
  });
});

describe("RenameProjectDialog", () => {
  it("starts from the current name and renames", async () => {
    const onClose = vi.fn();
    render(() => <RenameProjectDialog project={BRACKETS} onClose={onClose} />);
    const name = screen.getByRole("textbox", { name: "Name" });
    expect(name).toHaveValue("Brackets");
    await fireEvent.input(name, { target: { value: "Wall brackets" } });
    await fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    expect(libraryStoreMock.renameProject).toHaveBeenCalledWith(BRACKETS.id, "Wall brackets");
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  });

  it("shows VALIDATION inline on the name field", async () => {
    libraryStoreMock.renameProject.mockRejectedValueOnce(VALIDATION);
    const onClose = vi.fn();
    render(() => <RenameProjectDialog project={BRACKETS} onClose={onClose} />);
    const name = screen.getByRole("textbox", { name: "Name" });
    await fireEvent.input(name, { target: { value: "Calibration" } });
    await fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    expect(await screen.findByText("A Project with this name already exists.")).toBeInTheDocument();
    expect(name).toHaveAttribute("aria-invalid", "true");
    expect(onClose).not.toHaveBeenCalled();
  });
});

describe("DeleteProjectDialog", () => {
  it("states that the Models stay, counting those that become Unfiled from projectIds, then deletes", async () => {
    const onDeleted = vi.fn();
    render(() => <DeleteProjectDialog project={BRACKETS} onClose={vi.fn()} onDeleted={onDeleted} />);
    expect(screen.getByRole("dialog", { name: "Delete Project" }))
      .toHaveTextContent("Delete Brackets? Its 3 Models stay in the Library. 1 of them will become Unfiled.");
    await fireEvent.click(screen.getByRole("button", { name: "Delete Project" }));
    expect(libraryStoreMock.deleteProject).toHaveBeenCalledWith(BRACKETS.id);
    await waitFor(() => expect(onDeleted).toHaveBeenCalledWith(BRACKETS.id));
    expect(libraryStoreMock.deleteModel).not.toHaveBeenCalled();
  });

  it("words a Project with one Model, and one with none", () => {
    const solo = project({ id: "prj-solo", name: "Solo" });
    setLibraryState({ projects: [solo], models: [model({ id: "mdl-a", projectIds: ["prj-solo"] })] });
    render(() => <DeleteProjectDialog project={solo} onClose={vi.fn()} onDeleted={vi.fn()} />);
    expect(screen.getByRole("dialog")).toHaveTextContent("Delete Solo? Its 1 Model stays in the Library. It will become Unfiled.");
    cleanup();

    setLibraryState({ models: [] });
    render(() => <DeleteProjectDialog project={solo} onClose={vi.fn()} onDeleted={vi.fn()} />);
    expect(screen.getByRole("dialog")).toHaveTextContent("Delete Solo? It has no Models.");
  });

  it("shows a failure inline", async () => {
    libraryStoreMock.deleteProject.mockRejectedValueOnce({
      contractVersion: 1, code: "NOT_FOUND", message: "This Project no longer exists.", recovery: [], retryable: false,
    });
    const onDeleted = vi.fn();
    render(() => <DeleteProjectDialog project={BRACKETS} onClose={vi.fn()} onDeleted={onDeleted} />);
    await fireEvent.click(screen.getByRole("button", { name: "Delete Project" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("This Project no longer exists.");
    expect(onDeleted).not.toHaveBeenCalled();
  });
});
