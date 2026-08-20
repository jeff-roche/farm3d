import { fireEvent, render, screen } from "@solidjs/testing-library";
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

  it("shows a drift banner and calls resolveDrift on Accept", async () => {
    const drifted: ResolvedPrinter = {
      ...PRINTER,
      profileDrift: [{ field: "printableHeightMm", from: 250, to: 256 }],
    };
    render(() => <PrinterProfilePanel printer={drifted} />);
    await fireEvent.click(screen.getByText("Accept"));
    expect(resolveDrift).toHaveBeenCalledWith("prn-1", "accept");
  });
});
