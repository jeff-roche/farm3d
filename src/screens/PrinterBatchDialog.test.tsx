import { createSignal } from "solid-js";
import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CreatePrintersBatchInput } from "../generated/contracts/command/CreatePrintersBatchInput";
import type { CreatePrintersBatchOutput } from "../generated/contracts/command/CreatePrintersBatchOutput";
import type { BatchRowResult } from "../generated/contracts/command/BatchRowResult";
import type { ResolvedPrinter } from "../printers/types";
import { PrinterBatchDialog, toRowError } from "./PrinterBatchDialog";

vi.mock("../printers/printer-catalog", () => ({
  listCatalogModels: vi.fn().mockResolvedValue([
    { modelId: "Elegoo-CC", vendor: "Elegoo", model: "Elegoo Centauri Carbon" },
  ]),
  listCatalogVariants: vi.fn().mockResolvedValue([
    { variant: "Elegoo Centauri Carbon 0.4 nozzle", printerVariant: "0.4" },
    { variant: "Elegoo Centauri Carbon 0.6 nozzle", printerVariant: "0.6" },
  ]),
  previewProfile: vi.fn().mockResolvedValue({
    bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
    printableHeightMm: 256,
    nozzleDiameterMm: [0.4],
    bedExcludeAreas: [],
    defaultBedType: "4",
    nozzleType: "hardened_steel",
    gcodeFlavor: "klipper",
    hasAuxiliaryFan: true,
    supportsAirFiltration: true,
    supportsMultiFilament: true,
    suggestedHostType: "moonraker",
  }),
}));

vi.mock("../spools/spool-store", () => ({
  get spoolState() {
    return { spools: [], loaded: true };
  },
  ensureInventoryLoaded: () => Promise.resolve(),
}));

const createPrintersBatch = vi.hoisted(() => vi.fn());
const cancelBatch = vi.hoisted(() => vi.fn());
const setConnection = vi.hoisted(() => vi.fn());
const probeCandidate = vi.hoisted(() => vi.fn());
const discoverPrinters = vi.hoisted(() => vi.fn());
const credentialStoreInfo = vi.hoisted(() => vi.fn());
vi.mock("../printers/printer-store", () => ({
  createPrintersBatch,
  cancelBatch,
  setConnection,
  probeCandidate,
  discoverPrinters,
  credentialStoreInfo,
}));

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
  cancelBatch.mockResolvedValue(undefined);
  discoverPrinters.mockResolvedValue([]);
  credentialStoreInfo.mockResolvedValue({ kind: "keychain" });
  probeCandidate.mockResolvedValue(A_PROBE);
});
const A_PROBE = {
  kind: "moonraker", hostSoftware: "Moonraker", firmware: "Klipper", reportedName: "Voron",
  state: "ready", stateMessage: "", reported: {},
};
cancelBatch.mockResolvedValue(undefined);
probeCandidate.mockResolvedValue(A_PROBE);
discoverPrinters.mockResolvedValue([]);
credentialStoreInfo.mockResolvedValue({ kind: "keychain" });

const CENTAURI_CATALOG_REF = {
  vendor: "Elegoo",
  model: "Elegoo Centauri Carbon",
  variant: "Elegoo Centauri Carbon 0.4 nozzle",
  modelId: "Elegoo-CC",
  printerVariant: "0.4",
};

function existingPrinter(name: string, host?: string): ResolvedPrinter {
  return {
    id: `prn-${name}`,
    name,
    catalogRef: CENTAURI_CATALOG_REF,
    ...(host ? { connection: { kind: "moonraker", host, port: 7125, useTls: false } } : {}),
  } as unknown as ResolvedPrinter;
}

function renderDialog(existingPrinters: ResolvedPrinter[] = [], onOpenChange = vi.fn(), onEquip = vi.fn()) {
  render(() => <PrinterBatchDialog open onOpenChange={onOpenChange} existingPrinters={existingPrinters} onEquip={onEquip} />);
  return onOpenChange;
}

async function pickModel() {
  const brandInput = await screen.findByRole("combobox", { name: "Brand" });
  await fireEvent.pointerDown(brandInput, { pointerType: "mouse", button: 0 });
  await fireEvent.input(brandInput, { target: { value: "Elegoo" } });
  await fireEvent.pointerUp(await screen.findByText("Elegoo"), { pointerType: "mouse", button: 0 });

  const modelTrigger = await screen.findByRole("button", { name: "Model" });
  await fireEvent.pointerDown(modelTrigger, { pointerType: "mouse", button: 0 });
  await fireEvent.click(await screen.findByText("Centauri Carbon"));
  await waitFor(() => expect(nextButton().disabled).toBe(false));
}

