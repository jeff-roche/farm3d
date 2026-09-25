import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { darkTheme, lightTheme, registerTheme, setThemeMode } from "../design-system";
import { encodeMeshBuffer, decodeMeshBuffer } from "../slicing/mesh-buffer";
import { composeTransform } from "../slicing/transforms";
import {
  failNextFakeLoad,
  failNextFakeMount,
  lastFakeRenderer,
  type FakeViewportRenderer,
} from "../slicing/viewport/fake-renderer";
import type { ViewportInstance } from "../slicing/viewport/plate-view";
import type { BuildVolume } from "../slicing/viewport/renderer";
import { DESCRIPTION_DEBOUNCE_MS, PlateViewport, type PlateViewportProps, type ViewportTool } from "./PlateViewport";

vi.mock("../settings/settings-store", () => ({
  loadSettings: vi.fn(async () => ({ themeMode: "system" })),
  updateSettings: vi.fn(async () => undefined),
}));

// A 10 mm cube from Z −5, so the Z drop shows in the matrix.
const cube = decodeMeshBuffer(encodeMeshBuffer(
  Float32Array.from([0, 0, -5, 10, 0, -5, 10, 10, -5, 0, 10, -5, 0, 0, 5, 10, 0, 5, 10, 10, 5, 0, 10, 5]),
  Uint32Array.from([0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7]),
));

const instances: ViewportInstance[] = [
  { instanceKey: "a", objectKey: 1, name: "Lid", transform: { translateMm: [20, 30], rotateDeg: [0, 0, 90], scale: [1, 1, 1] } },
  { instanceKey: "b", objectKey: 1, name: "Latch", outOfBounds: true, transform: { translateMm: [300, 0], rotateDeg: [0, 0, 0], scale: [2, 2, 2] } },
];

const volume: BuildVolume = {
  bed: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
  heightMm: 256,
  excludeAreas: [[{ xMm: 0, yMm: 0 }, { xMm: 20, yMm: 0 }, { xMm: 20, yMm: 20 }]],
};

function setToken(name: string, value: string) {
  document.documentElement.style.setProperty(name, value);
}

async function mountViewport(overrides: Partial<PlateViewportProps> = {}): Promise<FakeViewportRenderer> {
  const before = lastFakeRenderer();
  render(() => (
    <PlateViewport
      label="3D view of Enclosure"
      plateKey="p1"
      plateName="Lid"
      meshes={new Map([[1, cube]])}
      instances={instances}
      buildVolume={volume}
      {...overrides}
    />
  ));
  await waitFor(() => {
    const current = lastFakeRenderer();
    expect(current && current !== before && (current.canvas || current.failMount)).toBeTruthy();
  });
  return lastFakeRenderer()!;
}

const lastTheme = (renderer: FakeViewportRenderer) => renderer.themes[renderer.themes.length - 1];

const canvas = () => screen.getByRole("img", { name: "3D view of Enclosure" });

beforeEach(() => {
  setToken("--f3d-color-bg", "#101010");
  setToken("--f3d-color-accent", "#00ff00");
  setToken("--f3d-color-danger", "#ff0000");
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  document.documentElement.removeAttribute("style");
});

