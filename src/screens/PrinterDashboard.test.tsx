import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import { groupPrintersByModel, PrinterDashboard, summarizePrinters } from "./PrinterDashboard";
import type { ResolvedPrinter } from "../printers/types";

afterEach(() => {
  document.body.innerHTML = "";
});

const PROFILE = {
  bedShape: { kind: "rectangular" as const, widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
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
};

function printer(overrides: Partial<ResolvedPrinter>): ResolvedPrinter {
  return {
    id: "prn-1",
    name: "Printer",
    group: "",
    notes: "",
    catalogRef: {
      vendor: "Elegoo", model: "Elegoo Centauri Carbon",
      variant: "Elegoo Centauri Carbon 0.4 nozzle", modelId: "Elegoo-CC", printerVariant: "0.4",
    },
    catalogStatus: "ok",
    modelLabel: "Elegoo Centauri Carbon",
    variantLabel: "Elegoo Centauri Carbon 0.4 nozzle",
    profile: PROFILE,
    overriddenFields: [],
    inherited: {},
    profileDrift: [],
    unknownOverrideKeys: [],
    connection: null,
    ...overrides,
  };
}

describe("groupPrintersByModel", () => {
  it("groups printers under their catalog model, sorted alphabetically by group then name", () => {
    const printers = [
      printer({ id: "a", name: "Bay 2", catalogRef: { vendor: "Prusa", model: "Prusa MK4", variant: "v", modelId: "Prusa-MK4", printerVariant: "0.4" }, modelLabel: "Prusa MK4" }),
      printer({ id: "b", name: "Bay 1" }),
      printer({ id: "c", name: "Bay 3" }),
    ];
    const groups = groupPrintersByModel(printers);
    expect(groups.map((g) => g.modelLabel)).toEqual(["Elegoo Centauri Carbon", "Prusa MK4"]);
    expect(groups[0].printers.map((p) => p.name)).toEqual(["Bay 1", "Bay 3"]);
  });

  it("puts unresolved printers in a trailing Unlinked group", () => {
    const printers = [printer({ id: "a" }), printer({ id: "b", catalogStatus: "variantMissing" })];
    const groups = groupPrintersByModel(printers);
    expect(groups[groups.length - 1].modelLabel).toBe("Unlinked");
    expect(groups[groups.length - 1].printers.map((p) => p.id)).toEqual(["b"]);
  });
});

describe("summarizePrinters", () => {
  it("reports a count with correct pluralization", () => {
    expect(summarizePrinters([])).toBe("No printers");
    expect(summarizePrinters([printer({ id: "a" })])).toBe("1 printer");
    expect(summarizePrinters([printer({ id: "a" }), printer({ id: "b" })])).toBe("2 printers");
  });
});

describe("PrinterDashboard", () => {
  it("renders one group header per catalog model with the right counts", () => {
    render(() => (
      <PrinterDashboard
        printers={[
          printer({ id: "a", name: "Bay 1" }),
          printer({ id: "b", name: "Bay 2" }),
        ]}
      />
    ));
    expect(screen.getByText("Elegoo Centauri Carbon")).toBeInTheDocument();
    expect(screen.getByText("2")).toBeInTheDocument();
  });

  it("selecting a card opens the detail aside", async () => {
    render(() => <PrinterDashboard printers={[printer({ id: "a", name: "Bay 1" })]} />);
    await fireEvent.click(screen.getByText("Bay 1"));
    expect(screen.getByLabelText("Printer detail")).toBeInTheDocument();
  });
});
