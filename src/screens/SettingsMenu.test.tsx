import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import { SettingsMenu } from "./SettingsMenu";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("SettingsMenu", () => {
  it("lists theme options and an 'Open settings file' action", async () => {
    render(() => <SettingsMenu />);

    await fireEvent.pointerDown(screen.getByLabelText("Settings"), {
      pointerType: "mouse",
      button: 0,
    });

    expect(await screen.findByText("System")).toBeInTheDocument();
    expect(screen.getByText("Light")).toBeInTheDocument();
    expect(screen.getByText("Dark")).toBeInTheDocument();
    expect(screen.getByText("Open settings file")).toBeInTheDocument();
  });
});
