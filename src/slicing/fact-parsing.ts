/** D16/P8: the G-code facts dialog's pure half. It reads a file's claims
 *  into typed values for **Use the file's value**, validates what the
 *  operator typed with the backend's own rules, and turns the draft into
 *  `create_external_slice_revision`'s `facts`. Nothing here fills a fact on
 *  its own: a claim only becomes a fact when the operator confirms it. */
import type { GcodeClaim } from "../generated/contracts/domain/GcodeClaim";
import type { CatalogModelSummary, CatalogVariantSummary } from "../printers/types";
import { MATERIAL_FAMILIES } from "../spools/materials";
import type { CreateExternalSliceRevisionFacts, MaterialFamily, SliceTarget } from "./types";

export type FactField = "printerProfile" | "nozzleDiameterMm" | "materialFamily" | "materialOther" | "filamentDiameterMm";

/** Every fact the dialog asks for, in its order. `materialOther` belongs
 *  to the material row. */
export const FACT_FIELDS: readonly FactField[] = [
  "printerProfile",
  "nozzleDiameterMm",
  "materialFamily",
  "materialOther",
  "filamentDiameterMm",
];

/** The dialog's editable state. An empty string or `null` is a fact left
 *  empty, which is recorded as absent. */
export interface FactsDraft {
  printerProfile: SliceTarget | null;
  nozzleDiameterMm: string;
  materialFamily: MaterialFamily | null;
  materialOther: string;
  filamentDiameterMm: string;
}

export function emptyFactsDraft(): FactsDraft {
  return { printerProfile: null, nozzleDiameterMm: "", materialFamily: null, materialOther: "", filamentDiameterMm: "" };
}

// --- Claims --------------------------------------------------------------------

/** The allowlisted claim keys (P4 D11) each fact reads. The allowlist has
 *  no filament-diameter key today, so that fact always shows "Not in the
 *  file"; the key is listed so a future allowlist entry just works. */
export const FACT_CLAIM_KEYS = {
  printerProfile: ["printer_model", "printer_settings_id"],
  nozzleDiameterMm: ["nozzle_diameter"],
  materialFamily: ["filament_type"],
  filamentDiameterMm: ["filament_diameter"],
} as const satisfies Record<Exclude<FactField, "materialOther">, readonly string[]>;

/** The first claim for each of `keys`, in key order. */
export function claimsFor(claims: readonly GcodeClaim[], keys: readonly string[]): GcodeClaim[] {
  return keys.flatMap((key) => {
    const claim = claims.find((candidate) => candidate.key === key);
    return claim ? [claim] : [];
  });
}

/** OrcaSlicer writes one value per extruder, comma- or semicolon-separated.
 *  A claim is read only when every extruder agrees. */
function singleValue(raw: string): string | undefined {
  const parts = raw.split(/[,;]/).map((part) => part.trim());
  if (parts.some((part) => part === "")) return undefined;
  const first = parts[0];
  return parts.every((part) => part.toLowerCase() === first.toLowerCase()) ? first : undefined;
}

const DECIMAL = /^(?:\d+\.?\d*|\.\d+)$/;

/** A diameter claim ("0.4", "1.75,1.75") as millimetres, or `undefined`
 *  when it isn't one agreed, valid (D16) number. */
export function parseDiameterClaim(raw: string): number | undefined {
  const value = singleValue(raw);
  if (value === undefined || !DECIMAL.test(value)) return undefined;
  const parsed = Number(value);
  return validDiameter(parsed) ? parsed : undefined;
}

export interface MaterialValue {
  family: MaterialFamily;
  /** Only for `OTHER`: the file's own name for it. */
  other?: string;
}

/** A `filament_type` claim as a material. A known family matches
 *  regardless of case; anything else is `OTHER` with the file's name, if
 *  that name fits the Spool rule (1–32 characters). */
export function parseMaterialClaim(raw: string): MaterialValue | undefined {
  const value = singleValue(raw);
  if (value === undefined) return undefined;
  const family = MATERIAL_FAMILIES.find((candidate) => candidate !== "OTHER" && candidate.toLowerCase() === value.toLowerCase());
  if (family) return { family };
  return validMaterialOther(value) ? { family: "OTHER", other: value } : undefined;
}

/** The catalog model a `printer_model` claim names: exactly one model
 *  whose name matches, ignoring case. */
export function matchCatalogModel(claim: string, models: readonly CatalogModelSummary[]): CatalogModelSummary | undefined {
  const wanted = claim.trim().toLowerCase();
  if (!wanted) return undefined;
  const matches = models.filter((model) => model.model.toLowerCase() === wanted);
  return matches.length === 1 ? matches[0] : undefined;
}

