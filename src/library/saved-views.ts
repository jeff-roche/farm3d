/** D19: pure view filtering, search, sort, and counts over the Library
 *  store's Models. Saved views are frontend filters, never stored. */
import type { ModelRecord, ProjectRecord, SavedViewId } from "./types";

export type { SavedViewId } from "./types";

export type LibraryView = { kind: "view"; id: SavedViewId } | { kind: "project"; id: string };

export const SAVED_VIEW_IDS: readonly SavedViewId[] = ["all", "unfiled", "recent", "attention", "gcode"];

/** "Recently added": the current revision was captured within this window,
 *  inclusive of its boundary. */
const RECENT_WINDOW_MS = 14 * 24 * 60 * 60 * 1000;

const VIEW_FILTERS: Record<SavedViewId, (model: ModelRecord, now: Date) => boolean> = {
  all: () => true,
  unfiled: (model) => model.projectIds.length === 0,
  recent: (model, now) =>
    now.getTime() - Date.parse(model.currentRevision.capturedAt) <= RECENT_WINDOW_MS,
  attention: (model) => model.storageMode === "linked" && model.link !== null && model.link.state !== "ok",
  gcode: (model) => model.format === "gcode",
};

export function modelsFor(view: LibraryView, models: ModelRecord[], now: Date): ModelRecord[] {
  if (view.kind === "project") return models.filter((model) => model.projectIds.includes(view.id));
  const keep = VIEW_FILTERS[view.id];
  return models.filter((model) => keep(model, now));
}

function basename(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/** Matches the Model name, any of its Project names, and a linked source's
 *  file name (not its directories), case-insensitively. */
export function searchModels(models: ModelRecord[], projects: ProjectRecord[], query: string): ModelRecord[] {
  const needle = query.trim().toLocaleLowerCase();
  if (needle === "") return models;
  const projectNames = new Map(projects.map((project) => [project.id, project.name.toLocaleLowerCase()]));
  return models.filter((model) =>
    model.name.toLocaleLowerCase().includes(needle)
    || model.projectIds.some((id) => projectNames.get(id)?.includes(needle))
    || (model.link !== null && basename(model.link.path).toLocaleLowerCase().includes(needle)));
}

/** Returns a sorted copy. "recent" is newest current revision first, the
 *  same capture time the Recently added view uses. */
export function sortModels(models: ModelRecord[], by: "name" | "recent"): ModelRecord[] {
  const byName = (a: ModelRecord, b: ModelRecord) =>
    a.name.localeCompare(b.name, undefined, { sensitivity: "base" }) || a.id.localeCompare(b.id);
  const byRecent = (a: ModelRecord, b: ModelRecord) =>
    Date.parse(b.currentRevision.capturedAt) - Date.parse(a.currentRevision.capturedAt) || byName(a, b);
  return [...models].sort(by === "name" ? byName : byRecent);
}

export function viewCounts(models: ModelRecord[], now: Date): Record<SavedViewId, number> {
  const counts = {} as Record<SavedViewId, number>;
  for (const id of SAVED_VIEW_IDS) counts[id] = modelsFor({ kind: "view", id }, models, now).length;
  return counts;
}
