import { Collapsible } from "@kobalte/core/collapsible";
import { IconAlertTriangle, IconChevronRight, IconCircleCheck, IconCircleX } from "@tabler/icons-solidjs";
import { createMemo, createSignal, For, Index, onCleanup, onMount, Show } from "solid-js";
import { createStore } from "solid-js/store";
import {
  Button,
  Checkbox,
  Combobox,
  Dialog,
  Progress,
  RadioGroup,
  Select,
  TextField,
  Stepper,
  type StepperStep,
} from "../design-system";
import { isCommandError } from "../ipc/client";
import {
  buildRequest,
  mergeResults,
  rowBlockers,
  rowsFromInspection,
  applyProjectsToAll,
  decisionRequiredAtCommit,
  setRowAcknowledged,
  setRowAction,
  setRowName,
  setRowProjects,
  setRowStorage,
  setRowTarget,
  type ImportRow,
  type RowBlocker,
} from "../library/import-flow";
import {
  cancelSelection,
  importModels,
  inspectSelection,
  library,
  onImportProgress,
  reportLibraryError,
} from "../library/library-store";
import type {
  DuplicateAction,
  DuplicateMatch,
  ImportCandidate,
  ImportItemResult,
  ImportSelectionSummary,
  ModelRecord,
  ProjectRecord,
} from "../library/types";
import { formatBytes, formatLabel } from "./library-presentation";
import styles from "./ImportDialog.module.css";

export interface ImportDialogProps {
  /** The selection to import. The dialog is open while this is non-null,
   *  and starts over (inspecting) whenever it changes. */
  selection: ImportSelectionSummary | null;
  /** The Projects new rows start in: the viewed Project, or none (Unfiled)
   *  in a saved view. */
  defaultProjectIds: string[];
  /** Closed without finishing (Cancel, Close, Escape). The dialog has
   *  already cancelled the selection. */
  onClose: () => void;
  /** **Done**, with the first imported Model to select. */
  onDone: (modelId: string | undefined) => void;
  /** The selection expired: pick the files again. */
  onChooseAgain: () => void;
  /** A window drop arrived while this import was open and was refused. */
  dropRefused?: boolean;
  /** Where focus goes when the dialog closes (Done, Cancel, Escape, or the
   *  close button) — the **Import…** button that opened it, while it's
   *  still in the document. See `Dialog`'s `returnFocus`. */
  returnFocus?: () => HTMLElement | null | undefined;
}

const COMMITTED: ReadonlySet<ImportItemResult["outcome"]> = new Set(["imported", "revisionAdded", "reusedExisting"]);
const EXPIRED_MESSAGE = "This selection has expired. Choose the files again to import them.";
const UNSUPPORTED_NOTE = "Kept in the stored file. farm3d won't use these when slicing.";
const GCODE_NOTE = "Stored as pre-sliced G-code. It won't be sliced.";
const NEW_MODEL = "newModel";
const ACTION_LABELS: Record<DuplicateAction, string> = {
  useExisting: "Use existing",
  addAnother: "Add as another Model",
  addRevision: "Add as a new revision of…",
};

type ReadyCandidate = Extract<ImportCandidate, { status: "ready" }>;
type Step = "files" | "review" | "results";

function plural(count: number, one: string, many: string): string {
  return `${count} ${count === 1 ? one : many}`;
}

function committed(row: ImportRow): boolean {
  return row.result !== undefined && COMMITTED.has(row.result.outcome);
}

function failed(row: ImportRow): boolean {
  return row.result !== undefined && !committed(row);
}

/** Why **Import** (or a row's **Retry**) is not available yet, one line
 *  per kind of blocker. */
function blockerLines(rows: ImportRow[]): string[] {
  const counts = new Map<RowBlocker, number>();
  for (const row of rows) {
    for (const blocker of rowBlockers(row)) counts.set(blocker, (counts.get(blocker) ?? 0) + 1);
  }
  const lines: string[] = [];
  const duplicates = counts.get("duplicateDecision");
  if (duplicates) lines.push(`Choose what to do with ${plural(duplicates, "duplicate file", "duplicate files")}.`);
  const unacknowledged = counts.get("acknowledgeUnsupported");
  if (unacknowledged) {
    lines.push(`Acknowledge the unsupported contents of ${plural(unacknowledged, "file", "files")}.`);
  }
  const unnamed = counts.get("name");
  if (unnamed) lines.push(`Give ${plural(unnamed, "file", "files")} a name of 1 to 255 characters.`);
  return lines;
}

