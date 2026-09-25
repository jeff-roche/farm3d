import { describe, expect, it } from "vitest";
import type { CommandError } from "../generated/contracts/command/CommandError";
import {
  candidateLines,
  clampControl,
  failureText,
  layerHeightMax,
  logSegments,
  runtimeProblems,
  sliceErrorView,
} from "./slice-presentation";
import { runtimeStatus } from "./test-records";
import type { SliceFailureCode } from "./types";

function commandError(overrides: Partial<CommandError>): CommandError {
  return { contractVersion: 1, code: "INTERNAL", message: "Refused.", recovery: [], retryable: false, ...overrides };
}

describe("controls", () => {
  it("limits the layer height to 80% of the nozzle, never above the backend's limit", () => {
    expect(layerHeightMax(0.4)).toBe(0.32);
    expect(layerHeightMax(0.6)).toBe(0.48);
    expect(layerHeightMax(0.25)).toBe(0.2);
    expect(layerHeightMax(0.4)).toBeLessThanOrEqual(0.4 * 0.8);
  });

  it("clamps into D4's ranges and rounds whole-number controls", () => {
    expect(clampControl("wallLoops", 0)).toBe(1);
    expect(clampControl("wallLoops", 25)).toBe(20);
    expect(clampControl("wallLoops", 3.6)).toBe(4);
    expect(clampControl("infillDensityPercent", 140)).toBe(100);
    expect(clampControl("infillDensityPercent", 12.5)).toBe(12.5);
    expect(clampControl("supportThresholdAngleDeg", -5)).toBe(0);
    expect(clampControl("brimWidthMm", 30)).toBe(20);
    expect(clampControl("skirtLoops", 11)).toBe(10);
    expect(clampControl("topShellLayers", 51)).toBe(50);
    expect(clampControl("layerHeightMm", 0.5, 0.32)).toBe(0.32);
    expect(clampControl("layerHeightMm", 0.01, 0.32)).toBe(0.05);
  });
});

