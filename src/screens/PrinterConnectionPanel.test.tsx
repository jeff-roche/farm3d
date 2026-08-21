import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { buildMismatches, PrinterConnectionPanel } from "./PrinterConnectionPanel";
import type { PrinterProfile, ResolvedPrinter } from "../printers/types";

const setConnection = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
const testConnection = vi.hoisted(() => vi.fn());
vi.mock("../printers/printer-store", () => ({
  setConnection,
  clearConnection: vi.fn(),
  testConnection,
  discoverPrinters: vi.fn().mockResolvedValue([]),
  credentialStoreInfo: vi.fn().mockResolvedValue({ kind: "keychain" }),
}));

// This suite doesn't run with vitest's `globals: true`, so
// @solidjs/testing-library's own `afterEach(cleanup)` (gated on a global
// `afterEach` existing) never registers — without this, each `render()`
// below stacks onto the previous test's still-mounted DOM instead of
// replacing it. Mirrors the cleanup convention in sibling screen tests
// (e.g. PrinterProfilePanel.test.tsx).
afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
});

// `unknown as` (not `never`) so the fixture below can still be spread — a
// value typed `never` cannot be spread (TS2698).
const PROFILE = {
  bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
  printableHeightMm: 256,
} as unknown as PrinterProfile;

describe("buildMismatches", () => {
  it("is silent when the host agrees with the catalog", () => {
    expect(
      buildMismatches(PROFILE, { bedWidthMm: 256, bedDepthMm: 256, printableHeightMm: 256 }),
    ).toEqual([]);
  });

  it("reports a build volume the host disagrees with", () => {
    // A mismatch usually means the wrong catalog variant was picked when the
    // printer was added — worth surfacing, not hiding.
    const mismatches = buildMismatches(PROFILE, { bedWidthMm: 220 });
    expect(mismatches).toHaveLength(1);
    expect(mismatches[0]).toMatch(/220/);
  });

  it("says nothing about values the host did not report", () => {
    // An unhomed Klipper reports no axis limits. Absence is not disagreement.
    expect(buildMismatches(PROFILE, {})).toEqual([]);
  });

  it("tolerates a millimetre of float noise", () => {
    expect(buildMismatches(PROFILE, { bedWidthMm: 256.4 })).toEqual([]);
  });

  it("says nothing for a non-rectangular bed", () => {
    const polygon = { bedShape: { kind: "polygon", points: [] }, printableHeightMm: 256 } as never;
    expect(buildMismatches(polygon, { bedWidthMm: 1 })).toEqual([]);
  });
});

describe("PrinterConnectionPanel", () => {
  const printer = {
    id: "prn-1",
    name: "Bay 1",
    profile: PROFILE,
    connection: null,
  } as unknown as ResolvedPrinter;

  it("defaults the kind from the catalog's suggestedHostType", async () => {
    const suggested = {
      ...printer,
      profile: { ...PROFILE, suggestedHostType: "moonraker" },
    } as unknown as ResolvedPrinter;
    render(() => <PrinterConnectionPanel printer={suggested} />);
    expect(await screen.findByRole("button", { name: /Moonraker/ })).toBeInTheDocument();
  });

  it("submits host and port without echoing an unset API key", async () => {
    render(() => <PrinterConnectionPanel printer={printer} />);
    fireEvent.input(screen.getByLabelText("Host"), { target: { value: "voron.local" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(setConnection).toHaveBeenCalledWith(
      "prn-1",
      expect.objectContaining({ host: "voron.local", port: 7125 }),
    );
  });

  it("renders a probe failure inline rather than throwing it away", async () => {
    testConnection.mockRejectedValueOnce("Could not reach the printer: refused");
    render(() => <PrinterConnectionPanel printer={printer} />);
    fireEvent.click(screen.getByRole("button", { name: "Test connection" }));
    expect(await screen.findByText(/Could not reach the printer/)).toBeInTheDocument();
  });

  it("shows which credential store is live", async () => {
    render(() => <PrinterConnectionPanel printer={printer} />);
    expect(await screen.findByText(/OS keychain/)).toBeInTheDocument();
  });

  it("renders the Kind label exactly once, not doubled by an extra Field wrapper", async () => {
    render(() => <PrinterConnectionPanel printer={printer} />);
    await screen.findByRole("button", { name: /Moonraker/ });
    expect(screen.getAllByText("Kind")).toHaveLength(1);
  });
});
