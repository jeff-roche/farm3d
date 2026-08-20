import { createSignal } from "solid-js";
import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getThemeMode, initTheme, setThemeMode } from "../design-system";
import { ThemePopover } from "./ThemePopover";

beforeEach(async () => {
  // registerTheme() (needed for previewTheme's resolveTheme() to find the built-in
  // themes) only happens inside initTheme() — call it before setting a known mode.
  await initTheme();
  setThemeMode("farm3d-light");
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("ThemePopover", () => {
  it("shows a title and a hint to click an option to preview it", async () => {
    render(() => <ThemePopover open={true} onOpenChange={() => {}} />);

    expect(screen.getByText("Theme")).toBeInTheDocument();
    expect(screen.getByText("Click an option to preview it.")).toBeInTheDocument();
  });

  it("previews a theme on click without committing or persisting it", async () => {
    render(() => <ThemePopover open={true} onOpenChange={() => {}} />);

    await fireEvent.click(screen.getByText("Dark"));

    expect(document.documentElement.dataset.themeName).toBe("farm3d-dark");
    expect(getThemeMode()).toBe("farm3d-light");
  });

  it("does not preview a theme on hover", async () => {
    render(() => <ThemePopover open={true} onOpenChange={() => {}} />);

    await fireEvent.mouseOver(screen.getByText("Dark"));

    expect(document.documentElement.dataset.themeName).toBe("farm3d-light");
    expect(getThemeMode()).toBe("farm3d-light");
  });

  it("commits the previewed theme when Apply is clicked", async () => {
    const onOpenChange = vi.fn();
    render(() => <ThemePopover open={true} onOpenChange={onOpenChange} />);

    await fireEvent.click(screen.getByText("Dark"));
    await fireEvent.click(screen.getByText("Apply"));

    expect(getThemeMode()).toBe("farm3d-dark");
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("reverts the preview when closed without applying", async () => {
    function Harness() {
      const [open, setOpen] = createSignal(true);
      return <ThemePopover open={open()} onOpenChange={setOpen} />;
    }
    render(() => <Harness />);

    await fireEvent.click(screen.getByText("Dark"));
    expect(document.documentElement.dataset.themeName).toBe("farm3d-dark");

    // Simulate closing without applying (e.g. Escape / click-outside).
    const closeEvent = new KeyboardEvent("keydown", { key: "Escape", bubbles: true });
    document.dispatchEvent(closeEvent);

    expect(document.documentElement.dataset.themeName).toBe("farm3d-light");
    expect(getThemeMode()).toBe("farm3d-light");
  });
});
