import { createSignal, For, Match, onCleanup, onMount, Show, Switch } from "solid-js";
import { Button, Dialog, SeverityMarker, type SeverityMarkerProps } from "../design-system";
import { desktopAvailable, isCommandError } from "../ipc/client";
import {
  checkSlicerRuntime,
  pickPresetSource,
  pickSlicerEngine,
  resetSlicerRuntime,
  slicing,
} from "../slicing/slicing-store";
import { CANDIDATE_SOURCES, NOTHING_TRIED, PRESETS_UNREADABLE } from "../slicing/slice-presentation";
import type {
  EngineCandidate,
  EngineState,
  PresetSourceState,
  RuntimeChannel,
  SlicerRuntimeStatus,
} from "../slicing/types";
import styles from "./SlicerSettingsDialog.module.css";

export interface SlicerSettingsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** What the dialog can ask the backend to do, one at a time. */
type Action = "check" | "engine" | "presetFile" | "presetFolder" | "resetEngine" | "resetPresets";

const RUN: Record<Action, () => Promise<unknown>> = {
  check: checkSlicerRuntime,
  // A cancelled picker answers `null`: nothing changed, nothing to say.
  engine: pickSlicerEngine,
  presetFile: () => pickPresetSource("file"),
  presetFolder: () => pickPresetSource("folder"),
  resetEngine: () => resetSlicerRuntime({ engine: true, presetSource: false }),
  resetPresets: () => resetSlicerRuntime({ engine: false, presetSource: true }),
};

/** Said while an action runs. A probe can take up to 10 s (D2). */
const BUSY_TEXT: Record<Action, string> = {
  check: "Checking for OrcaSlicer… This can take up to 10 seconds.",
  engine: "Waiting for an engine to be chosen, then checking it…",
  presetFile: "Waiting for a preset source to be chosen, then reading its presets…",
  presetFolder: "Waiting for a preset source to be chosen, then reading its presets…",
  resetEngine: "Switching to automatic discovery…",
  resetPresets: "Switching to the engine's presets…",
};

interface Notice {
  /** `error` is announced as an alert; `info` politely. */
  tone: "error" | "info";
  message: string;
  /** What to do about it, when farm3d knows. */
  recovery?: string;
  /** Offer **Try again** for this action. */
  retry?: Action;
}

/** How a refusal from one of the runtime commands is shown. */
function noticeFor(error: unknown, action: Action): Notice {
  if (!isCommandError(error)) {
    return { tone: "error", message: "farm3d didn't get an answer about the slicer.", retry: action };
  }
  if (error.code === "CONFLICT") {
    return {
      tone: "info",
      message: "The slicer settings changed while this was open, so farm3d reloaded them. Check them, then try again.",
    };
  }
  // Web mode has no pickers or runtime to change: say so once, quietly.
  if (!desktopAvailable() && error.code === "PERSISTENCE_UNAVAILABLE") {
    return { tone: "info", message: error.message };
  }
  const retry = error.retryable || error.recovery.includes("RETRY") ? action : undefined;
  switch (error.code) {
    case "SLICER_UNAVAILABLE":
      return {
        tone: "error",
        message: error.message,
        recovery: "Nothing was saved. Choose an OrcaSlicer 2.x release or nightly, as an executable or AppImage.",
        retry,
      };
    case "PRESET_SOURCE_UNAVAILABLE":
      return {
        tone: "error",
        message: error.message,
        recovery: "Nothing was saved. Choose an OrcaSlicer 2.4 install or AppImage, or a folder that holds its resources.",
        retry,
      };
    default:
      return { tone: "error", message: error.message, retry };
  }
}

const ENGINE_SOURCE_TEXT: Record<Extract<EngineState, { state: "available" }>["source"], string> = {
  configured: "Chosen in Settings",
  path: "Found on PATH",
  wellKnown: "Found in a usual download folder",
};

function engineMarker(engine: EngineState): SeverityMarkerProps {
  switch (engine.state) {
    case "available":
      return { severity: "resolved", label: "Ready" };
    case "notFound":
      return { severity: "fatal", label: "Not found" };
    case "unsupportedVersion":
      return { severity: "warning", label: "Unsupported version" };
    case "probeFailed":
      return { severity: "fatal", label: "Couldn't be run" };
  }
}

function presetMarker(presets: PresetSourceState): SeverityMarkerProps {
  switch (presets.state) {
    case "available":
      return { severity: "resolved", label: "Ready" };
    case "notConfigured":
      return { severity: "fatal", label: "Not set up" };
    case "presetsUnreadable":
      return { severity: "fatal", label: "Unreadable" };
    case "unavailable":
      return { severity: "fatal", label: "Unavailable" };
  }
}

