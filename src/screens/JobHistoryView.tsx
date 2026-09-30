import { createEffect, createMemo, createSignal, For, onMount, Show, type JSX } from "solid-js";
import { Button, Chip, DataTable, Select, TextField } from "../design-system";
import type { DataTableColumn } from "../design-system";
import { history, loadMoreHistory, refreshHistory, setHistoryFilters, type HistoryFilters } from "../history/history-store";
import { jobHistoryStateLabel } from "../history/presentation";
import { addHistoryKnownIds } from "../history/known-ids-actions";
import type { JobHistoryRow, JobHistoryState } from "../history/types";
import type { Severity } from "../host-ops/presentation";
import { navigation } from "../navigation/navigation-store";
import { printers } from "../printers/printer-store";
import { cancelReasonLabel, settlementLabel } from "../queue/presentation";
import { formatDateTime } from "../slicing/revision-presentation";
import { SeverityLabel } from "./SeverityLabel";
import { showQueueEntry } from "./QueueRecoveryButton";
import styles from "./JobHistoryView.module.css";

const STATES: JobHistoryState[] = ["completed", "failed", "cancelled", "outcomeUnknown"];
/** What Rust returns when `states` is absent (D11). */
const DEFAULT_STATES: JobHistoryState[] = ["completed", "failed", "cancelled"];

const STATE_SEVERITY = {
  completed: "success",
  failed: "error",
  cancelled: "neutral",
  outcomeUnknown: "warning",
} satisfies Record<JobHistoryState, Severity>;

interface PrinterOption {
  id: string;
  label: string;
}
const ALL_PRINTERS: PrinterOption = { id: "", label: "All Printers" };

const CLEARED: Partial<HistoryFilters> = {
  text: undefined, states: undefined, printerId: undefined, endedAfter: undefined, endedBefore: undefined,
};

/** A `YYYY-MM-DD` day from a date field as the local start of that day
 *  (`offsetDays` 1 gives the start of the next day, the exclusive upper
 *  bound of an inclusive "on or before"). Rust compares RFC 3339 values. */
function dayBoundary(day: string, offsetDays: number): string | undefined {
  const [year, month, date] = day.split("-").map(Number);
  if (!year || !month || !date) return undefined;
  return new Date(year, month - 1, date + offsetDays).toISOString();
}

/** Job history (spec "Frontend architecture"): a search field, state,
 *  Printer, and date filters over one `DataTable` of settled Jobs with a
 *  "Load more" row. Rust decides what every filter means and what the rows
 *  say; this only sends the filters and presents the pages. Selecting a row
 *  opens the Job at `queue/job/<id>`. */
