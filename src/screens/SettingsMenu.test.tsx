import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import { SettingsMenu } from "./SettingsMenu";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("SettingsMenu", () => {
  it("shows theme, Slicer, Notifications and retention, and explicit settings import/export actions", async () => {
    render(() => <SettingsMenu />);

    await fireEvent.pointerDown(screen.getByLabelText("Settings"), {
      pointerType: "mouse",
      button: 0,
    });

    expect(await screen.findByText("Theme...")).toBeInTheDocument();
    expect(screen.getByText("Slicer...")).toBeInTheDocument();
    expect(screen.getByText("Notifications and retention...")).toBeInTheDocument();
    expect(screen.getByText("Export settings...")).toBeInTheDocument();
    expect(screen.getByText("Import settings...")).toBeInTheDocument();
  });

  it("opens the Notifications and retention dialog when its item is selected", async () => {
    render(() => <SettingsMenu />);

    await fireEvent.pointerDown(screen.getByLabelText("Settings"), {
      pointerType: "mouse",
      button: 0,
    });
    const item = await screen.findByText("Notifications and retention...");
    await fireEvent.pointerUp(item, { button: 0 });

    expect(await screen.findByText("Notify for")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Fatal failures" })).toBeInTheDocument();
  });

  it("opens the theme popover with theme options when 'Theme...' is selected", async () => {
    render(() => <SettingsMenu />);

    await fireEvent.pointerDown(screen.getByLabelText("Settings"), {
      pointerType: "mouse",
      button: 0,
    });
    const themeItem = await screen.findByText("Theme...");
    await fireEvent.pointerUp(themeItem, { button: 0 });

    expect(await screen.findByText("System")).toBeInTheDocument();
    expect(screen.getByText("Light")).toBeInTheDocument();
    expect(screen.getByText("Dark")).toBeInTheDocument();
    expect(screen.getByText("Apply")).toBeInTheDocument();
  });
});