function candidateDetail(candidate: ReadyCandidate): string {
  const summary = candidate.summary;
  const detail = summary.format === "stl"
    ? plural(summary.triangleCount, "triangle", "triangles")
    : summary.format === "3mf"
      ? plural(summary.objectCount, "object", "objects")
      : plural(summary.lineCount, "line", "lines");
  return `${formatLabel(candidate.format)} · ${formatBytes(candidate.sizeBytes)} · ${detail}`;
}

function outcomeText(result: ImportItemResult): string {
  const name = result.model?.name ?? "";
  switch (result.outcome) {
    case "imported":
      return `Imported as “${name}”`;
    case "revisionAdded":
      return result.revision
        ? `Added as revision ${result.revision.sequence} of “${name}”`
        : `Added as a new revision of “${name}”`;
    case "reusedExisting":
      return `Already in the Library as “${name}”`;
    case "cancelled":
      return "Cancelled";
    case "rejected":
      return "Not imported";
  }
}

/** D13's import flow over one selection: Files (inspection progress),
 *  Review (per-row choices), and Results (per-row outcomes, with Retry).
 *  Rows are component-local and only ever changed through
 *  `import-flow.ts`; nothing about them reaches `library-store`. */
export function ImportDialog(props: ImportDialogProps) {
  const cancel = (selectionId: string) => {
    // The dialog is gone by the time this settles, so a failure has no
    // inline place to show and goes to the Library banner.
    void cancelSelection(selectionId).catch(reportLibraryError);
  };
  const dismiss = () => {
    const selection = props.selection;
    if (selection) cancel(selection.selectionId);
    props.onClose();
  };

  return (
    <Dialog
      title="Import Models"
      open={props.selection !== null}
      onOpenChange={(open) => {
        if (!open) dismiss();
      }}
      returnFocus={props.returnFocus}
    >
      <Show when={props.selection} keyed>
        {(selection) => (
          <ImportFlow
            selection={selection}
            defaultProjectIds={props.defaultProjectIds}
            onCancel={dismiss}
            onDone={(modelId) => {
              // Frees the staging of any rows left uncommitted; a no-op
              // once every row has committed.
              cancel(selection.selectionId);
              props.onDone(modelId);
            }}
            onChooseAgain={props.onChooseAgain}
            dropRefused={props.dropRefused ?? false}
          />
        )}
      </Show>
    </Dialog>
  );
}

interface ImportFlowProps {
  selection: ImportSelectionSummary;
  defaultProjectIds: string[];
  onCancel: () => void;
  onDone: (modelId: string | undefined) => void;
  onChooseAgain: () => void;
  dropRefused: boolean;
}