export function JobHistoryView() {
  const [afterDay, setAfterDay] = createSignal("");
  const [beforeDay, setBeforeDay] = createSignal("");
  onMount(() => void refreshHistory());
  createEffect(() => addHistoryKnownIds(history.rows().flatMap((row) => (row.incidentId ? [row.jobId, row.incidentId] : [row.jobId]))));

  const filters = () => history.filters();
  const selectedStates = () => filters().states ?? DEFAULT_STATES;
  const filtersActive = () => {
    const held = filters();
    return Boolean(held.text?.trim() || held.printerId || held.states || held.endedAfter || held.endedBefore);
  };
  const clearFilters = () => {
    setAfterDay("");
    setBeforeDay("");
    setHistoryFilters(CLEARED);
  };

  const toggleState = (state: JobHistoryState, selected: boolean) => {
    const next = STATES.filter((candidate) => (candidate === state ? selected : selectedStates().includes(candidate)));
    if (next.length === 0) return; // 1 to 4 states: the last one stays on.
    const isDefault = next.length === DEFAULT_STATES.length && DEFAULT_STATES.every((candidate) => next.includes(candidate));
    setHistoryFilters({ states: isDefault ? undefined : next });
  };

  const printerOptions = createMemo<PrinterOption[]>(() => [
    ALL_PRINTERS,
    ...[...printers()]
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((printer) => ({ id: printer.id, label: printer.archivedAt ? `${printer.name} (archived)` : printer.name })),
  ]);
  const selectedPrinter = () => printerOptions().find((option) => option.id === (filters().printerId ?? "")) ?? ALL_PRINTERS;

  const selectedJobId = () => {
    const target = navigation.target();
    return target.destination === "queue" && target.selection?.kind === "job" ? target.selection.id : null;
  };

  const columns: DataTableColumn<JobHistoryRow>[] = [
    { id: "ended", header: "Ended", cell: (row) => (row.endedAt ? formatDateTime(row.endedAt) : "—") },
    {
      id: "model", header: "Model",
      cell: (row) => (row.plateName ? `${row.modelName} — ${row.plateName}` : row.modelName),
    },
    {
      id: "printer", header: "Printer",
      cell: (row) => (
        <span class={styles.stack}>
          <span>{row.printerSnapshotName}</span>
          <Show when={row.printerArchived}><span class={styles.muted}>Printer archived</span></Show>
        </span>
      ),
    },
    {
      id: "state", header: "State",
      cell: (row) => (
        <span class={styles.stack}>
          <SeverityLabel severity={STATE_SEVERITY[row.state]} text={jobHistoryStateLabel(row.state)} />
          <Show when={row.cancelReason}>{(reason) => <span class={styles.muted}>{cancelReasonLabel(reason())}</span>}</Show>
        </span>
      ),
    },
    { id: "spool", header: "Spool", cell: (row) => `#${row.spoolNumber}` },
    { id: "material", header: "Material", cell: (row) => settlementLabel(row.settlement) },
    {
      id: "evidence", header: "Evidence",
      cell: (row) => {
        const parts = [
          row.incidentId ? "Incident" : null,
          row.snapshotCount > 0 ? (row.snapshotCount === 1 ? "1 snapshot" : `${row.snapshotCount} snapshots`) : null,
        ].filter((part): part is string => part !== null);
        return parts.length > 0 ? parts.join(", ") : "—";
      },
    },
  ];

  const empty = (): JSX.Element => {
    if (history.status() === "error") {
      return (
        <div class={styles.empty} role="alert">
          <p>{history.error()?.message ?? "The history couldn't be loaded."}</p>
          <Button variant="secondary" onClick={() => void refreshHistory()}>Retry</Button>
        </div>
      );
    }
    if (history.status() === "idle" || history.status() === "loading") {
      return <p class={styles.emptyNotice} role="status">Loading history…</p>;
    }
    if (filtersActive()) {
      return (
        <div class={styles.empty}>
          <p>No Jobs match these filters</p>
          <Button variant="ghost" onClick={clearFilters}>Clear filters</Button>
        </div>
      );
    }
    return (
      <div class={styles.empty}>
        <p>No finished Jobs yet</p>
        <p class={styles.muted}>A Job appears here once it completes, fails, or is cancelled.</p>
      </div>
    );
  };

  return (
    <div class={styles.view}>
      <div class={styles.toolbar} role="group" aria-label="History filters">
        <TextField
          class={styles.search}
          aria-label="Search history"
          placeholder="Model, Printer, plate, Spool #, or Job id"
          value={filters().text ?? ""}
          onChange={(value) => setHistoryFilters({ text: value })}
        />
        <div class={styles.chips} role="group" aria-label="State">
          <For each={STATES}>
            {(state) => (
              <Chip selected={selectedStates().includes(state)} onSelectedChange={(selected) => toggleState(state, selected)}>
                {jobHistoryStateLabel(state)}
              </Chip>
            )}
          </For>
        </div>
        <Select<PrinterOption>
          class={styles.printer}
          label="Printer"
          options={printerOptions()}
          value={selectedPrinter()}
          optionValue={(option) => option.id}
          optionLabel={(option) => option.label}
          onChange={(option) => setHistoryFilters({ printerId: option.id || undefined })}
        />
        <TextField
          class={styles.date}
          type="date"
          label="Ended on or after"
          value={afterDay()}
          onChange={(day) => {
            setAfterDay(day);
            setHistoryFilters({ endedAfter: day ? dayBoundary(day, 0) : undefined });
          }}
        />
        <TextField
          class={styles.date}
          type="date"
          label="Ended on or before"
          value={beforeDay()}
          onChange={(day) => {
            setBeforeDay(day);
            setHistoryFilters({ endedBefore: day ? dayBoundary(day, 1) : undefined });
          }}
        />
        <Show when={filtersActive()}>
          <Button variant="ghost" size="sm" onClick={clearFilters}>Clear filters</Button>
        </Show>
      </div>

      <Show when={history.status() === "error" && history.rows().length > 0}>
        <div class={styles.banner} role="alert">
          <p class={styles.bannerText}>{history.error()?.message ?? "The history couldn't be loaded."}</p>
          <Button variant="ghost" size="sm" onClick={() => void refreshHistory()}>Retry</Button>
        </div>
      </Show>

      <div class={styles.tableArea} aria-busy={history.status() === "loading"}>
        <DataTable
          label="Job history"
          rows={history.rows()}
          rowId={(row) => row.jobId}
          columns={columns}
          selectedId={selectedJobId()}
          onSelect={showQueueEntry}
          onActivate={showQueueEntry}
          empty={empty()}
        />
        <Show when={history.rows().length > 0}>
          <div class={styles.footer}>
            <span class={styles.count} aria-live="polite">
              {history.rows().length === 1 ? "1 Job" : `${history.rows().length} Jobs`}
              {history.hasMore() ? " so far" : ""}
              {history.status() === "loading" ? ", updating…" : ""}
            </span>
            <Show when={history.hasMore()}>
              <Show when={history.status() === "ready" && history.error()}>
                <span class={styles.footerError} role="alert">More Jobs couldn't be loaded.</span>
              </Show>
              <Button variant="secondary" size="sm" disabled={history.loadingMore()} onClick={() => void loadMoreHistory()}>
                {history.loadingMore() ? "Loading…" : "Load more"}
              </Button>
            </Show>
          </div>
        </Show>
      </div>
    </div>
  );
}
