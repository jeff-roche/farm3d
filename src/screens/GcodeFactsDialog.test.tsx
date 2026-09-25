import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GcodeClaim } from "../generated/contracts/domain/GcodeClaim";
import { libraryStoreMock, resetLibraryStoreMock } from "../library/library-store-mock";
import { buildWebLibraryFixture } from "../library/web-fixtures";
import type { ResolvedPrinter } from "../printers/types";
import {
  loadWebSlicingFixture,
  refuseDesktopOnlyActions,
  resetSlicingStoreMock,
  slicingStoreMock,
} from "../slicing/slicing-store-mock";
import { buildWebSlicingFixture, WEB_SLICING_REVISION_EXTERNAL } from "../slicing/web-fixtures";
import { GcodeFactsDialog } from "./GcodeFactsDialog";
import { ModelDetailsPanel } from "./ModelDetailsPanel";

vi.mock("../library/library-store", async () => (await import("../library/library-store-mock")).libraryStoreMock);
vi.mock("../slicing/slicing-store", async () => (await import("../slicing/slicing-store-mock")).slicingStoreMock);
const printerState = vi.hoisted(() => ({ list: [] as unknown[] }));
vi.mock("../printers/printer-store", () => ({ printers: () => printerState.list }));
vi.mock("../printers/printer-catalog", () => ({
  listCatalogModels: vi.fn(async () => [
    { modelId: "Elegoo-CC", vendor: "Elegoo", model: "Elegoo Centauri Carbon" },
    { modelId: "Prusa-MK4", vendor: "Prusa", model: "Prusa MK4" },
  ]),
  listCatalogVariants: vi.fn(async (vendor: string) => (vendor === "Prusa"
    ? [{ variant: "Prusa MK4 0.4 nozzle", printerVariant: "0.4" }, { variant: "Prusa MK4 0.6 nozzle", printerVariant: "0.6" }]
    : [
        { variant: "Elegoo Centauri Carbon 0.4 nozzle", printerVariant: "0.4" },
        { variant: "Elegoo Centauri Carbon 0.6 nozzle", printerVariant: "0.6" },
      ])),
  previewProfile: vi.fn(async () => ({})),
}));
vi.mock("./ModelPlateInspector", () => ({ ModelPlateInspector: () => <p>Inspector</p> }));

const NOW = new Date("2026-09-24T12:00:00Z");
const library = buildWebLibraryFixture(NOW);
const CENTAURI = {
  vendor: "Elegoo",
  model: "Elegoo Centauri Carbon",
  variant: "Elegoo Centauri Carbon 0.4 nozzle",
  modelId: "Elegoo-CC",
  printerVariant: "0.4",
};

const CLAIMS: GcodeClaim[] = [
  { key: "printer_model", value: "Elegoo Centauri Carbon", line: 12 },
  { key: "nozzle_diameter", value: "0.4", line: 14 },
  { key: "filament_type", value: "PLA", line: 41_201 },
];

beforeEach(() => {
  resetLibraryStoreMock();
  resetSlicingStoreMock();
  loadWebSlicingFixture(NOW);
  printerState.list = [{ id: "prn-cc-1", name: "CC 1", catalogRef: { ...CENTAURI } } as unknown as ResolvedPrinter];
  libraryStoreMock.loadRevisions.mockImplementation(async (modelId: string) => library.revisions[modelId] ?? []);
  const record = buildWebSlicingFixture(NOW).revisionRecords[WEB_SLICING_REVISION_EXTERNAL];
  slicingStoreMock.createExternalSliceRevision.mockResolvedValue(record);
});
afterEach(cleanup);

function renderDialog(claims: GcodeClaim[] = CLAIMS) {
  const onCreated = vi.fn();
  const onOpenChange = vi.fn();
  render(() => (
    <GcodeFactsDialog
      open
      onOpenChange={onOpenChange}
      sourceRevisionId="msr-web-cube-gcode-1"
      sourceRevisionSequence={1}
      claims={claims}
      onCreated={onCreated}
    />
  ));
  return { onCreated, onOpenChange, dialog: screen.getByRole("dialog", { name: "Create Slice Revision" }) };
}

const row = (name: string) => screen.getByRole("group", { name });
const input = (name: string) => screen.getByRole("textbox", { name }) as HTMLInputElement;
const useFile = (fact: string) => screen.getByRole("button", { name: `Use the file's value for ${fact}` });
const submitButton = () => screen.getByRole("button", { name: "Create Slice Revision" });

