/**
 * D1: weight parsing/formatting is the one place grams and milligrams meet.
 * Everything below the UI (Rust, SQLite, the wire, and every other frontend
 * module) is integer milligrams; this module is the sole boundary that
 * converts to and from the grams a user types or reads.
 *
 * The largest valid amount is 50 kg (5*10^7 mg), the ceiling shared by every
 * D1 field (current/low-threshold net weight). `parseGrams` enforces that
 * shared ceiling; a field with a tighter range (nominal net weight's 1 g
 * floor, tare's 5 kg ceiling) enforces the rest itself.
 */

const MAX_MG = 50_000_000;

export type ParseGramsResult =
  | { ok: true; mg: number }
  | { ok: false; reason: "empty" | "format" | "precision" | "range" };

const GRAMS_PATTERN = /^(-?)(\d+)(?:\.(\d+))?$/;

/** Parses a user-typed grams string into integer milligrams. At most one
 *  decimal digit is accepted (D1: input accepts at most 0.1 g). */
export function parseGrams(input: string): ParseGramsResult {
  const trimmed = input.trim();
  if (trimmed === "") return { ok: false, reason: "empty" };

  const match = GRAMS_PATTERN.exec(trimmed);
  if (!match) return { ok: false, reason: "format" };

  const [, sign, wholePart, decimalPart] = match;
  if (decimalPart !== undefined && decimalPart.length > 1) {
    return { ok: false, reason: "precision" };
  }

  // Integer arithmetic throughout -- avoids float artifacts like
  // 612.3 * 1000 === 612299.9999999999.
  const tenthsOfGram = Number(wholePart) * 10 + Number(decimalPart ?? "0");
  const magnitudeMg = tenthsOfGram * 100;
  const mg = sign === "-" ? -magnitudeMg : magnitudeMg;

  if (mg < 0 || mg > MAX_MG) return { ok: false, reason: "range" };
  return { ok: true, mg };
}

/** Formats integer milligrams as a grams string, rounding half away from
 *  zero (D1: tables show whole grams, detail/entry fields show one
 *  decimal). All arithmetic is integer-based to keep the rounding exact. */
export function formatGrams(mg: number, precision: 0 | 1): string {
  const displayUnitMg = precision === 1 ? 100 : 1000; // one displayed unit, in mg
  const roundedUnits = Math.round(Math.abs(mg) / displayUnitMg);
  const sign = mg < 0 && roundedUnits !== 0 ? "-" : "";

  if (precision === 0) return `${sign}${roundedUnits} g`;

  const whole = Math.trunc(roundedUnits / 10);
  const tenth = roundedUnits % 10;
  return `${sign}${whole}.${tenth} g`;
}