function nextButton() {
  return screen.getByRole("button", { name: "Next →" }) as HTMLButtonElement;
}

function stepItem(label: string) {
  return screen.getAllByText(label).find((el) => el.closest("li"))!.closest("li")!;
}

function generate(quantity: string, pattern: string, locations: string) {
  fireEvent.input(screen.getByLabelText("Quantity per location"), { target: { value: quantity } });
  fireEvent.input(screen.getByLabelText("Name pattern"), { target: { value: pattern } });
  fireEvent.input(screen.getByLabelText("Locations"), { target: { value: locations } });
  fireEvent.click(screen.getByRole("button", { name: "Generate rows" }));
}

function nameValues() {
  return screen.queryAllByLabelText(/^Name for row \d+$/).map((el) => (el as HTMLInputElement).value);
}

async function toRowsStep() {
  await pickModel();
  fireEvent.click(nextButton());
  await screen.findByLabelText("Name pattern");
}

async function toConnectStep(quantity = "3") {
  await toRowsStep();
  generate(quantity, "Voron {nn}", "Bay A");
  fireEvent.click(nextButton());
  await screen.findByLabelText("Shared credential");
}

async function toReviewStep(beforeReview?: () => void) {
  await toConnectStep();
  fireEvent.input(screen.getByLabelText("Host for row 2"), { target: { value: "10.0.0.2" } });
  beforeReview?.();
  fireEvent.click(nextButton());
  await screen.findByRole("button", { name: "Create" });
}

function marker(label: string) {
  return screen.getAllByRole("status", { name: label });
}

function result(rowId: string, outcome: BatchRowResult["outcome"], extra: Partial<BatchRowResult> = {}): BatchRowResult {
  return { rowId, outcome, credentialStored: false, errors: [], warnings: [], ...extra };
}

function record(id: string) {
  return { id } as unknown as BatchRowResult["printer"];
}

/** created / createdSetupIncomplete (AUTHENTICATION_FAILED) / rejected (VALIDATION name). */
function mixedOutcome(input: CreatePrintersBatchInput): CreatePrintersBatchOutput {
  const [a, b, c] = input.rows;
  return {
    batchId: input.batchId,
    rows: [
      result(a.rowId, "created", { printer: record("prn-1") }),
      result(b.rowId, "createdSetupIncomplete", {
        printer: record("prn-2"),
        errors: [{ code: "AUTHENTICATION_FAILED", message: "The API key was rejected", fieldPath: "connection.credential" }],
      }),
      result(c.rowId, "rejected", { errors: [{ code: "VALIDATION", message: "Name is not allowed", fieldPath: "name" }] }),
    ],
  };
}

describe("PrinterBatchDialog — Shared step", () => {
  it("enables Next once model, nozzle, bed type, and safety are chosen", async () => {
    renderDialog();
    expect(nextButton().disabled).toBe(true);
    await pickModel();
    expect(screen.getByRole("button", { name: /^Nozzle/ })).toHaveTextContent("0.4 mm");

    fireEvent.pointerDown(screen.getByRole("button", { name: /^Bed type/ }), { pointerType: "mouse", button: 0 });
    fireEvent.click(await screen.findByRole("option", { name: "Textured PEI Plate" }));
    fireEvent.click(screen.getByLabelText("Allow unattended starts"));

    expect(nextButton().disabled).toBe(false);
    fireEvent.click(nextButton());
    expect(stepItem("Rows").getAttribute("aria-current")).toBe("step");
  });
});