describe("failureText", () => {
  const text = (code: SliceFailureCode, message = "backend text") => failureText({ code, message });

  it("uses D11's text for each code", () => {
    expect(text({ kind: "objectsOutsidePlate" })).toBe("An object is outside the printable area.");
    expect(text({ kind: "presetInvalid" })).toBe("OrcaSlicer couldn't read the presets farm3d prepared.");
    expect(text({ kind: "inputMissing" })).toBe("OrcaSlicer couldn't find its input.");
    expect(text({ kind: "inputInvalid" })).toBe("OrcaSlicer couldn't read the prepared plate.");
    expect(text({ kind: "presetIncompatible" })).toBe("The quality preset isn't compatible with this printer.");
    expect(text({ kind: "outputMissing" })).toBe("OrcaSlicer reported success but wrote no G-code.");
    expect(text({ kind: "timeout" })).toBe("Slicing took longer than 30 minutes.");
    expect(text({ kind: "engineCrashed", signal: 11 })).toBe("OrcaSlicer stopped unexpectedly (signal 11).");
    expect(text({ kind: "spawnFailed" })).toBe("farm3d couldn't start OrcaSlicer.");
  });

  it("uses the engine's own error string and the output check's reason", () => {
    expect(text({ kind: "engineError", returnCode: -2 }, "Out of memory")).toBe("Out of memory");
    expect(text({ kind: "engineError", returnCode: -2 }, "")).toBe("OrcaSlicer failed with return code -2.");
    expect(text({ kind: "outputInvalid", reason: "Printed outside the bed" }, "")).toBe("Printed outside the bed");
  });

  it("says storage and internal faults are farm3d's, not OrcaSlicer's", () => {
    expect(text({ kind: "storageFailed" })).toMatch(/^farm3d couldn't save the slice result/);
    expect(text({ kind: "internalError" })).toMatch(/^Something went wrong inside farm3d/);
  });
});

describe("runtime", () => {
  it("says why each part can't be used, by basename", () => {
    const runtime = runtimeStatus({
      engine: { state: "probeFailed", reason: "exit code 127", executableName: "orca-slicer" },
      presetSource: { state: "presetsUnreadable" },
      canSlice: false,
    });
    expect(runtimeProblems(runtime)).toEqual([
      "orca-slicer couldn't be run: exit code 127",
      "This OrcaSlicer build stores its presets in a format farm3d can't read. Choose an OrcaSlicer 2.4 install or AppImage as the preset source.",
    ]);
    expect(runtimeProblems(runtimeStatus())).toEqual([]);
  });

  it("explains each engine candidate discovery tried", () => {
    const runtime = runtimeStatus({
      engineCandidates: [
        { source: "configured", executableName: "OrcaSlicer-3.0.AppImage", path: "/x", result: { kind: "unsupportedVersion", version: "3.0.0" } },
        { source: "path", executableName: "orca-slicer", path: "/y", result: { kind: "probeFailed", reason: "no display" } },
        { source: "wellKnown", executableName: "OrcaSlicer_2.4.2.AppImage", path: "/z", result: { kind: "chosen", version: "2.4.2" } },
      ],
    });
    expect(candidateLines(runtime)).toEqual([
      "OrcaSlicer-3.0.AppImage (chosen in Settings): OrcaSlicer 3.0.0 isn't supported.",
      "orca-slicer (on PATH): couldn't be run: no display",
      "OrcaSlicer_2.4.2.AppImage (in a usual download folder): used, OrcaSlicer 2.4.2.",
    ]);
  });
});

describe("sliceErrorView", () => {
  it("links backend-only errors to their field", () => {
    expect(sliceErrorView(commandError({ code: "FILAMENT_INCOMPATIBLE", message: "Not for this printer." })))
      .toMatchObject({ message: "Not for this printer.", field: "material", stale: false });
    expect(sliceErrorView(commandError({
      code: "UNSUPPORTED_SETTING_FOR_RUNTIME",
      recovery: ["EDIT_PREPARATION", "OPEN_SLICER_SETTINGS"],
      details: { key: "brim_type", presetSourceVersion: "2.3.0" },
    }))).toMatchObject({ field: "brimType", openSettings: true });
    expect(sliceErrorView(commandError({ code: "UNMAPPED_PROFILE_OVERRIDE", details: { field: "nozzleType" } })))
      .toMatchObject({ field: "target" });
    expect(sliceErrorView(commandError({ code: "PRESET_NOT_FOUND", details: { kind: "process" } })))
      .toMatchObject({ field: "quality" });
    expect(sliceErrorView(commandError({ code: "VALIDATION", details: { fieldPath: "controls.wallLoops" } })))
      .toMatchObject({ field: "wallLoops" });
    expect(sliceErrorView(commandError({ code: "PREPARATION_INVALID", details: { plateKey: "plt-2", reason: "empty" } })))
      .toMatchObject({ plateKey: "plt-2" });
  });

  it("marks a stale refusal, and offers a retry only when no answer came", () => {
    expect(sliceErrorView(commandError({ code: "PREPARATION_STALE", recovery: ["RELOAD_PREPARATION"] })))
      .toMatchObject({ stale: true, retry: false });
    expect(sliceErrorView(new TypeError("IPC closed"))).toMatchObject({ retry: true });
  });
});

describe("logSegments", () => {
  it("keeps noise lines apart and joins the rest, losing nothing", () => {
    const text = "noise one\n[info] a\n[info] b\nnoise two\n[info] c";
    const segments = logSegments(text, [1, 4]);
    expect(segments).toEqual([
      { text: "noise one\n", noise: true },
      { text: "[info] a\n[info] b\n", noise: false },
      { text: "noise two\n", noise: true },
      { text: "[info] c", noise: false },
    ]);
    expect(segments.map((segment) => segment.text).join("")).toBe(text);
  });

  it("handles an empty log, and a trailing line break", () => {
    expect(logSegments("", [])).toEqual([]);
    expect(logSegments("a\n", [])).toEqual([{ text: "a\n", noise: false }]);
  });
});
