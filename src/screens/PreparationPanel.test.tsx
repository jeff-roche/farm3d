import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CommandError } from "../generated/contracts/command/CommandError";
import { libraryStoreMock, resetLibraryStoreMock } from "../library/library-store-mock";
import { buildWebLibraryFixture } from "../library/web-fixtures";
import type { ResolvedPrinter } from "../printers/types";
import { SAVE_DEBOUNCE_MS } from "../slicing/preparation-editor";
import { closeSlicerSettings, slicerSettingsOpen } from "../slicing/slicer-settings-opener";
import {
  loadWebSlicingFixture,
  resetSlicingStoreMock,
  setSlicingProgress,
  setSlicingState,
  slicingStoreMock,
} from "../slicing/slicing-store-mock";
import { operation, runtimeStatus } from "../slicing/test-records";
import type { PreparationDocument, PreparationRecord, SliceOperationRecord } from "../slicing/types";
import type { WebSlicingFixture } from "../slicing/web-fixtures";
import { lastFakeRenderer } from "../slicing/viewport/fake-renderer";
import { PreparationMode } from "./PreparationMode";
import { PreparationPanel } from "./PreparationPanel";
import { createPreparationSession, type PreparationSession } from "./preparation-session";
import { PreparationWorkspace } from "./PreparationWorkspace";

vi.mock("../library/library-store", async () => (await import("../library/library-store-mock")).libraryStoreMock);
vi.mock("../slicing/slicing-store", async () => (await import("../slicing/slicing-store-mock")).slicingStoreMock);
vi.mock("../settings/settings-store", () => ({
  loadSettings: vi.fn(async () => ({ themeMode: "system" })),
  updateSettings: vi.fn(async () => undefined),
}));
const printerState = vi.hoisted(() => ({ list: [] as unknown[] }));
vi.mock("../printers/printer-store", () => ({ printers: () => printerState.list }));
vi.mock("../printers/printer-catalog", () => ({
  listCatalogModels: vi.fn(async () => [
    { modelId: "Elegoo-CC", vendor: "Elegoo", model: "Elegoo Centauri Carbon" },
    { modelId: "Prusa-MK4", vendor: "Prusa", model: "Prusa MK4" },
  ]),
  listCatalogVariants: vi.fn(async (vendor: string) => (vendor === "Prusa"
    ? [{ variant: "Prusa MK4 0.4 nozzle", printerVariant: "0.4" }, { variant: "Prusa MK4 0.6 nozzle", printerVariant: "0.6" }]
    : [{ variant: "Elegoo Centauri Carbon 0.4 nozzle", printerVariant: "0.4" }])),
  previewProfile: vi.fn(async () => ({})),
}));

const NOW = new Date("2026-09-24T12:00:00Z");
const library = buildWebLibraryFixture(NOW);
const ENCLOSURE = "mdl-web-enclosure";
const PREPARATION = "prp-web-enclosure";
const LID_PLATE = "plt-web-enclosure-1";
const LATCH_PLATE = "plt-web-enclosure-2";
const enclosure = () => library.models.find((model) => model.id === ENCLOSURE)!;

const CENTAURI = {
  vendor: "Elegoo",
  model: "Elegoo Centauri Carbon",
  variant: "Elegoo Centauri Carbon 0.4 nozzle",
  modelId: "Elegoo-CC",
  printerVariant: "0.4",
};

function printer(id: string, name: string, nozzle = 0.4): ResolvedPrinter {
  return {
    id,
    name,
    catalogRef: { ...CENTAURI },
    location: `Bench ${name.slice(-1)}`,
    profile: { nozzleDiameterMm: [nozzle] },
  } as unknown as ResolvedPrinter;
}

let fixture: WebSlicingFixture;
const held = () => slicingStoreMock.slicing.preparation(ENCLOSURE)!;
const shownDocument = (): PreparationDocument => held().document;

