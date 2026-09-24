import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ModelDetailsPanel } from "./ModelDetailsPanel";
import { buildWebLibraryFixture } from "../library/web-fixtures";
import { libraryStoreMock, resetLibraryStoreMock } from "../library/library-store-mock";
import type { ModelRecord } from "../library/types";

vi.mock("../library/library-store", async () => (await import("../library/library-store-mock")).libraryStoreMock);

const NOW = new Date("2026-09-24T12:00:00Z");
const fixture = buildWebLibraryFixture(NOW);
const BRACKETS = "prj-web-brackets";
const CALIBRATION = "prj-web-calibration";

function fixtureModel(id: string): ModelRecord {
  return fixture.models.find((m) => m.id === id)!;
}

beforeEach(() => {
  resetLibraryStoreMock();
  libraryStoreMock.loadRevisions.mockImplementation(async (modelId: string) => fixture.revisions[modelId] ?? []);
});
afterEach(cleanup);

function renderPanel(record: ModelRecord, extra: Partial<Parameters<typeof ModelDetailsPanel>[0]> = {}) {
  const props = { model: record, projects: fixture.projects, ...extra };
  render(() => <ModelDetailsPanel {...props} />);
  return props;
}

async function openCombobox() {
  const input = screen.getByRole("combobox", { name: "Add to Project…" });
  await fireEvent.pointerDown(input, { pointerType: "mouse", button: 0 });
  await fireEvent.pointerDown(screen.getByRole("button", { name: /show suggestions/i }), { pointerType: "mouse", button: 0 });
}

async function pickOption(name: string) {
  const option = await screen.findByRole("option", { name });
  await fireEvent.pointerDown(option, { pointerType: "mouse", button: 0 });
  await fireEvent.pointerUp(option, { pointerType: "mouse", button: 0 });
}

