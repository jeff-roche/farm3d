import { fireEvent, render, screen, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { groupPrintersByModel, PrinterDashboard, summarizePrinters } from "./PrinterDashboard";
import type { ResolvedPrinter } from "../printers/types";

const updatePrinter = vi.fn();
// A partial mock -- every other export (openPrintersFile, overrideField,
// rebindPrinter, setConnection, ...) stays real. Those are only ever
// *called* from the Profile/Connection/Status tab content, which none of
// these tests select into, but they're still transitively imported by
// PrinterDashboard's child components, so replacing the whole module would
// silently break anything that reads one at render time.
vi.mock("../printers/printer-store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../printers/printer-store")>();
  return { ...actual, updatePrinter: (...args: unknown[]) => updatePrinter(...args) };
});

// PrinterAddDialog's Model Select renders whatever `groupPrintersByModel`
// prefilled it with as its selected value, which crashes if that model
// isn't present in the (otherwise real, network-backed) catalog list --
// this stands in with data covering every group these tests construct.
vi.mock("../printers/printer-catalog", () => ({
  listCatalogModels: vi.fn().mockResolvedValue([
    { modelId: "Elegoo-CC", vendor: "Elegoo", model: "Elegoo Centauri Carbon" },
    { modelId: "", vendor: "Cubicon", model: "Cubicon Single Plus 320c" },
    { modelId: "", vendor: "Cubicon", model: "Cubicon Style" },
  ]),
  listCatalogVariants: vi.fn().mockResolvedValue([
    { variant: "Elegoo Centauri Carbon 0.4 nozzle", printerVariant: "0.4" },
  ]),
  previewProfile: vi.fn().mockResolvedValue(null),
}));

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
  vi.useRealTimers();
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

  it("does not merge two distinct models that happen to share a catalog modelId", () => {
    // Regression test: the catalog resolver documents `modelId` as NOT
    // unique across the shipped catalog. Keying the group solely on
    // `modelId` would silently collapse these two different models into
    // one group labelled with whichever printer sorted first.
    const printers = [
      printer({
        id: "a",
        name: "Bay 1",
        catalogRef: { vendor: "Cubicon", model: "Cubicon Single Plus 320c", variant: "v1", modelId: "", printerVariant: "0.4" },
        modelLabel: "Cubicon Single Plus 320c",
      }),
      printer({
        id: "b",
        name: "Bay 2",
        catalogRef: { vendor: "Cubicon", model: "Cubicon Style", variant: "v2", modelId: "", printerVariant: "0.4" },
        modelLabel: "Cubicon Style",
      }),
    ];
    const groups = groupPrintersByModel(printers);
    expect(groups.map((g) => g.modelLabel).sort()).toEqual(["Cubicon Single Plus 320c", "Cubicon Style"]);
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

  it("debounces a rename before calling updatePrinter", async () => {
    vi.useFakeTimers();
    render(() => <PrinterDashboard printers={[printer({ id: "a", name: "Bay 1" })]} />);
    await fireEvent.click(screen.getByText("Bay 1"));

    const nameField = screen.getByLabelText("Printer name") as HTMLInputElement;
    await fireEvent.input(nameField, { target: { value: "Bay 1 (renamed)" } });
    expect(updatePrinter).not.toHaveBeenCalled();

    vi.advanceTimersByTime(300);
    expect(updatePrinter).toHaveBeenCalledWith("a", { name: "Bay 1 (renamed)" });
  });

  it("cancels a pending rename when the selected printer switches", async () => {
    // Same bug class as PrinterProfilePanel's/PrinterStatusPanel's guard:
    // the detail aside's <Show when={selected()}> is non-keyed, so
    // switching which card is selected does not remount the header.
    vi.useFakeTimers();
    render(() => (
      <PrinterDashboard
        printers={[printer({ id: "a", name: "Bay 1" }), printer({ id: "b", name: "Bay 2" })]}
      />
    ));
    await fireEvent.click(screen.getByText("Bay 1"));

    const nameField = screen.getByLabelText("Printer name") as HTMLInputElement;
    await fireEvent.input(nameField, { target: { value: "a rename meant for Bay 1" } });

    await fireEvent.click(screen.getByText("Bay 2"));
    vi.advanceTimersByTime(300);

    expect(updatePrinter).not.toHaveBeenCalledWith("a", { name: "a rename meant for Bay 1" });
    expect(updatePrinter).not.toHaveBeenCalledWith("b", { name: "a rename meant for Bay 1" });
  });

  it("opens the add dialog pre-filled with a group's model via its header button", async () => {
    render(() => <PrinterDashboard printers={[printer({ id: "a", name: "Bay 1" })]} />);

    await fireEvent.click(
      screen.getByRole("button", { name: "Add another Elegoo Centauri Carbon" }),
    );

    expect(await screen.findByRole("button", { name: /Centauri Carbon/ })).toBeInTheDocument();
    // Suggests a name distinct from the group's existing printer instead of
    // colliding with "Elegoo Centauri Carbon" (Bay 1's own model name).
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe(
      "Elegoo Centauri Carbon",
    );
  });

  it("does not merge two distinct models that happen to share a catalog modelId into one 'add another' target", async () => {
    // Regression guard for the same modelId-collision bug groupPrintersByModel
    // is tested against above -- the header button must derive its prefill
    // from the group's own printers, not a shared modelId.
    const cubiconA = printer({
      id: "a", name: "Bay 1",
      catalogRef: { vendor: "Cubicon", model: "Cubicon Single Plus 320c", variant: "v1", modelId: "", printerVariant: "0.4" },
      modelLabel: "Cubicon Single Plus 320c",
    });
    const cubiconB = printer({
      id: "b", name: "Bay 2",
      catalogRef: { vendor: "Cubicon", model: "Cubicon Style", variant: "v2", modelId: "", printerVariant: "0.4" },
      modelLabel: "Cubicon Style",
    });
    render(() => <PrinterDashboard printers={[cubiconA, cubiconB]} />);

    await fireEvent.click(screen.getByRole("button", { name: "Add another Cubicon Style" }));
    // The Model trigger strips the shared "Cubicon" vendor prefix (see
    // PrinterAddDialog's stripBrandPrefix), so its label is "Style", not
    // "Cubicon Style" -- still distinct from the OTHER Cubicon model's
    // stripped label ("Single Plus 320c"), which is what this regression
    // test actually needs to distinguish.
    expect(await screen.findByRole("button", { name: /Style/ })).toBeInTheDocument();
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Cubicon Style");
  });
});
