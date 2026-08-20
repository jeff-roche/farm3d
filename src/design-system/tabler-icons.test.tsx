import { render } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import { IconPrinter } from "@tabler/icons-solidjs";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("@tabler/icons-solidjs", () => {
  it("renders an icon as an inline svg", () => {
    const { container } = render(() => <IconPrinter size={16} />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("width")).toBe("16");
  });
});
