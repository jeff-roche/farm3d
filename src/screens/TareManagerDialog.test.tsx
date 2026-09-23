import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TareManagerDialog } from "./TareManagerDialog";
import type { Tare } from "../generated/contracts/domain/Tare";

const createTare = vi.fn();
const updateTare = vi.fn();
const deleteTare = vi.fn();
const tares: Tare[] = [
  { id: "tar-1", revision: 1, name: "Cardboard", weightMg: 200_000, createdAt: "", updatedAt: "" },
];

vi.mock("../spools/spool-store", () => ({
  get spoolState() {
    return { tares };
  },
  createTare: (...args: unknown[]) => createTare(...args),
  updateTare: (...args: unknown[]) => updateTare(...args),
  deleteTare: (...args: unknown[]) => deleteTare(...args),
}));

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
});

describe("TareManagerDialog", () => {
  it("adds a tare with mg values", async () => {
    createTare.mockResolvedValue({ id: "tar-2", revision: 1, name: "Plastic", weightMg: 180_000, createdAt: "", updatedAt: "" });
    render(() => <TareManagerDialog open onOpenChange={vi.fn()} />);

    await fireEvent.input(screen.getByLabelText("New tare name"), { target: { value: "Plastic" } });
    await fireEvent.input(screen.getByLabelText("Weight (g)"), { target: { value: "180" } });
    await fireEvent.click(screen.getByRole("button", { name: "Add tare" }));

    expect(createTare).toHaveBeenCalledWith("Plastic", 180_000);
  });

  it("renames a tare", async () => {
    updateTare.mockResolvedValue({ id: "tar-1", revision: 2, name: "Cardboard spool", weightMg: 210_000, createdAt: "", updatedAt: "" });
    render(() => <TareManagerDialog open onOpenChange={vi.fn()} />);

    await fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    const nameInput = screen.getByLabelText("Tare name for Cardboard");
    await fireEvent.input(nameInput, { target: { value: "Cardboard spool" } });
    const weightInput = screen.getByLabelText("Tare weight for Cardboard");
    await fireEvent.input(weightInput, { target: { value: "210" } });
    await fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(updateTare).toHaveBeenCalledWith("tar-1", "Cardboard spool", 210_000);
  });

  it("deletes a tare", async () => {
    render(() => <TareManagerDialog open onOpenChange={vi.fn()} />);
    await fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(deleteTare).toHaveBeenCalledWith("tar-1");
  });

  it("shows a VALIDATION rejection inline under the new tare's name field and stays open", async () => {
    createTare.mockRejectedValue({
      contractVersion: 1, code: "VALIDATION", message: "A tare with that name already exists.",
      recovery: ["EDIT_FIELDS"], retryable: false, details: { fieldPath: "name" },
    });
    const onOpenChange = vi.fn();
    render(() => <TareManagerDialog open onOpenChange={onOpenChange} />);

    await fireEvent.input(screen.getByLabelText("New tare name"), { target: { value: "Cardboard" } });
    await fireEvent.input(screen.getByLabelText("Weight (g)"), { target: { value: "200" } });
    await fireEvent.click(screen.getByRole("button", { name: "Add tare" }));

    expect(await screen.findByText("A tare with that name already exists.")).toBeInTheDocument();
    expect(screen.getByLabelText("New tare name")).toHaveAttribute("aria-invalid", "true");
    expect(onOpenChange).not.toHaveBeenCalled();
  });

  it("shows a dialog-level message and stays open on a rename CONFLICT", async () => {
    updateTare.mockRejectedValue({
      contractVersion: 1, code: "CONFLICT", message: "Someone else renamed this tare.",
      recovery: ["RETRY"], retryable: true,
    });
    const onOpenChange = vi.fn();
    render(() => <TareManagerDialog open onOpenChange={onOpenChange} />);

    await fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    await fireEvent.input(screen.getByLabelText("Tare name for Cardboard"), { target: { value: "Cardboard spool" } });
    await fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(/reloaded/i);
    expect(onOpenChange).not.toHaveBeenCalled();
    // Stays in edit mode too -- the rename row's own fields are still shown.
    expect(screen.getByLabelText("Tare name for Cardboard")).toBeInTheDocument();
  });
});
