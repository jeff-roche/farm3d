import { createSignal } from "solid-js";
import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import type { ResolvedPrinter } from "../printers/types";
import {
  MaterialSlotsEditor,
  MaterialSlotsSection,
  MULTI_MATERIAL_HINT,
  type InitialLoad,
  type SlotDraft,
} from "./MaterialSlotsEditor";

const moveSpool = vi.hoisted(() => vi.fn());
const spools = vi.hoisted(() => [] as SpoolRecord[]);
vi.mock("../spools/spool-store", () => ({
  get spoolState() {
    return { spools, loaded: true };
  },
  ensureInventoryLoaded: () => Promise.resolve(),
  moveSpool: (...args: unknown[]) => moveSpool(...args),
}));

const setSlotLayout = vi.hoisted(() => vi.fn());
const reloadPrinter = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
vi.mock("../printers/printer-store", () => ({
  setSlotLayout: (...args: unknown[]) => setSlotLayout(...args),
  reloadPrinter: (...args: unknown[]) => reloadPrinter(...args),
  printers: () => [],
}));

// The real dialog is covered by its own tests; here only what it was opened with matters.
vi.mock("./MoveSpoolDialog", () => ({
  MoveSpoolDialog: (props: { open: boolean; spool: SpoolRecord; presetDestination?: { printerId: string; slotId: string } }) => (
    <>{props.open ? <div data-testid="move-dialog">{`${props.spool.id} -> ${props.presetDestination?.printerId}/${props.presetDestination?.slotId}`}</div> : null}</>
  ),
}));

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
  spools.length = 0;
});

function spool(overrides: Partial<SpoolRecord>): SpoolRecord {
  return {
    id: "spl-1", revision: 1, spoolNumber: 1,
    manufacturer: "Prusament", materialFamily: "PLA", colorName: "Galaxy Black", colorHex: "#0B0B0F", diameter: "1.75",
    nominalMg: 1_000_000, lowThresholdMg: 100_000, lifecycle: "active",
    location: { kind: "storage", storageLabel: null },
    availability: { currentMg: 812_000, reservedMg: 0, availableMg: 812_000 },
    facets: { loaded: false, reserved: false, low: false, confidence: "measured" },
    createdAt: "", updatedAt: "",
    ...overrides,
  };
}

function renderLayout(initial: SlotDraft[], extra: { multiMaterialHint?: boolean } = {}) {
  const [slots, setSlots] = createSignal(initial);
  const onChange = vi.fn((next: SlotDraft[]) => setSlots(next));
  render(() => (
    <MaterialSlotsEditor mode="layout" slots={slots()} onChange={onChange} multiMaterialHint={extra.multiMaterialHint ?? false} />
  ));
  return { slots, onChange };
}

const draft = (key: string, name: string, feederLabel = "", id?: string): SlotDraft => ({ key, name, feederLabel, ...(id ? { id } : {}) });

async function pick(trigger: HTMLElement, option: RegExp) {
  await fireEvent.pointerDown(trigger, { button: 0, pointerType: "mouse" });
  const item = await screen.findByRole("option", { name: option });
  await fireEvent.pointerDown(item, { button: 0, pointerType: "mouse" });
  await fireEvent.pointerUp(item, { button: 0, pointerType: "mouse" });
}

