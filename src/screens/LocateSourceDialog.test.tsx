import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConvertToManagedDialog, LocateSourceDialog } from "./LocateSourceDialog";
import { buildWebLibraryFixture } from "../library/web-fixtures";
import { libraryStoreMock, resetLibraryStoreMock, setLibraryState } from "../library/library-store-mock";
import type { ImportSelectionSummary, ModelRecord } from "../library/types";

vi.mock("../library/library-store", async () => (await import("../library/library-store-mock")).libraryStoreMock);

const desktop = vi.hoisted(() => ({ available: true }));
vi.mock("../ipc/client", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../ipc/client")>()),
  desktopAvailable: () => desktop.available,
}));

const fixture = buildWebLibraryFixture(new Date("2026-09-24T12:00:00Z"));
const CLIP = fixture.models.find((m) => m.id === "mdl-web-clip")!;

const LOCATED: ImportSelectionSummary = {
  selectionId: "sel-locate",
  purpose: "locate",
  files: [{ fileIndex: 0, fileName: "cable-clip-v2.stl", sizeBytes: 900 }],
};

function commandError(code: string, message: string, details?: Record<string, string>) {
  return { contractVersion: 1, code, message, recovery: [], retryable: false, ...(details ? { details } : {}) };
}

const DIFFERS = commandError("SOURCE_CONTENT_DIFFERS", "The located file differs from the current revision.", {
  currentSha256: "c".repeat(64), locatedSha256: "d".repeat(64), locatedFileName: "cable-clip-v2.stl",
});

beforeEach(() => {
  resetLibraryStoreMock();
  setLibraryState({ projects: fixture.projects, models: fixture.models });
  desktop.available = true;
});
afterEach(cleanup);

function renderLocate(model: ModelRecord = CLIP) {
  const onClose = vi.fn();
  render(() => <LocateSourceDialog model={model} onClose={onClose} />);
  return { onClose };
}

