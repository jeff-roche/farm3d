/** A pure reducer for the import dialog's rows (spec §Frontend State):
 *  inspection -> per-row choices -> request -> results merged by
 *  `fileIndex`. The dialog keeps the rows as component-local state; nothing
 *  here touches `library-store`. */
import type {
  DuplicateAction,
  DuplicateMatch,
  ImportCandidate,
  ImportInspection,
  ImportItemRequest,
  ImportItemResult,
  ImportModelsResult,
  ModelRecord,
} from "./types";

export type ImportRow = {
  fileIndex: number;
  candidate: ImportCandidate;
  name: string;
  /** `[]` is Unfiled. */
  projectIds: string[];
  storageMode: "managed" | "linked";
  duplicateAction?: DuplicateAction;
  /** The `addRevision`/`useExisting` target. May be pre-filled with a
   *  same-name Model (D14) without any action being chosen. */
  targetModelId?: string;
  /** Set by `setRowTarget`: the user chose the target, so a Projects or
   *  name change no longer replaces it with a suggestion. */
  targetPickedByUser?: boolean;
  acknowledgeUnsupported: boolean;
  result?: ImportItemResult;
};

export type RowBlocker = "duplicateDecision" | "acknowledgeUnsupported" | "name" | "rejected";

type ReadyCandidate = Extract<ImportCandidate, { status: "ready" }>;

const MAX_MODEL_NAME_CHARS = 255;
const COMMITTED_OUTCOMES: ReadonlySet<string> = new Set(["imported", "revisionAdded", "reusedExisting"]);

function nameFromFile(fileName: string): string {
  const dot = fileName.lastIndexOf(".");
  return (dot > 0 ? fileName.slice(0, dot) : fileName).trim();
}

/** D14's same-name suggestion: a Model named the same (case-insensitively)
 *  in any of the row's Projects -- or Unfiled, for a row with none -- that
 *  could take the file as a new revision (managed, same format). The most
 *  recently updated wins. */
function sameNameTarget(
  name: string,
  format: ReadyCandidate["format"],
  projectIds: string[],
  models: ModelRecord[],
): string | undefined {
  const wanted = name.trim().toLocaleLowerCase();
  const inScope = (model: ModelRecord) => projectIds.length === 0
    ? model.projectIds.length === 0
    : model.projectIds.some((id) => projectIds.includes(id));
  const matches = models.filter((model) =>
    model.name.trim().toLocaleLowerCase() === wanted
    && model.storageMode === "managed"
    && model.format === format
    && inScope(model));
  matches.sort((a, b) => Date.parse(b.updatedAt) - Date.parse(a.updatedAt));
  return matches[0]?.id;
}

export function rowsFromInspection(
  inspection: ImportInspection,
  ctx: { projectIds: string[]; models: ModelRecord[] },
): ImportRow[] {
  return inspection.items.map((candidate) => {
    const name = nameFromFile(candidate.fileName);
    const row: ImportRow = {
      fileIndex: candidate.fileIndex,
      candidate,
      name,
      projectIds: [...ctx.projectIds],
      storageMode: "managed",
      acknowledgeUnsupported: false,
    };
    if (candidate.status === "ready") {
      const target = sameNameTarget(name, candidate.format, ctx.projectIds, ctx.models);
      if (target !== undefined) row.targetModelId = target;
    }
    return row;
  });
}

/** Re-derives the D14 suggestion from the row's current name and Projects,
 *  clearing it when nothing matches. A target the user owns -- picked
 *  directly, or held once they chose Add as a new revision -- is kept. The
 *  suggestion never chooses the action. */
function withSuggestion(row: ImportRow, models: ModelRecord[]): ImportRow {
  if (row.candidate.status !== "ready" || row.targetPickedByUser || row.duplicateAction === "addRevision") return row;
  const { targetModelId: _previous, ...rest } = row;
  const target = sameNameTarget(row.name, row.candidate.format, row.projectIds, models);
  return target === undefined ? rest : { ...rest, targetModelId: target };
}

export function setRowProjects(row: ImportRow, projectIds: string[], models: ModelRecord[]): ImportRow {
  if (row.candidate.status === "rejected") return row;
  return withSuggestion({ ...row, projectIds: [...projectIds] }, models);
}

export function setRowName(row: ImportRow, name: string, models: ModelRecord[]): ImportRow {
  if (row.candidate.status === "rejected") return row;
  return withSuggestion({ ...row, name }, models);
}

/** The row's resolution: a D14 duplicate action, `addRevision` for any
 *  ready row, or `undefined` for a plain new Model. Clearing
 *  `addRevision` lets the same-name suggestion follow the row again. */
export function setRowAction(row: ImportRow, action: DuplicateAction | undefined, models: ModelRecord[]): ImportRow {
  if (row.candidate.status === "rejected") return row;
  const { duplicateAction: _previous, ...rest } = row;
  return withSuggestion(action === undefined ? rest : { ...rest, duplicateAction: action }, models);
}

export function setRowStorage(row: ImportRow, storageMode: ImportRow["storageMode"]): ImportRow {
  if (row.candidate.status === "rejected") return row;
  return { ...row, storageMode };
}

