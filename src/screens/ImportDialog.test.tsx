import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ImportDialog } from "./ImportDialog";
import {
  emitImportProgress,
  libraryStoreMock,
  resetLibraryStoreMock,
  setLibraryState,
} from "../library/library-store-mock";
import { model, project } from "../library/test-records";
import type { CommandError } from "../generated/contracts/command/CommandError";
import type {
  ImportCandidate,
  ImportInspection,
  ImportItemResult,
  ImportSelectionSummary,
} from "../library/types";

vi.mock("../library/library-store", async () => (await import("../library/library-store-mock")).libraryStoreMock);

const SELECTION: ImportSelectionSummary = {
  selectionId: "sel-1",
  purpose: "import",
  files: [
    { fileIndex: 0, fileName: "bracket.stl", sizeBytes: 1024 },
    { fileIndex: 1, fileName: "cube.stl", sizeBytes: 2048 },
  ],
};

const BRACKETS = project({ id: "prj-b", name: "Brackets" });
const CALIBRATION = project({ id: "prj-c", name: "Calibration" });
const EXISTING_CUBE = model({ id: "mdl-existing", name: "Cube", projectIds: ["prj-b"], revision: 4 });

function ready(fileIndex: number, fileName: string, overrides: Partial<Extract<ImportCandidate, { status: "ready" }>> = {}): ImportCandidate {
  return {
    status: "ready",
    fileIndex,
    fileName,
    format: "stl",
    sizeBytes: 684,
    sha256: String(fileIndex).repeat(64),
    summary: { format: "stl", triangleCount: 12, boundsMm: { min: [0, 0, 0], max: [10, 10, 10] }, unitsAssumed: true },
    unsupported: [],
    warnings: [],
    duplicates: [],
    ...overrides,
  };
}

const DUPLICATE_OF_CUBE = {
  modelId: "mdl-existing",
  modelName: "Cube",
  projectIds: ["prj-b"],
  revisionId: "msr-existing",
  sequence: 1,
  isCurrent: true,
};

function inspection(...items: ImportCandidate[]): ImportInspection {
  return { selectionId: "sel-1", items };
}

