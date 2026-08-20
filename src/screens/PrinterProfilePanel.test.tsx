import { fireEvent, render, screen } from "@solidjs/testing-library";
import { createSignal, Show } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PrinterProfilePanel } from "./PrinterProfilePanel";
import type { ResolvedPrinter } from "../printers/types";

const overrideField = vi.fn();
const revertField = vi.fn();
const resolveDrift = vi.fn();
vi.mock("../printers/printer-store", () => ({
  overrideField: (...args: unknown[]) => overrideField(...args),
  revertField: (...args: unknown[]) => revertField(...args),
  resolveDrift: (...args: unknown[]) => resolveDrift(...args),
}));

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
  vi.useRealTimers();
});

const PRINTER: ResolvedPrinter = {
  id: "prn-1",
  name: "Centauri Carbon — Bay 1",
  group: "",
  notes: "",
  catalogRef: {
    vendor: "Elegoo", model: "Elegoo Centauri Carbon",
    variant: "Elegoo Centauri Carbon 0.4 nozzle", modelId: "Elegoo-CC", printerVariant: "0.4",
  },
  catalogStatus: "ok",
  modelLabel: "Elegoo Centauri Carbon",
  variantLabel: "Elegoo Centauri Carbon 0.4 nozzle",
  profile: {
    bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
    printableHeightMm: 256,
    bedExcludeAreas: [],
    defaultBedType: "4",
    nozzleDiameterMm: [0.4],
    nozzleType: "hardened_steel",
    gcodeFlavor: "klipper",
    hasAuxiliaryFan: true,
    supportsAirFiltration: true,
    supportsMultiFilament: true,
    suggestedHostType: "elegoolink",
  },
  overriddenFields: [],
  inherited: {},
  profileDrift: [],
  unknownOverrideKeys: [],
  connection: null,
};

describe("PrinterProfilePanel", () => {
  it("renders no revert control on an inherited field", () => {
    render(() => <PrinterProfilePanel printer={PRINTER} />);
    expect(screen.queryByLabelText("Revert Printable height to inherited")).not.toBeInTheDocument();
  });

  it("renders a revert control and inherited hint on an overridden field", () => {
    const overridden: ResolvedPrinter = {
      ...PRINTER,
      overriddenFields: ["printableHeightMm"],
      inherited: { printableHeightMm: 256 },
      profile: { ...PRINTER.profile, printableHeightMm: 240 },
    };
    render(() => <PrinterProfilePanel printer={overridden} />);
    expect(screen.getByLabelText("Revert Printable height to inherited")).toBeInTheDocument();
    expect(screen.getByText("inherited: 256")).toBeInTheDocument();
  });

  it("debounces a height edit before calling overrideField", async () => {
    vi.useFakeTimers();
    render(() => <PrinterProfilePanel printer={PRINTER} />);
    const input = screen.getByLabelText("Printable height") as HTMLInputElement;

    await fireEvent.input(input, { target: { value: "240" } });
    expect(overrideField).not.toHaveBeenCalled();

    vi.advanceTimersByTime(300);
    expect(overrideField).toHaveBeenCalledWith("prn-1", "printableHeightMm", 240);
  });

  it("does not persist an override merely from mounting -- Kobalte's NumberField mount-time echo must not be treated as a real edit", () => {
    vi.useFakeTimers();
    render(() => <PrinterProfilePanel printer={PRINTER} />);

    // No interaction with any control -- just let the debounce window pass.
    vi.advanceTimersByTime(400);

    expect(overrideField).not.toHaveBeenCalled();
  });

  it("shows a drift banner and calls resolveDrift on Accept", async () => {
    const drifted: ResolvedPrinter = {
      ...PRINTER,
      profileDrift: [{ field: "printableHeightMm", from: 250, to: 256 }],
    };
    render(() => <PrinterProfilePanel printer={drifted} />);
    await fireEvent.click(screen.getByText("Accept"));
    expect(resolveDrift).toHaveBeenCalledWith("prn-1", "accept");
  });

  it("cancels a pending debounced edit when the printer switches under a non-keyed Show", async () => {
    // Mirrors PrinterDashboard.tsx's actual `<Show when={selected()}>{(printer) => (...)}</Show>`
    // pattern: a truthy->truthy change of `selected()` does NOT remount the
    // child, so this exercises the same non-remounting path production code
    // takes (unlike calling `render()` again, which would remount and fail
    // to reproduce the bug).
    vi.useFakeTimers();
    // Distinct printableHeightMm (300) so any call carrying it is unambiguously
    // B's own current value, never confusable with A's typed-but-uncommitted 240.
    const printerB: ResolvedPrinter = {
      ...PRINTER,
      id: "prn-2",
      profile: { ...PRINTER.profile, printableHeightMm: 300 },
    };
    const [selected, setSelected] = createSignal<ResolvedPrinter>(PRINTER);

    render(() => (
      <Show when={selected()}>{(printer) => <PrinterProfilePanel printer={printer()} />}</Show>
    ));

    const input = screen.getByLabelText("Printable height") as HTMLInputElement;
    await fireEvent.input(input, { target: { value: "240" } });
    expect(overrideField).not.toHaveBeenCalled();

    setSelected(printerB);
    vi.advanceTimersByTime(300);

    // The critical corruption this guards against: A's in-flight edit (240)
    // must never be committed against B's id (Kobalte's controlled NumberField
    // does independently resync-fire onChange with B's *own* unedited value
    // when the identity of the `rawValue` prop's source changes -- that's a
    // separate, harmless, idempotent quirk unrelated to this bug, so it's not
    // asserted against here).
    expect(overrideField).not.toHaveBeenCalledWith("prn-2", "printableHeightMm", 240);
    expect(overrideField).not.toHaveBeenCalledWith("prn-1", "printableHeightMm", 240);
  });

  it("shows 'Default' in the closed bed-type trigger when defaultBedType is empty -- Kobalte treats a selected key of '' as no selection and falls back to the placeholder", () => {
    // 942 of 971 catalog variants carry defaultBedType: "" -- this is the
    // overwhelming common case, not an edge case.
    const noBedType: ResolvedPrinter = {
      ...PRINTER,
      profile: { ...PRINTER.profile, defaultBedType: "" },
    };
    render(() => <PrinterProfilePanel printer={noBedType} />);
    expect(screen.getByText("Default")).toBeInTheDocument();
  });

  it("merges width and depth edits made within the same debounce window", async () => {
    vi.useFakeTimers();
    render(() => <PrinterProfilePanel printer={PRINTER} />);

    const width = screen.getByLabelText("Bed width") as HTMLInputElement;
    const depth = screen.getByLabelText("Bed depth") as HTMLInputElement;

    await fireEvent.input(width, { target: { value: "300" } });
    await fireEvent.input(depth, { target: { value: "310" } });
    expect(overrideField).not.toHaveBeenCalled();

    vi.advanceTimersByTime(300);

    const bedShapeCalls = overrideField.mock.calls.filter(([, field]) => field === "bedShape");
    expect(bedShapeCalls).toHaveLength(1);
    expect(bedShapeCalls[0]).toEqual([
      "prn-1",
      "bedShape",
      { kind: "rectangular", widthMm: 300, depthMm: 310, originXMm: 0, originYMm: 0 },
    ]);
  });
});
