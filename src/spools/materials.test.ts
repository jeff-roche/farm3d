import { describe, expect, it } from "vitest";
import { MATERIAL_FAMILIES, materialFamilyLabel, materialLabel } from "./materials";

describe("materialFamilyLabel", () => {
  it("labels every D2 family, including the multi-part PA-CF/PET-CF spellings", () => {
    expect(materialFamilyLabel("PLA")).toBe("PLA");
    expect(materialFamilyLabel("PLA-CF")).toBe("PLA-CF");
    expect(materialFamilyLabel("PETG")).toBe("PETG");
    expect(materialFamilyLabel("PET-CF")).toBe("PET-CF");
    expect(materialFamilyLabel("ABS")).toBe("ABS");
    expect(materialFamilyLabel("ASA")).toBe("ASA");
    expect(materialFamilyLabel("TPU")).toBe("TPU");
    expect(materialFamilyLabel("PA")).toBe("PA (Nylon)");
    expect(materialFamilyLabel("PA-CF")).toBe("PA-CF (Nylon)");
    expect(materialFamilyLabel("PC")).toBe("PC");
    expect(materialFamilyLabel("PVA")).toBe("PVA");
    expect(materialFamilyLabel("HIPS")).toBe("HIPS");
    expect(materialFamilyLabel("PP")).toBe("PP");
    expect(materialFamilyLabel("OTHER")).toBe("Other");
  });
});

describe("materialLabel", () => {
  it("shows the family label directly for every family but OTHER", () => {
    expect(materialLabel("PETG")).toBe("PETG");
  });

  it("shows the trimmed materialOther text for OTHER", () => {
    expect(materialLabel("OTHER", "  Wood-fill  ")).toBe("Wood-fill");
  });

  it("falls back to the generic label when OTHER has no materialOther text", () => {
    expect(materialLabel("OTHER")).toBe("Other");
    expect(materialLabel("OTHER", "   ")).toBe("Other");
  });
});

describe("MATERIAL_FAMILIES", () => {
  it("lists exactly D2's 14 closed families, in a stable order ending in OTHER", () => {
    expect(MATERIAL_FAMILIES).toEqual([
      "PLA", "PLA-CF", "PETG", "PET-CF", "ABS", "ASA", "TPU",
      "PA", "PA-CF", "PC", "PVA", "HIPS", "PP", "OTHER",
    ]);
  });
});
