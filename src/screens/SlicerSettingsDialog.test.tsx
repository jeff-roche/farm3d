import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CommandError } from "../generated/contracts/command/CommandError";
import {
  refuseDesktopOnlyActions,
  resetSlicingStoreMock,
  setSlicingState,
  slicingStoreMock,
} from "../slicing/slicing-store-mock";
import { closeSlicerSettings, openSlicerSettings, slicerSettingsOpen } from "../slicing/slicer-settings-opener";
import { NOTHING_TRIED, PRESETS_UNREADABLE } from "../slicing/slice-presentation";
import { runtimeStatus } from "../slicing/test-records";
import type { SlicerRuntimeStatus } from "../slicing/types";
import { SettingsMenu } from "./SettingsMenu";
import { SlicerSettingsHost } from "./SlicerSettingsHost";

vi.mock("../slicing/slicing-store", async () => (await import("../slicing/slicing-store-mock")).slicingStoreMock);
vi.mock("../settings/settings-store", () => ({ exportSettings: vi.fn(), importSettings: vi.fn() }));
const desktop = vi.hoisted(() => ({ available: true }));
vi.mock("../ipc/client", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../ipc/client")>()),
  desktopAvailable: () => desktop.available,
}));

function commandError(code: CommandError["code"], message: string, overrides: Partial<CommandError> = {}): CommandError {
  return { contractVersion: 1, code, message, recovery: [], retryable: false, ...overrides };
}

/** A configured AppImage that failed, then `orca-slicer` on PATH used. */
function fellThroughRuntime(): SlicerRuntimeStatus {
  return runtimeStatus({
    revision: 4,
    engineCandidates: [
      {
        source: "configured", executableName: "OrcaSlicer_Linux_V2.3.0.AppImage",
        path: "/home/pat/Applications/OrcaSlicer_Linux_V2.3.0.AppImage",
        result: { kind: "probeFailed", reason: "timed out after 10 s" },
      },
      {
        source: "path", executableName: "orca-slicer", path: "/usr/bin/orca-slicer",
        result: { kind: "chosen", version: "2.4.2" },
      },
    ],
  });
}

function renderHost() {
  return render(() => (
    <>
      <SettingsMenu />
      <SlicerSettingsHost />
    </>
  ));
}

async function openDialog(): Promise<HTMLElement> {
  openSlicerSettings();
  return screen.findByRole("dialog", { name: "Slicer" });
}

function section(dialog: HTMLElement, name: "Engine" | "Preset source"): HTMLElement {
  return within(dialog).getByRole("region", { name });
}

function button(name: string | RegExp): HTMLElement {
  return screen.getByRole("button", { name });
}

function tabbables(container: HTMLElement): HTMLElement[] {
  return [...container.querySelectorAll<HTMLElement>("button, input, [tabindex]")].filter((element) =>
    !(element as HTMLButtonElement).disabled
    && element.tabIndex >= 0
    // Kobalte's focus-trap sentinels only wrap focus around.
    && !element.hasAttribute("data-focus-trap")
    && !element.closest("[aria-hidden='true']"));
}

/** Tab: focus moves to the next tabbable element in the container. */
function tab(container: HTMLElement): HTMLElement {
  const order = tabbables(container);
  const next = order[order.indexOf(document.activeElement as HTMLElement) + 1] ?? order[0];
  next.focus();
  return next;
}

/** Enter on the focused button: jsdom doesn't synthesize the click a
 *  browser does. */
function pressEnter() {
  const focused = document.activeElement as HTMLElement;
  fireEvent.keyDown(focused, { key: "Enter" });
  fireEvent.click(focused);
}

beforeEach(() => {
  resetSlicingStoreMock();
  desktop.available = true;
  setSlicingState({ runtime: runtimeStatus() });
});

afterEach(() => {
  closeSlicerSettings();
  cleanup();
  document.body.innerHTML = "";
});