function ImportFlow(props: ImportFlowProps) {
  const selectionId = props.selection.selectionId;
  const [step, setStep] = createSignal<Step>("files");
  const [inspecting, setInspecting] = createSignal(true);
  const [importing, setImporting] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [expired, setExpired] = createSignal(false);
  const [progress, setProgress] = createStore<Record<number, { done: number; total: number }>>({});
  const [state, setState] = createStore<{
    rows: ImportRow[];
    retrying: Record<number, boolean>;
    retryErrors: Record<number, string | undefined>;
  }>({ rows: [], retrying: {}, retryErrors: {} });
  let disposed = false;
  onCleanup(() => {
    disposed = true;
  });

  const updateRow = (fileIndex: number, change: (row: ImportRow) => ImportRow) => {
    setState("rows", (rows) => rows.map((row) => (row.fileIndex === fileIndex ? change(row) : row)));
  };

  const showFailure = (failure: unknown) => {
    if (isCommandError(failure) && failure.code === "SELECTION_EXPIRED") {
      setExpired(true);
      setError(null);
    } else {
      setError(isCommandError(failure) ? failure.message : "The files could not be imported.");
    }
  };

  const inspect = async () => {
    setInspecting(true);
    setError(null);
    try {
      const inspection = await inspectSelection(selectionId);
      if (disposed) return;
      setState("rows", rowsFromInspection(inspection, {
        projectIds: props.defaultProjectIds,
        models: library.models(),
      }));
      setStep("review");
    } catch (failure) {
      if (!disposed) showFailure(failure);
    } finally {
      if (!disposed) setInspecting(false);
    }
  };

  onMount(() => {
    const stop = onImportProgress((id, update) => {
      if (id === selectionId) setProgress(update.fileIndex, { done: update.bytesDone, total: update.bytesTotal });
    });
    onCleanup(stop);
    void inspect();
  });

  const importable = createMemo(() => state.rows.filter((row) => row.candidate.status === "ready"));
  const blockers = createMemo(() => {
    const lines = blockerLines(importable());
    return importable().length === 0 ? ["None of these files can be imported."] : lines;
  });

  // Each commit sends a fresh operationId: a transport retry inside
  // `importModels` reuses it, but a new attempt must not replay the last.
  const commit = async (rows: ImportRow[]) => {
    const items = buildRequest(rows, library.models());
    const result = await importModels(selectionId, items, crypto.randomUUID());
    if (!disposed) setState("rows", (current) => mergeResults(current, result, library.models()));
  };

  const importAll = async () => {
    setImporting(true);
    setError(null);
    setStep("results");
    try {
      await commit(state.rows);
    } catch (failure) {
      if (disposed) return;
      setStep("review");
      showFailure(failure);
    } finally {
      if (!disposed) setImporting(false);
    }
  };

  const retry = async (fileIndex: number) => {
    const row = state.rows.find((candidate) => candidate.fileIndex === fileIndex);
    if (!row) return;
    setState("retrying", fileIndex, true);
    setState("retryErrors", fileIndex, undefined);
    try {
      await commit([row]);
    } catch (failure) {
      if (disposed) return;
      if (isCommandError(failure) && failure.code === "SELECTION_EXPIRED") setExpired(true);
      setState("retryErrors", fileIndex, isCommandError(failure) ? failure.message : "The file could not be imported.");
    } finally {
      if (!disposed) setState("retrying", fileIndex, false);
    }
  };

  // One commit at a time: overlapping retries would meet the backend's
  // CONFLICT, and Done would cancel the selection mid-commit.
  const retryInFlight = () => Object.values(state.retrying).some(Boolean);

  const cancelImport = () => {
    void cancelSelection(selectionId).catch(showFailure);
  };

  const firstImported = () => state.rows.find((row) => committed(row) && row.result?.model)?.result?.model?.id;

  const steps = (): StepperStep[] => [
    { id: "files", label: "Files", ...(step() !== "files" ? { state: "complete" as const } : {}) },
    { id: "review", label: "Review", ...(step() === "results" ? { state: "complete" as const } : {}) },
    { id: "results", label: "Results" },
  ];

  const editor = (row: ImportRow, disabled: boolean) => (
    <RowEditor
      row={row}
      disabled={disabled}
      projects={library.projects()}
      models={library.models()}
      canApplyToAll={importable().length > 1 && step() === "review"}
      onChange={(change) => updateRow(row.fileIndex, change)}
      onApplyToAll={() => setState("rows", (rows) => applyProjectsToAll(rows, row.projectIds, library.models()))}
    />
  );

  return (
    <div class={styles.body}>
      <Stepper aria-label="Import steps" steps={steps()} current={step()} />

      <Show when={props.dropRefused}>
        <p class={styles.dropNotice} role="status">Finish or cancel this import before adding more files.</p>
      </Show>
      <Show when={expired()}>
        <div class={styles.notice} role="alert">
          <p class={styles.noticeText}>{EXPIRED_MESSAGE}</p>
          <Button variant="secondary" onClick={() => props.onChooseAgain()}>Choose files again</Button>
        </div>
      </Show>
      <Show when={error()}>
        {(message) => (
          <div class={styles.notice} role="alert">
            <p class={styles.noticeText}>{message()}</p>
            <Show when={step() === "files"}>
              <Button variant="secondary" onClick={() => void inspect()}>Try again</Button>
            </Show>
          </div>
        )}
      </Show>

      <Show when={step() === "files"}>
        <ul class={styles.rows} aria-label="Files">
          <For each={props.selection.files}>
            {(file) => {
              const value = () => {
                const entry = progress[file.fileIndex];
                return entry && entry.total > 0 ? Math.round((entry.done / entry.total) * 100) : 0;
              };
              return (
                <li class={styles.fileRow}>
                  <Progress label={file.fileName} value={value()} showValue />
                  <span class={styles.meta}>
                    {file.sizeBytes === null ? "Not a readable file" : formatBytes(file.sizeBytes)}
                  </span>
                </li>
              );
            }}
          </For>
        </ul>
        <div class={styles.footer}>
          <span class={styles.footerText}>{inspecting() ? "Checking the files…" : ""}</span>
          <Button variant="secondary" onClick={props.onCancel}>Cancel</Button>
        </div>
      </Show>

      <Show when={step() === "review"}>
        <ul class={styles.rows} aria-label="Files to import">
          <Index each={state.rows}>
            {(row) => (
              <li class={styles.row} aria-labelledby={`import-row-${selectionId}-${row().fileIndex}`}>
                <RowHeader row={row()} id={`import-row-${selectionId}-${row().fileIndex}`} />
                <Show
                  when={row().candidate.status === "ready"}
                  fallback={<RejectedReason row={row()} />}
                >
                  {editor(row(), false)}
                </Show>
              </li>
            )}
          </Index>
        </ul>
        <div class={styles.footer}>
          <p id={`import-blockers-${selectionId}`} class={styles.footerText}>
            <For each={blockers()}>{(line) => <span class={styles.blocker}>{line}</span>}</For>
          </p>
          <Button variant="secondary" onClick={props.onCancel}>Cancel</Button>
          <Button
            variant="primary"
            disabled={blockers().length > 0}
            aria-describedby={blockers().length > 0 ? `import-blockers-${selectionId}` : undefined}
            onClick={() => void importAll()}
          >
            Import
          </Button>
        </div>
      </Show>

      <Show when={step() === "results"}>
        <ul class={styles.rows} aria-label="Import results">
          <Index each={state.rows}>
            {(row) => (
              <li class={styles.row} aria-labelledby={`import-result-${selectionId}-${row().fileIndex}`}>
                <RowHeader row={row()} id={`import-result-${selectionId}-${row().fileIndex}`} />
                <Show when={row().candidate.status === "ready"} fallback={<RejectedReason row={row()} />}>
                  <Show
                    when={row().result}
                    fallback={<p class={styles.meta} role="status">Importing…</p>}
                  >
                    {(result) => <Outcome result={result()} />}
                  </Show>
                  <Show when={failed(row())}>
                    {editor(row(), state.retrying[row().fileIndex] === true)}
                    <Show when={state.retryErrors[row().fileIndex]}>
                      {(message) => <p class={styles.error} role="alert">{message()}</p>}
                    </Show>
                    <div class={styles.rowActions}>
                      <span class={styles.meta}>{blockerLines([row()]).join(" ")}</span>
                      <Button
                        variant="secondary"
                        size="sm"
                        disabled={rowBlockers(row()).length > 0 || retryInFlight()}
                        onClick={() => void retry(row().fileIndex)}
                      >
                        Retry
                      </Button>
                    </div>
                  </Show>
                </Show>
              </li>
            )}
          </Index>
        </ul>
        <div class={styles.footer}>
          <span class={styles.footerText}>{importing() ? "Importing…" : ""}</span>
          <Show
            when={!importing()}
            fallback={<Button variant="secondary" onClick={cancelImport}>Cancel</Button>}
          >
            <Button variant="primary" disabled={retryInFlight()} onClick={() => props.onDone(firstImported())}>
              Done
            </Button>
          </Show>
        </div>
      </Show>
    </div>
  );
}

