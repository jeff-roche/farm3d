import { createSignal } from "solid-js";
import { fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  CameraSourceSection,
  draftToCameraSource,
  EMPTY_CAMERA_DRAFT,
  type CameraSourceDraft,
} from "./CameraSourceSection";
import type { HostWebcam } from "../attention/types";
import type { ConnectionSubmission } from "../printers/types";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

const listHostWebcams = vi.hoisted(() => vi.fn());
const testCamera = vi.hoisted(() => vi.fn());
vi.mock("../cameras/camera-store", () => ({ listHostWebcams, testCamera }));

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
});

function webcam(overrides: Partial<HostWebcam> = {}): HostWebcam {
  return { name: "front", service: "webrtc", ...overrides };
}

/** A real reactive stand-in for the wizard's own controlling state --
 *  `value`/`onChange` need genuine Solid signals for the component's
 *  internal `createEffect`s (listing webcams, clearing a stale test
 *  result on a kind switch) to actually re-run during a test. */
function DraftHarness(props: { initial: CameraSourceDraft; connection?: ConnectionSubmission }) {
  const [value, setValue] = createSignal(props.initial);
  return <CameraSourceSection mode="draft" value={value()} onChange={setValue} connection={props.connection} />;
}

describe("CameraSourceSection — draft mode", () => {
  it("defaults to None, showing no webcam picker, URL field, or Test button", () => {
    render(() => (
      <CameraSourceSection mode="draft" value={{ ...EMPTY_CAMERA_DRAFT }} onChange={vi.fn()} />
    ));

    expect(screen.getByRole("radio", { name: "None" })).toBeChecked();
    expect(screen.queryByLabelText("Webcam")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Snapshot URL")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Test snapshot" })).not.toBeInTheDocument();
  });

  it("explains why the webcam list is empty when there's no Connection yet", () => {
    render(() => <DraftHarness initial={{ ...EMPTY_CAMERA_DRAFT, kind: "hostWebcam" }} />);

    expect(screen.getByText("Connect a Connection first to list this printer's webcams.")).toBeInTheDocument();
    expect(listHostWebcams).not.toHaveBeenCalled();
  });

  it("lists host webcams once a Connection is given", async () => {
    listHostWebcams.mockResolvedValueOnce([webcam({ name: "front" }), webcam({ name: "bed" })]);
    render(() => (
      <DraftHarness
        initial={{ ...EMPTY_CAMERA_DRAFT, kind: "hostWebcam" }}
        connection={{ kind: "moonraker", host: "192.0.2.10", port: 7125, useTls: false }}
      />
    ));

    await waitFor(() => expect(listHostWebcams).toHaveBeenCalledWith({
      connection: { kind: "moonraker", host: "192.0.2.10", port: 7125, useTls: false },
    }));
    expect(await screen.findByLabelText("Webcam")).toBeInTheDocument();
  });

  it("shows a manual URL's VALIDATION error inline on the field, by its Rust field path", async () => {
    testCamera.mockRejectedValueOnce({
      contractVersion: 1,
      code: "VALIDATION",
      message: "The snapshot URL is not valid.",
      recovery: [],
      retryable: false,
      details: { fieldPath: "source.snapshotUrl" },
    });
    render(() => <DraftHarness initial={{ ...EMPTY_CAMERA_DRAFT, kind: "snapshotUrl", snapshotUrl: "not a url" }} />);

    fireEvent.click(screen.getByRole("button", { name: "Test snapshot" }));

    expect(await screen.findByText("The snapshot URL is not valid.")).toBeInTheDocument();
    // Shown as the field's own error, not the generic test-failure area.
    expect(screen.getByLabelText("Snapshot URL")).toHaveAttribute("aria-invalid", "true");
  });

  it("Test snapshot shows the returned image, and never calls a save command", async () => {
    testCamera.mockResolvedValueOnce({
      header: { contentType: "image/png", capturedAt: "2026-09-27T12:00:00Z", byteLen: 1, snapshotId: null },
      image: new Uint8Array([1]),
    });
    render(() => <DraftHarness initial={{ ...EMPTY_CAMERA_DRAFT, kind: "snapshotUrl", snapshotUrl: "http://192.0.2.10/snap" }} />);

    fireEvent.click(screen.getByRole("button", { name: "Test snapshot" }));

    expect(await screen.findByAltText("Camera test snapshot")).toBeInTheDocument();
    expect(tauriMock.invoke).not.toHaveBeenCalled();
  });

  it("clears the webcam port to an empty string, never the literal \"NaN\", when the field is emptied", () => {
    const onChange = vi.fn();
    render(() => (
      <CameraSourceSection
        mode="draft"
        value={{ ...EMPTY_CAMERA_DRAFT, kind: "hostWebcam", webcamName: "front", webPort: "8080" }}
        onChange={onChange}
      />
    ));

    fireEvent.input(screen.getByLabelText("Port (optional)"), { target: { value: "" } });

    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ webPort: "" }));
    expect(onChange.mock.calls.some(([draft]) => draft.webPort === "NaN")).toBe(false);
  });

  it("Test snapshot shows a typed non-validation error in the general test area", async () => {
    testCamera.mockRejectedValueOnce({
      contractVersion: 1,
      code: "CAMERA_FAILED",
      message: "The camera did not answer in time.",
      recovery: ["RETRY"],
      retryable: true,
      details: { kind: "timeout" },
    });
    render(() => <DraftHarness initial={{ ...EMPTY_CAMERA_DRAFT, kind: "snapshotUrl", snapshotUrl: "http://192.0.2.10/snap" }} />);

    fireEvent.click(screen.getByRole("button", { name: "Test snapshot" }));

    expect(await screen.findByText("The camera did not answer in time.")).toBeInTheDocument();
  });
});

