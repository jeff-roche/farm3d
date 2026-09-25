import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DeleteModelDialog } from "./DeleteModelDialog";
import { libraryStoreMock, resetLibraryStoreMock } from "../library/library-store-mock";
import { model } from "../library/test-records";

vi.mock("../library/library-store", async () => (await import("../library/library-store-mock")).libraryStoreMock);

const CUBE = model({ id: "mdl-cube", name: "Cube" });

beforeEach(resetLibraryStoreMock);
afterEach(cleanup);

describe("DeleteModelDialog", () => {
  it("states D18's wording and deletes on confirm", async () => {
    const onDeleted = vi.fn();
    render(() => <DeleteModelDialog model={CUBE} onClose={vi.fn()} onDeleted={onDeleted} />);
    expect(screen.getByRole("dialog", { name: "Delete Model" }))
      .toHaveTextContent("Delete Cube? Its imported revisions are deleted. The original file on disk is not.");
    await fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(libraryStoreMock.deleteModel).toHaveBeenCalledWith("mdl-cube");
    await waitFor(() => expect(onDeleted).toHaveBeenCalledWith("mdl-cube"));
  });

  it("lists the blockers of a LIFECYCLE_BLOCKED delete, and keeps the Model", async () => {
    libraryStoreMock.deleteModel.mockRejectedValueOnce({
      contractVersion: 1, code: "LIFECYCLE_BLOCKED", recovery: [], retryable: false,
      message: "This action is blocked: Cube is in Slice Revision 3. Cube is queued.",
      details: {
        blockers: [
          { action: "delete", code: "inUse", message: "Cube is in Slice Revision 3." },
          { action: "delete", code: "inUse", message: "Cube is queued." },
        ],
      },
    });
    const onDeleted = vi.fn();
    render(() => <DeleteModelDialog model={CUBE} onClose={vi.fn()} onDeleted={onDeleted} />);
    await fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Cube can't be deleted yet.");
    const blockers = within(alert).getAllByRole("listitem").map((item) => item.textContent);
    expect(blockers).toEqual(["Cube is in Slice Revision 3.", "Cube is queued."]);
    expect(onDeleted).not.toHaveBeenCalled();
  });

  it("says where to delete the Slice Revisions that block a delete", async () => {
    libraryStoreMock.deleteModel.mockRejectedValueOnce({
      contractVersion: 1, code: "LIFECYCLE_BLOCKED", recovery: [], retryable: false,
      message: "This action is blocked: Delete this Model's 2 Slice Revisions first.",
      details: {
        blockers: [
          { action: "delete", code: "SLICE_REVISIONS_EXIST", message: "Delete this Model's 2 Slice Revisions first." },
        ],
      },
    });
    render(() => <DeleteModelDialog model={CUBE} onClose={vi.fn()} onDeleted={vi.fn()} />);
    await fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    const alert = await screen.findByRole("alert");
    expect(within(alert).getByRole("listitem")).toHaveTextContent("Delete this Model's 2 Slice Revisions first.");
    expect(alert).toHaveTextContent("Open each one under Slice Revisions in this Model's details to delete it.");
  });

  it("shows any other failure's message inline", async () => {
    libraryStoreMock.deleteModel.mockRejectedValueOnce({
      contractVersion: 1, code: "CONFLICT", message: "This Model changed. Try again.", recovery: [], retryable: false,
    });
    render(() => <DeleteModelDialog model={CUBE} onClose={vi.fn()} onDeleted={vi.fn()} />);
    await fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("This Model changed. Try again.");
  });

  it("can't be closed while the delete is running", async () => {
    let finish!: () => void;
    libraryStoreMock.deleteModel.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    const onClose = vi.fn();
    const onDeleted = vi.fn();
    render(() => <DeleteModelDialog model={CUBE} onClose={onClose} onDeleted={onDeleted} />);
    await fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    const cancel = screen.getByRole("button", { name: "Cancel" });
    expect(cancel).toBeDisabled();
    await fireEvent.click(cancel);
    await fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    await fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).not.toHaveBeenCalled();

    finish();
    await waitFor(() => expect(onDeleted).toHaveBeenCalledWith("mdl-cube"));
    expect(screen.getByRole("button", { name: "Cancel" })).toBeEnabled();
  });
});