describe("MaterialSlotsEditor — layout mode", () => {
  it("adds, renames, sets a feeder label, reorders with buttons, and removes", async () => {
    const { slots } = renderLayout([draft("a", "Main")]);

    fireEvent.click(screen.getByRole("button", { name: "Add slot" }));
    expect(slots().map((s) => s.name)).toEqual(["Main", "Slot 2"]);

    fireEvent.input(screen.getByLabelText("Name for slot 2"), { target: { value: "Left" } });
    fireEvent.input(screen.getByLabelText("Feeder label for slot 2"), { target: { value: "AMS 1" } });
    expect(slots()[1]).toMatchObject({ name: "Left", feederLabel: "AMS 1" });

    fireEvent.click(screen.getByRole("button", { name: "Move Left up" }));
    expect(slots().map((s) => s.name)).toEqual(["Left", "Main"]);
    fireEvent.click(screen.getByRole("button", { name: "Move Left down" }));
    expect(slots().map((s) => s.name)).toEqual(["Main", "Left"]);

    fireEvent.click(screen.getByRole("button", { name: "Remove Main" }));
    expect(slots().map((s) => s.name)).toEqual(["Left"]);
    // The last slot can't be removed: every Printer has at least one (D4).
    expect(screen.getByRole("button", { name: "Remove Left" })).toBeDisabled();
  });

  it("reorders with Alt+Arrow keys from inside a row, keeping focus on the moved slot", async () => {
    const { slots } = renderLayout([draft("a", "Main"), draft("b", "Second")]);
    const second = screen.getByLabelText("Name for slot 2");
    second.focus();

    fireEvent.keyDown(second, { key: "ArrowUp", altKey: true });
    expect(slots().map((s) => s.name)).toEqual(["Second", "Main"]);
    expect(document.activeElement).toBe(screen.getByLabelText("Name for slot 1"));
    expect((document.activeElement as HTMLInputElement).value).toBe("Second");

    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown", altKey: true });
    expect(slots().map((s) => s.name)).toEqual(["Main", "Second"]);
  });

  it("disables the 17th add", () => {
    renderLayout(Array.from({ length: 16 }, (_, i) => draft(`k${i}`, `Slot ${i + 1}`)));
    expect(screen.getByRole("button", { name: "Add slot" })).toBeDisabled();
    expect(screen.getByText("16 slots is the most a Printer can have.")).toBeInTheDocument();
  });

  it("shows an inline error for a name that differs from another only by case", () => {
    renderLayout([draft("a", "Main"), draft("b", "MAIN")]);
    expect(screen.getAllByText("Another slot already has this name")).toHaveLength(2);
  });

  it("shows the multi-material hint only when asked to", () => {
    renderLayout([draft("a", "Main")], { multiMaterialHint: true });
    expect(screen.getByText(MULTI_MATERIAL_HINT)).toBeInTheDocument();
    document.body.innerHTML = "";
    renderLayout([draft("a", "Main")], { multiMaterialHint: false });
    expect(screen.queryByText(MULTI_MATERIAL_HINT)).not.toBeInTheDocument();
  });

  it("offers only active storage Spools as initial loads, and remaps a load when its slot moves", async () => {
    spools.push(
      spool({ id: "spl-ok", spoolNumber: 2, materialFamily: "PETG", colorName: "Clear" }),
      spool({ id: "spl-loaded", spoolNumber: 3, location: { kind: "slot", slotId: "x", printerId: "p" } }),
      spool({ id: "spl-empty", spoolNumber: 4, lifecycle: "empty" }),
    );
    const [slots, setSlots] = createSignal([draft("a", "Main"), draft("b", "Second")]);
    const [loads, setLoads] = createSignal<InitialLoad[]>([]);
    render(() => (
      <MaterialSlotsEditor
        mode="layout" slots={slots()} onChange={setSlots} multiMaterialHint={false}
        initialLoads={loads()} onInitialLoadsChange={setLoads}
      />
    ));

    const trigger = screen.getByRole("button", { name: /Load into Second/ });
    await fireEvent.pointerDown(trigger, { button: 0, pointerType: "mouse" });
    const options = await screen.findAllByRole("option");
    expect(options.map((o) => o.textContent)).toEqual(["None", "#2 PETG Clear — 812 g"]);
    await fireEvent.pointerDown(options[1], { button: 0, pointerType: "mouse" });
    await fireEvent.pointerUp(options[1], { button: 0, pointerType: "mouse" });
    expect(loads()).toEqual([{ slotIndex: 1, spoolId: "spl-ok" }]);

    fireEvent.click(screen.getByRole("button", { name: "Move Second up" }));
    expect(loads()).toEqual([{ slotIndex: 0, spoolId: "spl-ok" }]);
    fireEvent.click(screen.getByRole("button", { name: "Remove Second" }));
    expect(loads()).toEqual([]);
  });
});

const PRINTER = {
  id: "prn-1", name: "Bay 1", revision: 3,
  profile: { supportsMultiFilament: true },
  materialSlots: [
    { id: "slt-1", position: 0, name: "Main", occupantSpoolId: "spl-in" },
    { id: "slt-2", position: 1, name: "Aux" },
  ],
} as unknown as ResolvedPrinter;