function RowHeader(props: { row: ImportRow; id: string }) {
  return (
    <div class={styles.rowHeader}>
      <span id={props.id} class={styles.fileName}>{props.row.candidate.fileName}</span>
      <Show when={props.row.candidate.status === "ready" ? (props.row.candidate as ReadyCandidate) : undefined}>
        {(candidate) => <span class={styles.meta}>{candidateDetail(candidate())}</span>}
      </Show>
    </div>
  );
}

function RejectedReason(props: { row: ImportRow }) {
  const message = () => (props.row.candidate.status === "rejected" ? props.row.candidate.message : "");
  return (
    <p class={styles.rejected}>
      <IconCircleX size={14} aria-hidden="true" class={styles.dangerIcon} />
      <span>{message()}</span>
    </p>
  );
}

function Outcome(props: { result: ImportItemResult }) {
  const ok = () => COMMITTED.has(props.result.outcome);
  return (
    <div class={styles.outcome}>
      <p class={styles.outcomeLine}>
        <Show
          when={ok()}
          fallback={<IconCircleX size={14} aria-hidden="true" class={styles.dangerIcon} />}
        >
          <IconCircleCheck size={14} aria-hidden="true" class={styles.successIcon} />
        </Show>
        <span>{outcomeText(props.result)}</span>
      </p>
      <For each={props.result.errors}>{(error) => <p class={styles.error}>{error.message}</p>}</For>
      <Warnings messages={props.result.warnings.map((warning) => warning.message)} />
    </div>
  );
}