describe("CameraSourceSection — printer mode", () => {
  it("loads the saved source via get_printer_camera and saves through set_printer_camera", async () => {
    tauriMock.isTauri.mockReturnValue(true);
    tauriMock.invoke.mockImplementation((command: string) => {
      if (command === "get_printer_camera") {
        return Promise.resolve({
          contractVersion: 1,
          data: {
            printerId: "prn-1",
            revision: 1,
            source: { kind: "snapshotUrl", snapshotUrl: "http://192.0.2.10/snap" },
            updatedAt: "2026-09-27T00:00:00Z",
          },
        });
      }
      if (command === "set_printer_camera") {
        return Promise.resolve({
          contractVersion: 1,
          data: {
            printerId: "prn-1",
            revision: 2,
            sourceKind: "snapshotUrl",
            webcamName: null,
            webcamService: null,
            webPort: null,
            hasSnapshotUrl: true,
            updatedAt: "2026-09-27T00:01:00Z",
          },
        });
      }
      return Promise.reject(new Error(`unexpected command ${command}`));
    });

    render(() => <CameraSourceSection mode="printer" printerId="prn-1" hasConnection />);

    // Seeded from get_printer_camera: the URL shows in the field, which is
    // the only command that ever returns one (global constraint 3).
    expect(await screen.findByDisplayValue("http://192.0.2.10/snap")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(tauriMock.invoke).toHaveBeenCalledWith("set_printer_camera", expect.objectContaining({
      printerId: "prn-1",
      source: { kind: "snapshotUrl", snapshotUrl: "http://192.0.2.10/snap" },
    })));
    // The result (PrinterCameraSummary) carries no URL; the form keeps the
    // one it just submitted rather than being cleared by the response.
    expect(screen.getByDisplayValue("http://192.0.2.10/snap")).toBeInTheDocument();
  });

  it("keeps Test snapshot disabled for a saved host webcam pick without a Connection", async () => {
    tauriMock.isTauri.mockReturnValue(true);
    tauriMock.invoke.mockImplementation((command: string) => {
      if (command === "get_printer_camera") {
        return Promise.resolve({
          contractVersion: 1,
          data: {
            printerId: "prn-1",
            revision: 1,
            source: { kind: "hostWebcam", webcamName: "front", webcamService: "webrtc", webPort: null },
            updatedAt: "2026-09-27T00:00:00Z",
          },
        });
      }
      return Promise.reject(new Error(`unexpected command ${command}`));
    });

    render(() => <CameraSourceSection mode="printer" printerId="prn-1" hasConnection={false} />);

    await waitFor(() => expect(screen.getByRole("radio", { name: "Host webcam" })).toBeChecked());
    expect(screen.getByText("Connect a Connection first to list this printer's webcams.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Test snapshot" })).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "Test snapshot" }));
    expect(testCamera).not.toHaveBeenCalled();
  });

  it("clears the source through clear_printer_camera when None is saved", async () => {
    tauriMock.isTauri.mockReturnValue(true);
    tauriMock.invoke.mockImplementation((command: string) => {
      if (command === "get_printer_camera") {
        return Promise.resolve({ contractVersion: 1, data: null });
      }
      if (command === "clear_printer_camera") {
        return Promise.resolve({ contractVersion: 1, data: { printerId: "prn-1", cleared: true } });
      }
      return Promise.reject(new Error(`unexpected command ${command}`));
    });

    render(() => <CameraSourceSection mode="printer" printerId="prn-1" hasConnection={false} />);
    await waitFor(() => expect(screen.getByRole("radio", { name: "None" })).toBeChecked());

    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(tauriMock.invoke).toHaveBeenCalledWith("clear_printer_camera", expect.objectContaining({ printerId: "prn-1" })));
  });
});

// Sanity: draftToCameraSource is exercised indirectly above (Test snapshot
// only enables once it returns non-undefined); a direct unit test pins its
// exact shape.
describe("draftToCameraSource", () => {
  it("is undefined for None and an incomplete host webcam pick", () => {
    expect(draftToCameraSource({ ...EMPTY_CAMERA_DRAFT })).toBeUndefined();
    expect(draftToCameraSource({ ...EMPTY_CAMERA_DRAFT, kind: "hostWebcam" })).toBeUndefined();
  });

  it("builds a hostWebcam source with a parsed port", () => {
    expect(
      draftToCameraSource({ ...EMPTY_CAMERA_DRAFT, kind: "hostWebcam", webcamName: "front", webcamService: "webrtc", webPort: "8080" }),
    ).toEqual({ kind: "hostWebcam", webcamName: "front", webcamService: "webrtc", webPort: 8080 });
  });

  it("builds a snapshotUrl source, trimmed", () => {
    expect(draftToCameraSource({ ...EMPTY_CAMERA_DRAFT, kind: "snapshotUrl", snapshotUrl: "  http://192.0.2.10/snap  " })).toEqual({
      kind: "snapshotUrl",
      snapshotUrl: "http://192.0.2.10/snap",
    });
  });
});
