import { fireEvent, render, screen, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
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

  it("closes the detail aside via its close button, without removing the printer", async () => {
    const onRemovePrinter = vi.fn();
    render(() => (
      <PrinterDashboard printers={[printer({ id: "a", name: "Bay 1" })]} onRemovePrinter={onRemovePrinter} />
    ));
    await fireEvent.click(screen.getByText("Bay 1"));
    expect(screen.getByLabelText("Printer detail")).toBeInTheDocument();

    await fireEvent.click(screen.getByRole("button", { name: "Close printer detail" }));

    expect(screen.queryByLabelText("Printer detail")).not.toBeInTheDocument();
    expect(onRemovePrinter).not.toHaveBeenCalled();
    // The card itself is still there — only the aside closed.
    expect(screen.getByText("Bay 1")).toBeInTheDocument();
  });

  it("does not badge an auto-rematched printer as Unlinked, but still badges a genuinely unresolved one", () => {
    render(() => (
      <PrinterDashboard
        printers={[
          printer({ id: "a", name: "Bay 1", catalogStatus: "rematched" }),
          printer({ id: "b", name: "Bay 2", catalogStatus: "variantMissing" }),
        ]}
      />
    ));

    const rematchedCard = screen.getByText("Bay 1").closest("button") as HTMLElement;
    expect(within(rematchedCard).queryByText("Unlinked")).not.toBeInTheDocument();

    const unresolvedCard = screen.getByText("Bay 2").closest("button") as HTMLElement;
    expect(within(unresolvedCard).getByText("Unlinked")).toBeInTheDocument();
  });

  it("counts connection states once printers report them", () => {
    // Phase 1 could only say "3 printers" — there was nothing to count.
    const printers = [
      printer({ id: "a", runtimeStatus: { connectionState: "online", updatedAt: "" } }),
      printer({ id: "b", runtimeStatus: { connectionState: "offline", updatedAt: "" } }),
      printer({ id: "c" }),
    ];
    expect(summarizePrinters(printers)).toBe("3 printers — 1 online, 1 offline");
  });

  it("still says only the count when nothing has reported", () => {
    expect(summarizePrinters([printer({ id: "a" })])).toBe("1 printer");
  });

  it("badges a printer with its connection state", () => {
    const printers = [
      printer({ id: "a", runtimeStatus: { connectionState: "online", updatedAt: "" } }),
    ];
    render(() => <PrinterDashboard printers={printers} />);
    expect(screen.getByText("online")).toBeInTheDocument();
  });

  it("renders an unreported temperature as a dash, never as zero", () => {
    const printers = [
      printer({ id: "a", runtimeStatus: { connectionState: "online", updatedAt: "" } }),
    ];
    render(() => <PrinterDashboard printers={printers} />);
    expect(screen.queryByText(/0 °C/)).not.toBeInTheDocument();
  });
});