/** The nozzle variant the file names: its machine preset
 *  (`printer_settings_id`) if that is a variant's name, else the variant
 *  for its nozzle claim, else the only variant. `undefined` when the file
 *  doesn't say which. */
export function matchCatalogVariant(
  variants: readonly CatalogVariantSummary[],
  claims: { settingsId?: string; nozzleMm?: number },
): CatalogVariantSummary | undefined {
  const byName = claims.settingsId === undefined
    ? undefined
    : variants.find((variant) => variant.variant === claims.settingsId!.trim());
  if (byName) return byName;
  if (claims.nozzleMm !== undefined) {
    const byNozzle = variants.filter((variant) => Number.parseFloat(variant.printerVariant) === claims.nozzleMm);
    if (byNozzle.length === 1) return byNozzle[0];
  }
  return variants.length === 1 ? variants[0] : undefined;
}

// --- Validation (D16, the same rules the backend applies) -----------------------

function validDiameter(value: number): boolean {
  return Number.isFinite(value) && value > 0 && value <= 5;
}

function validMaterialOther(value: string): boolean {
  const length = [...value.trim()].length;
  return length >= 1 && length <= 32;
}

export const FACT_ERRORS = {
  nozzleDiameterMm: "Enter the nozzle diameter in millimetres: more than 0, at most 5.",
  filamentDiameterMm: "Enter the filament diameter in millimetres: more than 0, at most 5.",
  materialOther: "Name the material in 1 to 32 characters.",
} as const;

function diameterFact(text: string): { ok: true; value?: number } | { ok: false } {
  const trimmed = text.trim();
  if (trimmed === "") return { ok: true };
  if (!DECIMAL.test(trimmed)) return { ok: false };
  const value = Number(trimmed);
  return validDiameter(value) ? { ok: true, value } : { ok: false };
}

export type FactErrors = Partial<Record<FactField, string>>;

export type FactsValidation =
  | { ok: true; facts: CreateExternalSliceRevisionFacts }
  | { ok: false; errors: FactErrors };

/** Checks the draft and, if it passes, builds the request's `facts`. An
 *  empty fact is `absent`; `materialOther` is sent only for `OTHER`. */
export function validateFacts(draft: FactsDraft): FactsValidation {
  const errors: FactErrors = {};
  const nozzle = diameterFact(draft.nozzleDiameterMm);
  if (!nozzle.ok) errors.nozzleDiameterMm = FACT_ERRORS.nozzleDiameterMm;
  const filament = diameterFact(draft.filamentDiameterMm);
  if (!filament.ok) errors.filamentDiameterMm = FACT_ERRORS.filamentDiameterMm;
  const other = draft.materialFamily === "OTHER" ? draft.materialOther.trim() : undefined;
  if (other !== undefined && !validMaterialOther(other)) errors.materialOther = FACT_ERRORS.materialOther;
  if (!nozzle.ok || !filament.ok || errors.materialOther) return { ok: false, errors };

  const confirmed = <T,>(value: T | null | undefined) =>
    value === null || value === undefined ? { kind: "absent" as const } : { kind: "confirmed" as const, value };
  return {
    ok: true,
    facts: {
      printerProfile: confirmed(draft.printerProfile),
      nozzleDiameterMm: confirmed(nozzle.value),
      materialFamily: confirmed(draft.materialFamily),
      ...(other !== undefined ? { materialOther: other } : {}),
      filamentDiameterMm: confirmed(filament.value),
    },
  };
}

/** How many facts the draft leaves absent (`materialOther` qualifies the
 *  material and isn't a fact of its own). */
export function absentFactCount(draft: FactsDraft): number {
  return [
    draft.printerProfile === null,
    draft.nozzleDiameterMm.trim() === "",
    draft.materialFamily === null,
    draft.filamentDiameterMm.trim() === "",
  ].filter(Boolean).length;
}

/** The field a backend `VALIDATION` error's `fieldPath` names
 *  (`facts.nozzleDiameterMm`, …), if the dialog has it. */
export function factFieldAt(fieldPath: unknown): FactField | undefined {
  if (typeof fieldPath !== "string" || !fieldPath.startsWith("facts.")) return undefined;
  const field = fieldPath.slice("facts.".length).split(".")[0];
  return (FACT_FIELDS as readonly string[]).includes(field) ? (field as FactField) : undefined;
}