describe("PlateViewport", () => {
  it("gives the renderer the volume, meshes, placed instances, and token colours", async () => {
    const renderer = await mountViewport({ selectedInstanceKey: "a" });

    expect(renderer.canvas).toBe(canvas());
    expect(renderer.buildVolume).toEqual(volume);
    expect(renderer.meshes.get(1)).toBe(cube);
    expect(renderer.instances).toEqual([
      { instanceKey: "a", objectKey: 1, matrix: composeTransform(instances[0].transform, cube.positions), selected: true, outOfBounds: false },
      { instanceKey: "b", objectKey: 1, matrix: composeTransform(instances[1].transform, cube.positions), selected: false, outOfBounds: true },
    ]);
    // Resting on the bed: the cube's lowest vertex (Z −5) lands on Z 0.
    expect(renderer.instances[0].matrix[11]).toBe(5);
    expect(lastTheme(renderer)).toMatchObject({ background: "#101010", selected: "#00ff00", outOfBounds: "#ff0000" });
    expect(renderer.cameras).toContain("reset");
  });

  it("skips instances whose mesh has not loaded", async () => {
    const renderer = await mountViewport({ meshes: new Map() });
    expect(renderer.instances).toEqual([]);
  });

  it("shows each view from its toolbar button, with its shortcut hint", async () => {
    const renderer = await mountViewport();
    const toolbar = screen.getByRole("group", { name: "Viewport" });
    for (const [name, key, view] of [
      ["Top", "1", "top"], ["Front", "2", "front"], ["Left", "3", "left"],
      ["Right", "4", "right"], ["Iso", "5", "iso"], ["Reset", "0", "reset"],
    ] as const) {
      const button = screen.getByRole("button", { name });
      expect(toolbar).toContainElement(button);
      expect(button).toHaveAttribute("aria-keyshortcuts", key);
      expect(button.querySelector("kbd")).toHaveTextContent(key);
      renderer.cameras = [];
      fireEvent.click(button);
      expect(renderer.cameras).toEqual([view]);
    }
  });

  it("shows each view from its key while the viewport has focus", async () => {
    const renderer = await mountViewport();
    renderer.cameras = [];
    for (const key of ["1", "2", "3", "4", "5", "0"]) fireEvent.keyDown(canvas(), { key });
    expect(renderer.cameras).toEqual(["top", "front", "left", "right", "iso", "reset"]);
  });

  it("ignores view keys with a modifier, and keys typed outside the viewport", async () => {
    const renderer = await mountViewport();
    renderer.cameras = [];
    fireEvent.keyDown(canvas(), { key: "1", ctrlKey: true });
    fireEvent.keyDown(canvas(), { key: "!", shiftKey: true });
    fireEvent.keyDown(screen.getByRole("button", { name: "Top" }), { key: "2" });
    expect(renderer.cameras).toEqual([]);
  });

  it("is focusable, and Escape returns focus to the toolbar", async () => {
    await mountViewport();
    canvas().focus();
    expect(canvas()).toHaveFocus();
    fireEvent.keyDown(canvas(), { key: "Escape" });
    expect(screen.getByRole("button", { name: "Top" })).toHaveFocus();
  });

  it("selects the next and previous object with ] and [ and their buttons", async () => {
    const onSelect = vi.fn();
    await mountViewport({ onSelect, selectedInstanceKey: "a" });
    fireEvent.keyDown(canvas(), { key: "]" });
    expect(onSelect).toHaveBeenLastCalledWith("b");
    fireEvent.keyDown(canvas(), { key: "[" });
    expect(onSelect).toHaveBeenLastCalledWith("b"); // wraps from the first
    fireEvent.click(screen.getByRole("button", { name: "Next object" }));
    expect(onSelect).toHaveBeenLastCalledWith("b");
    fireEvent.click(screen.getByRole("button", { name: "Previous object" }));
    expect(onSelect).toHaveBeenLastCalledWith("b");
  });

  it("has no selection controls when nothing can be selected", async () => {
    await mountViewport();
    expect(screen.queryByRole("button", { name: /Next object/ })).toBeNull();
  });

  it("picks the object under a click within 4 px, but not at the end of an orbit drag", async () => {
    const onSelect = vi.fn();
    const renderer = await mountViewport({ onSelect });
    renderer.pickResult = { instanceKey: "b", pointMm: [1, 2, 3] };

    // A drag of 5 px orbits: no pick, no selection.
    fireEvent.pointerDown(canvas(), { button: 0, clientX: 100, clientY: 100 });
    fireEvent.pointerUp(canvas(), { button: 0, clientX: 103, clientY: 104 });
    expect(renderer.picks).toEqual([]);
    expect(onSelect).not.toHaveBeenCalled();

    // A 4 px wobble is still a click, picked where the pointer came up.
    fireEvent.pointerDown(canvas(), { button: 0, clientX: 100, clientY: 100 });
    fireEvent.pointerUp(canvas(), { button: 0, clientX: 104, clientY: 100 });
    expect(renderer.picks).toEqual([[104, 100]]);
    expect(onSelect).toHaveBeenCalledWith("b");

    // A click on empty space clears the selection.
    renderer.pickResult = null;
    fireEvent.pointerDown(canvas(), { button: 0, clientX: 10, clientY: 10 });
    fireEvent.pointerUp(canvas(), { button: 0, clientX: 10, clientY: 10 });
    expect(onSelect).toHaveBeenLastCalledWith(null);

    // Only the primary button picks.
    onSelect.mockClear();
    fireEvent.pointerDown(canvas(), { button: 2, clientX: 10, clientY: 10 });
    fireEvent.pointerUp(canvas(), { button: 2, clientX: 10, clientY: 10 });
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("refits the camera only when the plate changes, not on selection or instance changes", async () => {
    const [selected, setSelected] = createSignal<string | null>(null);
    const [shown, setShown] = createSignal(instances);
    const [plateKey, setPlateKey] = createSignal("p1");
    render(() => (
      <PlateViewport
        label="3D view of Enclosure" plateKey={plateKey()} plateName="Lid"
        meshes={new Map([[1, cube]])} instances={shown()} buildVolume={volume}
        selectedInstanceKey={selected()} onSelect={setSelected}
      />
    ));
    await waitFor(() => expect(lastFakeRenderer()?.canvas).toBe(canvas()));
    const renderer = lastFakeRenderer()!;
    expect(renderer.cameras).toEqual(["reset"]);

    setSelected("a");
    setShown([{ ...instances[0], transform: { ...instances[0].transform, translateMm: [25, 30] } }, instances[1]]);
    setShown([instances[0]]);
    expect(renderer.cameras).toEqual(["reset"]);

    setPlateKey("p2");
    expect(renderer.cameras).toEqual(["reset", "reset"]);
  });

  it("reuses a composed matrix across selection changes and replaces only XY on a move", async () => {
    const [selected, setSelected] = createSignal<string | null>(null);
    const [shown, setShown] = createSignal(instances);
    render(() => (
      <PlateViewport
        label="3D view of Enclosure" plateKey="p1" plateName="Lid"
        meshes={new Map([[1, cube]])} instances={shown()} buildVolume={volume}
        selectedInstanceKey={selected()} onSelect={setSelected}
      />
    ));
    await waitFor(() => expect(lastFakeRenderer()?.canvas).toBe(canvas()));
    const renderer = lastFakeRenderer()!;
    const before = renderer.instances[0].matrix;

    setSelected("a");
    expect(renderer.instances[0].selected).toBe(true);
    expect(renderer.instances[0].matrix).toBe(before);

    const moved = { ...instances[0].transform, translateMm: [25, 35] as [number, number] };
    setShown([{ ...instances[0], transform: moved }, instances[1]]);
    expect(renderer.instances[0].matrix).toEqual(composeTransform(moved, cube.positions));

    const turned = { ...moved, rotateDeg: [90, 0, 0] as [number, number, number] };
    setShown([{ ...instances[0], transform: turned }, instances[1]]);
    expect(renderer.instances[0].matrix).toEqual(composeTransform(turned, cube.positions));
    // The Z drop was redone: on its side, the cube's lowest vertex is at 0.
    expect(before[11]).toBe(5);
    expect(renderer.instances[0].matrix[11]).toBeCloseTo(0);
  });

  it("runs a tool from its button and its key", async () => {
    const run = vi.fn();
    const tool: ViewportTool = {
      id: "arrange", label: "Arrange", shortcut: "A", ariaKeyshortcuts: "A",
      matches: (event) => event.key === "a", run,
    };
    await mountViewport({ tools: [tool] });
    fireEvent.click(screen.getByRole("button", { name: "Arrange" }));
    fireEvent.keyDown(canvas(), { key: "a" });
    expect(run).toHaveBeenCalledTimes(2);
  });

  it("describes the plate, the selection, and what is out of bounds", async () => {
    await mountViewport({ selectedInstanceKey: "a", notes: ["Not placed (marked not printable in the file): Gasket."] });
    const description = document.getElementById(canvas().getAttribute("aria-describedby")!)!;
    expect(description).toHaveAttribute("aria-live", "polite");
    expect(description).toHaveTextContent(
      "Plate Lid: 2 objects. Selected: Lid, at X 20.0 mm, Y 30.0 mm; rotated 0°, 0°, 90° about X, Y, Z;"
        + " scaled 100%, 100%, 100%. Outside the printable area: Latch."
        + " Not placed (marked not printable in the file): Gasket.",
    );
  });

  it("announces a changed description only after it settles", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const [selected, setSelected] = createSignal<string | null>(null);
    render(() => (
      <PlateViewport
        label="3D view of Enclosure" plateKey="p1" plateName="Lid"
        meshes={new Map([[1, cube]])} instances={instances} buildVolume={null}
        selectedInstanceKey={selected()} onSelect={setSelected}
      />
    ));
    const description = document.getElementById(canvas().getAttribute("aria-describedby")!)!;
    expect(description).toHaveTextContent("No object selected.");

    setSelected("a");
    vi.advanceTimersByTime(DESCRIPTION_DEBOUNCE_MS - 1);
    setSelected("b");
    vi.advanceTimersByTime(DESCRIPTION_DEBOUNCE_MS - 1);
    expect(description).toHaveTextContent("No object selected.");
    vi.advanceTimersByTime(1);
    expect(description).toHaveTextContent("Selected: Latch");
  });

  it("repaints with the new tokens when the theme changes", async () => {
    registerTheme(lightTheme);
    registerTheme(darkTheme);
    const renderer = await mountViewport();
    setThemeMode("farm3d-dark");
    expect(lastTheme(renderer)).toMatchObject({
      background: darkTheme.color.bg,
      object: darkTheme.color.textMuted,
      selected: darkTheme.color.accent,
      outline: darkTheme.color.text,
      outOfBounds: darkTheme.color.danger,
      excludeArea: darkTheme.color.warning,
    });
    setThemeMode("farm3d-light");
    expect(lastTheme(renderer)).toMatchObject({ background: lightTheme.color.bg });
  });

  it("disposes the renderer and stops following the theme on unmount", async () => {
    registerTheme(darkTheme);
    const renderer = await mountViewport();
    const themes = renderer.themes.length;
    cleanup();
    expect(renderer.disposed).toBe(true);
    setThemeMode("farm3d-dark");
    expect(renderer.themes).toHaveLength(themes);
  });

  it("shows the unavailable panel when the WebGL context is lost", async () => {
    await mountViewport();
    fireEvent(canvas(), new Event("webglcontextlost"));
    expect(await screen.findByText("3D view unavailable (the graphics context was lost)")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Top" })).toBeDisabled();
  });

  it("says the viewer failed to load when its chunk can't load, and logs why", async () => {
    const failure = new Error("chunk fetch failed");
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    failNextFakeLoad(failure);
    render(() => (
      <PlateViewport
        label="3D view of Enclosure" plateKey="p1" plateName="Lid"
        meshes={new Map([[1, cube]])} instances={instances} buildVolume={null}
      />
    ));
    expect(await screen.findByText("3D view unavailable (the viewer failed to load)")).toBeInTheDocument();
    expect(log).toHaveBeenCalledWith("The 3D viewer failed to load:", failure);
    log.mockRestore();
  });

  it("says so in text when WebGL is unavailable, and keeps the description", async () => {
    failNextFakeMount();
    const renderer = await mountViewport();
    expect(await screen.findByText("3D view unavailable (WebGL is not available)")).toBeInTheDocument();
    expect(renderer.disposed).toBe(true);
    expect(screen.getByRole("button", { name: "Top" })).toBeDisabled();
    expect(screen.getByText(/Plate Lid: 2 objects/)).toBeInTheDocument();
  });
});
