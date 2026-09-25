import { createSignal } from "solid-js";
import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ConnectionFields, type ConnectionDraft } from "./ConnectionFields";
import type { ProbeResult } from "../printers/types";

const discoverPrinters = vi.hoisted(() => vi.fn().mockResolvedValue([]));
vi.mock("../printers/printer-store", () => ({ discoverPrinters }));

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
});

const DEFAULT_DRAFT: ConnectionDraft = {
  kind: "moonraker",
  host: "",
  port: 7125,
  useTls: false,
  credential: "",
};

function Harness(props: { onTest: () => Promise<ProbeResult> }) {
  const [value, setValue] = createSignal<ConnectionDraft>(DEFAULT_DRAFT);
  return <ConnectionFields value={value()} onChange={setValue} onTest={props.onTest} />;
}

describe("ConnectionFields", () => {
  it("fills kind, host, and port from a discovery click", async () => {
    discoverPrinters.mockResolvedValueOnce([
      { kind: "moonraker", name: "Voron 2.4", host: "voron.local", port: 7125, addresses: [] },
    ]);
    render(() => <Harness onTest={vi.fn()} />);

    const candidate = await screen.findByRole("button", { name: /Voron 2\.4/ });
    fireEvent.click(candidate);

    expect((screen.getByLabelText("Host") as HTMLInputElement).value).toBe("voron.local");
    expect((screen.getByLabelText("Port") as HTMLInputElement).value).toBe("7125");
    expect(await screen.findByRole("button", { name: /Moonraker/ })).toBeInTheDocument();
  });

  it("shows the online chip after a successful Test", async () => {
    const onTest = vi.fn().mockResolvedValue({
      kind: "moonraker",
      hostSoftware: "Moonraker 0.9",
      firmware: "Klipper v0.12",
      reportedName: "Voron 2.4",
      state: "online",
      stateMessage: "",
      reported: {},
    });
    render(() => <Harness onTest={onTest} />);

    fireEvent.click(screen.getByRole("button", { name: "Test connection" }));

    expect(await screen.findByText("online")).toBeInTheDocument();
    expect(onTest).toHaveBeenCalledTimes(1);
  });

  it("shows the error text inline when Test fails", async () => {
    const onTest = vi.fn().mockRejectedValue("Could not reach the printer: refused");
    render(() => <Harness onTest={onTest} />);

    fireEvent.click(screen.getByRole("button", { name: "Test connection" }));

    expect(await screen.findByText(/Could not reach the printer/)).toBeInTheDocument();
  });

  it("clears a verified probe result once the host is edited afterward", async () => {
    const onTest = vi.fn().mockResolvedValue({
      kind: "moonraker",
      hostSoftware: "Moonraker 0.9",
      firmware: "Klipper v0.12",
      reportedName: "Voron 2.4",
      state: "online",
      stateMessage: "",
      reported: {},
    });
    render(() => <Harness onTest={onTest} />);

    fireEvent.click(screen.getByRole("button", { name: "Test connection" }));
    expect(await screen.findByText("online")).toBeInTheDocument();

    fireEvent.input(screen.getByLabelText("Host"), { target: { value: "a-different-host.local" } });

    expect(screen.queryByText("online")).not.toBeInTheDocument();
    expect(screen.queryByText(/Moonraker 0\.9/)).not.toBeInTheDocument();
  });

  it("offers OctoPrint as a selectable kind and switches to its default port", async () => {
    render(() => <Harness onTest={vi.fn()} />);

    fireEvent.pointerDown(screen.getByRole("button", { name: /Moonraker/ }), { pointerType: "mouse", button: 0 });
    fireEvent.click(await screen.findByRole("option", { name: "OctoPrint" }));

    expect(await screen.findByRole("button", { name: /OctoPrint/ })).toBeInTheDocument();
    expect((screen.getByLabelText("Port") as HTMLInputElement).value).toBe("80");
  });

  it("seeds OctoPrint and port 80 from a catalog-suggested octoprint host type", () => {
    const onChange = vi.fn();
    render(() => (
      <ConnectionFields value={DEFAULT_DRAFT} onChange={onChange} suggestedKind="octoprint" onTest={vi.fn()} />
    ));

    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ kind: "octoprint", port: 80 }));
  });

  it("renders an OctoPrint probe without an empty firmware slot", async () => {
    const onTest = vi.fn().mockResolvedValue({
      kind: "octoprint",
      hostSoftware: "1.11.8",
      firmware: "",
      reportedName: "Default",
      state: "Operational",
      stateMessage: "",
      reported: {},
    });
    render(() => <Harness onTest={onTest} />);

    fireEvent.click(screen.getByRole("button", { name: "Test connection" }));

    expect(await screen.findByText("Operational")).toBeInTheDocument();
    expect(screen.getByText("Default — 1.11.8")).toBeInTheDocument();
  });

  it("ignores a catalog-suggested kind this build can't connect to", () => {
    const onChange = vi.fn();
    render(() => (
      <ConnectionFields value={DEFAULT_DRAFT} onChange={onChange} suggestedKind="prusalink" onTest={vi.fn()} />
    ));

    expect(onChange).not.toHaveBeenCalled();
  });
});