describe("opening the Slicer settings", () => {
  it("opens from the Settings menu's Slicer item", async () => {
    renderHost();
    await fireEvent.pointerDown(screen.getByLabelText("Settings"), { pointerType: "mouse", button: 0 });
    await fireEvent.pointerUp(await screen.findByText("Slicer..."), { button: 0 });
    expect(await screen.findByRole("dialog", { name: "Slicer" })).toBeInTheDocument();
    expect(slicerSettingsOpen()).toBe(true);
  });

  it("opens from openSlicerSettings (the Preparation panel's link), and Close closes it through the opener", async () => {
    renderHost();
    expect(screen.queryByRole("dialog")).toBeNull();
    const dialog = await openDialog();
    // The footer's Close, after the header's.
    fireEvent.click(within(dialog).getAllByRole("button", { name: "Close" })[1]);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(slicerSettingsOpen()).toBe(false);
  });
});

describe("status", () => {
  it("available: version, the full path, the source, and the preset source", async () => {
    renderHost();
    const dialog = await openDialog();
    const engine = section(dialog, "Engine");
    expect(within(engine).getByRole("status", { name: "Ready" })).toBeInTheDocument();
    expect(engine).toHaveTextContent("OrcaSlicer 2.4.2");
    expect(within(engine).getByText("/usr/bin/orca-slicer")).toBeInTheDocument();
    expect(engine).toHaveTextContent("Found on PATH");
    expect(within(engine).queryByText("prerelease")).toBeNull();
    // Automatic discovery is already in use: nothing to reset.
    expect(within(engine).queryByRole("button", { name: /automatic discovery/ })).toBeNull();

    const presets = section(dialog, "Preset source");
    expect(within(presets).getByRole("status", { name: "Ready" })).toBeInTheDocument();
    expect(presets).toHaveTextContent("The engine's presets");
    expect(presets).toHaveTextContent("62");
    expect(within(presets).queryByRole("button", { name: /engine's presets/ })).toBeNull();
    expect(dialog).toHaveTextContent("Ready to slice.");
    expect(within(engine).getByText(NOTHING_TRIED)).toBeInTheDocument();
  });

  it("unsupported version: the version and help to choose a 2.x engine", async () => {
    setSlicingState({
      runtime: runtimeStatus({
        engine: { state: "unsupportedVersion", version: "3.0.0", executableName: "orca-slicer" },
        presetSource: { state: "notConfigured" },
        canSlice: false,
        engineCandidates: [{
          source: "path", executableName: "orca-slicer", path: "/usr/bin/orca-slicer",
          result: { kind: "unsupportedVersion", version: "3.0.0" },
        }],
      }),
    });
    renderHost();
    const engine = section(await openDialog(), "Engine");
    expect(within(engine).getByRole("status", { name: "Unsupported version" })).toBeInTheDocument();
    expect(engine).toHaveTextContent(
      "orca-slicer is OrcaSlicer 3.0.0. farm3d works with OrcaSlicer 2.x, releases and nightlies. Choose a 2.x engine, or install one and check again.",
    );
    expect(engine).toHaveTextContent("OrcaSlicer 3.0.0 isn't supported.");
    expect(screen.getByRole("dialog")).toHaveTextContent("Slicing is unavailable until the engine and the presets are both ready.");
  });

  it("prerelease: labels a nightly or dev build", async () => {
    setSlicingState({
      runtime: runtimeStatus({
        engine: {
          state: "available", version: "2.5.0-dev", channel: "prerelease", source: "wellKnown",
          executableName: "OrcaSlicer_Linux_nightly.AppImage", path: "/home/pat/Downloads/OrcaSlicer_Linux_nightly.AppImage",
          extractAndRun: true,
        },
      }),
    });
    renderHost();
    const engine = section(await openDialog(), "Engine");
    expect(within(engine).getByText("prerelease")).toBeInTheDocument();
    expect(engine).toHaveTextContent("OrcaSlicer 2.5.0-dev");
    expect(engine).toHaveTextContent("This is a nightly or development build.");
    expect(engine).toHaveTextContent("Found in a usual download folder");
    // D22: the extract-and-run fallback's /tmp note.
    expect(engine).toHaveTextContent("extracted copy in /tmp (appimage_extracted_*)");
  });

  it("missing: not found, what discovery looked at, and no presets", async () => {
    setSlicingState({
      runtime: runtimeStatus({
        engine: { state: "notFound" }, presetSource: { state: "notConfigured" }, canSlice: false, engineCandidates: [],
      }),
    });
    renderHost();
    const dialog = await openDialog();
    const engine = section(dialog, "Engine");
    expect(within(engine).getByRole("status", { name: "Not found" })).toBeInTheDocument();
    expect(within(engine).getByText(NOTHING_TRIED)).toBeInTheDocument();
    expect(within(section(dialog, "Preset source")).getByRole("status", { name: "Not set up" })).toBeInTheDocument();
    expect(dialog).toHaveTextContent("Slicing is unavailable");
  });

  it("a chosen engine that failed: says so, lists each candidate with its full path, and offers automatic discovery", async () => {
    setSlicingState({ runtime: fellThroughRuntime() });
    renderHost();
    const engine = section(await openDialog(), "Engine");
    expect(engine).toHaveTextContent(
      "The engine chosen in Settings, OrcaSlicer_Linux_V2.3.0.AppImage, couldn't be used, so farm3d looked for another.",
    );
    const items = within(engine).getAllByRole("listitem");
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveTextContent("OrcaSlicer_Linux_V2.3.0.AppImage (chosen in Settings)Couldn't be run: timed out after 10 s");
    expect(items[0]).toHaveTextContent("/home/pat/Applications/OrcaSlicer_Linux_V2.3.0.AppImage");
    expect(items[1]).toHaveTextContent("orca-slicer (on PATH)Used: OrcaSlicer 2.4.2.");
    expect(within(engine).getByRole("button", { name: "Use automatic discovery…" })).toBeInTheDocument();
  });

  it("unreadable presets show D22's text; differing versions show a note", async () => {
    setSlicingState({ runtime: runtimeStatus({ presetSource: { state: "presetsUnreadable" }, canSlice: false }) });
    renderHost();
    const dialog = await openDialog();
    expect(within(section(dialog, "Preset source")).getByText(PRESETS_UNREADABLE)).toBeInTheDocument();

    setSlicingState({
      runtime: runtimeStatus({
        presetSource: {
          state: "available", version: "2.4.0", channel: "release", origin: "configured", vendorCount: 60,
          path: "/opt/orca-2.4.0",
        },
        versionsDiffer: true,
      }),
    });
    expect(dialog).toHaveTextContent("The presets come from OrcaSlicer 2.4.0, but the engine is OrcaSlicer 2.4.2.");
    expect(section(dialog, "Preset source")).toHaveTextContent("Chosen in Settings");
  });
});

describe("Choose engine", () => {
  it("success: the store's new status shows", async () => {
    const chosen = runtimeStatus({
      revision: 2,
      engine: {
        state: "available", version: "2.4.2", channel: "release", source: "configured",
        executableName: "OrcaSlicer.AppImage", path: "/home/pat/OrcaSlicer.AppImage", extractAndRun: false,
      },
    });
    slicingStoreMock.pickSlicerEngine.mockImplementation(async () => {
      setSlicingState({ runtime: chosen });
      return chosen;
    });
    renderHost();
    const dialog = await openDialog();
    fireEvent.click(button("Choose engine…"));
    expect(await within(dialog).findByText("/home/pat/OrcaSlicer.AppImage")).toBeInTheDocument();
    expect(section(dialog, "Engine")).toHaveTextContent("Chosen in Settings");
    expect(slicingStoreMock.pickSlicerEngine).toHaveBeenCalledTimes(1);
    expect(within(dialog).queryByRole("alert")).toBeNull();
  });

  it("cancel: nothing changes and nothing is said", async () => {
    slicingStoreMock.pickSlicerEngine.mockResolvedValue(null);
    renderHost();
    const dialog = await openDialog();
    fireEvent.click(button("Choose engine…"));
    await waitFor(() => expect(button("Choose engine…")).not.toHaveAttribute("aria-disabled"));
    expect(slicingStoreMock.pickSlicerEngine).toHaveBeenCalledTimes(1);
    expect(within(dialog).queryByRole("alert")).toBeNull();
    expect(within(section(dialog, "Engine")).getByText("/usr/bin/orca-slicer")).toBeInTheDocument();
  });

  it("error: the backend's message and what to do", async () => {
    slicingStoreMock.pickSlicerEngine.mockRejectedValue(commandError(
      "SLICER_UNAVAILABLE",
      "OrcaSlicer is not available: OrcaSlicer 3.0.0 is not supported. Choose an OrcaSlicer 2.x release or nightly.",
      { recovery: ["OPEN_SLICER_SETTINGS"] },
    ));
    renderHost();
    const dialog = await openDialog();
    fireEvent.click(button("Choose engine…"));
    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent("OrcaSlicer is not available: OrcaSlicer 3.0.0 is not supported.");
    expect(alert).toHaveTextContent("Nothing was saved.");
    expect(within(alert).queryByRole("button", { name: "Try again" })).toBeNull();
  });

  it("a transport failure offers Try again, which repeats the pick", async () => {
    slicingStoreMock.pickSlicerEngine.mockRejectedValueOnce(new Error("ipc down")).mockResolvedValueOnce(null);
    renderHost();
    const dialog = await openDialog();
    fireEvent.click(button("Choose engine…"));
    const alert = await within(dialog).findByRole("alert");
    fireEvent.click(within(alert).getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(slicingStoreMock.pickSlicerEngine).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(within(dialog).queryByRole("alert")).toBeNull());
  });
});

describe("Choose preset source", () => {
  it("a file and a folder pick with their kind", async () => {
    renderHost();
    await openDialog();
    fireEvent.click(button("Choose preset source file…"));
    await waitFor(() => expect(slicingStoreMock.pickPresetSource).toHaveBeenCalledWith("file"));
    await waitFor(() => expect(button("Choose preset source folder…")).not.toHaveAttribute("aria-disabled"));
    fireEvent.click(button("Choose preset source folder…"));
    await waitFor(() => expect(slicingStoreMock.pickPresetSource).toHaveBeenCalledWith("folder"));
    expect(slicingStoreMock.pickPresetSource).toHaveBeenCalledTimes(2);
  });

  it("a CONFLICT says the settings were reloaded, and the reloaded status shows", async () => {
    slicingStoreMock.pickPresetSource.mockImplementation(async () => {
      // The store re-backfills on CONFLICT; its fresh status arrives.
      setSlicingState({ runtime: fellThroughRuntime() });
      throw commandError("CONFLICT", "The revision changed.", { recovery: ["RELOAD"] });
    });
    renderHost();
    const dialog = await openDialog();
    fireEvent.click(button("Choose preset source folder…"));
    expect(await within(dialog).findByText(
      "The slicer settings changed while this was open, so farm3d reloaded them. Check them, then try again.",
    )).toBeInTheDocument();
    expect(within(dialog).queryByRole("alert")).toBeNull();
    expect(section(dialog, "Engine")).toHaveTextContent("OrcaSlicer_Linux_V2.3.0.AppImage");
  });

  it("PRESET_SOURCE_UNAVAILABLE shows its message and what to choose", async () => {
    slicingStoreMock.pickPresetSource.mockRejectedValue(commandError(
      "PRESET_SOURCE_UNAVAILABLE", "No OrcaSlicer presets are available: no profiles folder was found.",
    ));
    renderHost();
    const dialog = await openDialog();
    fireEvent.click(button("Choose preset source file…"));
    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent("No OrcaSlicer presets are available: no profiles folder was found.");
    expect(alert).toHaveTextContent("Choose an OrcaSlicer 2.4 install or AppImage, or a folder that holds its resources.");
  });
});

describe("Reset", () => {
  it("asks first, then goes back to automatic discovery; Keep cancels", async () => {
    setSlicingState({ runtime: fellThroughRuntime() });
    renderHost();
    const dialog = await openDialog();
    fireEvent.click(button("Use automatic discovery…"));
    const confirm = within(dialog).getByRole("group", { name: "Confirm" });
    expect(confirm).toHaveTextContent("farm3d will forget /home/pat/Applications/OrcaSlicer_Linux_V2.3.0.AppImage");
    await waitFor(() => expect(within(confirm).getByRole("button", { name: "Use automatic discovery" })).toHaveFocus());
    fireEvent.click(within(confirm).getByRole("button", { name: "Keep this engine" }));
    expect(within(dialog).queryByRole("group", { name: "Confirm" })).toBeNull();
    expect(button("Choose engine…")).toHaveFocus();
    expect(slicingStoreMock.resetSlicerRuntime).not.toHaveBeenCalled();

    fireEvent.click(button("Use automatic discovery…"));
    fireEvent.click(button("Use automatic discovery"));
    await waitFor(() => expect(slicingStoreMock.resetSlicerRuntime).toHaveBeenCalledWith({ engine: true, presetSource: false }));
  });

  it("offers the preset-source reset only when a source was chosen in Settings, working or not", async () => {
    setSlicingState({
      runtime: runtimeStatus({ presetSource: { state: "unavailable", reason: "no presets beside orca-slicer", origin: "engine" }, canSlice: false }),
    });
    renderHost();
    const dialog = await openDialog();
    const presets = section(dialog, "Preset source");
    expect(presets).toHaveTextContent("The presets couldn't be read: no presets beside orca-slicer");
    expect(within(presets).queryByRole("button", { name: "Use the engine's presets…" })).toBeNull();

    setSlicingState({
      runtime: runtimeStatus({ presetSource: { state: "unavailable", reason: "orca-2.4 no longer exists.", origin: "configured" }, canSlice: false }),
    });
    fireEvent.click(within(presets).getByRole("button", { name: "Use the engine's presets…" }));
    expect(within(dialog).getByRole("group", { name: "Confirm" })).toHaveTextContent(
      "farm3d will forget the preset source chosen in Settings and read the presets from the engine instead.",
    );
  });

  it("resets the preset source to the engine's presets", async () => {
    setSlicingState({
      runtime: runtimeStatus({
        presetSource: {
          state: "available", version: "2.4.2", channel: "release", origin: "configured", vendorCount: 62, path: "/opt/orca",
        },
      }),
    });
    renderHost();
    const dialog = await openDialog();
    fireEvent.click(button("Use the engine's presets…"));
    expect(within(dialog).getByRole("group", { name: "Confirm" })).toHaveTextContent(
      "farm3d will forget /opt/orca and read the presets from the engine instead.",
    );
    fireEvent.click(button("Use the engine's presets"));
    await waitFor(() => expect(slicingStoreMock.resetSlicerRuntime).toHaveBeenCalledWith({ engine: false, presetSource: true }));
  });
});

describe("Check again, live updates and web mode", () => {
  it("shows a busy state while the probe runs, and holds the other actions without freezing", async () => {
    let finish!: (status: SlicerRuntimeStatus) => void;
    slicingStoreMock.checkSlicerRuntime.mockImplementation(() => new Promise((resolve) => {
      finish = resolve;
    }));
    renderHost();
    const dialog = await openDialog();
    fireEvent.click(button("Check again"));
    const checking = await within(dialog).findByRole("button", { name: "Checking…" });
    expect(checking).toHaveAttribute("aria-busy", "true");
    expect(checking).toHaveAttribute("aria-disabled", "true");
    expect(dialog).toHaveTextContent("Checking for OrcaSlicer… This can take up to 10 seconds.");
    // The others wait, but stay focusable and the dialog stays live.
    const choose = button("Choose engine…");
    expect(choose).toHaveAttribute("aria-disabled", "true");
    expect(choose).not.toBeDisabled();
    fireEvent.click(choose);
    fireEvent.click(checking);
    expect(slicingStoreMock.pickSlicerEngine).not.toHaveBeenCalled();
    expect(slicingStoreMock.checkSlicerRuntime).toHaveBeenCalledTimes(1);
    setSlicingState({ runtime: fellThroughRuntime() });
    expect(section(dialog, "Engine")).toHaveTextContent("OrcaSlicer_Linux_V2.3.0.AppImage");

    finish(fellThroughRuntime());
    expect(await within(dialog).findByRole("button", { name: "Check again" })).not.toHaveAttribute("aria-disabled");
    expect(dialog).not.toHaveTextContent("This can take up to 10 seconds.");
  });

  it("reflects a slicing.runtime.changed the store applied, without asking again", async () => {
    renderHost();
    const dialog = await openDialog();
    setSlicingState({
      runtime: runtimeStatus({ engine: { state: "notFound" }, canSlice: false, engineCandidates: [] }),
    });
    expect(within(section(dialog, "Engine")).getByRole("status", { name: "Not found" })).toBeInTheDocument();
    expect(slicingStoreMock.checkSlicerRuntime).not.toHaveBeenCalled();
  });

  it("before the first status, says so and still offers Check again", async () => {
    setSlicingState({ runtime: null });
    renderHost();
    const dialog = await openDialog();
    expect(dialog).toHaveTextContent("farm3d hasn't checked for OrcaSlicer yet.");
    expect(button("Check again")).toBeInTheDocument();
  });

  it("web mode: a refused action is said once, quietly, not as an error", async () => {
    desktop.available = false;
    refuseDesktopOnlyActions();
    slicingStoreMock.checkSlicerRuntime.mockRejectedValue(commandError(
      "PERSISTENCE_UNAVAILABLE", "Checking for OrcaSlicer needs the desktop app.",
    ));
    renderHost();
    const dialog = await openDialog();
    fireEvent.click(button("Choose engine…"));
    expect(await within(dialog).findByText("Choosing an OrcaSlicer engine needs the desktop app.")).toBeInTheDocument();
    await waitFor(() => expect(button("Choose preset source file…")).not.toHaveAttribute("aria-disabled"));
    fireEvent.click(button("Choose preset source file…"));
    expect(await within(dialog).findByText("Choosing a preset source needs the desktop app.")).toBeInTheDocument();
    // The new refusal replaces the old: one message, never a pile.
    expect(within(dialog).queryByText("Choosing an OrcaSlicer engine needs the desktop app.")).toBeNull();
    await waitFor(() => expect(button("Check again")).not.toHaveAttribute("aria-disabled"));
    fireEvent.click(button("Check again"));
    expect(await within(dialog).findByText("Checking for OrcaSlicer needs the desktop app.")).toBeInTheDocument();
    expect(within(dialog).queryByRole("alert")).toBeNull();
  });

  it("works keyboard-only: open from the menu, reach each action in order, pick, and close with Escape", async () => {
    setSlicingState({ runtime: fellThroughRuntime() });
    renderHost();
    const trigger = screen.getByLabelText("Settings");
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "Enter" });
    const item = await screen.findByRole("menuitem", { name: "Slicer..." });
    item.focus();
    fireEvent.keyDown(item, { key: "Enter" });
    const dialog = await screen.findByRole("dialog", { name: "Slicer" });

    const names = tabbables(dialog).map((element) => element.getAttribute("aria-label") ?? element.textContent?.trim());
    expect(names).toEqual([
      "Close",
      "Choose engine…",
      "Use automatic discovery…",
      "Choose preset source file…",
      "Choose preset source folder…",
      "Check again",
      "Close",
    ]);
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));
    let focused = tab(dialog);
    while (focused.textContent?.trim() !== "Choose engine…") focused = tab(dialog);
    pressEnter();
    await waitFor(() => expect(slicingStoreMock.pickSlicerEngine).toHaveBeenCalledTimes(1));

    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(slicerSettingsOpen()).toBe(false);
  });
});
