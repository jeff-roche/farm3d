import { createSignal } from "solid-js";
import { fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";
import { PrinterSetupWizard } from "./PrinterSetupWizard";
import type { ResolvedPrinter } from "../printers/types";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";

const spools = vi.hoisted(() => [] as SpoolRecord[]);
vi.mock("../spools/spool-store", () => ({
  get spoolState() {
    return { spools, loaded: true };
  },
  ensureInventoryLoaded: () => Promise.resolve(),
}));

function storedSpool(overrides: Partial<SpoolRecord>): SpoolRecord {
  return {
    id: "spl-1", revision: 1, spoolNumber: 1,
    manufacturer: "Prusament", materialFamily: "PLA", colorName: "Galaxy Black", diameter: "1.75",
    nominalMg: 1_000_000, lowThresholdMg: 100_000, lifecycle: "active",
    location: { kind: "storage", storageLabel: null },
    availability: { currentMg: 812_000, reservedMg: 0, availableMg: 812_000 },
    facets: { loaded: false, reserved: false, low: false, confidence: "measured" },
    createdAt: "", updatedAt: "",
    ...overrides,
  };
}

const previewProfile = vi.hoisted(() =>
  vi.fn().mockImplementation((ref: { printerVariant: string }) => {
    const nozzle = ref.printerVariant === "0.6" ? 0.6 : 0.4;
    return Promise.resolve({
      bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
      printableHeightMm: 256,
      nozzleDiameterMm: [nozzle],
      bedExcludeAreas: [],
      defaultBedType: "4",
      nozzleType: "hardened_steel",
      gcodeFlavor: "klipper",
      hasAuxiliaryFan: true,
      supportsAirFiltration: true,
      supportsMultiFilament: true,
      suggestedHostType: "moonraker",
    });
  }),
);

vi.mock("../printers/printer-catalog", () => ({
  listCatalogModels: vi.fn().mockResolvedValue([
    { modelId: "Elegoo-CC", vendor: "Elegoo", model: "Elegoo Centauri Carbon" },
    { modelId: "Prusa-MK4", vendor: "Prusa", model: "Prusa MK4" },
  ]),
  listCatalogVariants: vi.fn().mockResolvedValue([
    { variant: "Elegoo Centauri Carbon 0.4 nozzle", printerVariant: "0.4" },
    { variant: "Elegoo Centauri Carbon 0.6 nozzle", printerVariant: "0.6" },
  ]),
  previewProfile,
}));

const createPrinter = vi.hoisted(() => vi.fn());
const probeCandidate = vi.hoisted(() => vi.fn());
const credentialStoreInfo = vi.hoisted(() => vi.fn().mockResolvedValue({ kind: "keychain" }));
const discoverPrinters = vi.hoisted(() => vi.fn().mockResolvedValue([]));
vi.mock("../printers/printer-store", () => ({
  createPrinter,
  probeCandidate,
  credentialStoreInfo,
  discoverPrinters,
}));

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
  spools.length = 0;
});

const CENTAURI_MODEL = { vendor: "Elegoo", model: "Elegoo Centauri Carbon" };
const CENTAURI_CATALOG_REF = {
  vendor: "Elegoo",
  model: "Elegoo Centauri Carbon",
  variant: "Elegoo Centauri Carbon 0.4 nozzle",
  modelId: "Elegoo-CC",
  printerVariant: "0.4",
};

function existingPrinter(name: string): ResolvedPrinter {
  return {
    id: `prn-${name}`,
    revision: 1,
    name,
    notes: "",
    overrides: {},
    catalogRef: CENTAURI_CATALOG_REF,
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
      suggestedHostType: "moonraker",
    },
    overriddenFields: [],
    inherited: {},
    profileDrift: [],
    unknownOverrideKeys: [],
    startSafety: "confirmBedClear",
    setupGaps: [],
    createdAt: "",
    updatedAt: "",
  } as unknown as ResolvedPrinter;
}

function stepItem(label: string) {
  return screen.getByText(label).closest("li")!;
}

