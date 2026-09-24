import { createSignal } from "solid-js";
import { fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DeletePrinterDialog } from "./DeletePrinterDialog";

const removePrinter = vi.hoisted(() => vi.fn().mockResolvedValue({ ok: true }));
vi.mock("../printers/printer-store", () => ({ removePrinter }));

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
  removePrinter.mockResolvedValue({ ok: true });
});

describe("DeletePrinterDialog", () => {
  it("warns that Spool movement history involving this Printer will be deleted (D10)", () => {
    render(() => <DeletePrinterDialog open onOpenChange={vi.fn()} printerId="prn-1" printerName="North Bay" />);
    expect(screen.getByText("Spool movement history involving this Printer will be deleted.")).toBeInTheDocument();
  });

  it("keeps 'Delete permanently' disabled until the typed name matches exactly, then calls removePrinter", async () => {
    const onOpenChange = vi.fn();
    render(() => (
      <DeletePrinterDialog
        open
        onOpenChange={onOpenChange}
        printerId="prn-1"
        printerName="North Bay"
      />
    ));

    const confirm = screen.getByRole("button", { name: "Delete permanently" });
    expect(confirm).toBeDisabled();

    const field = screen.getByLabelText("Type the Printer name to confirm");
    await fireEvent.input(field, { target: { value: "north bay" } });
    expect(confirm).toBeDisabled(); // case must match exactly

    await fireEvent.input(field, { target: { value: "North Bay " } });
    expect(confirm).toBeDisabled(); // no trimming

    await fireEvent.input(field, { target: { value: "North Bay" } });
    expect(confirm).not.toBeDisabled();

    fireEvent.click(confirm);

    await waitFor(() => expect(removePrinter).toHaveBeenCalledWith("prn-1"));
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  });

  it("calls onDeleted after a successful delete", async () => {
    const onDeleted = vi.fn();
    render(() => (
      <DeletePrinterDialog
        open
        onOpenChange={vi.fn()}
        printerId="prn-1"
        printerName="North Bay"
        onDeleted={onDeleted}
      />
    ));

    await fireEvent.input(screen.getByLabelText("Type the Printer name to confirm"), {
      target: { value: "North Bay" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Delete permanently" }));

    await waitFor(() => expect(onDeleted).toHaveBeenCalledOnce());
  });

  it("stays open and shows the error inline when the delete fails", async () => {
    removePrinter.mockResolvedValueOnce({ ok: false, message: "The Printer changed; reload and retry." });
    const onOpenChange = vi.fn();
    const onDeleted = vi.fn();
    render(() => (
      <DeletePrinterDialog
        open
        onOpenChange={onOpenChange}
        printerId="prn-1"
        printerName="North Bay"
        onDeleted={onDeleted}
      />
    ));

    await fireEvent.input(screen.getByLabelText("Type the Printer name to confirm"), {
      target: { value: "North Bay" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Delete permanently" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("The Printer changed; reload and retry.");
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
    expect(onDeleted).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Delete permanently" })).not.toBeDisabled();
  });

  it("resets the typed name each time the dialog re-opens", async () => {
    const [openState, setOpenState] = createSignal(false);
    render(() => (
      <DeletePrinterDialog
        open={openState()}
        onOpenChange={setOpenState}
        printerId="prn-1"
        printerName="North Bay"
      />
    ));

    setOpenState(true);
    await fireEvent.input(screen.getByLabelText("Type the Printer name to confirm"), {
      target: { value: "North Bay" },
    });
    expect(screen.getByRole("button", { name: "Delete permanently" })).not.toBeDisabled();

    setOpenState(false);
    setOpenState(true);
    expect((screen.getByLabelText("Type the Printer name to confirm") as HTMLInputElement).value).toBe("");
    expect(screen.getByRole("button", { name: "Delete permanently" })).toBeDisabled();
  });

  it("does not call window.confirm", async () => {
    const confirmSpy = vi.spyOn(window, "confirm");
    render(() => (
      <DeletePrinterDialog open onOpenChange={vi.fn()} printerId="prn-1" printerName="North Bay" />
    ));
    await fireEvent.input(screen.getByLabelText("Type the Printer name to confirm"), {
      target: { value: "North Bay" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Delete permanently" }));

    expect(confirmSpy).not.toHaveBeenCalled();
  });
});