beforeEach(() => {
  resetLibraryStoreMock();
  resetSlicingStoreMock();
  closeSlicerSettings();
  fixture = loadWebSlicingFixture(NOW);
  printerState.list = [printer("prn-web-cc-1", "CC 1"), printer("prn-web-cc-2", "CC 2"), printer("prn-web-mk4", "MK4", 0.6)];
  libraryStoreMock.loadRevisions.mockImplementation(async (modelId: string) => library.revisions[modelId] ?? []);
  slicingStoreMock.listSliceOptions.mockImplementation(async () => fixture.sliceOptions);
  slicingStoreMock.loadOperationLog.mockImplementation(async (id) => fixture.logs[id]);
  Object.defineProperty(window, "innerWidth", { configurable: true, writable: true, value: 1440 });
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function setPreparation(change: (record: PreparationRecord) => PreparationRecord) {
  const record = change(JSON.parse(JSON.stringify(held())) as PreparationRecord);
  setSlicingState({ preparations: { [ENCLOSURE]: record } });
}

/** Opens the workspace with its panel. Unless `sliceable` is false, waits
 *  until both plates' placements are checked and slicing is allowed. */
async function open({ sliceable = true } = {}) {
  render(() => <PreparationMode model={enclosure()} onBack={() => {}} />);
  await screen.findByRole("tab", { name: "Lid" });
  await waitFor(() => {
    expect(lastFakeRenderer()?.instances.length).toBe(1);
    expect(lastFakeRenderer()?.buildVolume).toBeTruthy();
  });
  await screen.findByText(/^OrcaSlicer printer:/);
  if (sliceable) await waitFor(() => expect(screen.getByRole("button", { name: "Slice all plates" })).toBeEnabled());
}

const panel = () => screen.getByRole("complementary", { name: "Preparation settings" });
const selectTrigger = (label: RegExp) => within(panel()).getByRole("button", { name: label });

async function choose(label: RegExp, option: string | RegExp) {
  await fireEvent.pointerDown(selectTrigger(label), { pointerType: "mouse", button: 0 });
  await fireEvent.pointerUp(await screen.findByRole("option", { name: option }), { pointerType: "mouse", button: 0 });
}

async function saved() {
  await waitFor(() => expect(slicingStoreMock.updatePreparation).toHaveBeenCalled(), { timeout: SAVE_DEBOUNCE_MS * 3 });
}

function typeInto(label: string, value: string) {
  const input = within(panel()).getByLabelText(label);
  input.focus();
  fireEvent.input(input, { target: { value } });
  fireEvent.keyDown(input, { key: "Enter" });
}

function commandError(overrides: Partial<CommandError>): CommandError {
  return { contractVersion: 1, code: "INTERNAL", message: "Refused.", recovery: [], retryable: false, ...overrides };
}

describe("PreparationPanel", () => {
  describe("controls", () => {
    it("saves a chosen quality and material through the editor", async () => {
      await open();
      await choose(/Process preset/, "0.12mm Fine @Elegoo CC 0.4 nozzle");
      await saved();
      expect(shownDocument().processPreset).toBe("0.12mm Fine @Elegoo CC 0.4 nozzle");

      slicingStoreMock.updatePreparation.mockClear();
      await choose(/Filament preset/, /Elegoo PETG @ECC · PETG/);
      await saved();
      expect(shownDocument().filamentPreset).toBe("Elegoo PETG @ECC");
    });

    it("keeps numbers inside D4's ranges, and an emptied field goes back to the preset", async () => {
      await open();
      expect(within(panel()).getByLabelText("Walls")).toHaveAttribute("placeholder", "Preset");
      typeInto("Walls", "25");
      await saved();
      expect(shownDocument().controls.wallLoops).toBe(20);

      slicingStoreMock.updatePreparation.mockClear();
      typeInto("Layer height", "0.5");
      await saved();
      // 80% of the target's 0.4 mm nozzle.
      expect(shownDocument().controls.layerHeightMm).toBe(0.32);

      slicingStoreMock.updatePreparation.mockClear();
      typeInto("Walls", "");
      await saved();
      expect(shownDocument().controls).not.toHaveProperty("wallLoops");
    });

    it("sets and clears a choice, with the preset's choice as the unset value", async () => {
      await open();
      await choose(/Infill pattern/, "Gyroid");
      await saved();
      expect(shownDocument().controls.infillPattern).toBe("gyroid");

      slicingStoreMock.updatePreparation.mockClear();
      await choose(/^Supports/, "Preset's choice");
      await saved();
      expect(shownDocument().controls).not.toHaveProperty("supports");
    });

    it("folds the advanced controls until asked", async () => {
      await open();
      expect(within(panel()).queryByLabelText("Top shells")).toBeNull();
      const advanced = within(panel()).getByRole("button", { name: "Advanced" });
      expect(advanced).toHaveAttribute("aria-expanded", "false");
      fireEvent.click(advanced);
      expect(within(panel()).getByLabelText("Top shells")).toBeInTheDocument();
      typeInto("Skirt loops", "12");
      await saved();
      expect(shownDocument().controls.skirtLoops).toBe(10);
    });

    it("shows the matching Printers with the roster, each with its state and place", async () => {
      (printerState.list[0] as { runtimeStatus?: unknown }).runtimeStatus = { operationalState: "ready" };
      await open();
      const roster = within(panel()).getByRole("button", { name: "2 matching Printers" });
      fireEvent.focus(roster);
      const first = (await screen.findByText("CC 1")).closest("li")!;
      expect(first).toHaveTextContent("CC 1 · Bench 1");
      expect(first).toHaveTextContent("Ready");
      expect(screen.getByText("CC 2").closest("li")).toHaveTextContent("Status unavailable");
    });

    it("reaches any catalog profile through Other printer profile…, with no Printers at all", async () => {
      printerState.list = [];
      await open();
      await choose(/Slice for/, "Other printer profile…");
      const dialog = await screen.findByRole("dialog", { name: "Other printer profile" });
      // Nothing changed yet.
      expect(shownDocument().target).toEqual({ kind: "profile", catalogRef: CENTAURI });
      const brand = within(dialog).getByRole("combobox", { name: "Brand" });
      await fireEvent.pointerDown(brand, { pointerType: "mouse", button: 0 });
      await fireEvent.input(brand, { target: { value: "Prusa" } });
      await fireEvent.pointerUp(await screen.findByRole("option", { name: "Prusa" }), { pointerType: "mouse", button: 0 });
      await fireEvent.pointerDown(await within(dialog).findByRole("button", { name: /^Model/ }), { pointerType: "mouse", button: 0 });
      await fireEvent.pointerUp(await screen.findByRole("option", { name: "MK4" }), { pointerType: "mouse", button: 0 });
      // The 0.4 mm nozzle is chosen by default.
      const use = within(dialog).getByRole("button", { name: "Use this profile" });
      await waitFor(() => expect(use).toBeEnabled());
      fireEvent.click(use);
      await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
      await waitFor(() => expect(shownDocument().target).toEqual({
        kind: "profile",
        catalogRef: { vendor: "Prusa", model: "Prusa MK4", variant: "Prusa MK4 0.4 nozzle", modelId: "Prusa-MK4", printerVariant: "0.4" },
      }), { timeout: SAVE_DEBOUNCE_MS * 3 });
      expect(selectTrigger(/Slice for/)).toHaveTextContent("Prusa MK4 0.4 nozzle");
      expect(slicingStoreMock.listSliceOptions).toHaveBeenLastCalledWith({
        kind: "profile",
        catalogRef: expect.objectContaining({ variant: "Prusa MK4 0.4 nozzle" }),
      });
    });

    it("re-queries the options for a new target, and takes its defaults for presets it doesn't offer", async () => {
      await open();
      const mk4Options = {
        ...fixture.sliceOptions,
        machinePreset: "Prusa MK4 0.6 nozzle",
        processPresets: [{ name: "0.32mm Speed @MK4 0.6" }],
        filamentPresets: [{ name: "Prusament PLA", filamentType: "PLA", materialFamily: "PLA" as const }],
        defaults: { processPreset: "0.32mm Speed @MK4 0.6", filamentPreset: "Prusament PLA" },
        matchingPrinterIds: ["prn-web-mk4"],
      };
      slicingStoreMock.listSliceOptions.mockImplementation(async (target) => (
        target.kind === "printer" && target.printerId === "prn-web-mk4" ? mk4Options : fixture.sliceOptions
      ));
      await choose(/Slice for/, "MK4");
      await waitFor(() => expect(slicingStoreMock.listSliceOptions).toHaveBeenLastCalledWith({ kind: "printer", printerId: "prn-web-mk4" }));
      await waitFor(() => expect(shownDocument().target).toEqual({ kind: "printer", printerId: "prn-web-mk4" }), { timeout: SAVE_DEBOUNCE_MS * 3 });
      await waitFor(() => expect(shownDocument().processPreset).toBe("0.32mm Speed @MK4 0.6"), { timeout: SAVE_DEBOUNCE_MS * 3 });
      expect(shownDocument().filamentPreset).toBe("Prusament PLA");
      expect(await within(panel()).findByText("OrcaSlicer printer: Prusa MK4 0.6 nozzle")).toBeInTheDocument();
      expect(within(panel()).getByRole("button", { name: "1 matching Printers" })).toBeInTheDocument();
    });
  });

  it("becomes an overlay behind Settings panel when narrow", async () => {
    Object.defineProperty(window, "innerWidth", { configurable: true, writable: true, value: 1024 });
    render(() => <PreparationMode model={enclosure()} onBack={() => {}} />);
    const toggle = await screen.findByRole("button", { name: "Settings panel" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("complementary", { name: "Preparation settings" })).toBeNull();
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("complementary", { name: "Preparation settings" })).toBeVisible();
  });

  describe("validation", () => {
    it("links an object's issue to the object, on its plate", async () => {
      setPreparation((record) => {
        record.document.plates[1].instances[0].transform.translateMm = [600, 128];
        return record;
      });
      await open({ sliceable: false });
      const row = await within(panel()).findByRole("button", { name: "Latch is outside the bed (Latch)." });
      fireEvent.click(row);
      await waitFor(() => expect(screen.getByRole("tab", { name: /Latch/ })).toHaveAttribute("aria-selected", "true"));
      await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("img", { name: "3D view of Enclosure lid" })));
      expect(lastFakeRenderer()!.instances.find((instance) => instance.instanceKey === "ins-web-latch")?.selected).toBe(true);
    });

    it("links a preset issue to its field", async () => {
      setPreparation((record) => {
        delete record.document.processPreset;
        return record;
      });
      render(() => <PreparationMode model={enclosure()} onBack={() => {}} />);
      fireEvent.click(await within(await screen.findByRole("complementary")).findByRole("button", { name: "Choose a quality preset." }));
      await waitFor(() => expect(document.activeElement).toBe(selectTrigger(/Process preset/)));
    });

    it("links an empty plate to its tab", async () => {
      setPreparation((record) => {
        record.document.plates[1].instances = [];
        return record;
      });
      render(() => <PreparationMode model={enclosure()} onBack={() => {}} />);
      fireEvent.click(await within(await screen.findByRole("complementary")).findByRole("button", { name: "Latch has no objects." }));
      await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("tab", { name: /Latch/ })));
    });

    it("lists the build items the file marks unprintable", async () => {
      await open();
      expect(within(panel()).getByText(/Marked not printable in the file, so it isn't sliced: Gasket\./)).toBeInTheDocument();
    });
  });

  describe("slice gating", () => {
    it("keeps both actions disabled, with the reason as text, while issues exist", async () => {
      setPreparation((record) => {
        record.document.plates[1].instances[0].transform.translateMm = [600, 128];
        return record;
      });
      render(() => <PreparationMode model={enclosure()} onBack={() => {}} />);
      // The Lid plate is fine; all plates include the Latch.
      await within(await screen.findByRole("complementary")).findByText("Slice all plates: Fix the issue listed above first.");
      expect(screen.getByRole("button", { name: "Slice all plates" })).toBeDisabled();
      expect(screen.getByRole("button", { name: "Slice plate" })).toBeEnabled();
    });

    it("waits for the runtime, for unsaved edits, and for placement checks", async () => {
      setSlicingState({ runtime: null });
      render(() => <PreparationMode model={enclosure()} onBack={() => {}} />);
      expect(await within(await screen.findByRole("complementary")).findByText("Checking for OrcaSlicer…")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Slice plate" })).toBeDisabled();
      cleanup();

      setSlicingState({ runtime: fixture.runtime });
      await open();
      typeInto("Walls", "3");
      expect(within(panel()).getByText("Saving your changes…")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Slice plate" })).toBeDisabled();
      await waitFor(() => expect(screen.getByRole("button", { name: "Slice plate" })).toBeEnabled(), { timeout: SAVE_DEBOUNCE_MS * 3 });

      // A turn is checked in a later task; until then the placement is old.
      const canvas = screen.getByRole("img", { name: "3D view of Enclosure lid" });
      fireEvent.keyDown(canvas, { key: "]" });
      fireEvent.keyDown(canvas, { key: "r" });
      expect(within(panel()).getByText("Checking placement…")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Slice plate" })).toBeDisabled();
      await waitFor(() => expect(within(panel()).queryByText("Checking placement…")).toBeNull());
    });

    it("won't slice a plate that is already slicing", async () => {
      await open();
      setSlicingState({
        operations: [...fixture.operations, operation({
          id: "sop-busy", preparationId: PREPARATION, plateKey: LID_PLATE, plateName: "Lid", state: "queued",
        })],
      });
      expect(await within(panel()).findByText("Already slicing Lid.")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Slice plate" })).toBeDisabled();
      expect(screen.getByRole("button", { name: "Slice all plates" })).toBeDisabled();
      // The Latch alone is free.
      fireEvent.click(screen.getByRole("tab", { name: /Latch/ }));
      await waitFor(() => expect(screen.getByRole("button", { name: "Slice plate" })).toBeEnabled());
      expect(within(panel()).getByText("Slice all plates: Already slicing Lid.")).toBeInTheDocument();
    });

    it("replaces the actions with the runtime's state and a way to its settings", async () => {
      setSlicingState({
        runtime: runtimeStatus({
          engine: { state: "notFound" },
          presetSource: { state: "unavailable", reason: "there is no engine", origin: "engine" },
          canSlice: false,
          engineCandidates: [
            { source: "path", executableName: "orca-slicer", path: "/usr/bin/orca-slicer", result: { kind: "probeFailed", reason: "exit status 127" } },
          ],
        }),
      });
      render(() => <PreparationMode model={enclosure()} onBack={() => {}} />);
      const region = await within(await screen.findByRole("complementary")).findByRole("region", { name: "OrcaSlicer unavailable" });
      expect(region).toHaveTextContent("OrcaSlicer wasn't found.");
      expect(region).toHaveTextContent("The OrcaSlicer presets couldn't be read: there is no engine");
      expect(region).toHaveTextContent("orca-slicer (on PATH): couldn't be run: exit status 127");
      expect(region).not.toHaveTextContent("/usr/bin");
      expect(screen.queryByRole("button", { name: "Slice plate" })).toBeNull();

      fireEvent.click(within(region).getByRole("button", { name: "Open Slicer settings" }));
      expect(slicerSettingsOpen()).toBe(true);

      // The runtime issue in the list moves focus to the same button.
      fireEvent.click(within(panel()).getByRole("button", { name: "OrcaSlicer isn't ready to slice." }));
      expect(document.activeElement).toBe(within(region).getByRole("button", { name: "Open Slicer settings" }));
    });
  });

  describe("start_slice", () => {
    it("slices the shown plate, or every plate, with one operation id per click", async () => {
      await open();
      fireEvent.click(screen.getByRole("button", { name: "Slice plate" }));
      await waitFor(() => expect(slicingStoreMock.startSlice).toHaveBeenCalledTimes(1));
      const [preparationId, plateKeys, options] = slicingStoreMock.startSlice.mock.calls[0];
      expect(preparationId).toBe(PREPARATION);
      expect(plateKeys).toEqual([LID_PLATE]);
      expect(options?.operationId).toMatch(/^[0-9a-f-]{36}$/);
      expect(options).not.toHaveProperty("continueWithSourceRevision");

      await waitFor(() => expect(screen.getByRole("button", { name: "Slice all plates" })).toBeEnabled());
      fireEvent.click(screen.getByRole("button", { name: "Slice all plates" }));
      await waitFor(() => expect(slicingStoreMock.startSlice).toHaveBeenCalledTimes(2));
      expect(slicingStoreMock.startSlice.mock.calls[1][1]).toEqual([LID_PLATE, LATCH_PLATE]);
      expect(slicingStoreMock.startSlice.mock.calls[1][2]?.operationId).not.toBe(options?.operationId);
    });

    it("saves pending edits before starting", async () => {
      await open();
      let savedFirst = false;
      slicingStoreMock.startSlice.mockImplementation(async () => {
        savedFirst = slicingStoreMock.updatePreparation.mock.calls.length > 0;
        return [];
      });
      typeInto("Walls", "4");
      // The debounce hasn't run; Slice waits for the save, which flush sends.
      await waitFor(() => expect(screen.getByRole("button", { name: "Slice plate" })).toBeEnabled(), { timeout: SAVE_DEBOUNCE_MS * 3 });
      fireEvent.click(screen.getByRole("button", { name: "Slice plate" }));
      await waitFor(() => expect(slicingStoreMock.startSlice).toHaveBeenCalled());
      expect(savedFirst).toBe(true);
    });

    it("slices despite a notice left from an earlier save", async () => {
      await open();
      slicingStoreMock.updatePreparation.mockRejectedValueOnce(commandError({ code: "VALIDATION", message: "Walls are wrong." }));
      typeInto("Walls", "3");
      expect(await screen.findByText("Your latest edit was not saved: Walls are wrong.", {}, { timeout: SAVE_DEBOUNCE_MS * 3 }))
        .toBeInTheDocument();
      await waitFor(() => expect(screen.getByRole("button", { name: "Slice plate" })).toBeEnabled());
      fireEvent.click(screen.getByRole("button", { name: "Slice plate" }));
      await waitFor(() => expect(slicingStoreMock.startSlice).toHaveBeenCalledTimes(1));
    });

    it("refuses when the save made just before starting fails, with a single alert", async () => {
      // An edit that lands between the click and the flush (Slice is
      // disabled while edits are pending, so only a race gets here).
      let session: PreparationSession | undefined;
      render(() => {
        session = createPreparationSession(() => enclosure());
        return <PreparationWorkspace session={session} onBack={() => {}} dock={<PreparationPanel session={session} />} />;
      });
      await waitFor(() => expect(screen.getByRole("button", { name: "Slice plate" })).toBeEnabled());
      const editor = session!.editor;
      const flush = editor.flush;
      editor.flush = () => {
        editor.edit((document) => ({ ...document, controls: { ...document.controls, wallLoops: 7 } }));
        return flush();
      };
      slicingStoreMock.updatePreparation.mockRejectedValueOnce(commandError({ code: "VALIDATION", message: "Walls are wrong." }));
      fireEvent.click(screen.getByRole("button", { name: "Slice plate" }));
      expect(await within(panel()).findByText("Your latest changes weren't saved, so nothing was sliced.")).toBeInTheDocument();
      expect(slicingStoreMock.startSlice).not.toHaveBeenCalled();
      const alerts = screen.getAllByRole("alert");
      expect(alerts).toHaveLength(1);
      expect(alerts[0]).toHaveTextContent("Your latest edit was not saved: Walls are wrong.");
    });

    it("sends the continue choice for a stale Preparation", async () => {
      setPreparation((record) => ({ ...record, stale: true }));
      setSlicingState({ continueWith: { [PREPARATION]: "msr-web-enclosure-1" } });
      await open();
      fireEvent.click(screen.getByRole("button", { name: "Slice plate" }));
      await waitFor(() => expect(slicingStoreMock.startSlice).toHaveBeenCalled());
      expect(slicingStoreMock.startSlice.mock.calls[0][2]).toMatchObject({ continueWithSourceRevision: "msr-web-enclosure-1" });
    });

    it("keeps a stale Preparation without the choice from slicing", async () => {
      setPreparation((record) => ({ ...record, stale: true }));
      render(() => <PreparationMode model={enclosure()} onBack={() => {}} />);
      await within(await screen.findByRole("complementary")).findByRole("button", { name: "The Model changed since this Preparation was made." });
      expect(screen.getByRole("button", { name: "Slice plate" })).toBeDisabled();
      fireEvent.click(within(panel()).getByRole("button", { name: "The Model changed since this Preparation was made." }));
      await waitFor(() => expect(document.activeElement).toBe(
        within(screen.getByRole("region", { name: "Source changed" })).getAllByRole("button")[0],
      ));
    });

    it("tries again with the same operation id when no answer came", async () => {
      await open();
      slicingStoreMock.startSlice.mockRejectedValueOnce(new TypeError("IPC closed"));
      fireEvent.click(screen.getByRole("button", { name: "Slice plate" }));
      const alert = await within(panel()).findByRole("alert");
      expect(alert).toHaveTextContent("farm3d didn't hear back about starting the slice.");
      fireEvent.click(within(alert).getByRole("button", { name: "Try again" }));
      await waitFor(() => expect(slicingStoreMock.startSlice).toHaveBeenCalledTimes(2));
      const [first, second] = slicingStoreMock.startSlice.mock.calls;
      expect(second[2]?.operationId).toBe(first[2]?.operationId);
      expect(second[1]).toEqual(first[1]);
      await waitFor(() => expect(within(panel()).queryByRole("alert")).toBeNull());
    });

    it("says a stale refusal's recovery and reloads the store", async () => {
      await open();
      slicingStoreMock.startSlice.mockRejectedValueOnce(commandError({
        code: "PREPARATION_STALE",
        message: "The Model changed since this Preparation was made.",
        recovery: ["RELOAD_PREPARATION", "EDIT_PREPARATION"],
      }));
      fireEvent.click(screen.getByRole("button", { name: "Slice plate" }));
      const alert = await within(panel()).findByRole("alert");
      expect(alert).toHaveTextContent("The Model changed since this Preparation was made.");
      expect(alert).toHaveTextContent(/Source changed banner/);
      expect(slicingStoreMock.refreshSlicing).toHaveBeenCalled();
      expect(within(alert).queryByRole("button", { name: "Try again" })).toBeNull();
    });

    it("shows a backend-only refusal with a link to its field", async () => {
      await open();
      slicingStoreMock.startSlice.mockRejectedValueOnce(commandError({
        code: "FILAMENT_INCOMPATIBLE",
        message: "The filament preset \"Elegoo PLA @ECC\" is not made for \"Elegoo Centauri Carbon 0.4 nozzle\".",
        recovery: ["EDIT_PREPARATION"],
      }));
      fireEvent.click(screen.getByRole("button", { name: "Slice plate" }));
      const alert = await within(panel()).findByRole("alert");
      expect(alert).toHaveTextContent("is not made for");
      fireEvent.click(within(alert).getByRole("button", { name: "Go to Material" }));
      await waitFor(() => expect(document.activeElement).toBe(selectTrigger(/Filament preset/)));
    });

    it("focuses a field whose control is disabled while its presets reload", async () => {
      await open();
      slicingStoreMock.startSlice.mockRejectedValueOnce(commandError({ code: "FILAMENT_INCOMPATIBLE", message: "Not for this printer." }));
      fireEvent.click(screen.getByRole("button", { name: "Slice plate" }));
      const alert = await within(panel()).findByRole("alert");
      slicingStoreMock.listSliceOptions.mockImplementation(() => new Promise(() => {}));
      await choose(/Slice for/, "CC 1");
      await waitFor(() => expect(selectTrigger(/Filament preset/)).toBeDisabled());
      fireEvent.click(within(alert).getByRole("button", { name: "Go to Material" }));
      await waitFor(() => expect(document.activeElement).toBe(panel().querySelector("[data-panel-field='material']")));
    });

    it("links an unsupported setting to its control, opening Advanced, and offers the settings", async () => {
      await open();
      slicingStoreMock.startSlice.mockRejectedValueOnce(commandError({
        code: "UNSUPPORTED_SETTING_FOR_RUNTIME",
        message: "OrcaSlicer 2.3.0 does not support the setting \"brim_width\".",
        recovery: ["EDIT_PREPARATION", "OPEN_SLICER_SETTINGS"],
        details: { key: "brim_width", presetSourceVersion: "2.3.0" },
      }));
      fireEvent.click(screen.getByRole("button", { name: "Slice plate" }));
      const alert = await within(panel()).findByRole("alert");
      fireEvent.click(within(alert).getByRole("button", { name: "Go to Brim width" }));
      await waitFor(() => expect(document.activeElement).toBe(within(panel()).getByLabelText("Brim width")));
      fireEvent.click(within(alert).getByRole("button", { name: "Open Slicer settings" }));
      expect(slicerSettingsOpen()).toBe(true);
    });
  });

  describe("operations", () => {
    const running = (overrides: Partial<SliceOperationRecord> = {}) => operation({
      id: "sop-running",
      preparationId: PREPARATION,
      plateKey: LID_PLATE,
      plateName: "Lid",
      plateIndex: 1,
      state: "running",
      queuedAt: "2026-09-24T12:30:00Z",
      startedAt: "2026-09-24T12:30:01Z",
      ...overrides,
    });

    function addOperation(record: SliceOperationRecord) {
      setSlicingState({ operations: [...fixture.operations, record] });
    }
    const rowFor = (title: string) => screen.getAllByRole("listitem").find((item) => item.textContent?.startsWith(title))!;

    it("shows determinate progress when there are percentages, and indeterminate otherwise", async () => {
      await open();
      addOperation(running());
      const bar = await within(panel()).findByRole("progressbar");
      expect(bar).toHaveAttribute("data-indeterminate");
      expect(bar).toHaveTextContent("Slicing…");

      setSlicingProgress("sop-running", { message: "Generating G-code", totalPercent: 42, platePercent: 40 });
      await waitFor(() => expect(bar).toHaveAttribute("aria-valuenow", "42"));
      expect(bar).not.toHaveAttribute("data-indeterminate");
      expect(bar).toHaveTextContent("Generating G-code");
      expect(bar).toHaveAttribute("aria-valuetext", "42%");

      setSlicingProgress("sop-running", { message: "Slicing…" });
      await waitFor(() => expect(bar).toHaveAttribute("data-indeterminate"));
    });

    it("cancels a running operation", async () => {
      await open();
      addOperation(running());
      fireEvent.click(await within(panel()).findByRole("button", { name: "Cancel" }));
      await waitFor(() => expect(slicingStoreMock.cancelSliceOperation).toHaveBeenCalledWith("sop-running"));
    });

    it("cancels a queued operation", async () => {
      await open();
      addOperation(running({ state: "queued", startedAt: undefined }));
      expect(await within(panel()).findByRole("progressbar")).toHaveTextContent("Waiting to start…");
      fireEvent.click(within(panel()).getByRole("button", { name: "Cancel" }));
      await waitFor(() => expect(slicingStoreMock.cancelSliceOperation).toHaveBeenCalledWith("sop-running"));
    });

    it("says a running slice's log comes when it finishes, and there is nothing to copy yet", async () => {
      await open();
      addOperation(running());
      slicingStoreMock.loadOperationLog.mockImplementation(async () => ({ text: "", truncated: false, noiseLines: [] }));
      const row = await waitFor(() => rowFor("Plate 1: Lid"));
      fireEvent.click(within(row).getByRole("button", { name: "Show log" }));
      expect(await within(row).findByText("The log is saved when the slice finishes.")).toBeInTheDocument();
      fireEvent.click(within(row).getByRole("button", { name: "Copy log" }));
      expect(await within(row).findByText("There's nothing to copy. The log is saved when the slice finishes.")).toBeInTheDocument();
    });

    it("says why a cancel was refused", async () => {
      await open();
      addOperation(running());
      slicingStoreMock.cancelSliceOperation.mockRejectedValueOnce(commandError({
        code: "OPERATION_NOT_CANCELLABLE", message: "This slice has already finished.",
      }));
      fireEvent.click(await within(panel()).findByRole("button", { name: "Cancel" }));
      expect(await within(panel()).findByText("This slice has already finished.")).toBeInTheDocument();
    });

    it("keeps a succeeded operation's log folded, and shows a failed one's with its failure", async () => {
      await open();
      const succeeded = rowFor("Plate 1: Lid");
      expect(within(succeeded).getByRole("button", { name: "Show log" })).toHaveAttribute("aria-expanded", "false");
      expect(within(succeeded).queryByRole("region")).toBeNull();
      expect(succeeded).toHaveTextContent("Saved as a Slice Revision.");

      const failed = rowFor("Plate 2: Latch");
      expect(failed).toHaveTextContent("An object is outside the printable area.");
      expect(within(failed).getByRole("button", { name: "Hide log" })).toHaveAttribute("aria-expanded", "true");
      const log = within(failed).getByLabelText("Log for Plate 2: Latch");
      await waitFor(() => expect(log).toHaveTextContent("return_code -50"));
      // An old failure doesn't take focus when the panel opens.
      expect(document.activeElement).not.toBe(log);
    });

    it("dims the backend's noise lines without hiding them", async () => {
      await open();
      const log = within(rowFor("Plate 2: Latch")).getByLabelText("Log for Plate 2: Latch");
      await waitFor(() => expect(log).toHaveTextContent("Loading plate 1 of 1"));
      const noise = log.querySelectorAll("[data-noise]");
      expect(noise).toHaveLength(1);
      expect(noise[0].textContent).toBe("(orca-slicer:48213): Gtk-WARNING **: cannot open display: \n");
      expect(log.textContent).toBe(fixture.logs["sop-web-enclosure-latch"].text);
      expect(within(rowFor("Plate 2: Latch")).getByText("Dimmed lines are known, harmless messages.")).toBeInTheDocument();
    });

    it("expands a log and moves focus to it when its operation fails", async () => {
      await open();
      addOperation(running());
      await within(panel()).findByRole("progressbar");
      slicingStoreMock.loadOperationLog.mockImplementation(async () => ({ text: "[error] boom", truncated: false, noiseLines: [] }));
      setSlicingState({
        operations: [...fixture.operations, running({
          state: "failed",
          failure: { code: { kind: "timeout" }, message: "Slicing took longer than 30 minutes." },
          finishedAt: "2026-09-24T13:00:00Z",
        })],
      });
      const row = rowFor("Plate 1: Lid");
      const log = await within(row).findByLabelText("Log for Plate 1: Lid");
      await waitFor(() => expect(document.activeElement).toBe(log));
      expect(row).toHaveTextContent("Slicing took longer than 30 minutes.");
      await waitFor(() => expect(log).toHaveTextContent("[error] boom"));
      const description = document.getElementById(log.getAttribute("aria-describedby")!);
      expect(description).toHaveTextContent("Slicing took longer than 30 minutes.");
    });

    it("copies the log, loading it when folded", async () => {
      const writeText = vi.fn(async (_text: string) => {});
      Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
      await open();
      const succeeded = rowFor("Plate 1: Lid");
      fireEvent.click(within(succeeded).getByRole("button", { name: "Copy log" }));
      await waitFor(() => expect(writeText).toHaveBeenCalledWith(fixture.logs["sop-web-enclosure-lid"].text));
      expect(await within(succeeded).findByText("Log copied.")).toBeInTheDocument();
    });

    it("says so when the clipboard refuses", async () => {
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: { writeText: vi.fn(async () => { throw new Error("denied"); }) },
      });
      await open();
      const succeeded = rowFor("Plate 1: Lid");
      fireEvent.click(within(succeeded).getByRole("button", { name: "Copy log" }));
      expect(await within(succeeded).findByText("The log couldn't be copied.")).toBeInTheDocument();
    });

    it("shows the folded panel when a slice fails, and Escape folds it again", async () => {
      Object.defineProperty(window, "innerWidth", { configurable: true, writable: true, value: 1024 });
      render(() => <PreparationMode model={enclosure()} onBack={() => {}} />);
      const toggle = await screen.findByRole("button", { name: "Settings panel" });
      addOperation(running());
      slicingStoreMock.loadOperationLog.mockImplementation(async () => ({ text: "[error] boom", truncated: false, noiseLines: [] }));
      expect(screen.queryByRole("complementary", { name: "Preparation settings" })).toBeNull();
      setSlicingState({
        operations: [...fixture.operations, running({
          state: "failed",
          failure: { code: { kind: "spawnFailed" }, message: "farm3d couldn't start OrcaSlicer." },
        })],
      });
      const aside = await screen.findByRole("complementary", { name: "Preparation settings" });
      expect(toggle).toHaveAttribute("aria-expanded", "true");
      await waitFor(() => expect(document.activeElement).toBe(within(aside).getByLabelText("Log for Plate 1: Lid")));
      fireEvent.keyDown(document.activeElement!, { key: "Escape" });
      expect(screen.queryByRole("complementary", { name: "Preparation settings" })).toBeNull();
      expect(document.activeElement).toBe(toggle);
    });

    it("says so when a log went with its deleted Slice Revision", async () => {
      await open();
      slicingStoreMock.loadOperationLog.mockRejectedValue(commandError({
        code: "NOT_FOUND", message: "This slice's log was deleted with its Slice Revision.", recovery: ["RELOAD"],
      }));
      const succeeded = rowFor("Plate 1: Lid");
      fireEvent.click(within(succeeded).getByRole("button", { name: "Show log" }));
      expect(await within(succeeded).findByText("This slice's log was deleted with its Slice Revision.")).toBeInTheDocument();
    });

    it("shows only this Preparation's operations", async () => {
      await open();
      setSlicingState({
        operations: [...fixture.operations, running({ id: "sop-other", preparationId: "prp-other", plateName: "Elsewhere" })],
      });
      await waitFor(() => expect(within(panel()).getAllByRole("listitem")).toHaveLength(2));
      expect(within(panel()).queryByText(/Elsewhere/)).toBeNull();
    });
  });
});
