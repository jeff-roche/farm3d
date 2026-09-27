import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { libraryStoreMock, resetLibraryStoreMock } from "../library/library-store-mock";
import { buildWebLibraryFixture } from "../library/web-fixtures";
import type { ModelRecord } from "../library/types";
import {
  loadWebSlicingFixture,
  resetSlicingStoreMock,
  setSlicingState,
  slicingStoreMock,
} from "../slicing/slicing-store-mock";
import {
  WEB_SLICING_REVISION_EXTERNAL,
  WEB_SLICING_REVISION_FARM3D,
  WEB_SLICING_REVISION_FARM3D_OLDER,
} from "../slicing/web-fixtures";
import { ModelDetailsPanel } from "./ModelDetailsPanel";

vi.mock("../library/library-store", async () => (await import("../library/library-store-mock")).libraryStoreMock);
vi.mock("../slicing/slicing-store", async () => (await import("../slicing/slicing-store-mock")).slicingStoreMock);
vi.mock("../printers/printer-store", () => ({ printers: () => [] }));
vi.mock("../host-ops/host-operations-store", async () =>
  (await import("../host-ops/host-operations-store-mock")).hostOperationsStoreMock);
vi.mock("../host-ops/capabilities-store", async () =>
  (await import("../host-ops/capabilities-store-mock")).capabilitiesStoreMock);
// The 3D inspector isn't under test here.
vi.mock("./ModelPlateInspector", () => ({ ModelPlateInspector: () => <p>Inspector</p> }));

const NOW = new Date("2026-09-24T12:00:00Z");
const library = buildWebLibraryFixture(NOW);
const model = (id: string): ModelRecord => library.models.find((candidate) => candidate.id === id)!;

beforeEach(() => {
  resetLibraryStoreMock();
  resetSlicingStoreMock();
  loadWebSlicingFixture(NOW);
  libraryStoreMock.loadRevisions.mockImplementation(async (modelId: string) => library.revisions[modelId] ?? []);
});
afterEach(cleanup);

function renderPanel(record: ModelRecord, extra: Partial<Parameters<typeof ModelDetailsPanel>[0]> = {}) {
  render(() => (
    <ModelDetailsPanel
      model={record}
      projects={library.projects}
      onLocateSource={vi.fn()}
      onConvertToManaged={vi.fn()}
      onDelete={vi.fn()}
      {...extra}
    />
  ));
}

const LID = { open: /^Open Plate 1: Lid/, title: "Plate 1: Lid" };
const EXTERNAL = { open: /^Open External G-code/, title: "External G-code" };

/** The lazy chunks can be slow to arrive the first time, under a full run. */
const SLOW = { timeout: 5000 };

/** Opens a revision's review from the list, once it has loaded. */
async function openReview(record: ModelRecord, revision: { open: RegExp; title: string }) {
  renderPanel(record);
  const list = await screen.findByRole("list", { name: "Slice Revisions" }, SLOW);
  // Newest first: the first match is the newest revision of that name.
  fireEvent.click(within(list).getAllByRole("button", { name: revision.open })[0]);
  const heading = await screen.findByRole("heading", { level: 3, name: revision.title }, SLOW);
  await screen.findByRole("button", { name: "Add to Queue…" }, SLOW);
  return heading;
}

function factRow(label: string): HTMLElement {
  return screen.getAllByRole("listitem").find((item) => item.dataset.fact !== undefined && item.textContent?.startsWith(label))!;
}

describe("Model details' Slice Revisions", () => {
  it("lists a Model's Slice Revisions as a Timeline with their target and filament", async () => {
    renderPanel(model("mdl-web-enclosure"));
    const list = await screen.findByRole("list", { name: "Slice Revisions" }, SLOW);
    const entries = within(list).getAllByRole("listitem");
    expect(entries).toHaveLength(2);
    expect(entries[0]).toHaveTextContent("Plate 1: Lid");
    expect(entries[0]).toHaveTextContent("Elegoo Centauri Carbon 0.4 nozzle · 38.6 g");
    expect(entries[0]).not.toHaveTextContent("Prerelease OrcaSlicer");
    expect(entries[0]).not.toHaveTextContent("Needs manual Printer selection");
    // The older one was sliced with a prerelease engine.
    expect(entries[1]).toHaveTextContent("41.2 g");
    expect(entries[1]).toHaveTextContent("Prerelease OrcaSlicer");
    expect(slicingStoreMock.loadSliceRevisions).toHaveBeenCalledWith("mdl-web-enclosure");
  });

  it("marks an external revision that needs a Printer chosen by hand", async () => {
    renderPanel(model("mdl-web-cube-gcode"));
    const list = await screen.findByRole("list", { name: "Slice Revisions" }, SLOW);
    expect(within(list).getByRole("listitem")).toHaveTextContent("External G-code");
    expect(within(list).getByRole("listitem")).toHaveTextContent("Needs manual Printer selection");
  });

  it("says how to make one when there are none", async () => {
    renderPanel(model("mdl-web-knob"));
    expect(await screen.findByText("None yet. Prepare… the Model and slice a plate to make one.")).toBeInTheDocument();
  });
});