function Warnings(props: { messages: string[] }) {
  return (
    <Show when={props.messages.length > 0}>
      <ul class={styles.warnings}>
        <For each={props.messages}>
          {(message) => (
            <li class={styles.warning}>
              <IconAlertTriangle size={14} aria-hidden="true" class={styles.warningIcon} />
              <span>{message}</span>
            </li>
          )}
        </For>
      </ul>
    </Show>
  );
}

interface RowEditorProps {
  row: ImportRow;
  disabled: boolean;
  projects: ProjectRecord[];
  models: ModelRecord[];
  canApplyToAll: boolean;
  onChange: (change: (row: ImportRow) => ImportRow) => void;
  onApplyToAll: () => void;
}

/** One ready row's choices. Every change goes through `import-flow.ts`, so
 *  the same-name suggestion stays current. */
function RowEditor(props: RowEditorProps) {
  const candidate = () => props.row.candidate as ReadyCandidate;
  const chosenProjects = () => props.projects.filter((project) => props.row.projectIds.includes(project.id));
  // D14: only a managed Model of the same format can take a new revision.
  const revisionTargets = () => props.models.filter((model) =>
    model.storageMode === "managed" && model.format === candidate().format);
  const target = () => props.models.find((model) => model.id === props.row.targetModelId);
  const duplicates = () => candidate().duplicates;
  const action = () => props.row.duplicateAction;
  const createsModel = () => action() === undefined || action() === "addAnother";

  const setAction = (value: string) => {
    const next = value === NEW_MODEL ? undefined : (value as DuplicateAction);
    props.onChange((row) => setRowAction(row, next, props.models));
  };

  const duplicateOptions = () => {
    const actions: DuplicateAction[] = duplicates().length > 0
      ? ["useExisting", "addAnother", "addRevision"]
      : ["addAnother", "addRevision"];
    return actions.map((value) => ({ value, label: ACTION_LABELS[value] }));
  };

  return (
    <div class={styles.editor}>
      <Warnings messages={candidate().warnings.map((warning) => warning.message)} />
      <div class={styles.fields}>
        <TextField
          label="Name"
          value={props.row.name}
          disabled={props.disabled}
          onChange={(name) => props.onChange((row) => setRowName(row, name, props.models))}
        />
        <div class={styles.projects}>
          <Combobox<ProjectRecord>
            multiple
            label="Projects"
            placeholder={props.row.projectIds.length === 0 ? "Unfiled" : undefined}
            options={props.projects}
            optionValue={(project) => project.id}
            optionLabel={(project) => project.name}
            value={chosenProjects()}
            disabled={props.disabled}
            onChange={(projects) =>
              props.onChange((row) => setRowProjects(row, projects.map((project) => project.id), props.models))}
          />
          <Show when={props.canApplyToAll}>
            <Button variant="ghost" size="sm" disabled={props.disabled} onClick={props.onApplyToAll}>
              Apply Projects to all rows
            </Button>
          </Show>
        </div>
      </div>

      <Show
        when={duplicates().length > 0 || decisionRequiredAtCommit(props.row)}
        fallback={
          <Show when={revisionTargets().length > 0}>
            <RadioGroup
              label="Import as"
              class={styles.choice}
              options={[
                { value: NEW_MODEL, label: "New Model" },
                { value: "addRevision", label: ACTION_LABELS.addRevision },
              ]}
              value={action() === "addRevision" ? "addRevision" : NEW_MODEL}
              disabled={props.disabled}
              onChange={setAction}
            />
          </Show>
        }
      >
        <div class={styles.duplicate}>
          <p class={styles.duplicateText}>
            <IconAlertTriangle size={14} aria-hidden="true" class={styles.warningIcon} />
            <span>
              {duplicates().length > 0
                ? `The Library already has this file as ${duplicates().map((match) => `“${match.modelName}”`).join(", ")}.`
                : "The Library already has this file."}
            </span>
          </p>
          <RadioGroup
            label="Duplicate file"
            class={styles.choice}
            options={duplicateOptions()}
            // `""` matches no option, so nothing is pre-selected (D14).
            value={action() ?? ""}
            disabled={props.disabled}
            onChange={setAction}
          />
          <Show when={action() === "useExisting" && duplicates().length > 1}>
            <Select<DuplicateMatch>
              label="Existing Model"
              placeholder="Choose a Model"
              options={duplicates()}
              optionValue={(match) => match.modelId}
              optionLabel={(match) => match.modelName}
              value={duplicates().find((match) => match.modelId === props.row.targetModelId)}
              disabled={props.disabled}
              onChange={(match) => props.onChange((row) => setRowTarget(row, match.modelId))}
            />
          </Show>
        </div>
      </Show>

      <Show when={action() === "addRevision"}>
        <Combobox<ModelRecord>
          label="Model"
          placeholder="Choose a Model"
          options={revisionTargets()}
          optionValue={(model) => model.id}
          optionLabel={(model) => model.name}
          value={target()}
          disabled={props.disabled}
          onChange={(model) => props.onChange((row) => setRowTarget(row, model.id))}
        />
      </Show>

      <Show when={createsModel()}>
        <RadioGroup
          label="Storage"
          class={styles.choice}
          options={[
            { value: "managed", label: "Managed" },
            { value: "linked", label: "Linked" },
          ]}
          value={props.row.storageMode}
          disabled={props.disabled}
          onChange={(mode) => props.onChange((row) => setRowStorage(row, mode as ImportRow["storageMode"]))}
        />
      </Show>

      <Show when={candidate().format === "gcode"}>
        <p class={styles.meta}>{GCODE_NOTE}</p>
      </Show>

      <Show when={candidate().unsupported.length > 0}>
        <div class={styles.unsupported}>
          <Collapsible>
            <Collapsible.Trigger class={styles.disclosure}>
              <IconChevronRight size={14} aria-hidden="true" class={styles.disclosureIcon} />
              <span>Unsupported contents ({candidate().unsupported.length})</span>
            </Collapsible.Trigger>
            <Collapsible.Content class={styles.disclosureContent}>
              <p class={styles.meta}>{UNSUPPORTED_NOTE}</p>
              <ul class={styles.unsupportedList}>
                <For each={candidate().unsupported}>
                  {(entry) => (
                    <li>
                      <span>{entry.detail}</span>
                      <span class={styles.part}>{entry.part}</span>
                    </li>
                  )}
                </For>
              </ul>
            </Collapsible.Content>
          </Collapsible>
          <Checkbox
            checked={props.row.acknowledgeUnsupported}
            disabled={props.disabled}
            onChange={(checked) => props.onChange((row) => setRowAcknowledged(row, checked))}
          >
            Import it anyway. farm3d won't use these contents.
          </Checkbox>
        </div>
      </Show>
    </div>
  );
}

