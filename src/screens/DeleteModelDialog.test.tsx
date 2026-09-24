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
    await waitFor(() => expect(onDeleted).toHaveBeenCalledOnce());
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

  it("shows any other failure's message inline", async () => {
    libraryStoreMock.deleteModel.mockRejectedValueOnce({
      contractVersion: 1, code: "CONFLICT", message: "This Model changed. Try again.", recovery: [], retryable: false,
    });
    render(() => <DeleteModelDialog model={CUBE} onClose={vi.fn()} onDeleted={vi.fn()} />);
    await fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("This Model changed. Try again.");
  });
});
