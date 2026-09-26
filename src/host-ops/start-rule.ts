/** D9: a pure mirror of `src-tauri/src/host_ops/start_rule.rs`'s Start
 *  table and control rule, over the live `PrinterStatus` the printer store
 *  holds. Nothing here calls a command; it only decides what the Start
 *  dialog and the Job tab's controls offer and why. */
import type { OperationalState, PriorState, PrinterStatus, TelemetryFreshness } from "./types";

export type StartOffer =
  | { offered: false; reason: string }
  | { offered: true; priorState: PriorState; confirmLabel: string };

export type ControlVerb = "pause" | "resume" | "cancel";

export interface ControlOffer {
  offered: boolean;
  reason?: string;
}

const UNRESOLVED_REASON = "A printer operation is pending.";
const UNRESOLVED_CONTROL_REASON =
  "A printer operation is pending. You can still pause or cancel on the printer itself.";
const FAILED_REASON = "Clear the error on the printer first.";

/** The short state name the backend's `START_NOT_ALLOWED`/
 *  `CONTROL_NOT_ALLOWED` messages end with (mirrors
 *  `host_ops::start_rule::state_label` exactly, so the two never drift).
 *  Telemetry that isn't fresh is named as such, since that, not the
 *  state, is the reason. */
export function stateLabel(state: OperationalState, freshness: TelemetryFreshness): string {
  if (freshness !== "fresh") return "its status is out of date";
  switch (state) {
    case "setupIncomplete": return "setup is incomplete";
    case "error": return "it reports an error";
    case "offline": return "it is offline";
    case "connecting": return "it is still connecting";
    case "unknown": return "its state is unknown";
    case "printing": return "it is printing";
    case "paused": return "it is paused";
    case "busy": return "it is busy";
    case "finished": return "the last print finished";
    case "cancelled": return "the last print was cancelled";
    case "failed": return "the last print failed";
    case "ready": return "it is ready";
  }
}

/** D9's Start table. Not offered while a Host Operation is unresolved,
 *  whatever the state (mirrors `start_staged_artifact`'s step 4, which
 *  runs before `start_rule::check`). */
export function startOffer(status: PrinterStatus, hasUnresolved: boolean): StartOffer {
  if (hasUnresolved) return { offered: false, reason: UNRESOLVED_REASON };
  const { operationalState: state, freshness } = status;
  if (freshness === "fresh") {
    if (state === "ready") return { offered: true, priorState: "ready", confirmLabel: "The bed is clear." };
    if (state === "finished") {
      return { offered: true, priorState: "finished", confirmLabel: "The previous print finished. The bed is clear." };
    }
    if (state === "cancelled") {
      return { offered: true, priorState: "cancelled", confirmLabel: "The previous print was cancelled. The bed is clear." };
    }
    if (state === "failed") return { offered: false, reason: FAILED_REASON };
  }
  return { offered: false, reason: stateLabel(state, freshness) };
}

/** D9's control rule: pause only from `printing`, resume only from
 *  `paused`, cancel from either, all with fresh telemetry. Not offered
 *  while a Host Operation is unresolved. */
export function controlOffer(status: PrinterStatus, verb: ControlVerb, hasUnresolved: boolean): ControlOffer {
  if (hasUnresolved) return { offered: false, reason: UNRESOLVED_CONTROL_REASON };
  const { operationalState: state, freshness } = status;
  const allowed = freshness === "fresh" && (
    (verb === "pause" && state === "printing")
    || (verb === "resume" && state === "paused")
    || (verb === "cancel" && (state === "printing" || state === "paused"))
  );
  if (allowed) return { offered: true };
  return { offered: false, reason: stateLabel(state, freshness) };
}
