import type { MaterialFamily } from "../generated/contracts/domain/MaterialFamily";

/** D2's closed material-family enum, in the order it's offered in every
 *  picker (facet chips, the Add/Edit form's family select) -- alphabetical
 *  by base polymer, composites immediately after their base, OTHER last. */
export const MATERIAL_FAMILIES: MaterialFamily[] = [
  "PLA", "PLA-CF", "PETG", "PET-CF", "ABS", "ASA", "TPU",
  "PA", "PA-CF", "PC", "PVA", "HIPS", "PP", "OTHER",
];

const FAMILY_LABELS: Record<MaterialFamily, string> = {
  PLA: "PLA",
  "PLA-CF": "PLA-CF",
  PETG: "PETG",
  "PET-CF": "PET-CF",
  ABS: "ABS",
  ASA: "ASA",
  TPU: "TPU",
  PA: "PA (Nylon)",
  "PA-CF": "PA-CF (Nylon)",
  PC: "PC",
  PVA: "PVA",
  HIPS: "HIPS",
  PP: "PP",
  OTHER: "Other",
};

/** The D2 display label for a family on its own (independent of any
 *  particular Spool's `materialOther` text). */
export function materialFamilyLabel(family: MaterialFamily): string {
  return FAMILY_LABELS[family];
}

/** The label to show for a Spool's material: the family label, except for
 *  `OTHER`, which shows the Spool's own `materialOther` text (D2: required,
 *  1-32 chars trimmed, for every `OTHER` Spool) and falls back to the
 *  generic label only if that text is missing or blank. */
export function materialLabel(family: MaterialFamily, materialOther?: string): string {
  if (family !== "OTHER") return materialFamilyLabel(family);
  const trimmed = materialOther?.trim();
  return trimmed ? trimmed : "Other";
}