async function pickCentauriCarbon() {
  const brandInput = await screen.findByRole("combobox", { name: "Brand" });
  await fireEvent.pointerDown(brandInput, { pointerType: "mouse", button: 0 });
  await fireEvent.input(brandInput, { target: { value: "Elegoo" } });
  await fireEvent.pointerUp(await screen.findByText("Elegoo"), { pointerType: "mouse", button: 0 });

  const modelTrigger = await screen.findByRole("button", { name: "Model" });
  await fireEvent.pointerDown(modelTrigger, { pointerType: "mouse", button: 0 });
  await fireEvent.click(await screen.findByText("Centauri Carbon"));
  await screen.findByText(/Nozzle: 0\.4 mm/);
}

describe("PrinterSetupWizard — Identify (ported from PrinterAddDialog)", () => {
  it("picks a brand, then a model + auto-selected variant, and enables Next", async () => {
    render(() => <PrinterSetupWizard open onOpenChange={vi.fn()} existingPrinters={[]} />);
    await pickCentauriCarbon();

    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Elegoo Centauri Carbon");
    const nextButton = screen.getByRole("button", { name: "Next →" }) as HTMLButtonElement;
    expect(nextButton.disabled).toBe(false);

    fireEvent.click(nextButton);
    expect(stepItem("Connect").getAttribute("aria-current")).toBe("step");
  });

  it("hides the Model dropdown until a brand is chosen, and disables Next until a model is selected", () => {
    render(() => <PrinterSetupWizard open onOpenChange={vi.fn()} existingPrinters={[]} />);
    expect(screen.queryByRole("button", { name: "Model" })).not.toBeInTheDocument();
    const nextButton = screen.getByRole("button", { name: "Next →" }) as HTMLButtonElement;
    expect(nextButton.disabled).toBe(true);
  });

  it("strips a model's own vendor prefix in the Model dropdown, but not when the model doesn't start with it", async () => {
    render(() => <PrinterSetupWizard open onOpenChange={vi.fn()} existingPrinters={[]} />);

    const brandInput = await screen.findByRole("combobox", { name: "Brand" });
    await fireEvent.pointerDown(brandInput, { pointerType: "mouse", button: 0 });
    await fireEvent.input(brandInput, { target: { value: "Prusa" } });
    await fireEvent.pointerUp(await screen.findByText("Prusa"), { pointerType: "mouse", button: 0 });

    const modelTrigger = await screen.findByRole("button", { name: "Model" });
    await fireEvent.pointerDown(modelTrigger, { pointerType: "mouse", button: 0 });
    expect(await screen.findByText("MK4")).toBeInTheDocument();
    expect(screen.queryByText("Prusa MK4")).not.toBeInTheDocument();
  });

  it("regenerates the profile preview when the nozzle selection changes, not just on model selection", async () => {
    render(() => <PrinterSetupWizard open onOpenChange={vi.fn()} existingPrinters={[]} />);
    await pickCentauriCarbon();

    const nozzleTrigger = await screen.findByRole("button", { name: /Nozzle/ });
    await fireEvent.pointerDown(nozzleTrigger, { pointerType: "mouse", button: 0 });
    await fireEvent.click(await screen.findByText("0.6 mm"));

    await screen.findByText(/Nozzle: 0\.6 mm/);
    expect(screen.queryByText(/Nozzle: 0\.4 mm/)).not.toBeInTheDocument();
  });

  it("prefills a group's model and suggests a unique name, avoiding an existing one", async () => {
    render(() => (
      <PrinterSetupWizard
        open
        onOpenChange={vi.fn()}
        prefillModel={CENTAURI_MODEL}
        existingPrinters={[existingPrinter("Elegoo Centauri Carbon")]}
      />
    ));

    expect(await screen.findByRole("button", { name: /Centauri Carbon/ })).toBeInTheDocument();
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Elegoo Centauri Carbon 2");
  });

  it("resets every signal after closing, even with no prefill on the next open", async () => {
    const [open, setOpen] = createSignal(true);
    render(() => <PrinterSetupWizard open={open()} onOpenChange={setOpen} existingPrinters={[]} />);

    const brandInput = await screen.findByRole("combobox", { name: "Brand" });
    await fireEvent.pointerDown(brandInput, { pointerType: "mouse", button: 0 });
    await fireEvent.input(brandInput, { target: { value: "Elegoo" } });
    await fireEvent.pointerUp(await screen.findByText("Elegoo"), { pointerType: "mouse", button: 0 });
    expect(await screen.findByRole("button", { name: "Model" })).toBeInTheDocument();

    fireEvent.input(screen.getByLabelText("Name"), { target: { value: "half-typed name" } });
    fireEvent.input(screen.getByLabelText("Location"), { target: { value: "half-typed location" } });

    setOpen(false);
    setOpen(true);

    expect(screen.queryByRole("button", { name: "Model" })).not.toBeInTheDocument();
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("");
    expect((screen.getByLabelText("Location") as HTMLInputElement).value).toBe("");
    expect(stepItem("Identify").getAttribute("aria-current")).toBe("step");
  });

  it("warns on a duplicate name without disabling Next", async () => {
    render(() => (
      <PrinterSetupWizard
        open
        onOpenChange={vi.fn()}
        existingPrinters={[existingPrinter("Elegoo Centauri Carbon")]}
      />
    ));
    await pickCentauriCarbon();

    expect(await screen.findByText("Another printer is already named this")).toBeInTheDocument();
    const nextButton = screen.getByRole("button", { name: "Next →" }) as HTMLButtonElement;
    expect(nextButton.disabled).toBe(false);
  });
});

