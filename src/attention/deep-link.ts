/** P8 decision 8's deep-link targets: where opening an Attention Event or
 *  an Incident goes, in F1's `NavigationTarget` format. Mirrors
 *  `src-tauri/src/attention/deep_link.rs`'s table exactly:
 *
 *  | Source | Target |
 *  |---|---|
 *  | Printer | `monitor/printer/<id>` |
 *  | Job, or a Reconciliation Requirement's Job | `queue/job/<id>` |
 *  | Spool | `spools/spool/<id>` |
 *  | (source gone) | `monitor/attention/<eventId>` |
 *
 *  An archived Printer stays a valid `monitor/printer` target -- Rust's
 *  `source_exists` checks only that the row exists, not whether it's
 *  archived, and this module does the same: it trusts whatever
 *  `availableIds` the caller passes (which, for a Printer, includes
 *  archived ones; `App.tsx`'s `navigationContext`). An Incident (not an
 *  Event's source, but its own navigable object) is
 *  `monitor/incident/<id>`, both are tested against the same rows. */
import type { NavigationTarget } from "../navigation/navigation-store";
import type { AttentionEvent } from "./types";

type SelectionKind = NonNullable<NavigationTarget["selection"]>["kind"];

function target(destination: NavigationTarget["destination"], kind: SelectionKind, id: string): NavigationTarget {
  return { version: 1, destination, selection: { kind, id } };
}

/** `monitor/attention/<eventId>`: the Event itself. */
export function eventTarget(event: AttentionEvent): NavigationTarget {
  return target("monitor", "attention", event.id);
}

/** `monitor/incident/<id>`: an Incident, on its own (not derived from an
 *  Event's source). */
export function incidentTarget(incidentId: string): NavigationTarget {
  return target("monitor", "incident", incidentId);
}

/** The Event's source's target, assuming it still exists. A requirement
 *  opens its Job; one without a Job (never in practice) opens the Event. */
export function sourceTarget(event: AttentionEvent): NavigationTarget {
  switch (event.source.kind) {
    case "printer":
      return target("monitor", "printer", event.source.id);
    case "job":
      return target("queue", "job", event.source.id);
    case "reconciliationRequirement":
      return event.jobId !== null ? target("queue", "job", event.jobId) : eventTarget(event);
    case "spool":
      return target("spools", "spool", event.source.id);
  }
}

/** Decision 8, at click time: the source's target when the source row
 *  still exists, otherwise the Event itself. Mirrors
 *  `attention::deep_link::target_for`. */
export function targetForSource(event: AttentionEvent, sourceExists: boolean): NavigationTarget {
  return sourceExists ? sourceTarget(event) : eventTarget(event);
}

/** The id `sourceTarget` would resolve, so a caller can check it against
 *  what it already knows exists (mirrors Rust's `source_exists`, but from
 *  in-memory state rather than a fresh row check -- only the backend can
 *  answer that authoritatively; this is the frontend's best local guess,
 *  used only to route a click, never to decide anything persisted). */
export function sourceId(event: AttentionEvent): string | null {
  return event.source.kind === "reconciliationRequirement" ? event.jobId : event.source.id;
}

/** `openTargetFor`: the target a click on `event` should navigate to,
 *  given the ids the frontend currently knows about (`App.tsx`'s
 *  `navigationContext.availableIds`). Falls back to the Event itself when
 *  the source id isn't among them. */
export function openTargetFor(event: AttentionEvent, availableIds: string[]): NavigationTarget {
  const id = sourceId(event);
  return targetForSource(event, id !== null && availableIds.includes(id));
}
