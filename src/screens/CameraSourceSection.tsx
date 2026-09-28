import { createEffect, createSignal, on, onCleanup, Show } from "solid-js";
import { Button, NumberField, RadioGroup, Select, TextField } from "../design-system";
import { command, desktopAvailable, isCommandError, retryOnTransportFailure } from "../ipc/client";
import { listHostWebcams, testCamera, type TestCameraArgs } from "../cameras/camera-store";
import { frameObjectUrl } from "../cameras/frame";
import type { CameraSource, CameraSourceInput, HostWebcam } from "../attention/types";
import type { ConnectionSubmission } from "../printers/types";
import styles from "./CameraSourceSection.module.css";

export type CameraSourceKindOption = "none" | "hostWebcam" | "snapshotUrl";

/** The section's own editable shape -- distinct from the wire
 *  `CameraSourceInput` union so every field (the port text, an
 *  incomplete webcam pick) can stay in the form without being valid yet.
 *  `draftToCameraSource` is the only place that turns it back into one. */
export interface CameraSourceDraft {
  kind: CameraSourceKindOption;
  webcamName: string;
  webcamService: string | null;
  webPort: string;
  snapshotUrl: string;
}

export const EMPTY_CAMERA_DRAFT: CameraSourceDraft = {
  kind: "none",
  webcamName: "",
  webcamService: null,
  webPort: "",
  snapshotUrl: "",
};

function parsePort(text: string): number | null {
  const trimmed = text.trim();
  if (trimmed === "") return null;
  const n = Number(trimmed);
  return Number.isInteger(n) && n > 0 ? n : null;
}

/** `undefined` for "none" (D4: camera is optional -- nothing is sent when
 *  no source is chosen) or an incomplete `hostWebcam` pick (no name yet). */
export function draftToCameraSource(draft: CameraSourceDraft): CameraSourceInput | undefined {
  if (draft.kind === "none") return undefined;
  if (draft.kind === "hostWebcam") {
    if (draft.webcamName.trim() === "") return undefined;
    return {
      kind: "hostWebcam",
      webcamName: draft.webcamName.trim(),
      webcamService: draft.webcamService,
      webPort: parsePort(draft.webPort),
    };
  }
  const url = draft.snapshotUrl.trim();
  return url === "" ? undefined : { kind: "snapshotUrl", snapshotUrl: url };
}

function sourceToDraft(source: CameraSource | null): CameraSourceDraft {
  if (!source) return { ...EMPTY_CAMERA_DRAFT };
  if (source.kind === "hostWebcam") {
    return {
      kind: "hostWebcam",
      webcamName: source.webcamName,
      webcamService: source.webcamService,
      webPort: source.webPort !== null ? String(source.webPort) : "",
      snapshotUrl: "",
    };
  }
  return { kind: "snapshotUrl", webcamName: "", webcamService: null, webPort: "", snapshotUrl: source.snapshotUrl };
}

/** Review's one-line summary (spec "Setup": "Review summarizes both"). */
export function cameraDraftSummary(draft: CameraSourceDraft): string {
  if (draft.kind === "none") return "No camera";
  if (draft.kind === "hostWebcam") {
    return draft.webcamName.trim() ? `Host webcam: ${draft.webcamName.trim()}` : "Host webcam (not chosen yet)";
  }
  return draft.snapshotUrl.trim() ? "Manual snapshot URL" : "Manual snapshot URL (not entered yet)";
}

const KIND_OPTIONS = [
  { value: "none", label: "None" },
  { value: "hostWebcam", label: "Host webcam" },
  { value: "snapshotUrl", label: "Manual snapshot URL" },
];

export type CameraSourceSectionProps =
  | {
      mode: "draft";
      /** The wizard's own local draft (submitted with `createPrinter` at
       *  Save) -- Equip never blocks Next on this (D4 "Optionality"). */
      value: CameraSourceDraft;
      onChange: (value: CameraSourceDraft) => void;
      /** The Connect step's (possibly untested) submission, for
       *  `list_host_webcams`/`test_camera`. `undefined` means no
       *  Connection yet. */
      connection?: ConnectionSubmission;
    }
  | {
      mode: "printer";
      /** An existing Printer (the dock's Setup tab): loads its saved
       *  source via `get_printer_camera`, and Save/clearing go through
       *  `set_printer_camera`/`clear_printer_camera` directly. */
      printerId: string;
      hasConnection: boolean;
    };

/** The Printer wizard's Equip step and the dock's Setup tab share this one
 *  editor (spec "Frontend architecture" → "Setup"): "None" (default, never
 *  blocks Next), a host webcam picked from `list_host_webcams` once
 *  there's a Connection, or a manual snapshot URL validated inline by the
 *  Rust field path. "Test snapshot" never saves anything (D4 "Capture
 *  triggers": Setup test is never stored). */