describe("PrinterSetupWizard — Stepper", () => {
  it("shows Identify, Connect, Equip, Operate, and Review, with Identify current", () => {
    render(() => <PrinterSetupWizard open onOpenChange={vi.fn()} existingPrinters={[]} />);
    const labels = [...document.querySelectorAll("li")].map((li) => li.textContent?.replace(/^\d+/, "").trim());
    expect(labels).toEqual(["Identify", "Connect", "Equip", "Operate", "Review"]);
    for (const label of ["Identify", "Connect", "Equip", "Operate", "Review"]) {
      expect(screen.getByText(label)).toBeInTheDocument();
    }
    expect(stepItem("Identify").getAttribute("aria-current")).toBe("step");
  });
});

describe("PrinterSetupWizard — Connect", () => {
  it("Skip — save Profile-only goes to Equip", async () => {
    render(() => <PrinterSetupWizard open onOpenChange={vi.fn()} existingPrinters={[]} />);
    await pickCentauriCarbon();
    fireEvent.click(screen.getByRole("button", { name: "Next →" }));

    fireEvent.click(screen.getByRole("button", { name: "Skip — save Profile-only" }));
    expect(stepItem("Equip").getAttribute("aria-current")).toBe("step");
  });

  it("shows the setup-incomplete notice on Review after skipping Connect", async () => {
    render(() => <PrinterSetupWizard open onOpenChange={vi.fn()} existingPrinters={[]} />);
    await pickCentauriCarbon();
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // connect
    fireEvent.click(screen.getByRole("button", { name: "Skip — save Profile-only" })); // equip
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // operate
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // review

    expect(
      await screen.findByText("This Printer will be Setup incomplete until it has a Connection"),
    ).toBeInTheDocument();
  });

  it("Test calls probeCandidate with the draft submission, and createPrinter has not been called", async () => {
    probeCandidate.mockResolvedValueOnce({
      kind: "moonraker",
      hostSoftware: "Moonraker 0.9",
      firmware: "Klipper v0.12",
      reportedName: "Bay 1",
      state: "online",
      stateMessage: "",
      reported: {},
    });
    render(() => <PrinterSetupWizard open onOpenChange={vi.fn()} existingPrinters={[]} />);
    await pickCentauriCarbon();
    fireEvent.click(screen.getByRole("button", { name: "Next →" }));

    fireEvent.input(screen.getByLabelText("Host"), { target: { value: "voron.local" } });
    fireEvent.click(screen.getByRole("button", { name: "Test connection" }));

    await waitFor(() =>
      expect(probeCandidate).toHaveBeenCalledWith(
        expect.objectContaining({ kind: "moonraker", host: "voron.local", port: 7125, useTls: false }),
      ),
    );
    expect(createPrinter).not.toHaveBeenCalled();
  });
});

