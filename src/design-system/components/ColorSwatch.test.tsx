import { render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import { ColorSwatch } from "./ColorSwatch";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("ColorSwatch", () => {
  it("has an accessible name of `name` and an inline background from `hex`", () => {
    render(() => <ColorSwatch hex="#2f7a3c" name="Farm Green" />);

    const swatch = screen.getByRole("img", { name: "Farm Green" });
    expect(swatch.style.backgroundColor).toBe("rgb(47, 122, 60)");
  });

  it("renders the no-color pattern when hex is null, without setting an inline background", () => {
    render(() => <ColorSwatch hex={null} name="Unknown" />);

    const swatch = screen.getByRole("img", { name: "Unknown" });
    expect(swatch.style.backgroundColor).toBe("");
    expect(swatch.getAttribute("data-color")).toBe("none");
  });
});
