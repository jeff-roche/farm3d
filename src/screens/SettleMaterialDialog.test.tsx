import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { queueStoreMock, resetQueueStoreMock } from "../queue/queue-store-mock";
import { job } from "../queue/test-records";
import type { Job } from "../queue/types";
import { pressEscape, tabOrder } from "./dialog-test-helpers";
import { SettleMaterialDialog } from "./SettleMaterialDialog";

vi.mock("../queue/queue-store", async () => (await import("../queue/queue-store-mock")).queueStoreMock);
const inventory: { spools: { id: string; spoolNumber: number; tareId?: string }[]; tares: { id: string; name: string; weightMg: number }[] } = {
  spools: [],
  tares: [],
};
vi.mock("../spools/spool-store", () => ({
  get spoolState() {
    return inventory;
  },
  ensureInventoryLoaded: () => Promise.resolve(),
}));

beforeEach(() => {
  resetQueueStoreMock();
  inventory.spools = [];
  inventory.tares = [];
});
afterEach(cleanup);

const FAILED = job({
  state: "failed", settlement: "pending", allowedActions: ["retry", "settleMaterial"],
  estimateMg: 25_000, maxProgressPct: 60, settlementPreview: { estimatedUseMg: 15_000 },
});

function renderDialog(record: Job, onOpenChange = vi.fn()) {
  render(() => <SettleMaterialDialog job={record} open onOpenChange={onOpenChange} />);
  return onOpenChange;
}

describe("SettleMaterialDialog", () => {
  it("shows Rust's settlementPreview on the estimate option and settles with it", async () => {
    renderDialog(FAILED);
    const dialog = await screen.findByRole("dialog", { name: "Settle material" });
    const estimate = within(dialog).getByLabelText("Use estimate (15.0 g)");
    fireEvent.click(estimate);
    fireEvent.click(within(dialog).getByRole("button", { name: "Settle" }));
    await waitFor(() => expect(queueStoreMock.settleJobMaterial).toHaveBeenCalledWith("job-1", { kind: "estimated" }));
  });

  it("offers Defer for a failed or cancelled Job, saying the amount stays unavailable", async () => {
    for (const state of ["failed", "cancelled"] as const) {
      const { unmount } = render(() => <SettleMaterialDialog job={{ ...FAILED, state }} open onOpenChange={vi.fn()} />);
      const dialog = await screen.findByRole("dialog", { name: "Settle material" });
      fireEvent.click(within(dialog).getByLabelText("Defer"));
      expect(within(dialog).getByText(/stays unavailable/)).toBeInTheDocument();
      fireEvent.click(within(dialog).getByRole("button", { name: "Defer" }));
      await waitFor(() => expect(queueStoreMock.settleJobMaterial).toHaveBeenLastCalledWith("job-1", { kind: "defer" }));
      unmount();
    }
  });

  it("doesn't offer Defer again once deferred", async () => {
    renderDialog({ ...FAILED, settlement: "deferred" });
    const dialog = await screen.findByRole("dialog", { name: "Settle material" });
    expect(within(dialog).queryByLabelText("Defer")).toBeNull();
    expect(within(dialog).getByText(/deferred/i)).toBeInTheDocument();
  });

  it("settles a measured remaining weight through P3's amount entry", async () => {
    renderDialog(FAILED);
    const dialog = await screen.findByRole("dialog", { name: "Settle material" });
    fireEvent.click(within(dialog).getByLabelText("Enter measured remaining weight"));
    fireEvent.input(within(dialog).getByLabelText("Net weight (g)"), { target: { value: "612" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Settle" }));
    await waitFor(() => expect(queueStoreMock.settleJobMaterial).toHaveBeenCalledWith("job-1", {
      kind: "measured", entry: { kind: "net", netMg: 612_000, confidence: "measured" },
    }));
  });

  it("corrects a completed Job with a measured weight only (no estimate, no Defer)", async () => {
    renderDialog(job({ state: "completed", settlement: "settled", settlementMethod: "estimated", allowedActions: ["retry", "correctMaterial"] }));
    const dialog = await screen.findByRole("dialog", { name: "Correct weight" });
    expect(within(dialog).queryByLabelText("Defer")).toBeNull();
    expect(within(dialog).queryByLabelText(/Use estimate/)).toBeNull();
    fireEvent.input(within(dialog).getByLabelText("Net weight (g)"), { target: { value: "500" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Record correction" }));
    await waitFor(() => expect(queueStoreMock.correctJobMaterial).toHaveBeenCalledWith("job-1", { kind: "net", netMg: 500_000, confidence: "measured" }));
  });

  it("is keyboard operable: nothing to confirm until a choice is made; Escape closes", async () => {
    const onOpenChange = renderDialog(FAILED);
    const dialog = await screen.findByRole("dialog", { name: "Settle material" });
    expect(within(dialog).getByRole("button", { name: "Settle" })).toBeDisabled();
    expect(tabOrder(dialog)).toEqual(["Close", "Use estimate (15.0 g)", "Cancel"]);
    pressEscape(dialog);
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  });

  it("starts Scale entry on the Spool's own tare, as P3's Record amount does", async () => {
    inventory.spools = [{ id: "spl-1", spoolNumber: 9, tareId: "tar-cardboard" }];
    inventory.tares = [{ id: "tar-cardboard", name: "Cardboard", weightMg: 200_000 }];
    renderDialog(FAILED);
    const dialog = await screen.findByRole("dialog", { name: "Settle material" });
    fireEvent.click(within(dialog).getByLabelText("Enter measured remaining weight"));
    fireEvent.click(within(dialog).getByLabelText("Scale"));
    expect(within(dialog).getByRole("button", { name: /Tare/ })).toHaveTextContent("Cardboard (200.0 g)");
    fireEvent.input(within(dialog).getByLabelText("Gross weight (g)"), { target: { value: "812" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Settle" }));
    await waitFor(() => expect(queueStoreMock.settleJobMaterial).toHaveBeenCalledWith("job-1", {
      kind: "measured", entry: { kind: "scale", grossMg: 812_000, tareId: "tar-cardboard" },
    }));
  });

  it("says why Settle is disabled while the measured weight is missing", async () => {
    renderDialog(FAILED);
    const dialog = await screen.findByRole("dialog", { name: "Settle material" });
    expect(within(dialog).getByText("Choose how much material the Job used.")).toBeInTheDocument();
    fireEvent.click(within(dialog).getByLabelText("Enter measured remaining weight"));
    expect(within(dialog).getByRole("button", { name: "Settle" })).toBeDisabled();
    expect(within(dialog).getByText("Enter the Spool's remaining weight.")).toBeInTheDocument();
    fireEvent.input(within(dialog).getByLabelText("Net weight (g)"), { target: { value: "612" } });
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Settle" })).toBeEnabled());
    expect(within(dialog).queryByText("Enter the Spool's remaining weight.")).toBeNull();
  });
});