describe("SliceRevisionReview", () => {
  it("reviews a farm3d revision: identity, target, estimates, facts from farm3d settings, and the runtime", async () => {
    const heading = await openReview(model("mdl-web-enclosure"), LID);
    await waitFor(() => expect(heading).toHaveFocus(), SLOW);
    expect(screen.getByText("Sliced by farm3d")).toBeInTheDocument();
    expect(screen.getByText("Revision 1")).toBeInTheDocument();
    expect(screen.getByText("0.20mm Standard @Elegoo CC 0.4 nozzle")).toBeInTheDocument();
    expect(screen.getByText("Other settings came from the process preset.")).toBeInTheDocument();
    expect(screen.getByText("1 h 30 min")).toBeInTheDocument();
    expect(screen.getByText("12.94 m")).toBeInTheDocument();
    expect(screen.getByText("38.6 g")).toBeInTheDocument();
    expect(screen.getByText("Engine 2.4.2 · presets 2.4.2")).toBeInTheDocument();
    for (const label of ["Printer profile", "Nozzle diameter", "Material", "Filament diameter"]) {
      expect(factRow(label)).toHaveTextContent("From farm3d settings");
    }
    expect(screen.queryByText(/Needs manual Printer selection/)).toBeNull();
    expect(screen.queryByText("What the file says (not verified)")).toBeNull();
  });

  it("tells absent and confirmed facts apart in text, not only colour", async () => {
    await openReview(model("mdl-web-cube-gcode"), EXTERNAL);
    const material = factRow("Material");
    const nozzle = factRow("Nozzle diameter");
    expect(material).toHaveTextContent("Not provided");
    expect(material).toHaveTextContent("—");
    expect(material).not.toHaveTextContent("Confirmed by you");
    expect(nozzle).toHaveTextContent("0.4 mm");
    expect(nozzle).toHaveTextContent("Confirmed by you");
    expect(nozzle).not.toHaveTextContent("Not provided");
    expect(screen.queryByText("From farm3d settings")).toBeNull();
    expect(screen.getByText(/Needs manual Printer selection: 1 fact not provided/)).toBeInTheDocument();
  });

  it("puts an external revision's claimed estimates under the not-verified heading, and has no log", async () => {
    await openReview(model("mdl-web-cube-gcode"), EXTERNAL);
    const claims = await screen.findByRole("group", { name: "What the file says (not verified)" });
    expect(within(claims).getByText("23 min 41 s")).toBeInTheDocument();
    expect(within(claims).getByText("Made by OrcaSlicer 2.3.0")).toBeInTheDocument();
    expect(screen.getByText("farm3d didn't slice this file, so it has no estimates of its own.")).toBeInTheDocument();
    expect(screen.getByText("External G-code has no slicing log: farm3d didn't slice it.")).toBeInTheDocument();
    expect(screen.queryByText(/^Engine/)).toBeNull();
  });

  it("announces the disabled Add to Queue… with its visible reason", async () => {
    await openReview(model("mdl-web-enclosure"), LID);
    const queue = await screen.findByRole("button", { name: "Add to Queue…" });
    expect(queue).toBeDisabled();
    expect(queue).toHaveAccessibleDescription("The Queue arrives in a later version.");
    expect(screen.getByText("The Queue arrives in a later version.")).toBeVisible();
  });

  it("offers Stage on Printer…, which opens the Stage dialog, while Add to Queue… stays disabled", async () => {
    await openReview(model("mdl-web-enclosure"), LID);
    const stage = screen.getByRole("button", { name: "Stage on Printer…" });
    expect(stage).toBeEnabled();
    expect(screen.getByRole("button", { name: "Add to Queue…" })).toBeDisabled();
    fireEvent.click(stage);
    const dialog = await screen.findByRole("dialog", { name: "Stage on Printer" });
    expect(dialog).toHaveTextContent("Plate 1: Lid");
    expect(within(dialog).getByRole("button", { name: "Stage" })).toBeDisabled();
  });

  it("shows the read-only log, read by the revision's own id", async () => {
    await openReview(model("mdl-web-enclosure"), LID);
    fireEvent.click(await screen.findByRole("button", { name: "Show log" }));
    const log = await screen.findByLabelText("Log for Plate 1: Lid");
    await waitFor(() => expect(log).toHaveTextContent("[info] Exported out/plate_1.gcode"));
    expect(slicingStoreMock.loadSliceRevisionLog).toHaveBeenCalledWith(WEB_SLICING_REVISION_FARM3D);
    expect(slicingStoreMock.loadOperationLog).not.toHaveBeenCalled();
    expect(log.querySelector("[data-noise]")).not.toBeNull();
  });

  it("shows an older revision's log after its operation has gone", async () => {
    setSlicingState({ operations: [] });
    renderPanel(model("mdl-web-enclosure"));
    const list = await screen.findByRole("list", { name: "Slice Revisions" }, SLOW);
    const [, older] = within(list).getAllByRole("button", { name: /^Open Plate 1: Lid/ });
    fireEvent.click(older);
    await screen.findByText("Engine 2.5.0-dev (prerelease) · presets 2.4.2", undefined, SLOW);
    expect(screen.getByText("The engine and its presets were different versions when this was sliced.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show log" }));
    const log = await screen.findByLabelText("Log for Plate 1: Lid");
    await waitFor(() => expect(log).toHaveTextContent("[info] OrcaSlicer 2.5.0-dev (prerelease)"));
    expect(slicingStoreMock.loadSliceRevisionLog).toHaveBeenCalledWith(WEB_SLICING_REVISION_FARM3D_OLDER);
  });

  it("goes back to the details with focus on the revision's entry", async () => {
    await openReview(model("mdl-web-enclosure"), LID);
    fireEvent.click(await screen.findByRole("button", { name: "Model details" }));
    await waitFor(() => expect(screen.getAllByRole("button", { name: /^Open Plate 1: Lid/ })[0]).toHaveFocus());
  });

  it("deletes after confirming, and lists what blocks a delete", async () => {
    await openReview(model("mdl-web-enclosure"), LID);
    slicingStoreMock.deleteSliceRevision.mockRejectedValueOnce({
      contractVersion: 1, code: "LIFECYCLE_BLOCKED", recovery: [], retryable: false,
      message: "This action is blocked.",
      details: { blockers: [{ action: "delete", code: "inUse", message: "A Queue Entry uses it." }] },
    });
    fireEvent.click(await screen.findByRole("button", { name: "Delete…" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete Slice Revision" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent("Plate 1: Lid can't be deleted yet.");
    expect(alert).toHaveTextContent("A Queue Entry uses it.");

    // The delete lands: the store drops the revision, as the real one does.
    slicingStoreMock.deleteSliceRevision.mockImplementationOnce(async (id: string) => {
      const held = slicingStoreMock.slicing.revisions("mdl-web-enclosure");
      setSlicingState({ revisionsByModel: { "mdl-web-enclosure": held.filter((revision) => revision.id !== id) } });
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(slicingStoreMock.deleteSliceRevision).toHaveBeenLastCalledWith(WEB_SLICING_REVISION_FARM3D));
    // Its entry is gone, so focus lands on the section.
    const heading = await screen.findByRole("heading", { name: "Slice Revisions" });
    await waitFor(() => expect(heading).toHaveFocus());
    expect(screen.getAllByRole("button", { name: /^Open Plate 1: Lid/ })).toHaveLength(1);
  });

  it("says so when the revision is deleted elsewhere while open", async () => {
    await openReview(model("mdl-web-cube-gcode"), EXTERNAL);
    setSlicingState({ revisionsByModel: { "mdl-web-cube-gcode": [] } });
    expect(await screen.findByText("This Slice Revision was deleted.")).toBeInTheDocument();
  });

  it("opens the review a finished slice asked for", async () => {
    const onRevisionOpened = vi.fn();
    renderPanel(model("mdl-web-cube-gcode"), { openRevisionId: WEB_SLICING_REVISION_EXTERNAL, onRevisionOpened });
    expect(await screen.findByRole("heading", { level: 3, name: "External G-code" })).toBeInTheDocument();
    expect(onRevisionOpened).toHaveBeenCalledTimes(1);
  });
});
