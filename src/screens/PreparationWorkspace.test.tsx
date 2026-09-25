import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { libraryStoreMock, resetLibraryStoreMock } from "../library/library-store-mock";
import type { ModelRecord } from "../library/types";
import { buildWebLibraryFixture } from "../library/web-fixtures";
import { SAVE_DEBOUNCE_MS } from "../slicing/preparation-editor";
import {
  loadWebSlicingFixture,
  resetSlicingStoreMock,
  setSlicingState,
  slicingStoreMock,
} from "../slicing/slicing-store-mock";
import type { PreparationDocument, PreparationRecord } from "../slicing/types";
import { lastFakeRenderer } from "../slicing/viewport/fake-renderer";
import { PreparationMode } from "./PreparationMode";

vi.mock("../library/library-store", async () => (await import("../library/library-store-mock")).libraryStoreMock);
vi.mock("../slicing/slicing-store", async () => (await import("../slicing/slicing-store-mock")).slicingStoreMock);
vi.mock("../settings/settings-store", () => ({
  loadSettings: vi.fn(async () => ({ themeMode: "system" })),
  updateSettings: vi.fn(async () => undefined),
}));

const NOW = new Date("2026-09-24T12:00:00Z");
const library = buildWebLibraryFixture(NOW);
const ENCLOSURE = "mdl-web-enclosure";
const enclosure = () => library.models.find((model) => model.id === ENCLOSURE)!;

let held: () => PreparationRecord;
let onBack: ReturnType<typeof vi.fn<() => void>>;

