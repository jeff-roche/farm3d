import { describe, expect, it } from "vitest";
import { farm3dTypography } from "./typography";

describe("farm3dTypography", () => {
  it("uses Manrope at weight 600 for heading", () => {
    expect(farm3dTypography.heading.fontFamily).toContain("Manrope");
    expect(farm3dTypography.heading.fontWeight).toBe(600);
  });

  it("uses Fira Sans for body, bodySmall, and label", () => {
    expect(farm3dTypography.body.fontFamily).toContain("Fira Sans");
    expect(farm3dTypography.body.fontWeight).toBe(400);
    expect(farm3dTypography.bodySmall.fontFamily).toContain("Fira Sans");
    expect(farm3dTypography.label.fontFamily).toContain("Fira Sans");
    expect(farm3dTypography.label.fontWeight).toBe(500);
  });

  it("uses Fira Code for mono", () => {
    expect(farm3dTypography.mono.fontFamily).toContain("Fira Code");
  });
});
