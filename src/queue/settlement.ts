/** Formats a Job's material settlement preview (spec "Material
 *  settlement"; ruling R4). Rust computes `estimatedUseMg` -- this module
 *  only formats it, the same boundary `spools/weight.ts` is for every
 *  other weight in the app. */
import { formatGrams } from "../spools/weight";
import type { SettlementPreview } from "./types";

/** `Job.settlementPreview.estimatedUseMg`, formatted the way the Spool
 *  screens format weights: whole grams for a table, one decimal for a
 *  dialog (`precision`, default one decimal for the settle dialog, spec:
 *  "The dialog shows it before confirming."). */
export function estimatedUseText(estimatedUseMg: number, precision: 0 | 1 = 1): string {
  return formatGrams(estimatedUseMg, precision);
}

/** As `estimatedUseText`, but for the whole preview -- `null` while
 *  settlement isn't `pending`/`deferred` (ruling R4: nothing to preview). */
export function settlementPreviewText(preview: SettlementPreview | null, precision: 0 | 1 = 1): string | null {
  return preview ? estimatedUseText(preview.estimatedUseMg, precision) : null;
}