export function CameraSourceSection(props: CameraSourceSectionProps) {
  const [printerValue, setPrinterValue] = createSignal<CameraSourceDraft>({ ...EMPTY_CAMERA_DRAFT });
  const [loading, setLoading] = createSignal(false);
  const [loadError, setLoadError] = createSignal<unknown>(null);
  const [saving, setSaving] = createSignal(false);
  const [saveError, setSaveError] = createSignal<unknown>(null);

  const value = (): CameraSourceDraft => (props.mode === "draft" ? props.value : printerValue());
  function setValue(next: CameraSourceDraft): void {
    if (props.mode === "draft") props.onChange(next);
    else setPrinterValue(next);
  }

  async function load(printerId: string): Promise<void> {
    setLoading(true);
    setLoadError(null);
    try {
      if (!desktopAvailable()) {
        setPrinterValue({ ...EMPTY_CAMERA_DRAFT });
        return;
      }
      const result = await command("get_printer_camera", { printerId });
      setPrinterValue(sourceToDraft(result?.source ?? null));
    } catch (e) {
      setLoadError(e);
    } finally {
      setLoading(false);
    }
  }

  createEffect(
    on(
      () => (props.mode === "printer" ? props.printerId : null),
      (printerId) => {
        if (printerId) void load(printerId);
      },
    ),
  );

  async function onSave(): Promise<void> {
    if (props.mode !== "printer") return;
    const printerId = props.printerId;
    const source = draftToCameraSource(value());
    setSaving(true);
    setSaveError(null);
    try {
      if (!source) {
        await retryOnTransportFailure(() =>
          command("clear_printer_camera", { operationId: crypto.randomUUID(), printerId }),
        );
      } else {
        await retryOnTransportFailure(() =>
          command("set_printer_camera", { operationId: crypto.randomUUID(), printerId, source }),
        );
      }
      // The result never carries a manual URL (`PrinterCameraSummary`), so
      // the form keeps the value it just submitted rather than being
      // overwritten from it (spec "Frontend architecture" → camera-store.ts).
    } catch (e) {
      setSaveError(e);
    } finally {
      setSaving(false);
    }
  }

  // --- Host webcam listing -----------------------------------------------

  const [webcams, setWebcams] = createSignal<HostWebcam[]>([]);
  const [webcamsLoading, setWebcamsLoading] = createSignal(false);
  const [webcamsError, setWebcamsError] = createSignal<unknown>(null);

  const hasConnection = (): boolean =>
    props.mode === "printer" ? props.hasConnection : props.connection !== undefined;

  /** A stable key for "did the Connection actually change", so editing an
   *  unrelated field elsewhere doesn't re-list on every render (the
   *  wizard builds a fresh `connection` object each render). */
  const connectionSignature = (): string | null => {
    if (props.mode === "printer") return props.hasConnection ? `printer:${props.printerId}` : null;
    const c = props.connection;
    return c ? `${c.kind}|${c.host}|${c.port}` : null;
  };

  async function loadWebcams(): Promise<void> {
    setWebcamsLoading(true);
    setWebcamsError(null);
    try {
      const args =
        props.mode === "printer" ? { printerId: props.printerId } : { connection: props.connection! };
      setWebcams(await listHostWebcams(args));
    } catch (e) {
      setWebcams([]);
      setWebcamsError(e);
    } finally {
      setWebcamsLoading(false);
    }
  }

  createEffect(
    on(
      () => [value().kind, connectionSignature()] as const,
      ([kind, signature]) => {
        if (kind !== "hostWebcam" || !signature) {
          setWebcams([]);
          setWebcamsError(null);
          return;
        }
        void loadWebcams();
      },
    ),
  );

  // --- Test snapshot -------------------------------------------------------

  const [testPending, setTestPending] = createSignal(false);
  const [testUrl, setTestUrl] = createSignal<string | null>(null);
  const [testError, setTestError] = createSignal<unknown>(null);
  const [urlFieldError, setUrlFieldError] = createSignal<string | null>(null);

  function revokeTestUrl(): void {
    const current = testUrl();
    if (current) URL.revokeObjectURL(current);
    setTestUrl(null);
  }

  function testArgs(): TestCameraArgs | null {
    const source = draftToCameraSource(value());
    if (!source) return null;
    if (source.kind === "hostWebcam") {
      if (!hasConnection()) return null;
      if (props.mode === "printer") return { printerId: props.printerId, source };
      return props.connection ? { connection: props.connection, source } : null;
    }
    return props.mode === "printer" ? { printerId: props.printerId, source } : { source };
  }

  const canTest = (): boolean => testArgs() !== null;

  async function onTestSnapshot(): Promise<void> {
    const args = testArgs();
    if (!args || testPending()) return;
    setTestPending(true);
    setTestError(null);
    setUrlFieldError(null);
    revokeTestUrl();
    try {
      const frame = await testCamera(args);
      setTestUrl(frameObjectUrl(frame));
    } catch (e) {
      const fieldPath = isCommandError(e) && e.code === "VALIDATION" ? e.details?.fieldPath : undefined;
      if (isCommandError(e) && typeof fieldPath === "string" && fieldPath.endsWith(".snapshotUrl")) {
        setUrlFieldError(e.message);
      } else {
        setTestError(e);
      }
    } finally {
      setTestPending(false);
    }
  }

  // A kind switch invalidates whatever the last test showed.
  createEffect(
    on(
      () => value().kind,
      () => {
        setTestError(null);
        setUrlFieldError(null);
        revokeTestUrl();
      },
      { defer: true },
    ),
  );
  onCleanup(revokeTestUrl);

  return (
    <div class={styles.section}>
      <h3 class={styles.title}>Camera</h3>
      <Show when={props.mode === "printer" && loading()}>
        <p class={styles.note}>Loading…</p>
      </Show>
      <Show when={props.mode === "printer" && loadError()}>
        {(held) => {
          const e = held();
          return (
            <p class={styles.error} role="alert">
              {isCommandError(e) ? e.message : "The camera source couldn't be loaded."}
            </p>
          );
        }}
      </Show>

      <RadioGroup
        label="Source"
        options={KIND_OPTIONS}
        value={value().kind}
        onChange={(kind) => setValue({ ...value(), kind: kind as CameraSourceKindOption })}
      />

      <Show when={value().kind === "hostWebcam"}>
        <Show
          when={hasConnection()}
          fallback={<p class={styles.note}>Connect a Connection first to list this printer's webcams.</p>}
        >
          <Show when={!webcamsLoading()} fallback={<p class={styles.note}>Listing webcams…</p>}>
            <Show
              when={!webcamsError()}
              fallback={
                <p class={styles.error} role="alert">
                  {(() => {
                    const e = webcamsError();
                    return isCommandError(e) ? e.message : "The webcam list couldn't be loaded.";
                  })()}
                </p>
              }
            >
              <Select
                label="Webcam"
                options={webcams()}
                optionValue={(w: HostWebcam) => w.name}
                optionLabel={(w: HostWebcam) => w.name}
                value={webcams().find((w) => w.name === value().webcamName) ?? null}
                placeholder="Choose a webcam"
                onChange={(w: HostWebcam) => setValue({ ...value(), webcamName: w.name, webcamService: w.service })}
              />
            </Show>
          </Show>
        </Show>
        <NumberField
          label="Port (optional)"
          value={value().webPort === "" ? undefined : (parsePort(value().webPort) ?? undefined)}
          onChange={(n) => setValue({ ...value(), webPort: Number.isNaN(n) ? "" : String(n) })}
          minValue={1}
          maxValue={65535}
          placeholder="80"
        />
      </Show>

      <Show when={value().kind === "snapshotUrl"}>
        <TextField
          label="Snapshot URL"
          value={value().snapshotUrl}
          onChange={(url) => setValue({ ...value(), snapshotUrl: url })}
          placeholder="http://192.0.2.20/webcam/?action=snapshot"
          error={urlFieldError() ?? undefined}
        />
      </Show>

      <Show when={value().kind !== "none"}>
        <div class={styles.testRow}>
          <Button variant="secondary" size="sm" disabled={!canTest() || testPending()} onClick={() => void onTestSnapshot()}>
            {testPending() ? "Testing…" : "Test snapshot"}
          </Button>
          <Show when={testUrl()}>
            {(src) => <img class={styles.testImage} src={src()} alt="Camera test snapshot" />}
          </Show>
          <Show when={testError()}>
            {(held) => {
              const e = held();
              return (
                <p class={styles.error} role="alert">
                  {isCommandError(e) ? e.message : "The camera test failed."}
                </p>
              );
            }}
          </Show>
        </div>
      </Show>

      <Show when={props.mode === "printer"}>
        <div class={styles.actions}>
          <Button variant="primary" disabled={saving()} onClick={() => void onSave()}>
            {saving() ? "Saving…" : "Save"}
          </Button>
          <Show when={saveError()}>
            {(held) => {
              const e = held();
              return (
                <p class={styles.error} role="alert">
                  {isCommandError(e) ? e.message : "The camera source couldn't be saved."}
                </p>
              );
            }}
          </Show>
        </div>
      </Show>
    </div>
  );
}
