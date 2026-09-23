import { render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SpoolDetailDock } from "./SpoolDetailDock";
import type { SpoolHistory } from "../generated/contracts/command/SpoolHistory";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";

const loadHistory = vi.fn();

vi.mock("../spools/spool-store", () => ({
  get spoolState() {
    return { spools: [], tares: [] };
  },
  loadHistory: (...args: unknown[]) => loadHistory(...args),
  moveSpool: vi.fn(),
  setLifecycle: vi.fn(),
  createSpool: vi.fn(),
  updateSpool: vi.fn(),
  recordAmount: vi.fn(),
}));

vi.mock("../printers/printer-store", () => ({
  printers: () => [],
}));

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
});

const ESTIMATED_SPOOL: SpoolRecord = {
  id: "spl-1", revision: 4, spoolNumber: 7,
  manufacturer: "Overture", product: "PETG", materialFamily: "PETG",
  colorName: "Clear", diameter: "1.75",
  nominalMg: 1_000_000, lowThresholdMg: 100_000,
  lifecycle: "active",
  location: { kind: "storage", storageLabel: "Shelf A2" },
  availability: { currentMg: 612_000, reservedMg: 0, availableMg: 612_000 },
  facets: { loaded: false, reserved: false, low: false, confidence: "estimated" },
  createdAt: "2026-08-01T00:00:00Z", updatedAt: "2026-09-10T00:00:00Z",
};

const HISTORY: SpoolHistory = {
  movements: [
    {
      id: "mov-1", operationId: "op-1", spoolId: "spl-1", reason: "unload",
      from: { slotId: "slt-1", printerId: "prn-1", storageLabel: null },
      to: { slotId: null, printerId: null, storageLabel: "Shelf A2" },
      occurredAt: "2026-09-05T00:00:00Z",
    },
  ],
  amountEvents: [
    {
      id: "evt-1", spoolId: "spl-1", sequence: 1, kind: "initial",
      afterMg: 1_000_000, confidenceAfter: "estimated", occurredAt: "2026-08-01T00:00:00Z", isCorrection: false,
    },
    {
      id: "evt-2", spoolId: "spl-1", sequence: 2, kind: "consumption",
      beforeMg: 1_000_000, afterMg: 594_000, confidenceAfter: "estimated",
      occurredAt: "2026-08-20T00:00:00Z", isCorrection: false,
    },
    {
      id: "evt-3", spoolId: "spl-1", sequence: 3, kind: "measurement",
      beforeMg: 594_000, afterMg: 612_000, confidenceAfter: "measured",
      occurredAt: "2026-09-10T00:00:00Z", isCorrection: true,
    },
  ],
  reservations: [],
};

describe("SpoolDetailDock", () => {
  it("shows 'est.' as text for an estimated Remaining amount", () => {
    loadHistory.mockResolvedValue({ movements: [], amountEvents: [], reservations: [] });
    render(() => <SpoolDetailDock spool={ESTIMATED_SPOOL} mode="inline" onClose={vi.fn()} />);
    expect(screen.getByText("est.")).toBeInTheDocument();
  });

  it("merges movements and amount events newest first, with a correction line", async () => {
    loadHistory.mockResolvedValue(HISTORY);
    render(() => <SpoolDetailDock spool={ESTIMATED_SPOOL} mode="inline" onClose={vi.fn()} />);

    const correctionItem = await screen.findByText("Measured 612 g (corrected +18 g from the estimate)");
    expect(correctionItem).toBeInTheDocument();

    const list = screen.getByRole("list", { name: `History for Spool 7` });
    const titles = Array.from(list.querySelectorAll("li")).map((li) => li.textContent ?? "");
    // Newest first: the 2026-09-10 correction, then the 2026-09-05 unload, then the 2026-08-20 consumption, then the 2026-08-01 initial.
    expect(titles[0]).toContain("corrected +18 g from the estimate");
    expect(titles[1]).toContain("Unloaded to");
    expect(titles[2]).toContain("Consumed 594 g");
    expect(titles[3]).toContain("Initial");
  });
});