describe("PrinterSetupWizard — Operate", () => {
  async function reachOperate() {
    render(() => <PrinterSetupWizard open onOpenChange={vi.fn()} existingPrinters={[]} />);
    await pickCentauriCarbon();
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // connect
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // equip
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // operate
  }

  it("defaults the start-safety radio to Confirm the bed is clear before each start", async () => {
    await reachOperate();
    expect(
      screen.getByRole("radio", { name: "Confirm the bed is clear before each start" }),
    ).toBeChecked();
    expect(screen.getByRole("radio", { name: "Allow unattended starts" })).not.toBeChecked();
  });

  it("lists the variant's bed types in the bed-type Select, opened with pointerDown", async () => {
    await reachOperate();
    const bedTypeTrigger = screen.getByRole("button", { name: /Bed type/ });
    await fireEvent.pointerDown(bedTypeTrigger, { pointerType: "mouse", button: 0 });
    expect(await screen.findByText("Textured PEI Plate")).toBeInTheDocument();
  });

  it("shows Default in the closed bed-type Select when the catalog's bed type is blank", async () => {
    const original = previewProfile.getMockImplementation();
    onTestFinished(() => void previewProfile.mockImplementation(original!));
    previewProfile.mockImplementation(() =>
      Promise.resolve({
        bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
        printableHeightMm: 256, nozzleDiameterMm: [0.4], bedExcludeAreas: [], defaultBedType: "",
        nozzleType: "brass", gcodeFlavor: "marlin", hasAuxiliaryFan: false, supportsAirFiltration: false,
        supportsMultiFilament: false, suggestedHostType: null,
      }),
    );
    await reachOperate();

    expect(screen.getByRole("button", { name: /Bed type/ })).toHaveTextContent("Default");
  });

  it("labels the catalog's blank bed type as Default, not a blank list item", async () => {
    await reachOperate();
    const bedTypeTrigger = screen.getByRole("button", { name: /Bed type/ });
    await fireEvent.pointerDown(bedTypeTrigger, { pointerType: "mouse", button: 0 });
    expect(await screen.findByText("Default")).toBeInTheDocument();
  });
});

