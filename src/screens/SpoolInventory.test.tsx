import { createStore } from "solid-js/store";
import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SpoolInventory } from "./SpoolInventory";
import { navigation } from "../navigation/navigation-store";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";

const loadInventory = vi.fn();
const loadHistory = vi.fn();

interface MockSpoolStoreState {
  spools: SpoolRecord[];
  tares: unknown[];
  loaded: boolean;
  pending: Record<string, unknown>;
}

/** A real Solid store (not a plain getter) so `<Show when={spoolState.loaded}>`
 *  reacts when a test flips `loaded` after the component has already
 *  mounted -- exercising the same timing the real `spool-store.ts` module
 *  produces while `loadInventory()`'s promise is still in flight. */
const [spoolStoreState, setSpoolStoreState] = createStore<MockSpoolStoreState>({
  spools: [], tares: [], loaded: true, pending: {},
});

function setSpools(list: SpoolRecord[]) {
  setSpoolStoreState({ spools: list });
}

vi.mock("../spools/spool-store", () => ({
  get spoolState() {
    return spoolStoreState;
  },
  loadInventory: (...args: unknown[]) => loadInventory(...args),
  spoolStoreError: () => null,
  dismissSpoolStoreError: vi.fn(),
  loadHistory: (...args: unknown[]) => loadHistory(...args),
  moveSpool: vi.fn(),
  setLifecycle: vi.fn(),
  createSpool: vi.fn(),
  updateSpool: vi.fn(),
  recordAmount: vi.fn(),
  createTare: vi.fn(),
  updateTare: vi.fn(),
  deleteTare: vi.fn(),
}));

vi.mock("../printers/printer-store", () => ({
  printers: () => [],
}));

function spool(overrides: Partial<SpoolRecord> = {}): SpoolRecord {
  return {
    id: "spl-1", revision: 1, spoolNumber: 1,
    manufacturer: "Prusament", product: "PLA", materialFamily: "PLA",
    colorName: "Black", diameter: "1.75",
    nominalMg: 1_000_000, lowThresholdMg: 100_000,
    lifecycle: "active",
    location: { kind: "storage", storageLabel: null },
    availability: { currentMg: 500_000, reservedMg: 0, availableMg: 500_000 },
    facets: { loaded: false, reserved: false, low: false, confidence: "measured" },
    createdAt: "2026-09-01T00:00:00Z", updatedAt: "2026-09-01T00:00:00Z",
    ...overrides,
  };
}

beforeEach(() => {
  loadHistory.mockResolvedValue({ movements: [], amountEvents: [], reservations: [] });
  navigation.navigate({ version: 1, destination: "spools" });
  window.location.hash = "";
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
  setSpoolStoreState({ spools: [], tares: [], loaded: true, pending: {} });
});

describe("SpoolInventory", () => {
  it("shows the fixture rows", () => {
    setSpools([
      spool({ id: "spl-1", spoolNumber: 1 }),
      spool({ id: "spl-2", spoolNumber: 2, colorName: "Clear" }),
    ]);
    render(() => <SpoolInventory />);
    expect(screen.getByText("#1")).toBeInTheDocument();
    expect(screen.getByText("#2")).toBeInTheDocument();
  });

  it("narrows the rows when the Low and Estimated chips are toggled", async () => {
    setSpools([
      spool({ id: "spl-1", spoolNumber: 1 }),
      spool({
        id: "spl-2", spoolNumber: 2,
        facets: { loaded: false, reserved: false, low: true, confidence: "estimated" },
      }),
    ]);
    render(() => <SpoolInventory />);
    expect(screen.getByText("#1")).toBeInTheDocument();
    expect(screen.getByText("#2")).toBeInTheDocument();

    const lowChip = screen.getByRole("button", { name: "Low" });
    await fireEvent.click(lowChip);
    expect(lowChip).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByText("#1")).not.toBeInTheDocument();
    expect(screen.getByText("#2")).toBeInTheDocument();

    await fireEvent.click(lowChip);
    expect(lowChip).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByText("#1")).toBeInTheDocument();

    const estimatedChip = screen.getByRole("button", { name: "Estimated" });
    await fireEvent.click(estimatedChip);
    expect(screen.queryByText("#1")).not.toBeInTheDocument();
    expect(screen.getByText("#2")).toBeInTheDocument();
  });

  it("shows 'No Spools match' and Clear filters when the filters exclude everything", async () => {
    setSpools([spool({ id: "spl-1", spoolNumber: 1 })]);
    render(() => <SpoolInventory />);
    await fireEvent.click(screen.getByRole("button", { name: "Low" }));
    expect(screen.getByText("No Spools match")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Clear filters" }).length).toBeGreaterThan(0);
  });

  it("shows 'No Spools yet' and Add Spool with no Spools at all", () => {
    setSpools([]);
    render(() => <SpoolInventory />);
    expect(screen.getByText("No Spools yet")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Add Spool" }).length).toBeGreaterThan(0);
  });

  it("sets the deep link and opens the detail dock when a row is selected by keyboard", async () => {
    setSpools([
      spool({ id: "spl-1", spoolNumber: 1 }),
      spool({ id: "spl-2", spoolNumber: 2 }),
    ]);
    render(() => <SpoolInventory />);
    const grid = screen.getByRole("grid", { name: "Spools" });
    grid.focus();
    await fireEvent.keyDown(grid, { key: "ArrowDown" });

    expect(navigation.target()).toEqual({
      version: 1, destination: "spools", selection: { kind: "spool", id: "spl-1" },
    });
    expect(window.location.hash).toBe("#nav=v1/spools/spool/spl-1");
    expect(await screen.findByText("#1 PLA Black")).toBeInTheDocument();
  });

  it("keeps the dock inline on a wide workspace even when the inventory hasn't loaded yet at mount", async () => {
    // Regression test: the workspace `ref` used to live inside
    // `<Show when={spoolState.loaded}>`, so the `onMount` that measures the
    // workspace width and attaches the `ResizeObserver` bailed out
    // (`if (!workspace) return`) whenever the inventory was still loading
    // at mount -- exactly the case here. `dockMode` was then stuck at its
    // "overlay" default forever, even on a workspace wide enough for the
    // inline dock, once the inventory (and the selected Spool) arrived.
    class WideWorkspaceObserver {
      constructor(private readonly callback: ResizeObserverCallback) {}
      observe(target: Element) {
        this.callback([{ target, contentRect: { width: 1440 } } as ResizeObserverEntry], this as unknown as ResizeObserver);
      }
      disconnect() {}
      unobserve() {}
    }
    vi.stubGlobal("ResizeObserver", WideWorkspaceObserver);

    setSpoolStoreState({ loaded: false, spools: [] });
    navigation.navigate({
      version: 1, destination: "spools", selection: { kind: "spool", id: "spl-1" },
    }, { availableDestinations: ["spools"], availableIds: [] });

    render(() => <SpoolInventory />);
    expect(screen.getByText("Loading Spools…")).toBeInTheDocument();
    expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    setSpoolStoreState({ loaded: true, spools: [spool({ id: "spl-1", spoolNumber: 1 })] });

    expect(await screen.findByRole("complementary", { name: "Spool 1" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});
