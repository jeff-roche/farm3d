import { fireEvent, render, screen, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { BatchRowDraft } from "../printers/batch-intake";
import { BatchRowsTable, type BatchRowsTableProps } from "./BatchRowsTable";

afterEach(() => {
  document.body.innerHTML = "";
});

function row(overrides: Partial<BatchRowDraft> = {}): BatchRowDraft {
  return {
    rowId: "r1",
    name: "Voron 01",
    location: "Bay A",
    host: "",
    port: null,
    protocol: "moonraker",
    useTls: false,
    credential: { source: "none" },
    selected: true,
    ...overrides,
  };
}

function renderTable(overrides: Partial<BatchRowsTableProps> = {}) {
  const props: BatchRowsTableProps = {
    rows: [row()],
    onChange: vi.fn(),
    onToggle: vi.fn(),
    onToggleAll: vi.fn(),
    onRemove: vi.fn(),
    mode: "edit",
    preview: { defaultBedType: "Textured PEI Plate", startSafety: "confirmBedClear" },
    ...overrides,
  };
  render(() => <BatchRowsTable {...props} />);
  return props;
}

describe("BatchRowsTable — edit mode", () => {
  it("edits a row's name and location inline", () => {
    const props = renderTable();
    fireEvent.input(screen.getByLabelText("Name for row 1"), { target: { value: "Voron 99" } });
    expect(props.onChange).toHaveBeenCalledWith("r1", { name: "Voron 99" });
    fireEvent.input(screen.getByLabelText("Location for row 1"), { target: { value: "Bay Z" } });
    expect(props.onChange).toHaveBeenCalledWith("r1", { location: "Bay Z" });
  });

  it("removes a row", () => {
    const props = renderTable();
    fireEvent.click(screen.getByRole("button", { name: "Remove Voron 01" }));
    expect(props.onRemove).toHaveBeenCalledWith("r1");
  });

  it("toggles a row's selection with Space on its checkbox", () => {
    const props = renderTable();
    const checkbox = screen.getByRole("checkbox", { name: "Select Voron 01" });
    // Kobalte's Checkbox handles Space on its Control, the element that
    // follows the (visually hidden) native input.
    const control = checkbox.nextElementSibling as HTMLElement;
    fireEvent.keyDown(control, { key: " " });
    fireEvent.keyUp(control, { key: " " });
    expect(props.onToggle).toHaveBeenCalledWith("r1", false);
  });

  it("selects or clears every row from the header checkbox", () => {
    const props = renderTable({ rows: [row(), row({ rowId: "r2", name: "Voron 02", selected: false })] });
    fireEvent.click(screen.getByRole("checkbox", { name: "Select all rows" }));
    expect(props.onToggleAll).toHaveBeenCalledWith(true);
  });

  it("shows each row's preview of the copied bed type and start safety (D9)", () => {
    renderTable({ preview: { defaultBedType: "", startSafety: "unattended" } });
    const preview = screen.getByTestId("preview-r1");
    expect(preview).toHaveTextContent("Default");
    expect(preview).toHaveTextContent("Unattended starts");
  });

  it("warns (without blocking) on a name that collides with an existing Printer or another row (D11)", () => {
    renderTable({
      rows: [row(), row({ rowId: "r2", name: " voron 01 " }), row({ rowId: "r3", name: "Ender" })],
      existingNames: ["ender"],
    });
    expect(screen.getAllByText("Another row has this name")).toHaveLength(2);
    expect(screen.getByText("A Printer already has this name")).toBeInTheDocument();
  });

  it("shows a created row read-only, with no Remove control", () => {
    renderTable({ rows: [row({ printerId: "prn-1" })] });
    expect(screen.queryByLabelText("Name for row 1")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove Voron 01" })).not.toBeInTheDocument();
    expect(screen.getByText("Voron 01")).toBeInTheDocument();
  });
});

describe("BatchRowsTable — connect mode", () => {
  it("edits host and port inline and shows protocol, TLS, and credential source", () => {
    const props = renderTable({
      mode: "connect",
      rows: [row({ host: "10.0.0.5", port: 7125, useTls: true, credential: { source: "shared" } })],
    });
    expect(screen.getByLabelText("Host for row 1")).toHaveValue("10.0.0.5");
    fireEvent.input(screen.getByLabelText("Port for row 1"), { target: { value: "7130" } });
    expect(props.onChange).toHaveBeenCalledWith("r1", { port: 7130 });
    fireEvent.input(screen.getByLabelText("Host for row 1"), { target: { value: "10.0.0.6" } });
    expect(props.onChange).toHaveBeenCalledWith("r1", { host: "10.0.0.6" });
    const cells = screen.getByTestId("row-r1");
    expect(within(cells).getByText("moonraker")).toBeInTheDocument();
    expect(within(cells).getByText("On")).toBeInTheDocument();
    expect(within(cells).getByText("Shared")).toBeInTheDocument();
  });

  it("offers a masked per-row credential field only for a row credential source", () => {
    const props = renderTable({ mode: "connect", rows: [row({ credential: { source: "row", value: "" } })] });
    const field = screen.getByLabelText("Credential for row 1");
    expect(field).toHaveAttribute("type", "password");
    fireEvent.input(field, { target: { value: "k" } });
    expect(props.onChange).toHaveBeenCalledWith("r1", { credential: { source: "row", value: "k" } });
  });
});

describe("BatchRowsTable — results mode", () => {
  it("shows each outcome with an icon, text, and severity colour, plus its errors and warnings", () => {
    renderTable({
      mode: "results",
      rows: [
        row({ rowId: "a", name: "A", printerId: "p1", result: { rowId: "a", outcome: "created", credentialStored: false, errors: [], warnings: [{ code: "DUPLICATE_NAME", message: "Name already used" }] } }),
        row({
          rowId: "b",
          name: "B",
          host: "10.0.0.9",
          port: 7125,
          printerId: "p2",
          result: {
            rowId: "b",
            outcome: "createdSetupIncomplete",
            credentialStored: false,
            errors: [{ code: "AUTHENTICATION_FAILED", message: "The API key was rejected", fieldPath: "connection.credential" }],
            warnings: [],
          },
        }),
        row({ rowId: "c", name: "", result: { rowId: "c", outcome: "rejected", credentialStored: false, errors: [{ code: "VALIDATION", message: "Name is required", fieldPath: "name" }], warnings: [] } }),
        row({ rowId: "d", name: "D", result: { rowId: "d", outcome: "cancelled", credentialStored: false, errors: [], warnings: [] } }),
      ],
    });

    const expectMarker = (rowId: string, label: string, severity: string) => {
      const marker = within(screen.getByTestId(`row-${rowId}`)).getByRole("status", { name: label });
      expect(marker).toHaveAttribute("data-severity", severity);
      expect(marker.querySelector("svg")).not.toBeNull();
      expect(marker).toHaveTextContent(label);
    };
    expectMarker("a", "Created", "resolved");
    expectMarker("b", "Created — Setup incomplete", "warning");
    expectMarker("c", "Not created", "fatal");
    expectMarker("d", "Cancelled", "info");

    expect(screen.getByText("Name already used")).toBeInTheDocument();
    expect(screen.getByText(/The API key was rejected/)).toHaveTextContent("connection.credential");
    expect(screen.getByText(/Name is required/)).toHaveTextContent("name");
  });

  it("keeps a failed row's input editable for retry, and a created row read-only", () => {
    const props = renderTable({
      mode: "results",
      rows: [
        row({ rowId: "a", name: "A", printerId: "p1", result: { rowId: "a", outcome: "created", credentialStored: false, errors: [], warnings: [] } }),
        row({ rowId: "b", name: "B", host: "10.0.0.9", port: 7125, printerId: "p2", result: { rowId: "b", outcome: "createdSetupIncomplete", credentialStored: false, errors: [{ code: "TIMEOUT", message: "Timed out" }], warnings: [] } }),
        row({ rowId: "c", name: "Bad/Name", result: { rowId: "c", outcome: "rejected", credentialStored: false, errors: [{ code: "VALIDATION", message: "Invalid", fieldPath: "name" }], warnings: [] } }),
      ],
    });
    expect(screen.queryByLabelText("Name for row 1")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Host for row 1")).not.toBeInTheDocument();
    // Created without a connection: identity is fixed, connection is editable.
    expect(screen.queryByLabelText("Name for row 2")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Host for row 2")).toHaveValue("10.0.0.9");
    // Rejected: identity keeps its typed value and stays editable.
    expect(screen.getByLabelText("Name for row 3")).toHaveValue("Bad/Name");
    fireEvent.input(screen.getByLabelText("Name for row 3"), { target: { value: "Good" } });
    expect(props.onChange).toHaveBeenCalledWith("c", { name: "Good" });
  });

  it("shows in-progress rows as pending", () => {
    renderTable({ mode: "results", rows: [row()], pending: new Set(["r1"]) });
    expect(screen.getByRole("status", { name: "In progress" })).toBeInTheDocument();
  });
});
