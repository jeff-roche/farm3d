import { describe, expect, it } from "vitest";
import { formatGrams, parseGrams } from "./weight";

describe("parseGrams", () => {
  it("parses one decimal place of grams into integer milligrams", () => {
    expect(parseGrams("612.3")).toEqual({ ok: true, mg: 612_300 });
  });

  it("parses whole grams into integer milligrams", () => {
    expect(parseGrams("612")).toEqual({ ok: true, mg: 612_000 });
  });

  it("rejects more than one decimal place as a precision error", () => {
    expect(parseGrams("612.34")).toEqual({ ok: false, reason: "precision" });
  });

  it("rejects a negative amount as out of range", () => {
    expect(parseGrams("-1")).toEqual({ ok: false, reason: "range" });
  });

  it("rejects an amount above the 50 kg ceiling as out of range", () => {
    expect(parseGrams("50001000")).toEqual({ ok: false, reason: "range" });
  });

  it("accepts the 50 kg ceiling itself", () => {
    expect(parseGrams("50000")).toEqual({ ok: true, mg: 50_000_000 });
  });

  it("rejects a comma decimal separator as a format error", () => {
    expect(parseGrams("1,5")).toEqual({ ok: false, reason: "format" });
  });

  it("rejects letters as a format error", () => {
    expect(parseGrams("abc")).toEqual({ ok: false, reason: "format" });
  });

  it("rejects an empty string", () => {
    expect(parseGrams("")).toEqual({ ok: false, reason: "empty" });
  });

  it("rejects a blank (whitespace-only) string as empty", () => {
    expect(parseGrams("   ")).toEqual({ ok: false, reason: "empty" });
  });

  it("tolerates surrounding whitespace around a valid value", () => {
    expect(parseGrams("  12.5  ")).toEqual({ ok: true, mg: 12_500 });
  });
});

describe("formatGrams", () => {
  it("formats whole grams with no decimal at precision 0", () => {
    expect(formatGrams(612_300, 0)).toBe("612 g");
  });

  it("formats one decimal place at precision 1", () => {
    expect(formatGrams(612_300, 1)).toBe("612.3 g");
  });

  it("rounds half away from zero at precision 0", () => {
    expect(formatGrams(612_500, 0)).toBe("613 g");
  });

  it("rounds half away from zero at precision 1", () => {
    // 612.35 g rounds to 612.4 g, not 612.3 g (banker's rounding would give 612.4 too,
    // but 612.25 -> 612.3 pins the away-from-zero behavior specifically).
    expect(formatGrams(612_250, 1)).toBe("612.3 g");
  });

  it("formats zero", () => {
    expect(formatGrams(0, 0)).toBe("0 g");
    expect(formatGrams(0, 1)).toBe("0.0 g");
  });
});