function candidateResult(candidate: EngineCandidate): string {
  const result = candidate.result;
  switch (result.kind) {
    case "chosen":
      return `Used: OrcaSlicer ${result.version}.`;
    case "notChosen":
      return `OrcaSlicer ${result.version}, not used: another was chosen first.`;
    case "unsupportedVersion":
      return `OrcaSlicer ${result.version} isn't supported.`;
    case "probeFailed":
      return `Couldn't be run: ${result.reason}`;
  }
}

/** The engine chosen in Settings, if one is, whether or not it worked. */
function configuredEngine(runtime: SlicerRuntimeStatus): EngineCandidate | undefined {
  return runtime.engineCandidates.find((candidate) => candidate.source === "configured");
}

/** A chosen engine failed, and discovery fell through to another (D2). */
function fellThrough(runtime: SlicerRuntimeStatus): EngineCandidate | undefined {
  const configured = configuredEngine(runtime);
  return configured && configured.result.kind !== "chosen" ? configured : undefined;
}

/** Whether a preset source chosen in Settings may be in use, so **Use the
 *  engine's presets** has something to undo. An unavailable source may be
 *  the chosen one or the engine's; resetting is harmless either way. */
function presetSourceConfigured(presets: PresetSourceState): boolean {
  return (presets.state === "available" && presets.origin === "configured") || presets.state === "unavailable";
}

function Channel(props: { channel: RuntimeChannel }) {
  return (
    <Show when={props.channel === "prerelease"}>
      <span class={styles.badge}>prerelease</span>
    </Show>
  );
}

function Path(props: { path: string }) {
  return <code class={styles.path}>{props.path}</code>;
}

/** D22: the Slicer section of Settings. It shows the OrcaSlicer engine and
 *  preset source that slicing uses (D2), how discovery chose them, and the
 *  actions that change them. Full paths are shown here and nowhere else.
 *  The status comes live from the slicing store, which applies
 *  `slicing.runtime.changed`; nothing here polls. */