describe("PrinterSetupWizard — Review", () => {
  async function reachReview(
    host: string,
    overrides: { onOpenChange?: (open: boolean) => void; onCreated?: (printer: ResolvedPrinter) => void } = {},
  ) {
    render(() => (
      <PrinterSetupWizard
        open
        onOpenChange={overrides.onOpenChange ?? vi.fn()}
        existingPrinters={[]}
        onCreated={overrides.onCreated}
      />
    ));
    await pickCentauriCarbon();
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // connect
    if (host) fireEvent.input(screen.getByLabelText("Host"), { target: { value: host } });
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // equip
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // operate
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // review
  }

  it("trims the location input in the review summary", async () => {
    render(() => <PrinterSetupWizard open onOpenChange={vi.fn()} existingPrinters={[]} />);
    await pickCentauriCarbon();
    fireEvent.input(screen.getByLabelText("Location"), { target: { value: "  Bay 1  " } });
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // connect
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // equip
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // operate
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // review

    expect(await screen.findByText("Location: Bay 1")).toBeInTheDocument();
  });

  it("shows mismatches from the last probe and the credential-store location", async () => {
    probeCandidate.mockResolvedValueOnce({
      kind: "moonraker",
      hostSoftware: "Moonraker 0.9",
      firmware: "Klipper v0.12",
      reportedName: "Bay 1",
      state: "online",
      stateMessage: "",
      reported: { bedWidthMm: 220 },
    });
    render(() => <PrinterSetupWizard open onOpenChange={vi.fn()} existingPrinters={[]} />);
    await pickCentauriCarbon();
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // connect
    fireEvent.input(screen.getByLabelText("Host"), { target: { value: "voron.local" } });
    fireEvent.click(screen.getByRole("button", { name: "Test connection" }));
    await waitFor(() => expect(probeCandidate).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // equip
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // operate
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // review

    expect(
      await screen.findByText(/Bed width: catalog says 256 mm, the printer reports 220 mm/),
    ).toBeInTheDocument();
    expect(await screen.findByText(/OS keychain/)).toBeInTheDocument();
  });

  it("drops a stale probe result from Review after the host is edited post-Test", async () => {
    probeCandidate.mockResolvedValueOnce({
      kind: "moonraker",
      hostSoftware: "Moonraker 0.9",
      firmware: "Klipper v0.12",
      reportedName: "Bay 1",
      state: "online",
      stateMessage: "",
      reported: { bedWidthMm: 220 },
    });
    render(() => <PrinterSetupWizard open onOpenChange={vi.fn()} existingPrinters={[]} />);
    await pickCentauriCarbon();
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // connect
    fireEvent.input(screen.getByLabelText("Host"), { target: { value: "voron.local" } });
    fireEvent.click(screen.getByRole("button", { name: "Test connection" }));
    await waitFor(() => expect(probeCandidate).toHaveBeenCalled());
    await screen.findByText("online");

    // Editing the host after a successful Test invalidates that result --
    // it described "voron.local", not whatever the host field says now.
    fireEvent.input(screen.getByLabelText("Host"), { target: { value: "a-different-host.local" } });

    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // equip
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // operate
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // review

    expect(
      screen.queryByText(/Bed width: catalog says 256 mm, the printer reports 220 mm/),
    ).not.toBeInTheDocument();
  });

  it("Save calls createPrinter once with the assembled options, closes, and fires onCreated", async () => {
    const created = existingPrinter("Elegoo Centauri Carbon");
    createPrinter.mockResolvedValueOnce(created);
    const onOpenChange = vi.fn();
    const onCreated = vi.fn();
    render(() => (
      <PrinterSetupWizard open onOpenChange={onOpenChange} existingPrinters={[]} onCreated={onCreated} />
    ));
    await pickCentauriCarbon();
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // connect
    fireEvent.input(screen.getByLabelText("Host"), { target: { value: "voron.local" } });
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // equip
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // operate
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // review

    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(createPrinter).toHaveBeenCalledTimes(1));
    expect(createPrinter).toHaveBeenCalledWith({
      name: "Elegoo Centauri Carbon",
      catalogRef: CENTAURI_CATALOG_REF,
      location: undefined,
      startSafety: "confirmBedClear",
      defaultBedType: undefined,
      connection: { kind: "moonraker", host: "voron.local", port: 7125, useTls: false },
      slotLayout: [{ name: "Main" }],
      initialLoads: [],
    });
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(onCreated).toHaveBeenCalledWith(created);
  });

  it("stays on Review and preserves fields when Save fails", async () => {
    createPrinter.mockResolvedValueOnce(undefined);
    const onOpenChange = vi.fn();
    const onCreated = vi.fn();
    await reachReview("", { onOpenChange, onCreated });

    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(createPrinter).toHaveBeenCalledTimes(1));

    expect(onOpenChange).not.toHaveBeenCalledWith(false);
    expect(onCreated).not.toHaveBeenCalled();
    expect(stepItem("Review").getAttribute("aria-current")).toBe("step");
    expect(screen.getByText("Elegoo Centauri Carbon")).toBeInTheDocument();
  });
});

describe("PrinterSetupWizard — keyboard", () => {
  it("keeps Next reachable by Tab and advances the step on Enter", async () => {
    render(() => <PrinterSetupWizard open onOpenChange={vi.fn()} existingPrinters={[]} />);
    await pickCentauriCarbon();

    const nextButton = screen.getByRole("button", { name: "Next →" }) as HTMLButtonElement;
    expect(nextButton.disabled).toBe(false);
    expect(nextButton.tabIndex).toBe(0);

    fireEvent.keyDown(screen.getByLabelText("Name"), { key: "Enter" });
    expect(stepItem("Connect").getAttribute("aria-current")).toBe("step");
  });
});