export function setRowAcknowledged(row: ImportRow, acknowledgeUnsupported: boolean): ImportRow {
  if (row.candidate.status === "rejected") return row;
  return { ...row, acknowledgeUnsupported };
}

/** The user's own target choice (the Add as a new revision Model picker). */
export function setRowTarget(row: ImportRow, modelId: string): ImportRow {
  return { ...row, targetModelId: modelId, targetPickedByUser: true };
}

export function applyProjectsToAll(rows: ImportRow[], projectIds: string[], models: ModelRecord[]): ImportRow[] {
  return rows.map((row) => setRowProjects(row, projectIds, models));
}

/** `useExisting` must name a Model that holds these bytes. With a single
 *  duplicate that is the one shown; with several, only a duplicate the
 *  user picked themselves (D14: never silent) -- until then there is none. */
function useExistingTarget(row: ImportRow): string | undefined {
  if (row.candidate.status !== "ready") return undefined;
  const { duplicates } = row.candidate;
  if (duplicates.length === 1) return duplicates[0]!.modelId;
  const picked = row.targetPickedByUser && duplicates.some((match) => match.modelId === row.targetModelId);
  return picked ? row.targetModelId : undefined;
}

/** The commit found the file's content already in the Library although
 *  inspection listed no duplicate -- two identical files in one selection,
 *  or a racing import (D14: never silent). */
export function decisionRequiredAtCommit(row: ImportRow): boolean {
  return row.result?.errors.some((error) => error.code === "DUPLICATE_DECISION_REQUIRED") ?? false;
}

export function rowBlockers(row: ImportRow): RowBlocker[] {
  if (row.candidate.status === "rejected") return ["rejected"];
  const blockers: RowBlocker[] = [];
  const undecided =
    ((row.candidate.duplicates.length > 0 || decisionRequiredAtCommit(row)) && row.duplicateAction === undefined)
    || (row.duplicateAction === "addRevision" && row.targetModelId === undefined)
    || (row.duplicateAction === "useExisting" && useExistingTarget(row) === undefined);
  if (undecided) blockers.push("duplicateDecision");
  if (row.candidate.unsupported.length > 0 && !row.acknowledgeUnsupported) blockers.push("acknowledgeUnsupported");
  const nameLength = [...row.name.trim()].length;
  if (nameLength === 0 || nameLength > MAX_MODEL_NAME_CHARS) blockers.push("name");
  return blockers;
}

function succeeded(row: ImportRow): boolean {
  return row.result !== undefined && COMMITTED_OUTCOMES.has(row.result.outcome);
}

/** Only unblocked rows without a successful outcome, so a retry after
 *  `mergeResults` resends just the rows that still need committing. */
export function buildRequest(rows: ImportRow[], models: ModelRecord[]): ImportItemRequest[] {
  return rows
    .filter((row) => !succeeded(row) && rowBlockers(row).length === 0)
    .map((row) => {
      const item: ImportItemRequest = {
        fileIndex: row.fileIndex,
        name: row.name.trim(),
        projectIds: [...new Set(row.projectIds)],
        storageMode: row.storageMode,
        acknowledgeUnsupported: row.acknowledgeUnsupported,
      };
      if (row.duplicateAction !== undefined) item.duplicateAction = row.duplicateAction;
      if (row.duplicateAction === "useExisting") item.targetModelId = useExistingTarget(row);
      if (row.duplicateAction === "addRevision" && row.targetModelId !== undefined) {
        item.targetModelId = row.targetModelId;
        const target = models.find((model) => model.id === row.targetModelId);
        if (target) item.targetExpectedRevision = target.revision;
      }
      return item;
    });
}

/** The held Models whose current revision has these bytes, as duplicate
 *  matches. Only current revisions are visible here, so a match on an
 *  older revision stays unknown and Use existing is not offered for it. */
function libraryDuplicates(sha256: string, models: ModelRecord[]): DuplicateMatch[] {
  return models
    .filter((model) => model.currentRevision.sha256 === sha256)
    .map((model) => ({
      modelId: model.id,
      modelName: model.name,
      projectIds: [...model.projectIds],
      revisionId: model.currentRevision.id,
      sequence: model.currentRevision.sequence,
      isCurrent: true,
    }));
}

/** Attaches each item's outcome to its row by `fileIndex`, keeping every
 *  choice the user made so a failed row can be retried as it was. A row
 *  sent back with `DUPLICATE_DECISION_REQUIRED` that inspection listed no
 *  duplicates for gets them from `models` (which already hold this
 *  import's own results), so the user can choose Use existing. */
export function mergeResults(rows: ImportRow[], result: ImportModelsResult, models: ModelRecord[]): ImportRow[] {
  const byIndex = new Map(result.items.map((item) => [item.fileIndex, item]));
  return rows.map((row) => {
    const item = byIndex.get(row.fileIndex);
    if (!item) return row;
    const merged: ImportRow = { ...row, result: item };
    if (merged.candidate.status === "ready" && merged.candidate.duplicates.length === 0 && decisionRequiredAtCommit(merged)) {
      merged.candidate = { ...merged.candidate, duplicates: libraryDuplicates(merged.candidate.sha256, models) };
    }
    return merged;
  });
}