export function SlicerSettingsDialog(props: SlicerSettingsDialogProps) {
  const [busy, setBusy] = createSignal<Action | null>(null);
  const [notice, setNotice] = createSignal<Notice | null>(null);
  const [confirming, setConfirming] = createSignal<"resetEngine" | "resetPresets" | null>(null);
  let disposed = false;
  onCleanup(() => {
    disposed = true;
  });
  // Where focus goes back to when a confirmation closes: its section's
  // first action, which is always there (the reset button may not be).
  let engineButton: HTMLButtonElement | undefined;
  let presetButton: HTMLButtonElement | undefined;
  const endConfirmation = (section: "resetEngine" | "resetPresets") => {
    setConfirming(null);
    (section === "resetEngine" ? engineButton : presetButton)?.focus();
  };

  const run = async (action: Action) => {
    if (busy()) return;
    setBusy(action);
    setNotice(null);
    setConfirming(null);
    // A confirmation's buttons go away now; keep focus in its section.
    if (action === "resetEngine") engineButton?.focus();
    if (action === "resetPresets") presetButton?.focus();
    try {
      await RUN[action]();
    } catch (error) {
      if (!disposed) setNotice(noticeFor(error, action));
    } finally {
      if (!disposed) setBusy(null);
    }
  };

  /** While an action runs the others wait. They stay focusable, so focus
   *  isn't lost, and say why through the status line. */
  const waiting = () => (busy() ? { "aria-disabled": true as const, "data-disabled": "" } : {});

  return (
    <Dialog
      title="Slicer"
      description="The OrcaSlicer that farm3d slices with, and where its presets come from."
      open={props.open}
      onOpenChange={props.onOpenChange}
    >
      <div class={styles.body}>
        <Show
          when={slicing.runtime()}
          fallback={<p class={styles.text}>farm3d hasn't checked for OrcaSlicer yet.</p>}
        >
          {(runtime) => (
            <>
              <section class={styles.section} aria-labelledby="slicer-engine-heading">
                <div class={styles.sectionHeader}>
                  <h3 id="slicer-engine-heading" class={styles.heading}>Engine</h3>
                  <SeverityMarker {...engineMarker(runtime().engine)} />
                </div>
                <EngineDetails runtime={runtime()} />
                <div class={styles.actions}>
                  <Button ref={engineButton} {...waiting()} onClick={() => void run("engine")}>
                    Choose engine…
                  </Button>
                  <Show when={configuredEngine(runtime())}>
                    <Button variant="ghost" {...waiting()} onClick={() => !busy() && setConfirming("resetEngine")}>
                      Use automatic discovery…
                    </Button>
                  </Show>
                </div>
                <Show when={confirming() === "resetEngine" && configuredEngine(runtime())}>
                  {(configured) => (
                    <Confirmation
                      text={`farm3d will forget ${configured().path} and look for OrcaSlicer on PATH and in the usual download folders.`}
                      confirmLabel="Use automatic discovery"
                      cancelLabel="Keep this engine"
                      onConfirm={() => void run("resetEngine")}
                      onCancel={() => endConfirmation("resetEngine")}
                    />
                  )}
                </Show>
              </section>

              <section class={styles.section} aria-labelledby="slicer-presets-heading">
                <div class={styles.sectionHeader}>
                  <h3 id="slicer-presets-heading" class={styles.heading}>Preset source</h3>
                  <SeverityMarker {...presetMarker(runtime().presetSource)} />
                </div>
                <PresetDetails presets={runtime().presetSource} />
                <div class={styles.actions}>
                  <Button ref={presetButton} {...waiting()} onClick={() => void run("presetFile")}>
                    Choose preset source file…
                  </Button>
                  <Button {...waiting()} onClick={() => void run("presetFolder")}>
                    Choose preset source folder…
                  </Button>
                  <Show when={presetSourceConfigured(runtime().presetSource)}>
                    <Button variant="ghost" {...waiting()} onClick={() => !busy() && setConfirming("resetPresets")}>
                      Use the engine's presets…
                    </Button>
                  </Show>
                </div>
                <p class={styles.note}>
                  A file is an OrcaSlicer executable or AppImage; a folder is an OrcaSlicer install or its
                  resources folder.
                </p>
                <Show when={confirming() === "resetPresets"}>
                  <Confirmation
                    text="farm3d will forget the chosen preset source and read the presets from the engine."
                    confirmLabel="Use the engine's presets"
                    cancelLabel="Keep this source"
                    onConfirm={() => void run("resetPresets")}
                    onCancel={() => endConfirmation("resetPresets")}
                  />
                </Show>
              </section>

              <Show when={runtime().versionsDiffer && versionPair(runtime())}>
                {(pair) => (
                  <p class={styles.notice}>
                    The presets come from OrcaSlicer {pair().presets}, but the engine is OrcaSlicer {pair().engine}.
                    Slicing works, but a preset can behave differently between versions.
                  </p>
                )}
              </Show>

              <p class={styles.summary}>
                {runtime().canSlice ? "Ready to slice." : "Slicing is unavailable until the engine and the presets are both ready."}
              </p>
            </>
          )}
        </Show>

        <Show when={notice()}>
          {(current) => (
            <div class={current().tone === "error" ? styles.error : styles.info} role={current().tone === "error" ? "alert" : "status"}>
              <p>{current().message}</p>
              <Show when={current().recovery}>
                <p>{current().recovery}</p>
              </Show>
              <Show when={current().retry}>
                {(action) => (
                  <Button size="sm" {...waiting()} onClick={() => void run(action())}>
                    Try again
                  </Button>
                )}
              </Show>
            </div>
          )}
        </Show>

        <p class={styles.busy} role="status">
          {busy() ? BUSY_TEXT[busy()!] : ""}
        </p>

        <div class={styles.footer}>
          <Button
            {...waiting()}
            aria-busy={busy() === "check" ? true : undefined}
            onClick={() => void run("check")}
          >
            {busy() === "check" ? "Checking…" : "Check again"}
          </Button>
          <Button variant="ghost" onClick={() => props.onOpenChange(false)}>
            Close
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

function versionPair(runtime: SlicerRuntimeStatus): { engine: string; presets: string } | undefined {
  const { engine, presetSource } = runtime;
  if (engine.state !== "available" || presetSource.state !== "available") return undefined;
  return { engine: engine.version, presets: presetSource.version };
}

function EngineDetails(props: { runtime: SlicerRuntimeStatus }) {
  const engine = () => props.runtime.engine;
  return (
    <>
      <Switch>
        <Match when={engine().state === "available" && engine() as Extract<EngineState, { state: "available" }>}>
          {(available) => (
            <>
              <dl class={styles.facts}>
                <dt>Version</dt>
                <dd>
                  OrcaSlicer {available().version} <Channel channel={available().channel} />
                </dd>
                <dt>Executable</dt>
                <dd>
                  {available().executableName}
                  <Path path={available().path} />
                </dd>
                <dt>Source</dt>
                <dd>{ENGINE_SOURCE_TEXT[available().source]}</dd>
              </dl>
              <Show when={available().channel === "prerelease"}>
                <p class={styles.note}>
                  This is a nightly or development build. farm3d supports it, but it can change without notice; a
                  release is the safer choice.
                </p>
              </Show>
              <Show when={available().extractAndRun}>
                <p class={styles.notice}>
                  FUSE isn't available, so this AppImage runs through --appimage-extract-and-run. Each run leaves an
                  extracted copy in /tmp (appimage_extracted_*), which can be deleted when farm3d isn't slicing.
                </p>
              </Show>
            </>
          )}
        </Match>
        <Match when={engine().state === "unsupportedVersion" && engine() as Extract<EngineState, { state: "unsupportedVersion" }>}>
          {(unsupported) => (
            <p class={styles.text}>
              {unsupported().executableName} is OrcaSlicer {unsupported().version}. farm3d works with OrcaSlicer 2.x,
              releases and nightlies. Choose a 2.x engine, or install one and check again.
            </p>
          )}
        </Match>
        <Match when={engine().state === "probeFailed" && engine() as Extract<EngineState, { state: "probeFailed" }>}>
          {(failed) => (
            <p class={styles.text}>
              {failed().executableName} couldn't be run: {failed().reason}
            </p>
          )}
        </Match>
        <Match when={engine().state === "notFound"}>
          <p class={styles.text}>OrcaSlicer wasn't found. Choose its executable or AppImage, or install it and check again.</p>
        </Match>
      </Switch>

      <Show when={fellThrough(props.runtime)}>
        {(configured) => (
          <p class={styles.notice}>
            The engine chosen in Settings, {configured().executableName}, couldn't be used, so farm3d looked for
            another. Discovery, below, says why.
          </p>
        )}
      </Show>

      <div class={styles.candidates}>
        <h4 class={styles.subheading}>Discovery</h4>
        <Show when={props.runtime.engineCandidates.length > 0} fallback={<p class={styles.note}>{NOTHING_TRIED}</p>}>
          <ol class={styles.candidateList}>
            <For each={props.runtime.engineCandidates}>
              {(candidate) => (
                <li data-chosen={candidate.result.kind === "chosen" ? "" : undefined}>
                  <span class={styles.candidateName}>{candidate.executableName}</span>{" "}
                  <span class={styles.candidateSource}>({CANDIDATE_SOURCES[candidate.source]})</span>
                  <span class={styles.candidateResult}>{candidateResult(candidate)}</span>
                  <Path path={candidate.path} />
                </li>
              )}
            </For>
          </ol>
        </Show>
      </div>
    </>
  );
}

function PresetDetails(props: { presets: PresetSourceState }) {
  return (
    <Switch>
      <Match when={props.presets.state === "available" && props.presets as Extract<PresetSourceState, { state: "available" }>}>
        {(available) => (
          <dl class={styles.facts}>
            <dt>Version</dt>
            <dd>
              OrcaSlicer {available().version} <Channel channel={available().channel} />
            </dd>
            <dt>Origin</dt>
            <dd>{available().origin === "engine" ? "The engine's presets" : "Chosen in Settings"}</dd>
            <dt>Vendors</dt>
            <dd>{available().vendorCount}</dd>
            <dt>Location</dt>
            <dd>
              <Path path={available().path} />
            </dd>
          </dl>
        )}
      </Match>
      <Match when={props.presets.state === "presetsUnreadable"}>
        <p class={styles.text}>{PRESETS_UNREADABLE}</p>
        <p class={styles.note}>OrcaSlicer 2.5.0-dev nightlies are like this: they slice, but their own presets can't be read.</p>
      </Match>
      <Match when={props.presets.state === "unavailable" && props.presets as Extract<PresetSourceState, { state: "unavailable" }>}>
        {(unavailable) => <p class={styles.text}>The presets couldn't be read: {unavailable().reason}</p>}
      </Match>
      <Match when={props.presets.state === "notConfigured"}>
        <p class={styles.text}>
          Without an engine there are no presets. Choose an engine, or choose an OrcaSlicer install as the preset source.
        </p>
      </Match>
    </Switch>
  );
}

function Confirmation(props: {
  text: string;
  confirmLabel: string;
  cancelLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  let confirmButton: HTMLButtonElement | undefined;
  // The question takes focus, so a keyboard user answers it next.
  onMount(() => confirmButton?.focus());
  return (
    <div class={styles.confirm} role="group" aria-label="Confirm">
      <p class={styles.text}>{props.text}</p>
      <div class={styles.actions}>
        <Button ref={confirmButton} onClick={props.onConfirm}>
          {props.confirmLabel}
        </Button>
        <Button variant="ghost" onClick={props.onCancel}>
          {props.cancelLabel}
        </Button>
      </div>
    </div>
  );
}