describe("MaterialSlotsEditor — occupancy mode", () => {
  const slotsFor = (printer: ResolvedPrinter): SlotDraft[] =>
    printer.materialSlots.map((s) => draft(s.id, s.name, s.feederLabel ?? "", s.id));

  it("disables Remove on an occupied slot with \"Unload first\"", () => {
    spools.push(spool({ id: "spl-in", spoolNumber: 7, location: { kind: "slot", slotId: "slt-1", printerId: "prn-1" } }));
    render(() => <MaterialSlotsEditor mode="occupancy" printer={PRINTER} slots={slotsFor(PRINTER)} onChange={vi.fn()} multiMaterialHint={false} />);

    expect(screen.getByRole("button", { name: "Remove Main" })).toBeDisabled();
    expect(screen.getByText("Unload first")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove Aux" })).not.toBeDisabled();
  });

  it("Load… opens MoveSpoolDialog preset to that slot with the chosen storage Spool", async () => {
    spools.push(spool({ id: "spl-store", spoolNumber: 9, materialFamily: "ABS", colorName: "White" }));
    render(() => <MaterialSlotsEditor mode="occupancy" printer={PRINTER} slots={slotsFor(PRINTER)} onChange={vi.fn()} multiMaterialHint={false} />);

    fireEvent.click(screen.getByRole("button", { name: "Load… into Aux" }));
    await pick(screen.getByRole("button", { name: /Spool to load into Aux/ }), /#9 ABS White/);

    expect(screen.getByTestId("move-dialog")).toHaveTextContent("spl-store -> prn-1/slt-2");
  });

  it("Unload moves the occupant to storage", async () => {
    spools.push(spool({ id: "spl-in", revision: 4, spoolNumber: 7, location: { kind: "slot", slotId: "slt-1", printerId: "prn-1" } }));
    moveSpool.mockResolvedValue({ spools: [], printers: [], movements: [] });
    render(() => <MaterialSlotsEditor mode="occupancy" printer={PRINTER} slots={slotsFor(PRINTER)} onChange={vi.fn()} multiMaterialHint={false} />);

    fireEvent.click(screen.getByRole("button", { name: "Unload Main" }));

    await waitFor(() => expect(moveSpool).toHaveBeenCalledWith({
      spoolId: "spl-in", expectedSpoolRevision: 4, destination: { kind: "storage", storageLabel: null },
    }));
  });
});

describe("MaterialSlotsSection (Setup tab)", () => {
  it("saves the edited layout through setSlotLayout with existing ids kept", async () => {
    setSlotLayout.mockResolvedValue(PRINTER);
    render(() => <MaterialSlotsSection printer={PRINTER} />);

    expect(screen.getByText(MULTI_MATERIAL_HINT)).toBeInTheDocument();
    fireEvent.input(screen.getByLabelText("Name for slot 2"), { target: { value: "Left" } });
    fireEvent.click(screen.getByRole("button", { name: "Save slots" }));

    await waitFor(() => expect(setSlotLayout).toHaveBeenCalledWith("prn-1", [
      { id: "slt-1", name: "Main" },
      { id: "slt-2", name: "Left" },
    ]));
  });

  it("shows SLOT_OCCUPIED inline with \"Unload first\" and keeps the draft", async () => {
    setSlotLayout.mockRejectedValue({
      contractVersion: 1, code: "SLOT_OCCUPIED", message: "That slot still holds a Spool.", recovery: [], retryable: false,
      details: { slotId: "slt-2", spoolId: "spl-x" },
    });
    render(() => <MaterialSlotsSection printer={PRINTER} />);

    fireEvent.click(screen.getByRole("button", { name: "Remove Aux" }));
    fireEvent.click(screen.getByRole("button", { name: "Save slots" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Aux still holds a Spool. Unload first, then remove it.");
    expect(screen.queryByLabelText("Name for slot 2")).not.toBeInTheDocument();
  });

  it("reloads Printers and resets the draft on CONFLICT", async () => {
    setSlotLayout.mockRejectedValue({
      contractVersion: 1, code: "CONFLICT", message: "This Printer changed since you opened it.", recovery: ["RETRY"], retryable: true,
    });
    render(() => <MaterialSlotsSection printer={PRINTER} />);

    fireEvent.input(screen.getByLabelText("Name for slot 2"), { target: { value: "Left" } });
    fireEvent.click(screen.getByRole("button", { name: "Save slots" }));

    await waitFor(() => expect(reloadPrinter).toHaveBeenCalledWith("prn-1"));
    expect(within(await screen.findByRole("alert")).getByText(/changed since you opened it/)).toBeInTheDocument();
    expect(screen.getByLabelText("Name for slot 2")).toHaveValue("Aux");
  });
});