/** What Tab reaches inside the dialog, in order (no positive tabindex is
 *  used, so this is document order). */
function tabbables(container: HTMLElement): HTMLElement[] {
  return [...container.querySelectorAll<HTMLElement>("button, input, [tabindex]")].filter((element) =>
    !(element as HTMLButtonElement).disabled
    && element.tabIndex >= 0
    // Kobalte's focus-trap sentinels only wrap focus around.
    && !element.hasAttribute("data-focus-trap")
    && !element.closest("[aria-hidden='true']"));
}

/** Tab: focus moves to the next tabbable element in the dialog. */
function tab(container: HTMLElement): HTMLElement {
  const order = tabbables(container);
  const next = order[order.indexOf(document.activeElement as HTMLElement) + 1] ?? order[0];
  next.focus();
  return next;
}

/** Enter on the focused control: a button activates (jsdom doesn't
 *  synthesize the click a browser does); a text field submits its form. */
function pressEnter() {
  const focused = document.activeElement as HTMLElement;
  fireEvent.keyDown(focused, { key: "Enter" });
  if (focused instanceof HTMLButtonElement) fireEvent.click(focused);
  else fireEvent.submit(focused.closest("form")!);
}

describe("GcodeFactsDialog", () => {
  it("starts empty, with the file's claims beside each fact, not verified", async () => {
    renderDialog();
    expect(screen.getByText("What the file says (not verified)")).toBeInTheDocument();
    expect(input("Nozzle diameter (mm)")).toHaveValue("");
    expect(input("Filament diameter (mm)")).toHaveValue("");
    for (const fact of ["Printer profile", "Nozzle diameter", "Material", "Filament diameter"]) {
      expect(row(fact)).toHaveTextContent("Not provided");
      expect(row(fact)).not.toHaveTextContent("Confirmed by you");
    }
    expect(row("Nozzle diameter")).toHaveTextContent("nozzle_diameter 0.4 (line 14)");
    expect(row("Material")).toHaveTextContent("filament_type PLA (line 41,201)");
    expect(row("Filament diameter")).toHaveTextContent("Not in the file.");
    expect(useFile("Filament diameter")).toBeDisabled();
    expect(screen.getByText("4 facts not provided: this Slice Revision will need a Printer chosen by hand when it is queued."))
      .toBeInTheDocument();
    // The printer claim is looked up in the catalog before it can be used.
    await waitFor(() => expect(useFile("Printer profile")).toBeEnabled());
  });

  it("copies a claim with Use the file's value, which stays editable and can be cleared", async () => {
    renderDialog();
    const badgeIcon = () => row("Nozzle diameter").querySelector("[data-provenance] svg")!.getAttribute("class");
    expect(badgeIcon()).toContain("circle-dashed");
    fireEvent.click(useFile("Nozzle diameter"));
    expect(input("Nozzle diameter (mm)")).toHaveValue("0.4");
    expect(row("Nozzle diameter")).toHaveTextContent("Confirmed by you");
    // The badge's icon follows its text, not only its colour.
    expect(badgeIcon()).toContain("user-check");
    await waitFor(() => expect(input("Nozzle diameter (mm)")).toHaveFocus());
    fireEvent.input(input("Nozzle diameter (mm)"), { target: { value: "0.6" } });
    expect(input("Nozzle diameter (mm)")).toHaveValue("0.6");

    fireEvent.click(useFile("Material"));
    expect(within(row("Material")).getByRole("button", { name: /PLA/ })).toBeInTheDocument();

    await waitFor(() => expect(useFile("Printer profile")).toBeEnabled());
    fireEvent.click(useFile("Printer profile"));
    expect(within(row("Printer profile")).getByRole("button", { name: /Elegoo Centauri Carbon 0\.4 nozzle/ })).toBeInTheDocument();
    expect(screen.getByText("1 fact not provided: this Slice Revision will need a Printer chosen by hand when it is queued."))
      .toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Clear Nozzle diameter" }));
    expect(input("Nozzle diameter (mm)")).toHaveValue("");
    expect(row("Nozzle diameter")).toHaveTextContent("Not provided");

    fireEvent.click(submitButton());
    await waitFor(() => expect(slicingStoreMock.createExternalSliceRevision).toHaveBeenCalledTimes(1));
    const [sourceRevisionId, facts] = slicingStoreMock.createExternalSliceRevision.mock.calls[0];
    expect(sourceRevisionId).toBe("msr-web-cube-gcode-1");
    expect(facts).toEqual({
      printerProfile: { kind: "confirmed", value: { kind: "profile", catalogRef: CENTAURI } },
      nozzleDiameterMm: { kind: "absent" },
      materialFamily: { kind: "confirmed", value: "PLA" },
      filamentDiameterMm: { kind: "absent" },
    });
  });

  it("disables Use the file's value for a claim it can't read, and says why", async () => {
    renderDialog([
      { key: "printer_model", value: "Unknown Printer 9000", line: 3 },
      { key: "nozzle_diameter", value: "0.4,0.6", line: 4 },
      { key: "filament_type", value: "PLA;PETG", line: 5 },
    ]);
    expect(useFile("Nozzle diameter")).toBeDisabled();
    expect(row("Nozzle diameter")).toHaveTextContent("farm3d can't read this as one diameter from 0 to 5 mm.");
    expect(useFile("Material")).toBeDisabled();
    expect(await within(row("Printer profile")).findByText("This printer isn't in farm3d's printer catalog.")).toBeInTheDocument();
    expect(useFile("Printer profile")).toBeDisabled();
  });

  it("can be completed keyboard-only: tab order, Use the file's value, errors focusing their field, and submit", async () => {
    const { dialog, onCreated } = renderDialog();
    await waitFor(() => expect(useFile("Printer profile")).toBeEnabled());
    const names = tabbables(dialog).map((element) => (element instanceof HTMLInputElement
      ? `input: ${element.labels?.[0]?.textContent}`
      : element.getAttribute("aria-label") ?? element.textContent?.trim()));
    expect(names).toEqual([
      "Close",
      "Use the file's value for Printer profile",
      "Not provided",
      "Use the file's value for Nozzle diameter",
      "input: Nozzle diameter (mm)",
      "Use the file's value for Material",
      "Not provided",
      "input: Filament diameter (mm)",
      "Cancel",
      "Create Slice Revision",
    ]);

    // The dialog opens with focus on its first control.
    await waitFor(() => expect(screen.getByRole("button", { name: "Close" })).toHaveFocus());
    expect(tab(dialog)).toBe(useFile("Printer profile"));
    tab(dialog);
    expect(tab(dialog)).toBe(useFile("Nozzle diameter"));
    pressEnter();
    await waitFor(() => expect(input("Nozzle diameter (mm)")).toHaveFocus());
    fireEvent.input(input("Nozzle diameter (mm)"), { target: { value: "7" } });

    // Enter in the field submits; the bad value is refused here and keeps focus.
    pressEnter();
    expect(await within(row("Nozzle diameter")).findByText("Enter the nozzle diameter in millimetres: more than 0, at most 5.")).toBeInTheDocument();
    await waitFor(() => expect(input("Nozzle diameter (mm)")).toHaveFocus());
    expect(slicingStoreMock.createExternalSliceRevision).not.toHaveBeenCalled();

    fireEvent.input(input("Nozzle diameter (mm)"), { target: { value: "0.4" } });
    // A filled fact can be cleared, so Clear joins the order after its field.
    expect(tab(dialog)).toBe(screen.getByRole("button", { name: "Clear Nozzle diameter" }));
    expect(tab(dialog)).toBe(useFile("Material"));
    pressEnter();
    let focused = tab(dialog);
    while (focused !== submitButton()) focused = tab(dialog);
    pressEnter();
    await waitFor(() => expect(onCreated).toHaveBeenCalledTimes(1));
    expect(slicingStoreMock.createExternalSliceRevision.mock.calls[0][1]).toMatchObject({
      nozzleDiameterMm: { kind: "confirmed", value: 0.4 },
      materialFamily: { kind: "confirmed", value: "PLA" },
    });
  });

  it("validates OTHER's material name before sending", async () => {
    renderDialog([{ key: "filament_type", value: "PLA+", line: 5 }]);
    fireEvent.click(useFile("Material"));
    const name = input("Material name");
    expect(name).toHaveValue("PLA+");
    fireEvent.input(name, { target: { value: "  " } });
    fireEvent.click(submitButton());
    expect(await screen.findByText("Name the material in 1 to 32 characters.")).toBeInTheDocument();
    await waitFor(() => expect(input("Material name")).toHaveFocus());
    expect(slicingStoreMock.createExternalSliceRevision).not.toHaveBeenCalled();
  });

  it("shows a backend VALIDATION on the field its fieldPath names, and focuses it", async () => {
    renderDialog();
    for (const [fieldPath, focusedName] of [
      ["facts.filamentDiameterMm", "Filament diameter (mm)"],
      ["facts.printerProfile", undefined],
    ] as const) {
      slicingStoreMock.createExternalSliceRevision.mockRejectedValueOnce({
        contractVersion: 1, code: "VALIDATION", recovery: [], retryable: false,
        message: fieldPath === "facts.printerProfile" ? "That Printer no longer exists." : "Must be greater than 0 mm and at most 5 mm.",
        details: { fieldPath },
      });
      fireEvent.click(submitButton());
      if (focusedName) {
        expect(await within(row("Filament diameter")).findByText("Must be greater than 0 mm and at most 5 mm.")).toBeInTheDocument();
        await waitFor(() => expect(input(focusedName)).toHaveFocus());
      } else {
        expect(await within(row("Printer profile")).findByText("That Printer no longer exists.")).toBeInTheDocument();
        await waitFor(() => expect(within(row("Printer profile")).getByRole("button", { name: /Not provided/ })).toHaveFocus());
      }
    }
  });

  it("retries with the same operation id after the backend couldn't be reached, and a new one once the facts change", async () => {
    renderDialog();
    slicingStoreMock.createExternalSliceRevision.mockRejectedValueOnce(new Error("transport closed"));
    fireEvent.click(useFile("Nozzle diameter"));
    fireEvent.click(submitButton());
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("farm3d couldn't reach its backend. Your answers are kept.");

    slicingStoreMock.createExternalSliceRevision.mockRejectedValueOnce(new Error("transport closed"));
    fireEvent.click(within(alert).getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(slicingStoreMock.createExternalSliceRevision).toHaveBeenCalledTimes(2));
    const [first, second] = slicingStoreMock.createExternalSliceRevision.mock.calls;
    expect(first[2]).toEqual(expect.any(String));
    expect(second[2]).toBe(first[2]);

    await screen.findByRole("alert");
    fireEvent.input(input("Filament diameter (mm)"), { target: { value: "1.75" } });
    fireEvent.click(submitButton());
    await waitFor(() => expect(slicingStoreMock.createExternalSliceRevision).toHaveBeenCalledTimes(3));
    expect(slicingStoreMock.createExternalSliceRevision.mock.calls[2][2]).not.toBe(first[2]);
  });

  it("in web mode, says creating needs the desktop app and keeps the answers", async () => {
    refuseDesktopOnlyActions();
    const { onCreated } = renderDialog();
    fireEvent.click(useFile("Nozzle diameter"));
    fireEvent.click(submitButton());
    expect(await screen.findByRole("alert")).toHaveTextContent("Creating a Slice Revision needs the desktop app.");
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
    expect(input("Nozzle diameter (mm)")).toHaveValue("0.4");
    expect(onCreated).not.toHaveBeenCalled();
  });
});

describe("Create Slice Revision… in a G-code Model's details", () => {
  it("opens the dialog, and the new revision's review once it is created", async () => {
    const model = library.models.find((candidate) => candidate.id === "mdl-web-cube-gcode")!;
    render(() => (
      <ModelDetailsPanel
        model={model}
        projects={library.projects}
        onLocateSource={vi.fn()}
        onConvertToManaged={vi.fn()}
        onDelete={vi.fn()}
      />
    ));
    // The claims come from the current revision's inspection.
    await screen.findByRole("region", { name: "What the file says (not verified)" });
    fireEvent.click(screen.getByRole("button", { name: "Create Slice Revision…" }));
    const dialog = await screen.findByRole("dialog", { name: "Create Slice Revision" }, { timeout: 5000 });
    expect(within(dialog).getByRole("group", { name: "Material" })).toHaveTextContent("filament_type PLA");
    fireEvent.click(within(dialog).getByRole("button", { name: "Create Slice Revision" }));
    expect(await screen.findByRole("heading", { level: 3, name: "External G-code" }, { timeout: 5000 })).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});