describe("PrinterSetupWizard — Equip", () => {
  async function reachEquip() {
    render(() => <PrinterSetupWizard open onOpenChange={vi.fn()} existingPrinters={[]} onCreated={vi.fn()} />);
    await pickCentauriCarbon();
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // connect
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // equip
  }

  it("defaults to a single Main slot and shows the multi-material hint for a multi-material model", async () => {
    await reachEquip();
    expect(stepItem("Equip").getAttribute("aria-current")).toBe("step");
    expect(screen.getByLabelText("Name for slot 1")).toHaveValue("Main");
    expect(screen.queryByLabelText("Name for slot 2")).not.toBeInTheDocument();
    expect(screen.getByText(/This model can feed more than one material/)).toBeInTheDocument();
  });

  it("offers only active storage Spools for an initial load", async () => {
    spools.push(
      storedSpool({ id: "spl-ok", spoolNumber: 2, materialFamily: "PETG", colorName: "Clear" }),
      storedSpool({ id: "spl-loaded", spoolNumber: 3, location: { kind: "slot", slotId: "x", printerId: "p" } }),
      storedSpool({ id: "spl-archived", spoolNumber: 4, lifecycle: "archived" }),
    );
    await reachEquip();

    await fireEvent.pointerDown(screen.getByRole("button", { name: /Load into Main/ }), { button: 0, pointerType: "mouse" });
    const options = await screen.findAllByRole("option");
    expect(options.map((o) => o.textContent)).toEqual(["None", "#2 PETG Clear — 812 g"]);
  });

  it("leaves Enter on a button to the button instead of advancing the step", async () => {
    await reachEquip();
    const addSlot = screen.getByRole("button", { name: "Add slot" });
    const notPrevented = fireEvent.keyDown(addSlot, { key: "Enter" });
    expect(notPrevented).toBe(true);
    fireEvent.click(addSlot); // the button's own native Enter activation
    expect(stepItem("Equip").getAttribute("aria-current")).toBe("step");
    expect(screen.getByLabelText("Name for slot 2")).toHaveValue("Slot 2");
  });

  it("disables Next while a slot name is invalid", async () => {
    await reachEquip();
    fireEvent.input(screen.getByLabelText("Name for slot 1"), { target: { value: " " } });
    expect(screen.getByRole("button", { name: "Next →" })).toBeDisabled();
  });

  it("lists the slots and loads on Review, and Save sends slotLayout and initialLoads", async () => {
    spools.push(storedSpool({ id: "spl-ok", revision: 5, spoolNumber: 2, materialFamily: "PETG", colorName: "Clear" }));
    createPrinter.mockResolvedValueOnce(existingPrinter("Elegoo Centauri Carbon"));
    await reachEquip();

    fireEvent.click(screen.getByRole("button", { name: "Add slot" }));
    fireEvent.input(screen.getByLabelText("Feeder label for slot 2"), { target: { value: "AMS 1" } });
    const trigger = screen.getByRole("button", { name: /Load into Slot 2/ });
    await fireEvent.pointerDown(trigger, { button: 0, pointerType: "mouse" });
    const option = await screen.findByRole("option", { name: /#2 PETG Clear/ });
    await fireEvent.pointerDown(option, { button: 0, pointerType: "mouse" });
    await fireEvent.pointerUp(option, { button: 0, pointerType: "mouse" });

    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // operate
    fireEvent.click(screen.getByRole("button", { name: "Next →" })); // review
    expect(screen.getByText("Material Slots: Main, Slot 2 (AMS 1)")).toBeInTheDocument();
    expect(screen.getByText("Load at creation: Slot 2 — #2 PETG Clear")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(createPrinter).toHaveBeenCalledWith(expect.objectContaining({
      slotLayout: [{ name: "Main" }, { name: "Slot 2", feederLabel: "AMS 1" }],
      initialLoads: [{ slotIndex: 1, spoolId: "spl-ok", expectedSpoolRevision: 5 }],
    })));
  });
});