function commandError(code: CommandError["code"], message: string): CommandError {
  return { contractVersion: 1, code, message, recovery: [], retryable: false };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function renderDialog(options: { defaultProjectIds?: string[] } = {}) {
  const [selection, setSelection] = createSignal<ImportSelectionSummary | null>(SELECTION);
  const onClose = vi.fn(() => setSelection(null));
  const onDone = vi.fn((_modelId: string | undefined) => setSelection(null));
  const onChooseAgain = vi.fn();
  render(() => (
    <ImportDialog
      selection={selection()}
      defaultProjectIds={options.defaultProjectIds ?? ["prj-b"]}
      onClose={onClose}
      onDone={onDone}
      onChooseAgain={onChooseAgain}
    />
  ));
  return { onClose, onDone, onChooseAgain, setSelection };
}

function currentStep(): string {
  const steps = screen.getByRole("list", { name: "Import steps" });
  return steps.querySelector("[aria-current='step']")?.textContent ?? "";
}

function row(fileName: string): HTMLElement {
  return screen.getByRole("listitem", { name: fileName });
}

async function pick(combobox: HTMLElement, option: string) {
  const trigger = within(combobox.parentElement!).getByRole("button", { name: /show suggestions/i });
  await fireEvent.pointerDown(trigger, { pointerType: "mouse", button: 0 });
  await fireEvent.pointerUp(await screen.findByRole("option", { name: option }), { pointerType: "mouse", button: 0 });
}

function tokens(fileName: string): string[] {
  return within(row(fileName))
    .queryAllByRole("button", { name: /^Remove / })
    .map((button) => button.getAttribute("aria-label")!.replace(/^Remove /, ""));
}

async function toReview(...items: ImportCandidate[]) {
  libraryStoreMock.inspectSelection.mockResolvedValue(inspection(...items));
  const handles = renderDialog();
  await waitFor(() => expect(currentStep()).toContain("Review"));
  return handles;
}

beforeEach(() => {
  resetLibraryStoreMock();
  setLibraryState({ projects: [BRACKETS, CALIBRATION], models: [EXISTING_CUBE] });
  libraryStoreMock.inspectSelection.mockReset().mockImplementation(async (selectionId: string) => ({ selectionId, items: [] }));
  libraryStoreMock.importModels.mockReset().mockResolvedValue({ items: [] });
  libraryStoreMock.cancelSelection.mockReset().mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("ImportDialog", () => {
  describe("Files step", () => {
    it("opens on Files, inspects the selection, and shows each file's progress as text", async () => {
      const pending = deferred<ImportInspection>();
      libraryStoreMock.inspectSelection.mockReturnValue(pending.promise);
      renderDialog();

      expect(screen.getByRole("dialog", { name: "Import Models" })).toBeInTheDocument();
      expect(currentStep()).toContain("Files");
      expect(libraryStoreMock.inspectSelection).toHaveBeenCalledWith("sel-1");

      emitImportProgress("sel-1", { fileIndex: 1, bytesDone: 1024, bytesTotal: 2048 });
      emitImportProgress("sel-other", { fileIndex: 0, bytesDone: 1024, bytesTotal: 1024 });
      const cube = await screen.findByRole("progressbar", { name: "cube.stl" });
      await waitFor(() => expect(cube).toHaveAttribute("aria-valuenow", "50"));
      expect(within(cube).getByText("50%")).toBeInTheDocument();
      // A progress event for another selection is ignored.
      expect(screen.getByRole("progressbar", { name: "bracket.stl" })).toHaveAttribute("aria-valuenow", "0");

      pending.resolve(inspection(ready(0, "bracket.stl"), ready(1, "cube.stl")));
      await waitFor(() => expect(currentStep()).toContain("Review"));
    });

    it("Cancel during inspection cancels the selection and closes the dialog", async () => {
      libraryStoreMock.inspectSelection.mockReturnValue(new Promise(() => {}));
      const { onClose } = renderDialog();

      await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
      expect(libraryStoreMock.cancelSelection).toHaveBeenCalledWith("sel-1");
      expect(onClose).toHaveBeenCalled();
      await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    });

    it("offers Choose files again when the selection has expired", async () => {
      libraryStoreMock.inspectSelection.mockRejectedValue(commandError("SELECTION_EXPIRED", "This selection has expired."));
      const { onChooseAgain } = renderDialog();

      await fireEvent.click(await screen.findByRole("button", { name: "Choose files again" }));
      expect(onChooseAgain).toHaveBeenCalled();
    });

    it("shows any other inspection failure inline, with Try again", async () => {
      libraryStoreMock.inspectSelection
        .mockRejectedValueOnce(commandError("PERSISTENCE_UNAVAILABLE", "Importing Models needs the desktop app."))
        .mockResolvedValueOnce(inspection(ready(0, "bracket.stl")));
      renderDialog();

      expect(await screen.findByText("Importing Models needs the desktop app.")).toBeInTheDocument();
      await fireEvent.click(screen.getByRole("button", { name: "Try again" }));
      await waitFor(() => expect(currentStep()).toContain("Review"));
      expect(libraryStoreMock.inspectSelection).toHaveBeenCalledTimes(2);
    });
  });

  describe("Review step", () => {
    it("shows each row's name, Projects (defaulting to the viewed Project) and Managed storage", async () => {
      await toReview(ready(0, "bracket.stl"));

      expect(within(row("bracket.stl")).getByRole("textbox", { name: "Name" })).toHaveValue("bracket");
      expect(within(row("bracket.stl")).getByRole("combobox", { name: "Projects" })).toBeInTheDocument();
      expect(tokens("bracket.stl")).toEqual(["Brackets"]);
      expect(within(row("bracket.stl")).getByRole("radio", { name: "Managed" })).toBeChecked();
      expect(within(row("bracket.stl")).getByRole("radio", { name: "Linked" })).not.toBeChecked();
    });

    it("defaults to no Projects (Unfiled) in a saved view", async () => {
      libraryStoreMock.inspectSelection.mockResolvedValue(inspection(ready(0, "bracket.stl")));
      render(() => (
        <ImportDialog selection={SELECTION} defaultProjectIds={[]} onClose={vi.fn()} onDone={vi.fn()} onChooseAgain={vi.fn()} />
      ));
      await waitFor(() => expect(currentStep()).toContain("Review"));
      expect(tokens("bracket.stl")).toEqual([]);
    });

    it("sends both Projects chosen on a row, and the row's other choices", async () => {
      await toReview(ready(0, "bracket.stl"));

      await pick(within(row("bracket.stl")).getByRole("combobox", { name: "Projects" }), "Calibration");
      expect(tokens("bracket.stl")).toEqual(["Brackets", "Calibration"]);
      await fireEvent.click(within(row("bracket.stl")).getByRole("radio", { name: "Linked" }));
      await fireEvent.click(screen.getByRole("button", { name: "Import" }));

      await waitFor(() => expect(libraryStoreMock.importModels).toHaveBeenCalled());
      const [selectionId, items, operationId] = libraryStoreMock.importModels.mock.calls[0]!;
      expect(selectionId).toBe("sel-1");
      expect(typeof operationId).toBe("string");
      expect(items).toEqual([{
        fileIndex: 0, name: "bracket", projectIds: ["prj-b", "prj-c"], storageMode: "linked", acknowledgeUnsupported: false,
      }]);
    });

    it("Apply Projects to all rows copies one row's Projects to every other row", async () => {
      await toReview(ready(0, "bracket.stl"), ready(1, "lid.stl"));

      await pick(within(row("bracket.stl")).getByRole("combobox", { name: "Projects" }), "Calibration");
      await fireEvent.click(within(row("bracket.stl")).getByRole("button", { name: "Apply Projects to all rows" }));
      expect(tokens("lid.stl")).toEqual(["Brackets", "Calibration"]);
    });

    it("a rejected row shows its reason and has no controls", async () => {
      await toReview(
        ready(0, "bracket.stl"),
        { status: "rejected", fileIndex: 1, fileName: "notes.txt", code: "UNSUPPORTED_FORMAT", message: "notes.txt is not a supported format." },
      );

      const rejected = row("notes.txt");
      expect(within(rejected).getByText("notes.txt is not a supported format.")).toBeInTheDocument();
      expect(within(rejected).queryByRole("textbox")).toBeNull();
      expect(within(rejected).queryByRole("combobox")).toBeNull();
      expect(within(rejected).queryByRole("radio")).toBeNull();
    });

    it("a duplicate row offers the three actions with none chosen, and Import waits for a choice", async () => {
      await toReview(ready(0, "bracket.stl"), ready(1, "cube.stl", { duplicates: [DUPLICATE_OF_CUBE] }));

      const duplicate = row("cube.stl");
      const actions = within(duplicate).getByRole("radiogroup", { name: "Duplicate file" });
      expect(within(actions).getAllByRole("radio")).toHaveLength(3);
      for (const name of ["Use existing", "Add as another Model", "Add as a new revision of…"]) {
        expect(within(actions).getByRole("radio", { name })).not.toBeChecked();
      }

      const importButton = screen.getByRole("button", { name: "Import" });
      expect(importButton).toBeDisabled();
      expect(screen.getByText("Choose what to do with 1 duplicate file.")).toBeInTheDocument();

      await fireEvent.click(within(actions).getByRole("radio", { name: "Use existing" }));
      expect(importButton).toBeEnabled();
      await fireEvent.click(importButton);
      await waitFor(() => expect(libraryStoreMock.importModels).toHaveBeenCalled());
      expect(libraryStoreMock.importModels.mock.calls[0]![1][1]).toMatchObject({
        fileIndex: 1, duplicateAction: "useExisting", targetModelId: "mdl-existing",
      });
    });

    it("Add as a new revision of… reveals the Model picker, pre-filled with the same-name Model", async () => {
      await toReview(ready(1, "cube.stl", { duplicates: [DUPLICATE_OF_CUBE] }));

      expect(within(row("cube.stl")).queryByRole("combobox", { name: "Model" })).toBeNull();
      await fireEvent.click(within(row("cube.stl")).getByRole("radio", { name: "Add as a new revision of…" }));
      expect(within(row("cube.stl")).getByRole("combobox", { name: "Model" })).toHaveValue("Cube");

      await fireEvent.click(screen.getByRole("button", { name: "Import" }));
      await waitFor(() => expect(libraryStoreMock.importModels).toHaveBeenCalled());
      expect(libraryStoreMock.importModels.mock.calls[0]![1][0]).toMatchObject({
        duplicateAction: "addRevision", targetModelId: "mdl-existing", targetExpectedRevision: 4,
      });
    });

    it("offers Add as a new revision of… for a non-duplicate row when a Model could take it, without choosing it", async () => {
      await toReview(ready(0, "bracket.stl"));

      const choice = within(row("bracket.stl")).getByRole("radiogroup", { name: "Import as" });
      expect(within(choice).getByRole("radio", { name: "New Model" })).toBeChecked();
      await fireEvent.click(within(choice).getByRole("radio", { name: "Add as a new revision of…" }));
      await pick(within(row("bracket.stl")).getByRole("combobox", { name: "Model" }), "Cube");
      await fireEvent.click(screen.getByRole("button", { name: "Import" }));

      await waitFor(() => expect(libraryStoreMock.importModels).toHaveBeenCalled());
      expect(libraryStoreMock.importModels.mock.calls[0]![1][0]).toMatchObject({
        duplicateAction: "addRevision", targetModelId: "mdl-existing",
      });
    });

    it("lists a rich 3MF's unsupported contents in a disclosure, and its acknowledgment gates Import", async () => {
      await toReview(ready(0, "plate.3mf", {
        format: "3mf",
        summary: {
          format: "3mf", objectCount: 2, plateCount: 1, triangleCount: 24,
          boundsMm: { min: [0, 0, 0], max: [10, 10, 10] }, unsupportedCount: 2,
        },
        unsupported: [
          { part: "Metadata/Slic3r_PE.config", code: "SLICER_SETTINGS", detail: "PrusaSlicer print settings" },
          { part: "Metadata/plate_1.gcode", code: "EMBEDDED_GCODE", detail: "Embedded sliced G-code" },
        ],
      }));

      const plate = row("plate.3mf");
      const importButton = screen.getByRole("button", { name: "Import" });
      expect(importButton).toBeDisabled();

      await fireEvent.click(within(plate).getByRole("button", { name: /Unsupported contents/ }));
      expect(within(plate).getByText("Kept in the stored file. farm3d won't use these when slicing.")).toBeInTheDocument();
      expect(within(plate).getByText("PrusaSlicer print settings")).toBeInTheDocument();
      expect(within(plate).getByText("Embedded sliced G-code")).toBeInTheDocument();

      await fireEvent.click(within(plate).getByRole("checkbox"));
      expect(importButton).toBeEnabled();
      await fireEvent.click(importButton);
      await waitFor(() => expect(libraryStoreMock.importModels).toHaveBeenCalled());
      expect(libraryStoreMock.importModels.mock.calls[0]![1][0]).toMatchObject({ acknowledgeUnsupported: true });
    });

    it("says a G-code row is stored as pre-sliced G-code", async () => {
      await toReview(ready(0, "cube.gcode", {
        format: "gcode",
        summary: { format: "gcode", producer: null, lineCount: 1200, claimedPrinterModel: null, claimedEstimatedTime: null },
      }));
      expect(within(row("cube.gcode")).getByText("Stored as pre-sliced G-code. It won't be sliced.")).toBeInTheDocument();
    });
  });

  describe("Results step", () => {
    const IMPORTED: ImportItemResult = {
      fileIndex: 0, outcome: "imported", model: model({ id: "mdl-new", name: "bracket" }), errors: [], warnings: [],
    };

    it("shows each row's outcome, and Retry resends only the failed row with a new operationId", async () => {
      await toReview(ready(0, "bracket.stl"), ready(1, "lid.stl"));
      libraryStoreMock.importModels
        .mockResolvedValueOnce({
          items: [
            IMPORTED,
            { fileIndex: 1, outcome: "rejected", errors: [{ code: "SOURCE_UNREADABLE", message: "lid.stl could not be read." }], warnings: [] },
          ],
        })
        .mockResolvedValueOnce({
          items: [{ fileIndex: 1, outcome: "imported", model: model({ id: "mdl-lid", name: "Lid" }), errors: [], warnings: [] }],
        });

      await fireEvent.click(within(row("lid.stl")).getByRole("radio", { name: "Linked" }));
      await fireEvent.click(screen.getByRole("button", { name: "Import" }));
      await waitFor(() => expect(currentStep()).toContain("Results"));

      expect(within(row("bracket.stl")).getByText("Imported as “bracket”")).toBeInTheDocument();
      const failed = row("lid.stl");
      expect(within(failed).getByText("lid.stl could not be read.")).toBeInTheDocument();
      // The failed row keeps the user's choices.
      expect(within(failed).getByRole("radio", { name: "Linked" })).toBeChecked();

      await fireEvent.click(within(failed).getByRole("button", { name: "Retry" }));
      await waitFor(() => expect(libraryStoreMock.importModels).toHaveBeenCalledTimes(2));
      const [first, second] = libraryStoreMock.importModels.mock.calls;
      expect(second![0]).toBe("sel-1");
      expect(second![1]).toEqual([{
        fileIndex: 1, name: "lid", projectIds: ["prj-b"], storageMode: "linked", acknowledgeUnsupported: false,
      }]);
      expect(second![2]).toEqual(expect.any(String));
      expect(second![2]).not.toBe(first![2]);
      expect(await within(row("lid.stl")).findByText("Imported as “Lid”")).toBeInTheDocument();
    });

    it("Done closes the dialog and selects the first imported Model", async () => {
      const { onDone } = await toReview(ready(0, "bracket.stl"));
      libraryStoreMock.importModels.mockResolvedValue({ items: [IMPORTED] });

      await fireEvent.click(screen.getByRole("button", { name: "Import" }));
      await fireEvent.click(await screen.findByRole("button", { name: "Done" }));
      expect(onDone).toHaveBeenCalledWith("mdl-new");
    });

    it("lets the user decide a duplicate the commit found, then retries that row alone", async () => {
      // Two identical files: inspection saw no duplicate, but the second
      // meets the first one's content at commit time.
      const twin = ready(1, "twin.stl", { sha256: "f".repeat(64) });
      await toReview(ready(0, "bracket.stl", { sha256: "f".repeat(64) }), twin);
      const holder = model({ id: "mdl-new", name: "bracket" });
      holder.currentRevision = { ...holder.currentRevision, sha256: "f".repeat(64) };
      libraryStoreMock.importModels.mockImplementationOnce(async () => {
        setLibraryState({ models: [EXISTING_CUBE, holder] });
        return {
          items: [
            { ...IMPORTED, model: holder },
            {
              fileIndex: 1,
              outcome: "rejected",
              errors: [{ code: "DUPLICATE_DECISION_REQUIRED", message: "The Library already has this file. Choose what to do with it." }],
              warnings: [],
            },
          ],
        };
      });

      await fireEvent.click(screen.getByRole("button", { name: "Import" }));
      const failed = await screen.findByRole("listitem", { name: "twin.stl" });
      await waitFor(() => expect(currentStep()).toContain("Results"));
      expect(within(failed).getByText("The Library already has this file. Choose what to do with it.")).toBeInTheDocument();
      const retry = within(failed).getByRole("button", { name: "Retry" });
      expect(retry).toBeDisabled();

      await fireEvent.click(within(failed).getByRole("radio", { name: "Use existing" }));
      expect(retry).toBeEnabled();
      await fireEvent.click(retry);
      await waitFor(() => expect(libraryStoreMock.importModels).toHaveBeenCalledTimes(2));
      expect(libraryStoreMock.importModels.mock.calls[1]![1]).toEqual([expect.objectContaining({
        fileIndex: 1, duplicateAction: "useExisting", targetModelId: "mdl-new",
      })]);
    });

    it("shows a late re-commit's PERSISTENCE_UNAVAILABLE as an inline row error", async () => {
      await toReview(ready(0, "bracket.stl"), ready(1, "lid.stl"));
      libraryStoreMock.importModels
        .mockResolvedValueOnce({
          items: [IMPORTED, { fileIndex: 1, outcome: "cancelled", errors: [], warnings: [] }],
        })
        .mockResolvedValueOnce({
          items: [{
            fileIndex: 1,
            outcome: "rejected",
            errors: [{ code: "PERSISTENCE_UNAVAILABLE", message: "farm3d couldn't save this file to the Library." }],
            warnings: [],
          }],
        });

      await fireEvent.click(screen.getByRole("button", { name: "Import" }));
      await waitFor(() => expect(currentStep()).toContain("Results"));
      expect(within(row("lid.stl")).getByText("Cancelled")).toBeInTheDocument();
      await fireEvent.click(within(row("lid.stl")).getByRole("button", { name: "Retry" }));
      expect(await within(row("lid.stl")).findByText("farm3d couldn't save this file to the Library.")).toBeInTheDocument();
      expect(within(row("bracket.stl")).getByText("Imported as “bracket”")).toBeInTheDocument();
    });

    it("offers Choose files again when the selection expired before the import", async () => {
      const { onChooseAgain } = await toReview(ready(0, "bracket.stl"));
      libraryStoreMock.importModels.mockRejectedValue(commandError("SELECTION_EXPIRED", "This selection has expired."));

      await fireEvent.click(screen.getByRole("button", { name: "Import" }));
      await fireEvent.click(await screen.findByRole("button", { name: "Choose files again" }));
      expect(onChooseAgain).toHaveBeenCalled();
    });

    it("shows any other import failure inline and keeps the rows", async () => {
      await toReview(ready(0, "bracket.stl"));
      libraryStoreMock.importModels.mockRejectedValue(commandError("CONFLICT", "Another import is running."));

      await fireEvent.click(screen.getByRole("button", { name: "Import" }));
      expect(await screen.findByText("Another import is running.")).toBeInTheDocument();
      expect(within(row("bracket.stl")).getByRole("textbox", { name: "Name" })).toHaveValue("bracket");
    });
  });

  it("closing the dialog cancels the selection", async () => {
    const { onClose } = await toReview(ready(0, "bracket.stl"));
    await fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(libraryStoreMock.cancelSelection).toHaveBeenCalledWith("sel-1");
    expect(onClose).toHaveBeenCalled();
  });
});