beforeEach(() => {
  resetLibraryStoreMock();
  resetSlicingStoreMock();
  const fixture = loadWebSlicingFixture(NOW);
  held = () => slicingStoreMock.slicing.preparation(ENCLOSURE)!;
  libraryStoreMock.loadRevisions.mockImplementation(async (modelId: string) => library.revisions[modelId] ?? []);
  // Defaults answer from the web fixture, as the real store does in web mode.
  slicingStoreMock.listSliceOptions.mockImplementation(async () => fixture.sliceOptions);
  onBack = vi.fn<() => void>();
  setWindowWidth(1440);
  // Kobalte's menus scroll on open; jsdom has no scrolling.
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function setWindowWidth(width: number) {
  Object.defineProperty(window, "innerWidth", { configurable: true, writable: true, value: width });
  window.dispatchEvent(new Event("resize"));
}

async function open(model: ModelRecord = enclosure()) {
  render(() => <PreparationMode model={model} onBack={onBack} />);
  await screen.findByRole("tab", { name: "Lid" });
  // The viewport has its meshes, the volume, and the instance.
  await waitFor(() => {
    const renderer = lastFakeRenderer();
    expect(renderer?.instances.length).toBe(1);
    expect(renderer?.buildVolume).toBeTruthy();
  });
  return lastFakeRenderer()!;
}

const canvas = () => screen.getByRole("img", { name: "3D view of Enclosure lid" });
const shownDocument = (): PreparationDocument => held().document;
const lid = () => shownDocument().plates[0].instances[0];

function key(target: Element, init: KeyboardEventInit) {
  fireEvent.keyDown(target, init);
}

async function saved() {
  await waitFor(() => expect(slicingStoreMock.updatePreparation).toHaveBeenCalled(), { timeout: SAVE_DEBOUNCE_MS * 3 });
}

async function menu(item: string) {
  await fireEvent.pointerDown(screen.getByLabelText(/^Plate actions for/), { pointerType: "mouse", button: 0 });
  await fireEvent.pointerUp(await screen.findByText(item), { pointerType: "mouse", button: 0 });
}

describe("PreparationWorkspace", () => {
  it("opens the Model's Preparation with its plates, and goes back to the Library", async () => {
    const renderer = await open();
    expect(slicingStoreMock.createPreparation).toHaveBeenCalledWith(ENCLOSURE);
    expect(screen.getByRole("tab", { name: "Latch" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Lid" })).toHaveAttribute("aria-selected", "true");
    expect(renderer.buildVolume?.bed).toEqual({ kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 });
    fireEvent.click(screen.getByRole("button", { name: /Back to Library/ }));
    expect(onBack).toHaveBeenCalled();
  });

  it("at 1024 wide, folds the objects under the viewport behind a toggle", async () => {
    setWindowWidth(1024);
    await open();
    expect(screen.queryByRole("grid", { name: "Objects on this plate" })).toBeNull();
    const toggle = screen.getByRole("button", { name: "Show objects" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(toggle);
    expect(screen.getByRole("grid", { name: "Objects on this plate" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Hide objects" })).toHaveAttribute("aria-expanded", "true");
  });

  describe("plate tabs", () => {
    it("adds a plate and shows it", async () => {
      await open();
      fireEvent.click(screen.getByRole("button", { name: /Plate$/ }));
      const tab = await screen.findByRole("tab", { name: /^Plate 3/ });
      expect(tab).toHaveAttribute("aria-selected", "true");
      await saved();
      expect(shownDocument().plates).toHaveLength(3);
      expect(shownDocument().plates[2].instances).toEqual([]);
    });

    it("renames the shown plate", async () => {
      await open();
      await menu("Rename…");
      const field = await screen.findByRole("textbox", { name: "Plate name" });
      fireEvent.input(field, { target: { value: "Top cover" } });
      fireEvent.click(screen.getByRole("button", { name: "Rename" }));
      expect(await screen.findByRole("tab", { name: "Top cover" })).toBeInTheDocument();
      await saved();
      expect(shownDocument().plates[0].name).toBe("Top cover");
    });

    it("moves the shown plate right, and not past the start", async () => {
      await open();
      await fireEvent.pointerDown(screen.getByLabelText("Plate actions for Lid"), { pointerType: "mouse", button: 0 });
      expect((await screen.findByText("Move left")).closest("[role='menuitem']")).toHaveAttribute("data-disabled");
      await fireEvent.pointerUp(screen.getByText("Move right"), { pointerType: "mouse", button: 0 });
      await waitFor(() => expect(screen.getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["Latch", "Lid"]));
      // The moved plate stays the one shown.
      expect(screen.getByRole("tab", { name: "Lid" })).toHaveAttribute("aria-selected", "true");
      await saved();
      expect(shownDocument().plates.map((plate) => plate.plateKey)).toEqual(["plt-web-enclosure-2", "plt-web-enclosure-1"]);
    });

    it("deletes a plate after confirming, but never the last one", async () => {
      await open();
      await menu("Delete…");
      const dialog = await screen.findByRole("dialog", { name: "Delete Lid?" });
      fireEvent.click(within(dialog).getByRole("button", { name: "Delete plate" }));
      await waitFor(() => expect(screen.queryByRole("tab", { name: "Lid" })).toBeNull());
      expect(screen.getByRole("tab", { name: "Latch" })).toHaveAttribute("aria-selected", "true");
      await saved();
      expect(shownDocument().plates.map((plate) => plate.plateKey)).toEqual(["plt-web-enclosure-2"]);
    });

    it("can't delete the last plate", async () => {
      const only = JSON.parse(JSON.stringify(held())) as PreparationRecord;
      only.document.plates = [only.document.plates[0]];
      setSlicingState({ preparations: { [ENCLOSURE]: only } });
      await open();
      await fireEvent.pointerDown(screen.getByLabelText(/^Plate actions for/), { pointerType: "mouse", button: 0 });
      expect((await screen.findByText("Delete…")).closest("[role='menuitem']")).toHaveAttribute("data-disabled");
    });
  });

  describe("numeric fields", () => {
    it("commit a finished number on Enter, never the keystrokes on the way", async () => {
      await open();
      fireEvent.click(screen.getByRole("row", { name: /Lid/ }));
      const x = await screen.findByRole("spinbutton", { name: "X (mm)" });
      fireEvent.input(x, { target: { value: "1" } });
      fireEvent.input(x, { target: { value: "10" } });
      fireEvent.input(x, { target: { value: "100" } });
      expect(lastFakeRenderer()!.instances[0].matrix[9]).toBe(128);
      fireEvent.keyDown(x, { key: "Enter" });
      await waitFor(() => expect(lastFakeRenderer()!.instances[0].matrix[9]).toBe(100));
      await saved();
      expect(slicingStoreMock.updatePreparation).toHaveBeenCalledTimes(1);
      expect(lid().transform.translateMm).toEqual([100, 128]);
    });

    it("commit on leaving the field, and scale every axis with Uniform", async () => {
      await open();
      fireEvent.click(screen.getByRole("row", { name: /Lid/ }));
      const scaleY = await screen.findByRole("spinbutton", { name: "Y (%)" });
      fireEvent.input(scaleY, { target: { value: "150" } });
      fireEvent.focusOut(scaleY);
      await saved();
      expect(lid().transform.scale).toEqual([1.5, 1.5, 1.5]);
    });

    it("put the value back on Escape", async () => {
      await open();
      fireEvent.click(screen.getByRole("row", { name: /Lid/ }));
      const rotation = await screen.findByRole("spinbutton", { name: "Z (°)" });
      fireEvent.input(rotation, { target: { value: "45" } });
      fireEvent.keyDown(rotation, { key: "Escape" });
      await waitFor(() => expect(screen.getByRole("spinbutton", { name: "Z (°)" })).toHaveValue("0"));
      await new Promise((resolve) => setTimeout(resolve, SAVE_DEBOUNCE_MS + 50));
      expect(slicingStoreMock.updatePreparation).not.toHaveBeenCalled();
    });
  });

  describe("keyboard commands", () => {
    it("select, move, turn, scale, duplicate and delete from the viewport", async () => {
      await open();
      const view = canvas();
      key(view, { key: "]" });
      expect(screen.getByRole("row", { name: /Lid/ })).toHaveAttribute("aria-selected", "true");
      key(view, { key: "ArrowRight" });
      key(view, { key: "ArrowUp", shiftKey: true });
      key(view, { key: "r" });
      key(view, { key: "+" });
      await saved();
      const transform = lid().transform;
      expect(transform.rotateDeg).toEqual([0, 0, 15]);
      expect(transform.scale).toEqual([1.05, 1.05, 1.05]);
      key(view, { key: "d", ctrlKey: true });
      await waitFor(() => expect(screen.getAllByRole("row", { name: /Lid/ })).toHaveLength(2));
      key(view, { key: "Delete" });
      await waitFor(() => expect(screen.getAllByRole("row", { name: /Lid/ })).toHaveLength(1));
    });

    it("move by 1 mm, or 10 mm with Shift, and turn back with Shift+R", async () => {
      await open();
      const view = canvas();
      key(view, { key: "]" });
      key(view, { key: "ArrowLeft" });
      key(view, { key: "ArrowDown", shiftKey: true });
      key(view, { key: "R", shiftKey: true });
      await saved();
      expect(lid().transform.rotateDeg[2]).toBe(-15);
      // Turning keeps the object's centre: X and Y moved by the turn too,
      // so compare the move on its own.
      slicingStoreMock.updatePreparation.mockClear();
      key(view, { key: "ArrowRight" });
      const before = [...lid().transform.translateMm];
      await saved();
      expect(lid().transform.translateMm).toEqual([before[0] + 1, before[1]]);
    });

    it("also run from the object list, where the arrows move through the rows instead", async () => {
      await open();
      const row = screen.getByRole("row", { name: /Lid/ });
      fireEvent.click(row);
      key(row, { key: "ArrowRight" });
      key(row, { key: "r" });
      await saved();
      expect(lid().transform.rotateDeg).toEqual([0, 0, 15]);
    });

    it("ignore keys typed into a field", async () => {
      await open();
      fireEvent.click(screen.getByRole("row", { name: /Lid/ }));
      key(await screen.findByRole("spinbutton", { name: "X (mm)" }), { key: "r" });
      await new Promise((resolve) => setTimeout(resolve, SAVE_DEBOUNCE_MS + 50));
      expect(slicingStoreMock.updatePreparation).not.toHaveBeenCalled();
    });

    it("lay the selection flat with F, face by face", async () => {
      await open();
      key(canvas(), { key: "]" });
      key(canvas(), { key: "f" });
      key(canvas(), { key: "f" });
      await saved();
      // The second-largest face of the lid is its top: upside down.
      const [x, y] = lid().transform.rotateDeg;
      expect(Math.abs(x) + Math.abs(y)).toBeCloseTo(180, 6);
    });

    it("arrange the plate with A, and say what happened", async () => {
      await open();
      key(canvas(), { key: "a" });
      expect(await screen.findByText("Arranged 1 object.")).toBeInTheDocument();
      await saved();
      // The 120 × 80 lid, centred on the 256 mm bed.
      expect(lid().transform.translateMm).toEqual([68, 88]);
    });
  });

  describe("move to plate", () => {
    it("moves the selection with Shift+number", async () => {
      await open();
      key(canvas(), { key: "]" });
      key(canvas(), { key: "@", code: "Digit2", shiftKey: true });
      expect(await screen.findByText("Moved Lid to Latch.")).toBeInTheDocument();
      await saved();
      expect(shownDocument().plates[0].instances).toEqual([]);
      expect(shownDocument().plates[1].instances.map((i) => i.instanceKey)).toEqual(["ins-web-latch", "ins-web-lid"]);
      expect(screen.getByRole("tab", { name: /Lid/ })).toHaveTextContent("issues");
    });

    it("moves the selection with the plate Select", async () => {
      await open();
      fireEvent.click(screen.getByRole("row", { name: /Lid/ }));
      const trigger = within(await screen.findByRole("group", { name: "Position" }).then((el) => el.parentElement!))
        .getAllByRole("button").find((button) => button.textContent?.includes("Lid"))!;
      await fireEvent.pointerDown(trigger, { pointerType: "mouse", button: 0 });
      await fireEvent.pointerUp(await screen.findByRole("option", { name: "Latch" }), { pointerType: "mouse", button: 0 });
      await saved();
      expect(shownDocument().plates[1].instances.map((i) => i.instanceKey)).toContain("ins-web-lid");
    });
  });

  describe("saving", () => {
    it("debounces edits into one update_preparation with the latest document", async () => {
      await open();
      key(canvas(), { key: "]" });
      for (let i = 0; i < 5; i += 1) key(canvas(), { key: "ArrowRight" });
      expect(screen.getByText("Unsaved changes")).toBeInTheDocument();
      await new Promise((resolve) => setTimeout(resolve, SAVE_DEBOUNCE_MS - 150));
      expect(slicingStoreMock.updatePreparation).not.toHaveBeenCalled();
      await saved();
      expect(slicingStoreMock.updatePreparation).toHaveBeenCalledTimes(1);
      const [id, document] = slicingStoreMock.updatePreparation.mock.calls[0];
      expect(id).toBe("prp-web-enclosure");
      expect(document.plates[0].instances[0].transform.translateMm).toEqual([133, 128]);
      expect(await screen.findByText("All changes saved")).toBeInTheDocument();
    });

    it("on CONFLICT shows the latest record and says the edits were not saved", async () => {
      await open();
      const theirs: PreparationRecord = JSON.parse(JSON.stringify(held()));
      theirs.revision = 9;
      theirs.document.plates[0].name = "Renamed elsewhere";
      slicingStoreMock.updatePreparation.mockImplementationOnce(async () => {
        // The store reloads on CONFLICT, bringing the newer record.
        setSlicingState({ preparations: { [ENCLOSURE]: theirs } });
        throw { contractVersion: 1, code: "CONFLICT", message: "Changed elsewhere.", recovery: [], retryable: false };
      });
      key(canvas(), { key: "]" });
      key(canvas(), { key: "ArrowRight" });
      expect(await screen.findByRole("alert", {}, { timeout: SAVE_DEBOUNCE_MS * 3 })).toHaveTextContent(/changed elsewhere/);
      expect(screen.getByRole("tab", { name: "Renamed elsewhere" })).toBeInTheDocument();
      // The local move is gone; the viewport shows the latest record.
      await waitFor(() => expect(lastFakeRenderer()!.instances[0].matrix[9]).toBe(128));
    });
  });

  describe("stale source", () => {
    const stale = () => {
      const model = enclosure();
      const current = { ...model.currentRevision, id: "msr-web-enclosure-2", sequence: 2 };
      libraryStoreMock.loadRevisions.mockImplementation(async () => [
        { ...library.revisions[ENCLOSURE][0], id: "msr-web-enclosure-2", sequence: 2 },
        ...library.revisions[ENCLOSURE],
      ]);
      setSlicingState({ preparations: { [ENCLOSURE]: { ...held(), stale: true } } });
      return { ...model, currentRevision: current };
    };

    it("offers Continue with revision M, held in the store for start_slice", async () => {
      await open(stale());
      const banner = await screen.findByRole("region", { name: "Source changed" });
      const button = await within(banner).findByRole("button", { name: "Continue with revision 1" });
      expect(within(banner).getByRole("button", { name: "Reload onto revision 2" })).toBeInTheDocument();
      fireEvent.click(button);
      expect(slicingStoreMock.chooseContinueWithSourceRevision).toHaveBeenCalledWith("prp-web-enclosure", "msr-web-enclosure-1");
      expect(slicingStoreMock.slicing.continueWithSourceRevision("prp-web-enclosure")).toBe("msr-web-enclosure-1");
      expect(await within(banner).findByText(/Slicing will use revision 1, as you chose/)).toBeInTheDocument();
    });

    it("reloads onto revision N, saving pending edits first, and says what changed", async () => {
      await open(stale());
      slicingStoreMock.reloadPreparation.mockImplementationOnce(async () => {
        const rebased = { ...held(), stale: false, sourceRevisionId: "msr-web-enclosure-2", revision: held().revision + 1 };
        setSlicingState({ preparations: { [ENCLOSURE]: rebased } });
        return { preparation: rebased, removedObjectKeys: [2], addedObjectKeys: [] };
      });
      key(canvas(), { key: "]" });
      key(canvas(), { key: "ArrowRight" });
      const banner = await screen.findByRole("region", { name: "Source changed" });
      fireEvent.click(within(banner).getByRole("button", { name: "Reload onto revision 2" }));
      expect(await screen.findByText(/Reloaded onto revision 2\. Removed \(no longer in the file\): Latch\./)).toBeInTheDocument();
      expect(slicingStoreMock.updatePreparation).toHaveBeenCalledTimes(1);
      expect(slicingStoreMock.updatePreparation.mock.invocationCallOrder[0])
        .toBeLessThan(slicingStoreMock.reloadPreparation.mock.invocationCallOrder[0]);
      expect(slicingStoreMock.reloadPreparation).toHaveBeenCalledWith("prp-web-enclosure");
      await waitFor(() => expect(screen.queryByRole("region", { name: "Source changed" })).toBeNull());
    });

    it("shows why a reload failed", async () => {
      await open(stale());
      slicingStoreMock.reloadPreparation.mockRejectedValueOnce({
        contractVersion: 1, code: "SOURCE_UNAVAILABLE", message: "The source file is missing.", recovery: [], retryable: false,
      });
      const banner = await screen.findByRole("region", { name: "Source changed" });
      fireEvent.click(within(banner).getByRole("button", { name: "Reload onto revision 2" }));
      expect(await within(banner).findByRole("alert")).toHaveTextContent("The source file is missing.");
    });
  });

  describe("measure", () => {
    it("picks two points with M, and measures between objects without a pointer", async () => {
      const renderer = await open();
      // Two objects on the plate: duplicate the lid and arrange.
      key(canvas(), { key: "]" });
      key(canvas(), { key: "d", ctrlKey: true });
      key(canvas(), { key: "a" });
      key(canvas(), { key: "m" });
      const panel = await screen.findByRole("region", { name: "Measure" });
      renderer.pickResult = { instanceKey: "ins-web-lid", pointMm: [0, 0, 0] };
      fireEvent.pointerDown(canvas(), { button: 0, clientX: 10, clientY: 10 });
      fireEvent.pointerUp(canvas(), { button: 0, clientX: 10, clientY: 10 });
      renderer.pickResult = { instanceKey: "ins-web-lid", pointMm: [3, 4, 0] };
      fireEvent.pointerDown(canvas(), { button: 0, clientX: 20, clientY: 20 });
      fireEvent.pointerUp(canvas(), { button: 0, clientX: 20, clientY: 20 });
      expect(await within(panel).findByText(/Distance:/)).toHaveTextContent("Distance: 5.0 mm (X 3.0 mm, Y 4.0 mm, Z 0.0 mm)");
      await waitFor(() => expect(renderer.overlays.measure).toEqual([[0, 0, 0], [3, 4, 0]]));

      // The keyboard alternative: the selected object to another.
      fireEvent.click(within(panel).getByRole("button", { name: "Clear points" }));
      const trigger = within(panel).getAllByRole("button").find((button) => button.textContent?.includes("Choose an object"))!;
      await fireEvent.pointerDown(trigger, { pointerType: "mouse", button: 0 });
      await fireEvent.pointerUp(await screen.findByRole("option", { name: "Lid 1" }), { pointerType: "mouse", button: 0 });
      // Two 120 mm lids arranged 5 mm apart: centres 125 mm apart.
      expect(await within(panel).findByText(/Centre to centre/)).toHaveTextContent("Centre to centre: 125.0 mm. Gap: 5.0 mm.");
      expect(renderer.overlays.measure).toBeDefined();
    });
  });
});
