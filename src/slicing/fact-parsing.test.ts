import { describe, expect, it } from "vitest";
import {
  absentFactCount,
  claimsFor,
  emptyFactsDraft,
  FACT_CLAIM_KEYS,
  FACT_ERRORS,
  factFieldAt,
  matchCatalogModel,
  matchCatalogVariant,
  parseDiameterClaim,
  parseMaterialClaim,
  validateFacts,
} from "./fact-parsing";

describe("reading claims", () => {
  it("reads a diameter only when every extruder agrees and D16 allows it", () => {
    expect(parseDiameterClaim("0.4")).toBe(0.4);
    expect(parseDiameterClaim(" 1.75 , 1.75 ")).toBe(1.75);
    expect(parseDiameterClaim("0.4;0.4")).toBe(0.4);
    expect(parseDiameterClaim(".6")).toBe(0.6);
    expect(parseDiameterClaim("0.4,0.6")).toBeUndefined();
    expect(parseDiameterClaim("0")).toBeUndefined();
    expect(parseDiameterClaim("5")).toBe(5);
    expect(parseDiameterClaim("5.01")).toBeUndefined();
    expect(parseDiameterClaim("0.4mm")).toBeUndefined();
    expect(parseDiameterClaim("-0.4")).toBeUndefined();
    expect(parseDiameterClaim("")).toBeUndefined();
    expect(parseDiameterClaim("1e0")).toBeUndefined();
  });

  it("reads a material as a family, or OTHER with the file's name", () => {
    expect(parseMaterialClaim("PLA")).toEqual({ family: "PLA" });
    expect(parseMaterialClaim("petg;PETG")).toEqual({ family: "PETG" });
    expect(parseMaterialClaim("PA-CF")).toEqual({ family: "PA-CF" });
    expect(parseMaterialClaim("PLA+")).toEqual({ family: "OTHER", other: "PLA+" });
    expect(parseMaterialClaim("PLA;PETG")).toBeUndefined();
    expect(parseMaterialClaim("x".repeat(33))).toBeUndefined();
    // OTHER is farm3d's sentinel, never a family a file names.
    expect(parseMaterialClaim("OTHER")).toEqual({ family: "OTHER", other: "OTHER" });
  });

  it("finds the claims for a fact in key order", () => {
    const claims = [
      { key: "printer_settings_id", value: "Elegoo Centauri Carbon 0.4 nozzle", line: 20 },
      { key: "printer_model", value: "Elegoo Centauri Carbon", line: 12 },
    ];
    expect(claimsFor(claims, FACT_CLAIM_KEYS.printerProfile).map((claim) => claim.line)).toEqual([12, 20]);
    expect(claimsFor(claims, FACT_CLAIM_KEYS.filamentDiameterMm)).toEqual([]);
  });

  it("matches a printer claim to exactly one catalog model and the variant the file names", () => {
    const models = [
      { modelId: "a", vendor: "Elegoo", model: "Elegoo Centauri Carbon" },
      { modelId: "b", vendor: "Prusa", model: "Prusa MK4" },
      { modelId: "c", vendor: "Other", model: "Twin" },
      { modelId: "d", vendor: "Else", model: "twin" },
    ];
    expect(matchCatalogModel("elegoo centauri carbon ", models)?.modelId).toBe("a");
    expect(matchCatalogModel("Twin", models)).toBeUndefined();
    expect(matchCatalogModel("", models)).toBeUndefined();

    const variants = [
      { variant: "Elegoo Centauri Carbon 0.4 nozzle", printerVariant: "0.4" },
      { variant: "Elegoo Centauri Carbon 0.6 nozzle", printerVariant: "0.6" },
    ];
    expect(matchCatalogVariant(variants, { settingsId: "Elegoo Centauri Carbon 0.6 nozzle" })?.printerVariant).toBe("0.6");
    expect(matchCatalogVariant(variants, { nozzleMm: 0.4 })?.printerVariant).toBe("0.4");
    expect(matchCatalogVariant(variants, {})).toBeUndefined();
    expect(matchCatalogVariant(variants.slice(0, 1), {})?.printerVariant).toBe("0.4");
  });
});

describe("validating the draft", () => {
  it("records every empty fact as absent", () => {
    const result = validateFacts(emptyFactsDraft());
    expect(result).toEqual({
      ok: true,
      facts: {
        printerProfile: { kind: "absent" },
        nozzleDiameterMm: { kind: "absent" },
        materialFamily: { kind: "absent" },
        filamentDiameterMm: { kind: "absent" },
      },
    });
    expect(absentFactCount(emptyFactsDraft())).toBe(4);
  });

  it("confirms what was entered, and sends materialOther only for OTHER", () => {
    const target = { kind: "printer" as const, printerId: "prn-1" };
    const draft = {
      printerProfile: target,
      nozzleDiameterMm: " 0.4 ",
      materialFamily: "OTHER" as const,
      materialOther: "  Wood PLA ",
      filamentDiameterMm: "1.75",
    };
    expect(validateFacts(draft)).toEqual({
      ok: true,
      facts: {
        printerProfile: { kind: "confirmed", value: target },
        nozzleDiameterMm: { kind: "confirmed", value: 0.4 },
        materialFamily: { kind: "confirmed", value: "OTHER" },
        materialOther: "Wood PLA",
        filamentDiameterMm: { kind: "confirmed", value: 1.75 },
      },
    });
    expect(absentFactCount(draft)).toBe(0);
    const pla = validateFacts({ ...draft, materialFamily: "PLA" });
    expect(pla.ok && "materialOther" in pla.facts).toBe(false);
  });

  it("refuses what the backend would: diameters outside (0, 5] mm, and OTHER without a 1–32 character name", () => {
    for (const bad of ["0", "5.5", "abc", "-1", "Infinity", "NaN"]) {
      const result = validateFacts({ ...emptyFactsDraft(), nozzleDiameterMm: bad, filamentDiameterMm: bad });
      expect(result).toEqual({
        ok: false,
        errors: { nozzleDiameterMm: FACT_ERRORS.nozzleDiameterMm, filamentDiameterMm: FACT_ERRORS.filamentDiameterMm },
      });
    }
    expect(validateFacts({ ...emptyFactsDraft(), materialFamily: "OTHER", materialOther: "  " }))
      .toEqual({ ok: false, errors: { materialOther: FACT_ERRORS.materialOther } });
    expect(validateFacts({ ...emptyFactsDraft(), materialFamily: "OTHER", materialOther: "y".repeat(33) }).ok).toBe(false);
    expect(validateFacts({ ...emptyFactsDraft(), materialFamily: "OTHER", materialOther: "y".repeat(32) }).ok).toBe(true);
  });

  it("maps a backend fieldPath onto the dialog's field", () => {
    expect(factFieldAt("facts.nozzleDiameterMm")).toBe("nozzleDiameterMm");
    expect(factFieldAt("facts.filamentDiameterMm")).toBe("filamentDiameterMm");
    expect(factFieldAt("facts.materialOther")).toBe("materialOther");
    expect(factFieldAt("facts.printerProfile")).toBe("printerProfile");
    expect(factFieldAt("facts.printerProfile.printerId")).toBe("printerProfile");
    expect(factFieldAt("sourceRevisionId")).toBeUndefined();
    expect(factFieldAt("facts.somethingElse")).toBeUndefined();
    expect(factFieldAt(undefined)).toBeUndefined();
  });
});