describe("LocateSourceDialog", () => {
  it("names the missing file and where it was", () => {
    renderLocate();
    const dialog = screen.getByRole("dialog", { name: "Locate source" });
    expect(dialog).toHaveTextContent("farm3d can't find “cable-clip.stl”.");
    expect(dialog).toHaveTextContent("/home/maker/prints/cable-clip.stl");
  });

  it("Choose file… picks one file for locating, then locates without accepting different content", async () => {
    libraryStoreMock.pickFiles.mockResolvedValueOnce(LOCATED);
    const { onClose } = renderLocate();
    await fireEvent.click(screen.getByRole("button", { name: "Choose file…" }));
    await waitFor(() => expect(libraryStoreMock.locateSource).toHaveBeenCalledWith("mdl-web-clip", "sel-locate", 0, false));
    expect(libraryStoreMock.pickFiles).toHaveBeenCalledWith("locate");
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  });

  it("does nothing when the picker is cancelled", async () => {
    libraryStoreMock.pickFiles.mockResolvedValueOnce(null);
    const { onClose } = renderLocate();
    await fireEvent.click(screen.getByRole("button", { name: "Choose file…" }));
    await waitFor(() => expect(libraryStoreMock.pickFiles).toHaveBeenCalledOnce());
    expect(libraryStoreMock.locateSource).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("on SOURCE_CONTENT_DIFFERS, explains and offers Relink and import as a new revision", async () => {
    libraryStoreMock.pickFiles.mockResolvedValueOnce(LOCATED);
    libraryStoreMock.locateSource.mockRejectedValueOnce(DIFFERS);
    const { onClose } = renderLocate();
    await fireEvent.click(screen.getByRole("button", { name: "Choose file…" }));

    expect(await screen.findByText("This file is different from the last imported version.")).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Locate source" })).toHaveTextContent("cable-clip-v2.stl");
    await fireEvent.click(screen.getByRole("button", { name: "Relink and import as a new revision" }));
    await waitFor(() => expect(libraryStoreMock.locateSource).toHaveBeenLastCalledWith("mdl-web-clip", "sel-locate", 0, true));
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
    // The selection was used, so there's nothing to cancel.
    expect(libraryStoreMock.cancelSelection).not.toHaveBeenCalled();
  });

  it("shows a VALIDATION after consent inline, and keeps the dialog open", async () => {
    libraryStoreMock.pickFiles.mockResolvedValueOnce(LOCATED);
    libraryStoreMock.locateSource
      .mockRejectedValueOnce(DIFFERS)
      .mockRejectedValueOnce(commandError("VALIDATION", "This file isn't a readable STL."));
    const { onClose } = renderLocate();
    await fireEvent.click(screen.getByRole("button", { name: "Choose file…" }));
    await fireEvent.click(await screen.findByRole("button", { name: "Relink and import as a new revision" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("This file isn't a readable STL.");
    expect(libraryStoreMock.cancelSelection).toHaveBeenCalledWith("sel-locate");
    expect(onClose).not.toHaveBeenCalled();
    expect(libraryStoreMock.reportLibraryError).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Choose file…" })).toBeEnabled();
  });

  it("shows a refused first locate inline and releases its selection", async () => {
    libraryStoreMock.pickFiles.mockResolvedValueOnce(LOCATED);
    libraryStoreMock.locateSource.mockRejectedValueOnce(commandError("VALIDATION", "Choose an STL file."));
    renderLocate();
    await fireEvent.click(screen.getByRole("button", { name: "Choose file…" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Choose an STL file.");
    expect(libraryStoreMock.cancelSelection).toHaveBeenCalledWith("sel-locate");
    expect(screen.queryByRole("button", { name: "Relink and import as a new revision" })).toBeNull();
  });

  it("can't be closed while a locate is running", async () => {
    let refuse!: (error: unknown) => void;
    libraryStoreMock.pickFiles.mockResolvedValueOnce(LOCATED);
    libraryStoreMock.locateSource.mockImplementationOnce(() => new Promise((_resolve, reject) => { refuse = reject; }));
    const { onClose } = renderLocate();
    await fireEvent.click(screen.getByRole("button", { name: "Choose file…" }));
    await waitFor(() => expect(libraryStoreMock.locateSource).toHaveBeenCalled());

    expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
    await fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    await fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).not.toHaveBeenCalled();

    refuse(DIFFERS);
    expect(await screen.findByRole("button", { name: "Relink and import as a new revision" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeEnabled();
  });

  it("releases the selection of a SOURCE_CONTENT_DIFFERS that arrives after the dialog unmounts", async () => {
    let refuse!: (error: unknown) => void;
    libraryStoreMock.pickFiles.mockResolvedValueOnce(LOCATED);
    libraryStoreMock.locateSource.mockImplementationOnce(() => new Promise((_resolve, reject) => { refuse = reject; }));
    const onClose = vi.fn();
    const { unmount } = render(() => <LocateSourceDialog model={CLIP} onClose={onClose} />);
    await fireEvent.click(screen.getByRole("button", { name: "Choose file…" }));
    await waitFor(() => expect(libraryStoreMock.locateSource).toHaveBeenCalled());

    unmount();
    refuse(DIFFERS);
    await waitFor(() => expect(libraryStoreMock.cancelSelection).toHaveBeenCalledWith("sel-locate"));
    expect(onClose).not.toHaveBeenCalled();
  });

  it("cancels an unused selection when closed after SOURCE_CONTENT_DIFFERS", async () => {
    libraryStoreMock.pickFiles.mockResolvedValueOnce(LOCATED);
    libraryStoreMock.locateSource.mockRejectedValueOnce(DIFFERS);
    const { onClose } = renderLocate();
    await fireEvent.click(screen.getByRole("button", { name: "Choose file…" }));
    await screen.findByRole("button", { name: "Relink and import as a new revision" });
    await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(libraryStoreMock.cancelSelection).toHaveBeenCalledWith("sel-locate");
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("disables Choose file… in web mode, saying why", () => {
    desktop.available = false;
    renderLocate();
    const choose = screen.getByRole("button", { name: "Choose file…" });
    expect(choose).toBeDisabled();
    const reason = screen.getByText("Locating a source file needs the desktop app.");
    expect(choose.getAttribute("aria-describedby")).toBe(reason.id);
  });
});

describe("ConvertToManagedDialog", () => {
  it("confirms that farm3d stops following the file and keeps the revisions, then converts", async () => {
    const onClose = vi.fn();
    render(() => <ConvertToManagedDialog model={CLIP} onClose={onClose} />);
    const dialog = screen.getByRole("dialog", { name: "Convert to managed" });
    expect(dialog).toHaveTextContent("farm3d will stop following cable-clip.stl. Your existing revisions are kept.");
    expect(libraryStoreMock.convertToManaged).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByRole("button", { name: "Convert to managed" }));
    expect(libraryStoreMock.convertToManaged).toHaveBeenCalledWith("mdl-web-clip");
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  });

  it("shows a failure inline", async () => {
    libraryStoreMock.convertToManaged.mockRejectedValueOnce(commandError("CONFLICT", "This Model changed. Try again."));
    const onClose = vi.fn();
    render(() => <ConvertToManagedDialog model={CLIP} onClose={onClose} />);
    await fireEvent.click(screen.getByRole("button", { name: "Convert to managed" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("This Model changed. Try again.");
    expect(onClose).not.toHaveBeenCalled();
  });
});