describe("PrinterBatchDialog — Shared slot layout", () => {
  it("edits the shared slot layout (with the multi-material hint) and sends it as shared.slotLayout", async () => {
    createPrintersBatch.mockImplementation(async (input: CreatePrintersBatchInput) => mixedOutcome(input));
    renderDialog();
    await pickModel();
    expect(screen.getByLabelText("Name for slot 1")).toHaveValue("Main");
    expect(screen.getByText(/This model can feed more than one material/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add slot" }));
    fireEvent.input(screen.getByLabelText("Feeder label for slot 2"), { target: { value: "AMS 1" } });
    // Batch never loads Spools (D12, user decision 3).
    expect(screen.queryByRole("button", { name: /Load into/ })).not.toBeInTheDocument();

    fireEvent.click(nextButton());
    await screen.findByLabelText("Name pattern");
    generate("3", "Voron {nn}", "Bay A");
    fireEvent.click(nextButton());
    await screen.findByLabelText("Shared credential");
    fireEvent.click(nextButton());
    fireEvent.click(await screen.findByRole("button", { name: "Create" }));

    await waitFor(() => expect(createPrintersBatch).toHaveBeenCalledTimes(1));
    const input = createPrintersBatch.mock.calls[0][0] as CreatePrintersBatchInput;
    expect(input.shared.slotLayout).toEqual([{ name: "Main" }, { name: "Slot 2", feederLabel: "AMS 1" }]);
  });

  it("blocks Next while a slot name is duplicated", async () => {
    renderDialog();
    await pickModel();
    fireEvent.click(screen.getByRole("button", { name: "Add slot" }));
    fireEvent.input(screen.getByLabelText("Name for slot 2"), { target: { value: "main" } });
    expect(nextButton().disabled).toBe(true);
  });
});

describe("PrinterBatchDialog — Rows step", () => {
  it("generates quantity-per-location rows with continuing numbering (D11)", async () => {
    renderDialog();
    await toRowsStep();
    generate("2", "Voron {nn}", "Bay A, Bay B");
    expect(nameValues()).toEqual(["Voron 01", "Voron 02", "Voron 03", "Voron 04"]);
    const locations = screen.getAllByLabelText(/^Location for row \d+$/).map((el) => (el as HTMLInputElement).value);
    expect(locations).toEqual(["Bay A", "Bay A", "Bay B", "Bay B"]);
  });

  it("appends pasted rows", async () => {
    renderDialog();
    await toRowsStep();
    generate("1", "Voron {n}", "");
    fireEvent.input(screen.getByLabelText("Paste rows"), { target: { value: "name,location\nAlpha,Bay 1\nBeta,Bay 2" } });
    fireEvent.click(screen.getByRole("button", { name: "Add pasted rows" }));
    expect(nameValues()).toEqual(["Voron 1", "Alpha", "Beta"]);
  });

  it("lists intake errors and appends nothing for a credential-column paste", async () => {
    renderDialog();
    await toRowsStep();
    fireEvent.input(screen.getByLabelText("Paste rows"), { target: { value: "name,apikey\nAlpha,s3cret" } });
    fireEvent.click(screen.getByRole("button", { name: "Add pasted rows" }));
    expect(screen.getByText(/looks like a credential/)).toBeInTheDocument();
    expect(nameValues()).toEqual([]);
    expect(nextButton().disabled).toBe(true);
  });

  it("appends rows from a CSV file", async () => {
    renderDialog();
    await toRowsStep();
    const file = new File(["name,host\nGamma,10.0.0.7\n"], "farm.csv", { type: "text/csv" });
    fireEvent.change(screen.getByLabelText("Import CSV file"), { target: { files: [file] } });
    await waitFor(() => expect(nameValues()).toEqual(["Gamma"]));
  });

  it("opens the CSV file picker from a design-system button", async () => {
    renderDialog();
    await toRowsStep();
    const input = screen.getByLabelText("Import CSV file") as HTMLInputElement;
    const click = vi.spyOn(input, "click").mockImplementation(() => {});

    fireEvent.click(screen.getByRole("button", { name: "Import CSV file…" }));

    expect(click).toHaveBeenCalledOnce();
  });

  it("edits rows inline and removes them", async () => {
    renderDialog();
    await toRowsStep();
    generate("3", "Voron {nn}", "");
    fireEvent.input(screen.getByLabelText("Name for row 1"), { target: { value: "Front Voron" } });
    fireEvent.click(screen.getByRole("button", { name: "Remove Voron 02" }));
    expect(nameValues()).toEqual(["Front Voron", "Voron 03"]);
  });

  it("toggles a row's selection with Space on its checkbox", async () => {
    renderDialog();
    await toRowsStep();
    generate("1", "Voron {nn}", "");
    const checkbox = screen.getByRole("checkbox", { name: "Select Voron 01" }) as HTMLInputElement;
    expect(checkbox.checked).toBe(true);
    const control = checkbox.nextElementSibling as HTMLElement;
    fireEvent.keyDown(control, { key: " " });
    fireEvent.keyUp(control, { key: " " });
    await waitFor(() => expect(checkbox.checked).toBe(false));
  });

  it("shows each row's preview of the copied bed type and start safety (D9)", async () => {
    renderDialog();
    await pickModel();
    fireEvent.pointerDown(screen.getByRole("button", { name: /^Bed type/ }), { pointerType: "mouse", button: 0 });
    fireEvent.click(await screen.findByRole("option", { name: "Textured PEI Plate" }));
    fireEvent.click(screen.getByLabelText("Allow unattended starts"));
    fireEvent.click(nextButton());
    await screen.findByLabelText("Name pattern");
    generate("2", "Voron {nn}", "");
    expect(screen.getAllByText("Bed: Textured PEI Plate")).toHaveLength(2);
    expect(screen.getAllByText("Unattended starts")).toHaveLength(2);
  });
});

describe("PrinterBatchDialog — Connect step", () => {
  it("maps discovery: only an assignable candidate gets an Assign control, which fills host and port", async () => {
    discoverPrinters.mockResolvedValue([
      { kind: "moonraker", name: "Known", host: "10.0.0.1", port: 7125, addresses: [] },
      { kind: "moonraker", name: "Twin", host: "10.0.0.2", port: 7125, addresses: [] },
      { kind: "moonraker", name: "Fresh", host: "10.0.0.3", port: 7125, addresses: [] },
    ]);
    renderDialog([existingPrinter("Old", "10.0.0.1")]);
    await toConnectStep();
    fireEvent.input(screen.getByLabelText("Host for row 1"), { target: { value: "10.0.0.2" } });
    fireEvent.input(screen.getByLabelText("Host for row 2"), { target: { value: "10.0.0.2" } });

    const known = await screen.findByTestId("candidate-10.0.0.1:7125");
    expect(within(known).getByText("Already configured")).toBeInTheDocument();
    const twin = screen.getByTestId("candidate-10.0.0.2:7125");
    expect(within(twin).getByText(/Ambiguous/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Assign Known/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Assign Twin/ })).not.toBeInTheDocument();

    const assign = screen.getByRole("button", { name: /^Assign Fresh/ });
    fireEvent.pointerDown(assign, { pointerType: "mouse", button: 0 });
    fireEvent.click(await screen.findByRole("option", { name: "Voron 03" }));

    expect(screen.getByLabelText("Host for row 3")).toHaveValue("10.0.0.3");
    expect(screen.getByLabelText("Port for row 3")).toHaveValue("7125");
  });

  it("applies protocol, port, TLS, and credential source to the selected rows only", async () => {
    renderDialog();
    await toConnectStep();
    fireEvent.click(screen.getByRole("checkbox", { name: "Select Voron 03" }));

    fireEvent.pointerDown(screen.getByRole("button", { name: /^Protocol/ }), { pointerType: "mouse", button: 0 });
    fireEvent.click(await screen.findByRole("option", { name: "Moonraker (Klipper)" }));
    fireEvent.input(screen.getByLabelText("Port"), { target: { value: "7130" } });
    fireEvent.click(screen.getByLabelText("Use TLS"));
    fireEvent.pointerDown(screen.getByRole("button", { name: /^Credential source/ }), { pointerType: "mouse", button: 0 });
    fireEvent.click(await screen.findByRole("option", { name: "Shared" }));
    fireEvent.click(screen.getByRole("button", { name: "Apply to selected" }));

    const rows = screen.getAllByTestId(/^row-/);
    for (const row of rows.slice(0, 2)) {
      expect(within(row).getByLabelText(/^Port for row/)).toHaveValue("7130");
      expect(within(row).getByText("On")).toBeInTheDocument();
      expect(within(row).getByText("Shared")).toBeInTheDocument();
    }
    expect(within(rows[2]).getByLabelText(/^Port for row/)).toHaveValue("");
    expect(within(rows[2]).getByText("Off")).toBeInTheDocument();
    expect(within(rows[2]).getByText("None")).toBeInTheDocument();
  });

  it("offers a masked, empty shared credential field", async () => {
    renderDialog();
    await toConnectStep();
    const field = screen.getByLabelText("Shared credential") as HTMLInputElement;
    expect(field.type).toBe("password");
    expect(field.value).toBe("");
  });
});

describe("PrinterBatchDialog — Review & results", () => {
  it("creates with toBatchInput's shape, then shows each row's outcome and keeps failed input", async () => {
    createPrintersBatch.mockImplementation(async (input: CreatePrintersBatchInput) => mixedOutcome(input));
    renderDialog();
    await toReviewStep();
    fireEvent.click(screen.getByRole("button", { name: "Create" }));

    await waitFor(() => expect(createPrintersBatch).toHaveBeenCalledTimes(1));
    const input = createPrintersBatch.mock.calls[0][0] as CreatePrintersBatchInput;
    expect(input).toEqual({
      batchId: expect.any(String),
      shared: { catalogRef: CENTAURI_CATALOG_REF, startSafety: "confirmBedClear", slotLayout: [{ name: "Main" }] },
      probe: true,
      rows: [
        { rowId: expect.any(String), name: "Voron 01", location: "Bay A" },
        {
          rowId: expect.any(String),
          name: "Voron 02",
          location: "Bay A",
          connection: { kind: "moonraker", host: "10.0.0.2", port: 7125, useTls: false, credential: { source: "none" } },
        },
        { rowId: expect.any(String), name: "Voron 03", location: "Bay A" },
      ],
    });
    expect(new Set(input.rows.map((row) => row.rowId)).size).toBe(3);

    await screen.findByRole("status", { name: "Not created" });
    expect(marker("Created")[0]).toHaveAttribute("data-severity", "resolved");
    expect(marker("Created — Setup incomplete")[0]).toHaveAttribute("data-severity", "warning");
    expect(marker("Not created")[0]).toHaveAttribute("data-severity", "fatal");
    expect(marker("Not created")[0].querySelector("svg")).not.toBeNull();
    expect(screen.getByText(/The API key was rejected/)).toBeInTheDocument();

    expect(screen.getByLabelText("Name for row 3")).toHaveValue("Voron 03");
    expect(screen.getByLabelText("Host for row 2")).toHaveValue("10.0.0.2");
  });

  it("retries rejected rows as a new batch with the same rowIds, and reconnects the incomplete row via setConnection", async () => {
    createPrintersBatch.mockImplementationOnce(async (input: CreatePrintersBatchInput) => mixedOutcome(input));
    renderDialog();
    await toReviewStep();
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await screen.findByRole("status", { name: "Not created" });
    const first = createPrintersBatch.mock.calls[0][0] as CreatePrintersBatchInput;

    fireEvent.input(screen.getByLabelText("Name for row 3"), { target: { value: "Voron 3b" } });
    createPrintersBatch.mockImplementationOnce(async (input: CreatePrintersBatchInput) => ({
      batchId: input.batchId,
      rows: [result(input.rows[0].rowId, "created", { printer: record("prn-3") })],
    }));
    setConnection.mockResolvedValueOnce(undefined);
    fireEvent.click(screen.getByRole("button", { name: "Retry failed" }));

    await waitFor(() => expect(createPrintersBatch).toHaveBeenCalledTimes(2));
    const retry = createPrintersBatch.mock.calls[1][0] as CreatePrintersBatchInput;
    expect(retry.batchId).not.toBe(first.batchId);
    expect(retry.rows).toEqual([{ rowId: first.rows[2].rowId, name: "Voron 3b", location: "Bay A" }]);

    expect(setConnection).toHaveBeenCalledTimes(1);
    expect(setConnection.mock.calls[0]).toEqual([
      "prn-2",
      { kind: "moonraker", host: "10.0.0.2", port: 7125, useTls: false },
    ]);

    await waitFor(() => expect(marker("Created")).toHaveLength(3));
    expect(screen.queryByRole("status", { name: "Created — Setup incomplete" })).not.toBeInTheDocument();
  });

  it("never re-sends a created or createdSetupIncomplete row, even when its result has no printer record", async () => {
    createPrintersBatch.mockImplementationOnce(async (input: CreatePrintersBatchInput) => ({
      batchId: input.batchId,
      rows: [
        result(input.rows[0].rowId, "created"),
        result(input.rows[1].rowId, "createdSetupIncomplete", {
          errors: [{ code: "TIMEOUT", message: "Timed out" }],
        }),
        result(input.rows[2].rowId, "rejected", { errors: [{ code: "VALIDATION", message: "Bad name", fieldPath: "name" }] }),
      ],
    }));
    renderDialog();
    await toReviewStep();
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await screen.findByRole("status", { name: "Not created" });
    const first = createPrintersBatch.mock.calls[0][0] as CreatePrintersBatchInput;

    // Created rows' identity is fixed; only the rejected row stays editable.
    expect(screen.queryByLabelText("Name for row 1")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Name for row 2")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Name for row 3")).toBeInTheDocument();

    createPrintersBatch.mockImplementationOnce(async (input: CreatePrintersBatchInput) => ({
      batchId: input.batchId,
      rows: input.rows.map((row) => result(row.rowId, "created", { printer: record("prn-x") })),
    }));
    fireEvent.click(screen.getByRole("button", { name: "Retry failed" }));
    await waitFor(() => expect(createPrintersBatch).toHaveBeenCalledTimes(2));
    const retry = createPrintersBatch.mock.calls[1][0] as CreatePrintersBatchInput;
    expect(retry.rows.map((row) => row.rowId)).toEqual([first.rows[2].rowId]);
    // No printerId to reconnect against, so no setConnection either.
    expect(setConnection).not.toHaveBeenCalled();
  });

  it("offers no retry when the only failed row was created without a printer record", async () => {
    createPrintersBatch.mockImplementationOnce(async (input: CreatePrintersBatchInput) => ({
      batchId: input.batchId,
      rows: input.rows.map((row) =>
        result(row.rowId, "createdSetupIncomplete", { errors: [{ code: "TIMEOUT", message: "Timed out" }] }),
      ),
    }));
    renderDialog();
    await toReviewStep();
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(marker("Created — Setup incomplete")).toHaveLength(3));
    expect((screen.getByRole("button", { name: "Retry failed" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("shows a failed reconnection's error on its row", async () => {
    createPrintersBatch.mockImplementationOnce(async (input: CreatePrintersBatchInput) => mixedOutcome(input));
    renderDialog();
    await toReviewStep();
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await screen.findByRole("status", { name: "Not created" });

    createPrintersBatch.mockImplementationOnce(async (input: CreatePrintersBatchInput) => ({
      batchId: input.batchId,
      rows: [result(input.rows[0].rowId, "created", { printer: record("prn-3") })],
    }));
    setConnection.mockRejectedValueOnce({
      contractVersion: 1,
      code: "PRINTER_UNREACHABLE",
      message: "The printer did not answer",
      recovery: [],
      retryable: true,
    });
    fireEvent.click(screen.getByRole("button", { name: "Retry failed" }));

    expect(await screen.findByText("The printer did not answer")).toBeInTheDocument();
    await waitFor(() => expect(marker("Created — Setup incomplete")).toHaveLength(1));
    expect(screen.getByLabelText("Host for row 2")).toHaveValue("10.0.0.2");
  });

  async function toFailedReconnectRow(beforeReview?: () => void) {
    createPrintersBatch.mockImplementationOnce(async (input: CreatePrintersBatchInput) => mixedOutcome(input));
    renderDialog();
    await toReviewStep(beforeReview);
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await screen.findByRole("status", { name: "Not created" });
    createPrintersBatch.mockImplementationOnce(async (input: CreatePrintersBatchInput) => ({
      batchId: input.batchId,
      rows: [result(input.rows[0].rowId, "created", { printer: record("prn-3") })],
    }));
  }

  const PROBE_FAILURE = {
    contractVersion: 1,
    code: "AUTHENTICATION_FAILED",
    message: "The API key was rejected again",
    recovery: [],
    retryable: false,
  };

  it("probes a reconnection first and keeps the row failed, with no save, when the probe fails", async () => {
    await toFailedReconnectRow();
    probeCandidate.mockRejectedValueOnce(PROBE_FAILURE);

    fireEvent.click(screen.getByRole("button", { name: "Retry failed" }));

    expect(await screen.findByText("The API key was rejected again")).toBeInTheDocument();
    expect(probeCandidate).toHaveBeenCalledWith({ kind: "moonraker", host: "10.0.0.2", port: 7125, useTls: false });
    expect(setConnection).not.toHaveBeenCalled();
    await waitFor(() => expect(marker("Created — Setup incomplete")).toHaveLength(1));
    const row = screen.getAllByTestId(/^row-/)[1];
    expect(within(row).getByRole("button", { name: "Save anyway" })).toBeInTheDocument();
  });

  it("'Save anyway' saves the failed row's Connection unverified and marks it created", async () => {
    await toFailedReconnectRow();
    probeCandidate.mockRejectedValueOnce(PROBE_FAILURE);
    fireEvent.click(screen.getByRole("button", { name: "Retry failed" }));
    const row = screen.getAllByTestId(/^row-/)[1];
    const saveAnyway = await within(row).findByRole("button", { name: "Save anyway" });
    setConnection.mockResolvedValueOnce(undefined);

    fireEvent.click(saveAnyway);

    await waitFor(() => expect(setConnection).toHaveBeenCalledTimes(1));
    expect(setConnection.mock.calls[0]).toEqual([
      "prn-2",
      { kind: "moonraker", host: "10.0.0.2", port: 7125, useTls: false },
      true,
    ]);
    await waitFor(() => expect(marker("Created")).toHaveLength(3));
    expect(screen.queryByRole("button", { name: "Save anyway" })).not.toBeInTheDocument();
  });

  it("drops 'Save anyway' once the failed row's host is edited", async () => {
    await toFailedReconnectRow();
    probeCandidate.mockRejectedValueOnce(PROBE_FAILURE);
    fireEvent.click(screen.getByRole("button", { name: "Retry failed" }));
    const row = screen.getAllByTestId(/^row-/)[1];
    await within(row).findByRole("button", { name: "Save anyway" });

    fireEvent.input(screen.getByLabelText("Host for row 2"), { target: { value: "10.0.0.22" } });

    expect(within(row).queryByRole("button", { name: "Save anyway" })).not.toBeInTheDocument();
  });

  it("saves a reconnection directly, without probing, when testing is turned off", async () => {
    await toFailedReconnectRow(() => {});
    fireEvent.click(screen.getByLabelText("Test each Connection before saving it"));
    setConnection.mockResolvedValueOnce(undefined);

    fireEvent.click(screen.getByRole("button", { name: "Retry failed" }));

    await waitFor(() => expect(setConnection).toHaveBeenCalledTimes(1));
    expect(probeCandidate).not.toHaveBeenCalled();
    expect(setConnection.mock.calls[0]).toEqual([
      "prn-2",
      { kind: "moonraker", host: "10.0.0.2", port: 7125, useTls: false },
    ]);
    await waitFor(() => expect(marker("Created")).toHaveLength(3));
  });

  it("cancels a running batch and leaves cancelled rows retryable", async () => {
    let resolveBatch!: (output: CreatePrintersBatchOutput) => void;
    createPrintersBatch.mockImplementationOnce(
      () => new Promise<CreatePrintersBatchOutput>((resolve) => (resolveBatch = resolve)),
    );
    renderDialog();
    await toReviewStep();
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(createPrintersBatch).toHaveBeenCalledTimes(1));
    const input = createPrintersBatch.mock.calls[0][0] as CreatePrintersBatchInput;

    fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    expect(cancelBatch).toHaveBeenCalledWith(input.batchId);

    resolveBatch({
      batchId: input.batchId,
      rows: [
        result(input.rows[0].rowId, "created", { printer: record("prn-1") }),
        result(input.rows[1].rowId, "cancelled"),
        result(input.rows[2].rowId, "cancelled"),
      ],
    });
    await waitFor(() => expect(marker("Cancelled")).toHaveLength(2));
    const retry = screen.getByRole("button", { name: "Retry failed" }) as HTMLButtonElement;
    expect(retry.disabled).toBe(false);

    createPrintersBatch.mockImplementationOnce(async (next: CreatePrintersBatchInput) => ({
      batchId: next.batchId,
      rows: next.rows.map((row, i) => result(row.rowId, "created", { printer: record(`prn-r${i}`) })),
    }));
    fireEvent.click(retry);
    await waitFor(() => expect(createPrintersBatch).toHaveBeenCalledTimes(2));
    expect((createPrintersBatch.mock.calls[1][0] as CreatePrintersBatchInput).rows.map((row) => row.rowId)).toEqual([
      input.rows[1].rowId,
      input.rows[2].rowId,
    ]);
  });

  it("shows a whole-batch command failure without losing the rows", async () => {
    createPrintersBatch.mockRejectedValueOnce({
      contractVersion: 1,
      code: "PERSISTENCE_UNAVAILABLE",
      message: "The database is unavailable",
      recovery: [],
      retryable: true,
    });
    renderDialog();
    await toReviewStep();
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(await screen.findByText("The database is unavailable")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByLabelText("Name for row 1")).toHaveValue("Voron 01"));
  });
});

describe("PrinterBatchDialog — Equip", () => {
  it("offers Equip only on created rows; it closes the dialog and asks to open that Printer's Material Slots", async () => {
    createPrintersBatch.mockImplementation(async (input: CreatePrintersBatchInput) => {
      const [a, b, c] = input.rows;
      return {
        batchId: input.batchId,
        rows: [
          result(a.rowId, "created", { printer: record("prn-1") }),
          result(b.rowId, "createdSetupIncomplete", { printer: record("prn-2") }),
          result(c.rowId, "cancelled"),
        ],
      };
    });
    const onOpenChange = vi.fn();
    const onEquip = vi.fn();
    renderDialog([], onOpenChange, onEquip);
    await toReviewStep();
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await screen.findByRole("button", { name: "Equip Voron 01" });

    expect(screen.getByRole("button", { name: "Equip Voron 02" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Equip Voron 03" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Equip Voron 02" }));
    // A cancelled row counts as unfinished, so closing asks first.
    fireEvent.click(await screen.findByRole("button", { name: "Close anyway" }));

    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(onEquip).toHaveBeenCalledWith("prn-2");
  });

  it("Equip closes straight away when nothing is left unfinished", async () => {
    createPrintersBatch.mockImplementation(async (input: CreatePrintersBatchInput) => ({
      batchId: input.batchId,
      rows: input.rows.map((row, i) => result(row.rowId, "created", { printer: record(`prn-${i + 1}`) })),
    }));
    const onOpenChange = vi.fn();
    const onEquip = vi.fn();
    renderDialog([], onOpenChange, onEquip);
    await toReviewStep();
    fireEvent.click(screen.getByRole("button", { name: "Create" }));

    await screen.findByRole("button", { name: "Done" }); // the batch has settled
    fireEvent.click(screen.getByRole("button", { name: "Equip Voron 01" }));

    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(onEquip).toHaveBeenCalledWith("prn-1");
  });
});

describe("PrinterBatchDialog — closing", () => {
  it("asks for confirmation (in a Dialog) before closing with failed rows", async () => {
    createPrintersBatch.mockImplementationOnce(async (input: CreatePrintersBatchInput) => mixedOutcome(input));
    const onOpenChange = renderDialog();
    await toReviewStep();
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await screen.findByRole("status", { name: "Not created" });

    fireEvent.click(screen.getAllByRole("button", { name: "Close" })[0]);
    const confirm = await screen.findByRole("button", { name: "Close anyway" });
    expect(onOpenChange).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Close anyway" })).not.toBeInTheDocument());
    expect(onOpenChange).not.toHaveBeenCalled();

    fireEvent.click(screen.getAllByRole("button", { name: "Close" })[0]);
    fireEvent.click(await screen.findByRole("button", { name: "Close anyway" }));
    expect(confirm).toBeDefined();
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("closes without confirmation when nothing failed", async () => {
    const onOpenChange = renderDialog();
    fireEvent.click(await screen.findByRole("button", { name: "Close" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(screen.queryByRole("button", { name: "Close anyway" })).not.toBeInTheDocument();
  });

  it("clears the shared credential when the dialog closes", async () => {
    const [open, setOpen] = createSignal(true);
    render(() => <PrinterBatchDialog open={open()} onOpenChange={setOpen} existingPrinters={[]} />);
    await toConnectStep("1");
    fireEvent.input(screen.getByLabelText("Shared credential"), { target: { value: "s3cret" } });
    expect(screen.getByLabelText("Shared credential")).toHaveValue("s3cret");

    setOpen(false);
    await waitFor(() => expect(screen.queryByLabelText("Shared credential")).not.toBeInTheDocument());
    setOpen(true);
    await toConnectStep("1");
    expect(screen.getByLabelText("Shared credential")).toHaveValue("");
  });
});

describe("toRowError", () => {
  const failure = (code: string) => ({ contractVersion: 1, code, message: `msg ${code}`, recovery: [], retryable: false });

  it("keeps row-vocabulary codes and the command's message", () => {
    expect(toRowError(failure("TIMEOUT"))).toEqual({ code: "TIMEOUT", message: "msg TIMEOUT" });
  });

  it("maps codes outside the row vocabulary to the closest row code", () => {
    expect(toRowError(failure("CREDENTIAL_REQUIRED"))).toEqual({
      code: "CREDENTIAL_UNAVAILABLE",
      message: "msg CREDENTIAL_REQUIRED",
    });
    expect(toRowError(failure("CONFLICT")).code).toBe("VALIDATION");
    expect(toRowError(failure("NOT_FOUND")).code).toBe("VALIDATION");
    expect(toRowError(failure("CORRUPT_DATA")).code).toBe("PERSISTENCE_UNAVAILABLE");
    expect(toRowError(new Error("boom")).code).toBe("VALIDATION");
  });
});
