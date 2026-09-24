/** A pure reducer for the import dialog's rows (spec §Frontend State):
 *  inspection -> per-row choices -> request -> results merged by
 *  `fileIndex`. The dialog keeps the rows as component-local state; nothing
 *  here touches `library-store`. */
import type {
  DuplicateAction,
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

export function applyProjectsToAll(rows: ImportRow[], projectIds: string[]): ImportRow[] {
  return rows.map((row) => (row.candidate.status === "rejected" ? row : { ...row, projectIds: [...projectIds] }));
}

/** `useExisting` must name a Model that holds these bytes: the chosen
 *  target when it is one of the duplicates, otherwise the duplicate whose
 *  current revision matches, otherwise the first. */
function useExistingTarget(row: ImportRow): string | undefined {
  if (row.candidate.status !== "ready") return undefined;
  const { duplicates } = row.candidate;
  if (duplicates.some((match) => match.modelId === row.targetModelId)) return row.targetModelId;
  return (duplicates.find((match) => match.isCurrent) ?? duplicates[0])?.modelId;
}

export function rowBlockers(row: ImportRow): RowBlocker[] {
  if (row.candidate.status === "rejected") return ["rejected"];
  const blockers: RowBlocker[] = [];
  const undecided =
    (row.candidate.duplicates.length > 0 && row.duplicateAction === undefined)
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

/** Attaches each item's outcome to its row by `fileIndex`, keeping every
 *  choice the user made so a failed row can be retried as it was. */
export function mergeResults(rows: ImportRow[], result: ImportModelsResult): ImportRow[] {
  const byIndex = new Map(result.items.map((item) => [item.fileIndex, item]));
  return rows.map((row) => {
    const item = byIndex.get(row.fileIndex);
    return item ? { ...row, result: item } : row;
  });
}
