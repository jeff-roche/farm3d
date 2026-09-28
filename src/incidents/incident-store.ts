/** `list_incidents`, `get_incident`, and `add_incident_note`: read/write
 *  wrappers, not a reconciled store -- an Incident's live truth (the open
 *  list, and revision-gated upserts) lives in `attention-store.ts`; this
 *  module is the paged history browser and the detail fetcher it drives
 *  (spec "Frontend architecture"). Web mode is served from
 *  `attention/web-fixtures.ts`, the same fixture `attention-store.ts`
 *  loads, so an Incident id from one resolves in the other. */
import { command, desktopAvailable, needsDesktopError, retryOnTransportFailure } from "../ipc/client";
import { onAttentionIncidentChanged } from "../attention/attention-store";
import type { CommandError } from "../generated/contracts/command/CommandError";
import type { IncidentDetail, IncidentPage } from "../attention/types";

function notFound(what: string): CommandError {
  return { contractVersion: 1, code: "NOT_FOUND", message: `${what} was not found.`, recovery: [], retryable: false };
}

export interface ListIncidentsOptions {
  state?: "open" | "closed" | "all";
  printerId?: string;
  before?: string;
  limit?: number;
}

/** `list_incidents`: `openedAt` descending, then id (Rust's own order --
 *  this never re-sorts it). */
export function listIncidents(options: ListIncidentsOptions = {}): Promise<IncidentPage> {
  if (!desktopAvailable()) {
    return import("../attention/web-fixtures").then(({ webIncidentPage }) => webIncidentPage(options));
  }
  return retryOnTransportFailure(() => command("list_incidents", options));
}

/** `get_incident`: the merged timeline, linked Events, and snapshots. */
export function getIncident(incidentId: string): Promise<IncidentDetail> {
  if (!desktopAvailable()) {
    return import("../attention/web-fixtures").then(({ webIncidentDetail }) => {
      const detail = webIncidentDetail(incidentId);
      if (!detail) throw notFound(`Incident ${incidentId}`);
      return detail;
    });
  }
  return retryOnTransportFailure(() => command("get_incident", { incidentId }));
}

/** `add_incident_note`: one user-initiated write, a fresh `operationId`
 *  per call (mirrors `queue-store.ts`'s `write`). Web mode has no backend
 *  to write to and refuses. */
export async function addIncidentNote(incidentId: string, text: string): Promise<IncidentDetail> {
  if (!desktopAvailable()) throw needsDesktopError("Adding an Incident note");
  const operationId = crypto.randomUUID();
  return retryOnTransportFailure(() => command("add_incident_note", { operationId, incidentId, text }));
}

/** Spec "Events": "An open Incident's detail is refetched with
 *  `get_incident` when one with a higher revision arrives." `onChange`
 *  fires with the freshly fetched detail; a fetch failure goes to
 *  `onError` instead (default: dropped -- the caller decides whether a
 *  stale detail is worse than none). The caller owns the subscription's
 *  lifetime (a detail dock mounting/unmounting, Task 13/14). */
export function watchIncidentDetail(
  incidentId: string,
  onChange: (detail: IncidentDetail) => void,
  onError: (error: unknown) => void = () => {},
): () => void {
  return onAttentionIncidentChanged((incident) => {
    if (incident.id !== incidentId) return;
    getIncident(incidentId).then(onChange).catch(onError);
  });
}
