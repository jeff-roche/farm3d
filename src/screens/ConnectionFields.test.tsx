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
});
