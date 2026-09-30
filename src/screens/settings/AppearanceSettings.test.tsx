import { createSignal, Show } from "solid-js";
import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getThemeMode, initTheme, setThemeMode } from "../../design-system";
import { AppearanceSettings } from "./AppearanceSettings";

beforeEach(async () => {
  // registerTheme() (needed for previewTheme's resolveTheme() to find the built-in
  // themes) only happens inside initTheme() — call it before setting a known mode.
  await initTheme();
  setThemeMode("farm3d-light");
});

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

describe("AppearanceSettings", () => {
  it("shows a hint to click an option to preview it", () => {
    render(() => <AppearanceSettings />);

    expect(screen.getByText("Click an option to preview it.")).toBeInTheDocument();
  });

  it("previews a theme on click without committing or persisting it", async () => {
    render(() => <AppearanceSettings />);

    await fireEvent.click(screen.getByText("Dark"));

    expect(document.documentElement.dataset.themeName).toBe("farm3d-dark");
    expect(getThemeMode()).toBe("farm3d-light");
  });

  it("does not preview a theme on hover", async () => {
    render(() => <AppearanceSettings />);

    await fireEvent.mouseOver(screen.getByText("Dark"));

    expect(document.documentElement.dataset.themeName).toBe("farm3d-light");
  });

  it("commits the previewed theme when Apply is clicked", async () => {
    render(() => <AppearanceSettings />);

    await fireEvent.click(screen.getByText("Dark"));
    await fireEvent.click(screen.getByRole("button", { name: "Apply" }));

    expect(getThemeMode()).toBe("farm3d-dark");
    expect(document.documentElement.dataset.themeName).toBe("farm3d-dark");
  });

  it("restores the committed theme when Revert is clicked", async () => {
    render(() => <AppearanceSettings />);

    await fireEvent.click(screen.getByText("Dark"));
    expect(document.documentElement.dataset.themeName).toBe("farm3d-dark");
    await fireEvent.click(screen.getByRole("button", { name: "Revert" }));

    expect(document.documentElement.dataset.themeName).toBe("farm3d-light");
    expect(getThemeMode()).toBe("farm3d-light");
    expect(screen.getByRole("radio", { name: "Light" })).toBeChecked();
  });

  it("restores the committed theme when the category or workspace goes away with a pending preview", async () => {
    const [shown, setShown] = createSignal(true);
    render(() => (
      <Show when={shown()}>
        <AppearanceSettings />
      </Show>
    ));

    await fireEvent.click(screen.getByText("Dark"));
    expect(document.documentElement.dataset.themeName).toBe("farm3d-dark");
    setShown(false);

    expect(document.documentElement.dataset.themeName).toBe("farm3d-light");
    expect(getThemeMode()).toBe("farm3d-light");
  });

  it("previews System as whatever the OS prefers", async () => {
    vi.stubGlobal("matchMedia", (query: string) => ({
      matches: query.includes("dark"),
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    }));
    render(() => <AppearanceSettings />);

    await fireEvent.click(screen.getByText("System"));
    expect(document.documentElement.dataset.themeName).toBe("farm3d-dark");
    expect(getThemeMode()).toBe("farm3d-light");

    await fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(getThemeMode()).toBe("system");
  });
});