describe("ModelDetailsPanel", () => {
  it("saves a name edit on blur", async () => {
    renderPanel(fixtureModel("mdl-web-bracket"));
    const name = screen.getByRole("textbox", { name: "Name" });
    await fireEvent.input(name, { target: { value: "Bracket A" } });
    expect(libraryStoreMock.updateModel).not.toHaveBeenCalled();
    await fireEvent.focusOut(name);
    expect(libraryStoreMock.updateModel).toHaveBeenCalledWith("mdl-web-bracket", { name: "Bracket A" });
  });

  it("saves a name edit on Enter, and not an unchanged name", async () => {
    renderPanel(fixtureModel("mdl-web-bracket"));
    const name = screen.getByRole("textbox", { name: "Name" });
    await fireEvent.keyDown(name, { key: "Enter" });
    expect(libraryStoreMock.updateModel).not.toHaveBeenCalled();
    await fireEvent.input(name, { target: { value: "Bracket B" } });
    await fireEvent.keyDown(name, { key: "Enter" });
    expect(libraryStoreMock.updateModel).toHaveBeenCalledWith("mdl-web-bracket", { name: "Bracket B" });
  });

  it("keeps a name edit in progress when an unrelated update arrives, and refetches history only for a new revision", async () => {
    const [record, setRecord] = createSignal(fixtureModel("mdl-web-bracket"));
    render(() => <ModelDetailsPanel model={record()} projects={fixture.projects} />);
    await waitFor(() => expect(libraryStoreMock.loadRevisions).toHaveBeenCalledTimes(1));
    const name = screen.getByRole("textbox", { name: "Name" });
    await fireEvent.input(name, { target: { value: "Half-typed" } });

    setRecord({ ...record(), revision: record().revision + 1, projectIds: [BRACKETS] });
    expect(name).toHaveValue("Half-typed");
    expect(libraryStoreMock.loadRevisions).toHaveBeenCalledTimes(1);

    setRecord({ ...record(), revision: record().revision + 1, name: "Renamed elsewhere" });
    expect(name).toHaveValue("Renamed elsewhere");

    setRecord({ ...record(), currentRevision: { ...record().currentRevision, id: "msr-web-bracket-2", sequence: 2 } });
    await waitFor(() => expect(libraryStoreMock.loadRevisions).toHaveBeenCalledTimes(2));
  });

  it("shows a rejected name inline", async () => {
    libraryStoreMock.updateModel.mockRejectedValueOnce({
      contractVersion: 1, code: "VALIDATION", message: "The Model name must be 1-255 characters.",
      recovery: ["EDIT_FIELDS"], retryable: false, details: { fieldPath: "name" },
    });
    renderPanel(fixtureModel("mdl-web-bracket"));
    const name = screen.getByRole("textbox", { name: "Name" });
    await fireEvent.input(name, { target: { value: " " } });
    await fireEvent.focusOut(name);
    expect(await screen.findByText("The Model name must be 1-255 characters.")).toBeInTheDocument();
    expect(libraryStoreMock.reportLibraryError).not.toHaveBeenCalled();
  });

  it("lists Projects as removable chips; removing one sends that one remove", async () => {
    renderPanel(fixtureModel("mdl-web-bracket"));
    const projects = screen.getByRole("group", { name: "Projects" });
    const chip = within(projects).getByRole("button", { name: /Brackets/ });
    expect(within(projects).getByRole("button", { name: /Calibration/ })).toBeInTheDocument();
    await fireEvent.click(chip.querySelector("[aria-label='Remove']")!);
    expect(libraryStoreMock.setModelProjects).toHaveBeenCalledWith("mdl-web-bracket", { add: [], remove: [BRACKETS] });
  });

  it("says Unfiled when the Model has no Projects", () => {
    renderPanel(fixtureModel("mdl-web-knob"));
    expect(within(screen.getByRole("group", { name: "Projects" })).getByText("Unfiled")).toBeInTheDocument();
  });

  it("Add to Project… lists only the Projects the Model isn't in, and adds the one picked", async () => {
    renderPanel(fixtureModel("mdl-web-enclosure"));
    await openCombobox();
    expect(await screen.findByRole("option", { name: "Calibration" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Brackets" })).toBeNull();
    expect(screen.getByRole("option", { name: "New Project…" })).toBeInTheDocument();
    await pickOption("Calibration");
    expect(libraryStoreMock.setModelProjects).toHaveBeenCalledWith("mdl-web-enclosure", { add: [CALIBRATION], remove: [] });
  });

  it("New Project… creates the Project, then adds the Model to it", async () => {
    renderPanel(fixtureModel("mdl-web-knob"));
    await openCombobox();
    await pickOption("New Project…");
    const name = await screen.findByRole("textbox", { name: "New Project name" });
    await fireEvent.input(name, { target: { value: "Knobs" } });
    await fireEvent.click(screen.getByRole("button", { name: "Create and add" }));
    await waitFor(() => expect(libraryStoreMock.setModelProjects).toHaveBeenCalledWith("mdl-web-knob", { add: ["prj-new"], remove: [] }));
    expect(libraryStoreMock.createProject).toHaveBeenCalledWith("Knobs");
    expect(libraryStoreMock.createProject.mock.invocationCallOrder[0])
      .toBeLessThan(libraryStoreMock.setModelProjects.mock.invocationCallOrder[0]!);
  });

  it("shows a G-code Model's claims as unverified, with no Slice, Queue, or Dispatch", async () => {
    renderPanel(fixtureModel("mdl-web-cube-gcode"));
    expect(screen.getByText("Pre-sliced G-code. It can be sent to a Printer once G-code handoff is available.")).toBeInTheDocument();
    const claims = await screen.findByRole("region", { name: "What the file says (not verified)" });
    expect(within(claims).getByText("Elegoo Centauri Carbon")).toBeInTheDocument();
    expect(within(claims).getByText("printer_model")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Slice|Queue|Dispatch/ })).toBeNull();
  });

  it("shows a failed history load on a G-code Model without throwing", async () => {
    libraryStoreMock.loadRevisions.mockRejectedValueOnce(new Error("storage busy"));
    renderPanel(fixtureModel("mdl-web-cube-gcode"));
    expect(await screen.findByText("The revision history could not load.")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "What the file says (not verified)" })).toHaveTextContent("No claims found.");
  });

  it("shows a 3MF's Not used by farm3d list", async () => {
    renderPanel(fixtureModel("mdl-web-enclosure"));
    const unused = await screen.findByRole("region", { name: "Not used by farm3d" });
    expect(within(unused).getByText("Metadata/Slic3r_PE.config")).toBeInTheDocument();
  });

  it("lists the revision history with sequence, origin, capture time, and short hash", async () => {
    renderPanel(fixtureModel("mdl-web-clip"));
    const history = await screen.findByRole("list", { name: "Revision history" });
    await waitFor(() => expect(within(history).getAllByRole("listitem")).toHaveLength(2));
    expect(within(history).getByText("Revision 2 · Updated from source")).toBeInTheDocument();
    expect(within(history).getByText("Revision 1 · Imported")).toBeInTheDocument();
    expect(within(history).getByText(/c2c2c2c2c2c2/)).toBeInTheDocument();
    expect(history.querySelectorAll("time")).toHaveLength(2);
  });

  it("offers Locate source… and Convert to managed for a missing linked source", async () => {
    const onLocateSource = vi.fn();
    const onConvertToManaged = vi.fn();
    renderPanel(fixtureModel("mdl-web-clip"), { onLocateSource, onConvertToManaged });
    expect(screen.getByRole("status", { name: "Source missing" })).toBeInTheDocument();
    expect(screen.getByText("/home/maker/prints/cable-clip.stl")).toBeInTheDocument();
    await fireEvent.click(screen.getByRole("button", { name: "Locate source…" }));
    expect(onLocateSource).toHaveBeenCalledWith("mdl-web-clip");
    await fireEvent.click(screen.getByRole("button", { name: "Convert to managed" }));
    expect(onConvertToManaged).toHaveBeenCalledWith("mdl-web-clip");
  });

  it("offers neither recovery action for a managed Model", () => {
    renderPanel(fixtureModel("mdl-web-bracket"));
    expect(screen.queryByRole("button", { name: "Locate source…" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Convert to managed" })).toBeNull();
  });

  it("shows the build plate placeholder with the Model's name and approximate size", () => {
    renderPanel(fixtureModel("mdl-web-bracket"));
    const plate = screen.getByRole("figure", { name: "Build plate" });
    expect(plate).toHaveTextContent("Corner bracket");
    expect(plate).toHaveTextContent("40 × 40 × 5 mm");
  });
});
