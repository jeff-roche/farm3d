import { fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { buildMismatches, PrinterConnectionPanel } from "./PrinterConnectionPanel";
import type { PrinterProfile, ResolvedPrinter } from "../printers/types";

const setConnection = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
const testConnection = vi.hoisted(() => vi.fn());
const discoverPrinters = vi.hoisted(() => vi.fn().mockResolvedValue([]));
vi.mock("../printers/printer-store", () => ({
  setConnection,
  clearConnection: vi.fn(),
  testConnection,
  discoverPrinters,
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

  it("does not warn when the host reports more travel than the catalog volume", () => {
    // A0.1 (#9), decision B3: a Snapmaker U1 reports 271 × 335 × 281 mm of
    // axis travel for a 270 mm cube, because parking and tool-change moves
    // inflate the limits. Extra travel is not a wrong variant.
    const cube = {
      bedShape: { kind: "rectangular", widthMm: 270, depthMm: 270, originXMm: 0, originYMm: 0 },
      printableHeightMm: 270,
    } as unknown as PrinterProfile;
    expect(
      buildMismatches(cube, { bedWidthMm: 271, bedDepthMm: 335, printableHeightMm: 281 }),
    ).toEqual([]);
  });

  it("warns only for the axes where the host reports less than the catalog", () => {
    const cube = {
      bedShape: { kind: "rectangular", widthMm: 270, depthMm: 270, originXMm: 0, originYMm: 0 },
      printableHeightMm: 270,
    } as unknown as PrinterProfile;
    const mismatches = buildMismatches(cube, { bedWidthMm: 271, bedDepthMm: 220, printableHeightMm: 281 });
    expect(mismatches).toEqual(["Bed depth: catalog says 270 mm, the printer reports 220 mm"]);
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

  it("releases the API key field after a save settles", async () => {
    render(() => <PrinterConnectionPanel printer={printer} />);
    const apiKey = screen.getByLabelText("API key") as HTMLInputElement;
    fireEvent.input(apiKey, { target: { value: "submitted-secret" } });

    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(apiKey.value).toBe(""));
    expect(setConnection).toHaveBeenCalledWith(
      "prn-1",
      expect.objectContaining({ credential: "submitted-secret" }),
    );
  });

  it("shows a replacement probe failure inline with 'Save anyway', which resubmits with acceptUnverified (spec D8)", async () => {
    setConnection.mockRejectedValueOnce({
      contractVersion: 1,
      code: "AUTHENTICATION_FAILED",
      message: "Authentication failed",
      recovery: [],
      retryable: false,
    });
    setConnection.mockResolvedValueOnce(undefined);
    render(() => <PrinterConnectionPanel printer={printer} />);
    fireEvent.input(screen.getByLabelText("Host"), { target: { value: "voron.local" } });

    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText("Authentication failed");
    expect(screen.getByRole("button", { name: "Save anyway" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Save anyway" }));

    await waitFor(() => expect(setConnection).toHaveBeenCalledTimes(2));
    expect(setConnection).toHaveBeenLastCalledWith(
      "prn-1",
      expect.objectContaining({ host: "voron.local" }),
      true,
    );
    await waitFor(() => expect(screen.queryByText("Authentication failed")).not.toBeInTheDocument());
  });

  it("shows a non-probe Save error without offering 'Save anyway'", async () => {
    setConnection.mockRejectedValueOnce({
      contractVersion: 1,
      code: "DUPLICATE_HOST",
      message: "Another Printer already uses this host.",
      recovery: [],
      retryable: false,
    });
    render(() => <PrinterConnectionPanel printer={printer} />);
    fireEvent.input(screen.getByLabelText("Host"), { target: { value: "voron.local" } });

    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await screen.findByText("Another Printer already uses this host.");
    expect(screen.queryByRole("button", { name: "Save anyway" })).not.toBeInTheDocument();
  });

  it("drops the failed Save and its 'Save anyway' once the Connection is edited", async () => {
    setConnection.mockRejectedValueOnce({
      contractVersion: 1,
      code: "PRINTER_UNREACHABLE",
      message: "The printer could not be reached.",
      recovery: [],
      retryable: true,
    });
    render(() => <PrinterConnectionPanel printer={printer} />);
    fireEvent.input(screen.getByLabelText("Host"), { target: { value: "voron.local" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText("The printer could not be reached.");
    expect(screen.getByRole("button", { name: "Save anyway" })).toBeInTheDocument();

    fireEvent.input(screen.getByLabelText("Host"), { target: { value: "voron-2.local" } });

    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "Save anyway" })).not.toBeInTheDocument(),
    );
    expect(screen.queryByText("The printer could not be reached.")).not.toBeInTheDocument();
  });

  it("renders a probe failure inline rather than throwing it away", async () => {
    testConnection.mockRejectedValueOnce("Could not reach the printer: refused");
    render(() => <PrinterConnectionPanel printer={printer} />);
    fireEvent.click(screen.getByRole("button", { name: "Test connection" }));
    expect(await screen.findByText(/Could not reach the printer/)).toBeInTheDocument();
  });

  it("keeps a verified Test result visible after a Save that didn't edit the connection", async () => {
    testConnection.mockResolvedValueOnce({
      kind: "moonraker",
      hostSoftware: "Moonraker 0.9",
      firmware: "Klipper v0.12",
      reportedName: "Bay 1",
      state: "online",
      stateMessage: "",
      reported: {},
    });
    render(() => <PrinterConnectionPanel printer={printer} />);

    fireEvent.click(screen.getByRole("button", { name: "Test connection" }));
    expect(await screen.findByText("online")).toBeInTheDocument();

    // Save's own `finally` resets the (already-blank) credential field back
    // to "" -- that reset must not read as an edit that invalidates the
    // just-verified probe.
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(setConnection).toHaveBeenCalledTimes(1));

    expect(screen.getByText("online")).toBeInTheDocument();
  });

  it("shows which credential store is live", async () => {
    render(() => <PrinterConnectionPanel printer={printer} />);
    expect(await screen.findByText(/OS keychain/)).toBeInTheDocument();
  });

  it("distinguishes discovery failure from a successful empty scan", async () => {
    discoverPrinters.mockRejectedValueOnce(new Error("Discovery worker failed"));
    render(() => <PrinterConnectionPanel printer={printer} />);

    expect(await screen.findByText("Discovery failed — enter the host above.")).toBeInTheDocument();
    expect(screen.queryByText("Nothing found — enter the host above.")).not.toBeInTheDocument();
  });

  it("renders the Kind label exactly once, not doubled by an extra Field wrapper", async () => {
    render(() => <PrinterConnectionPanel printer={printer} />);
    await screen.findByRole("button", { name: /Moonraker/ });
    expect(screen.getAllByText("Kind")).toHaveLength(1);
  });
});

describe("PrinterConnectionPanel — Remove credentials", () => {
  const withCredential = {
    id: "prn-1",
    name: "Bay 1",
    profile: PROFILE,
    connection: {
      kind: "moonraker",
      host: "voron.local",
      port: 7125,
      useTls: false,
      credentialRef: "farm3d/credential/prn-1",
    },
  } as unknown as ResolvedPrinter;

  it("is offered only when a credential is stored", () => {
    const withoutCredential = {
      ...withCredential,
      connection: { ...withCredential.connection, credentialRef: undefined },
    } as unknown as ResolvedPrinter;
    render(() => <PrinterConnectionPanel printer={withoutCredential} />);

    expect(screen.queryByRole("button", { name: "Remove credentials" })).not.toBeInTheDocument();
  });

  it("asks for confirmation and does nothing when cancelled", async () => {
    render(() => <PrinterConnectionPanel printer={withCredential} />);

    fireEvent.click(screen.getByRole("button", { name: "Remove credentials" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent('deletes the stored API key for "Bay 1"');
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(setConnection).not.toHaveBeenCalled();
  });

  it("clears the stored credential on the saved Connection, ignoring unsaved edits", async () => {
    render(() => <PrinterConnectionPanel printer={withCredential} />);
    fireEvent.input(screen.getByLabelText("Host"), { target: { value: "unsaved.local" } });

    fireEvent.click(screen.getByRole("button", { name: "Remove credentials" }));
    await screen.findByRole("dialog");
    const confirm = screen
      .getAllByRole("button", { name: "Remove credentials" })
      .find((button) => button.closest("[role=dialog]"))!;
    fireEvent.click(confirm);

    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(setConnection).toHaveBeenCalledWith("prn-1", {
      kind: "moonraker",
      host: "voron.local",
      port: 7125,
      useTls: false,
      credential: "",
    });
  });

  it("keeps the dialog open and shows the error when removal fails", async () => {
    setConnection.mockRejectedValueOnce({
      contractVersion: 1, code: "CREDENTIAL_UNAVAILABLE", message: "The credential store is unavailable.",
      recovery: [], retryable: true,
    });
    render(() => <PrinterConnectionPanel printer={withCredential} />);

    fireEvent.click(screen.getByRole("button", { name: "Remove credentials" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(
      screen.getAllByRole("button", { name: "Remove credentials" }).find((button) => dialog.contains(button))!,
    );

    expect(await screen.findByText("The credential store is unavailable.")).toBeInTheDocument();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });
});

describe("unsupported suggested host types", () => {
  it("falls back to Moonraker when the catalog suggests a kind this build can't connect to", async () => {
    const prusa = {
      id: "prn-1",
      name: "Core One",
      profile: { ...PROFILE, suggestedHostType: "prusalink" },
    } as unknown as ResolvedPrinter;
    render(() => <PrinterConnectionPanel printer={prusa} />);
    fireEvent.input(screen.getByLabelText("Host"), { target: { value: "core.local" } });

    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByRole("button", { name: /Moonraker/ })).toBeInTheDocument();
    expect(setConnection).toHaveBeenCalledWith("prn-1", expect.objectContaining({ kind: "moonraker" }));
  });
});
