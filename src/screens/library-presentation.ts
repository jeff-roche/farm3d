/** Display labels for the Library screens. Pure: no store access. */
import type { LibraryView, SavedViewId } from "../library/saved-views";
import type { ModelRecord, ProjectRecord, RevisionOrigin, SourceState } from "../library/types";

export type ModelFormatLabel = "STL" | "3MF" | "G-code";

export const SAVED_VIEW_LABELS: Record<SavedViewId, string> = {
  all: "All Models",
  unfiled: "Unfiled",
  recent: "Recently added",
  attention: "Needs attention",
  gcode: "Pre-sliced G-code",
};

const FORMAT_LABELS: Record<ModelRecord["format"], ModelFormatLabel> = {
  stl: "STL",
  "3mf": "3MF",
  gcode: "G-code",
};

export function formatLabel(format: ModelRecord["format"]): ModelFormatLabel {
  return FORMAT_LABELS[format];
}

export function storageLabel(model: ModelRecord): string {
  return model.storageMode === "linked" ? "Linked" : "Managed";
}

const SOURCE_STATE: Record<SourceState, { label: string; severity: "warning" | "info" | "resolved" }> = {
  ok: { label: "Source OK", severity: "resolved" },
  missing: { label: "Source missing", severity: "warning" },
  unreadable: { label: "Source unreadable", severity: "warning" },
  notAFile: { label: "Source is not a file", severity: "warning" },
  invalidContent: { label: "Source content is invalid", severity: "warning" },
  changing: { label: "Source is changing", severity: "info" },
};

export function sourceStateMarker(state: SourceState): { label: string; severity: "warning" | "info" | "resolved" } {
  return SOURCE_STATE[state];
}

const ORIGIN_LABELS: Record<RevisionOrigin, string> = {
  import: "Imported",
  linkedChange: "Updated from source",
  relocate: "Relinked",
  addedRevision: "Added revision",
};

export function originLabel(origin: RevisionOrigin): string {
  return ORIGIN_LABELS[origin];
}

export function viewLabel(view: LibraryView, projects: ProjectRecord[]): string {
  if (view.kind === "view") return SAVED_VIEW_LABELS[view.id];
  return projects.find((project) => project.id === view.id)?.name ?? "this Project";
}

/** Names of the Model's Projects, in its `projectIds` order (by name). */
export function projectNames(model: ModelRecord, projects: ProjectRecord[]): string[] {
  return model.projectIds.flatMap((id) => {
    const name = projects.find((project) => project.id === id)?.name;
    return name ? [name] : [];
  });
}

/** The first two names, then `+N` for the rest. */
export function projectsSummary(model: ModelRecord, projects: ProjectRecord[]): string {
  const names = projectNames(model, projects);
  const shown = names.slice(0, 2).join(", ");
  return names.length > 2 ? `${shown} +${names.length - 2}` : shown;
}

export function shortHash(sha256: string): string {
  return sha256.slice(0, 12);
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KiB", "MiB", "GiB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(1)} ${units[unit]}`;
}

export function formatDate(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleDateString(undefined, { dateStyle: "medium" });
}
