import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cameraSnapshot } from "../attention/test-records";
import { SnapshotViewerDialog } from "./SnapshotViewerDialog";

const snapshotImageUrlMock = vi.hoisted(() => vi.fn(async (id: string) => `blob:${id}`));
const setSnapshotPinnedMock = vi.hoisted(() => vi.fn());
vi.mock("../cameras/camera-store", () => ({
  snapshotImageUrl: snapshotImageUrlMock,
  setSnapshotPinned: setSnapshotPinnedMock,
}));

afterEach(() => {
  cleanup();
  snapshotImageUrlMock.mockClear();
  setSnapshotPinnedMock.mockReset();
});

function commandError(code: string, message: string) {
  return { contractVersion: 1, code, message, recovery: [], retryable: false };
}

describe("SnapshotViewerDialog", () => {
  it("shows the image with alt text, the trigger, and the captured time", async () => {
    const snapshot = cameraSnapshot({ id: "snp-1", trigger: "incident", capturedAt: "2026-09-25T12:00:00Z" });
    render(() => <SnapshotViewerDialog snapshot={snapshot} printerName="Bay 8" open onOpenChange={vi.fn()} />);

    expect(snapshotImageUrlMock).toHaveBeenCalledWith("snp-1");
    const image = await screen.findByAltText(/Incident snapshot of Bay 8 at/);
    expect(image).toHaveAttribute("src", "blob:snp-1");
    expect(screen.getByText("Incident")).toBeInTheDocument();
  });

  it("a pruned snapshot shows 'Evidence pruned (<reason>)' and no image", async () => {
    const snapshot = cameraSnapshot({ id: "snp-2", prunedAt: "2026-09-26T00:00:00Z", pruneReason: "diskCap" });
    render(() => <SnapshotViewerDialog snapshot={snapshot} printerName="Bay 8" open onOpenChange={vi.fn()} />);

    expect(screen.getByText("Evidence pruned (disk cap reached)")).toBeInTheDocument();
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
    expect(snapshotImageUrlMock).not.toHaveBeenCalled();
  });

  it("pins from the viewer (Enter/Space press a button through its click) and switches to Unpin", async () => {
    const snapshot = cameraSnapshot({ id: "snp-1", pinnedAt: null });
    setSnapshotPinnedMock.mockResolvedValue(cameraSnapshot({ id: "snp-1", pinnedAt: "2026-09-25T13:00:00Z", revision: 2 }));
    render(() => <SnapshotViewerDialog snapshot={snapshot} printerName="Bay 8" open onOpenChange={vi.fn()} />);

    const pin = screen.getByRole("button", { name: "Pin" });
    pin.focus();
    await fireEvent.click(pin);

    await waitFor(() => expect(setSnapshotPinnedMock).toHaveBeenCalledWith("snp-1", true));
    expect(await screen.findByRole("button", { name: "Unpin" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Pin" })).not.toBeInTheDocument();
  });

  it("unpins from the viewer", async () => {
    const snapshot = cameraSnapshot({ id: "snp-1", pinnedAt: "2026-09-25T13:00:00Z" });
    setSnapshotPinnedMock.mockResolvedValue(cameraSnapshot({ id: "snp-1", pinnedAt: null, revision: 3 }));
    render(() => <SnapshotViewerDialog snapshot={snapshot} printerName="Bay 8" open onOpenChange={vi.fn()} />);

    await fireEvent.click(screen.getByRole("button", { name: "Unpin" }));
    await waitFor(() => expect(setSnapshotPinnedMock).toHaveBeenCalledWith("snp-1", false));
    expect(await screen.findByRole("button", { name: "Pin" })).toBeInTheDocument();
  });

  it("unpinning a pruned, pinned snapshot is offered; pinning a pruned one is not", () => {
    const snapshot = cameraSnapshot({ id: "snp-1", pinnedAt: "2026-09-25T13:00:00Z", prunedAt: "2026-09-26T00:00:00Z", pruneReason: "missingFile" });
    render(() => <SnapshotViewerDialog snapshot={snapshot} printerName="Bay 8" open onOpenChange={vi.fn()} />);

    expect(screen.getByRole("button", { name: "Unpin" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Pin" })).not.toBeInTheDocument();
  });

  it("a pruned, unpinned snapshot offers neither Pin nor Unpin", () => {
    const snapshot = cameraSnapshot({ id: "snp-1", pinnedAt: null, prunedAt: "2026-09-26T00:00:00Z", pruneReason: "age" });
    render(() => <SnapshotViewerDialog snapshot={snapshot} printerName="Bay 8" open onOpenChange={vi.fn()} />);

    expect(screen.queryByRole("button", { name: "Pin" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Unpin" })).not.toBeInTheDocument();
  });

  it("shows an error when pinning fails", async () => {
    const live = cameraSnapshot({ id: "snp-1", pinnedAt: null });
    setSnapshotPinnedMock.mockRejectedValue(commandError("EVIDENCE_PRUNED", "This snapshot's image was removed (past retention)."));
    render(() => <SnapshotViewerDialog snapshot={live} printerName="Bay 8" open onOpenChange={vi.fn()} />);

    await fireEvent.click(screen.getByRole("button", { name: "Pin" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("This snapshot's image was removed (past retention).");
  });
});
