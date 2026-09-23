import { createMemo, For, Match, Show, Switch, type JSX } from "solid-js";
import { Button, Checkbox, SeverityMarker, TextField, type SeverityMarkerProps } from "../design-system";
import { defaultPort, isRowCreated, type BatchRowDraft } from "../printers/batch-intake";
import type { StartSafety } from "../printers/types";
import type { BatchRowOutcome } from "../generated/contracts/command/BatchRowOutcome";
import { bedTypeLabel } from "./PrinterProfilePanel";
import styles from "./BatchRowsTable.module.css";

export interface BatchRowsTableProps {
  rows: BatchRowDraft[];
  onChange: (rowId: string, patch: Partial<BatchRowDraft>) => void;
  onToggle: (rowId: string, selected: boolean) => void;
  onToggleAll: (selected: boolean) => void;
  onRemove: (rowId: string) => void;
  mode: "edit" | "connect" | "results";
  /** The shared values every row receives at creation, shown per row (D9). */
  preview: { defaultBedType?: string; startSafety: StartSafety };
  /** Existing Printer names, for the D11 name-collision warning. */
  existingNames?: readonly string[];
  /** Rows currently being created or connected (results mode). */
  pending?: ReadonlySet<string>;
  /** Created rows whose reconnection probe failed and that may be saved
   *  unverified (results mode, D8). */
  unverified?: ReadonlySet<string>;
  onSaveAnyway?: (rowId: string) => void;
}

const OUTCOME_MARKERS: Record<BatchRowOutcome, { severity: SeverityMarkerProps["severity"]; label: string }> = {
  created: { severity: "resolved", label: "Created" },
  createdSetupIncomplete: { severity: "warning", label: "Created — Setup incomplete" },
  rejected: { severity: "fatal", label: "Not created" },
  cancelled: { severity: "info", label: "Cancelled" },
};

const SAFETY_PREVIEW: Record<StartSafety, string> = {
  confirmBedClear: "Confirm bed clear",
  unattended: "Unattended starts",
};

const CREDENTIAL_LABELS = { none: "None", shared: "Shared", row: "Per row" } as const;

const HEADERS: Record<BatchRowsTableProps["mode"], string[]> = {
  edit: ["Name", "Location", "Receives", ""],
  connect: ["Name", "Location", "Protocol", "Host", "Port", "TLS", "Credential", "Receives"],
  results: ["Name", "Location", "Host", "Port", "Outcome", "Details"],
};

function nameKey(name: string): string {
  return name.trim().toLowerCase();
}

/** The batch dialog's row grid (spec "PrinterBatchDialog"). Rows are fully
 *  controlled by the caller; this component only renders and reports
 *  edits. Scrolls horizontally inside its own container at narrow widths. */
export function BatchRowsTable(props: BatchRowsTableProps) {
  const selectable = () => props.mode !== "results";
  const selectedCount = () => props.rows.filter((row) => row.selected).length;

  // D11: collisions warn, never block.
  const nameWarnings = createMemo(() => {
    const counts = new Map<string, number>();
    for (const row of props.rows) {
      const key = nameKey(row.name);
      if (key) counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    const existing = new Set((props.existingNames ?? []).map(nameKey));
    const warnings = new Map<string, string>();
    for (const row of props.rows) {
      if (isRowCreated(row)) continue;
      const key = nameKey(row.name);
      if (!key) continue;
      if (existing.has(key)) warnings.set(row.rowId, "A Printer already has this name");
      else if ((counts.get(key) ?? 0) > 1) warnings.set(row.rowId, "Another row has this name");
    }
    return warnings;
  });

  const rowLabel = (row: BatchRowDraft, index: number) => row.name.trim() || `row ${index + 1}`;

  function portInput(row: BatchRowDraft, index: number) {
    return (
      <TextField
        aria-label={`Port for row ${index + 1}`}
        value={row.port === null ? "" : String(row.port)}
        placeholder={String(defaultPort(row.protocol))}
        onChange={(value) => {
          const trimmed = value.trim();
          if (trimmed === "") props.onChange(row.rowId, { port: null });
          else if (/^\d+$/.test(trimmed)) props.onChange(row.rowId, { port: Number(trimmed) });
        }}
      />
    );
  }

  function hostInput(row: BatchRowDraft, index: number) {
    return (
      <TextField
        aria-label={`Host for row ${index + 1}`}
        value={row.host}
        placeholder="Profile-only"
        onChange={(host) => props.onChange(row.rowId, { host })}
      />
    );
  }

  function nameCell(row: BatchRowDraft, index: number, editable: boolean): JSX.Element {
    return (
      <div role="cell" class={styles.cell}>
        <Show when={editable} fallback={<span class={styles.text}>{row.name}</span>}>
          <TextField
            aria-label={`Name for row ${index + 1}`}
            value={row.name}
            onChange={(name) => props.onChange(row.rowId, { name })}
          />
        </Show>
        <Show when={nameWarnings().get(row.rowId)}>
          {(warning) => <span class={styles.warn}>{warning()}</span>}
        </Show>
      </div>
    );
  }

  function locationCell(row: BatchRowDraft, index: number, editable: boolean): JSX.Element {
    return (
      <div role="cell" class={styles.cell}>
        <Show when={editable} fallback={<span class={styles.text}>{row.location}</span>}>
          <TextField
            aria-label={`Location for row ${index + 1}`}
            value={row.location}
            onChange={(location) => props.onChange(row.rowId, { location })}
          />
        </Show>
      </div>
    );
  }

  function previewCell(row: BatchRowDraft): JSX.Element {
    return (
      <div role="cell" class={[styles.cell, styles.preview].join(" ")} data-testid={`preview-${row.rowId}`}>
        <span>Bed: {bedTypeLabel(props.preview.defaultBedType ?? "")}</span>
        <span>{SAFETY_PREVIEW[props.preview.startSafety]}</span>
      </div>
    );
  }

  function outcomeCell(row: BatchRowDraft): JSX.Element {
    return (
      <div role="cell" class={styles.cell}>
        <Switch fallback={<span class={styles.muted}>Not sent</span>}>
          <Match when={props.pending?.has(row.rowId)}>
            <SeverityMarker severity="info" label="In progress" />
          </Match>
          <Match when={row.result}>
            {(result) => (
              <SeverityMarker
                severity={OUTCOME_MARKERS[result().outcome].severity}
                label={OUTCOME_MARKERS[result().outcome].label}
              />
            )}
          </Match>
        </Switch>
      </div>
    );
  }

  function detailsCell(row: BatchRowDraft): JSX.Element {
    return (
      <div role="cell" class={styles.cell}>
        <Show when={row.result}>
          {(result) => (
            <ul class={styles.messages}>
              <For each={result().errors}>
                {(error) => (
                  <li class={styles.error}>
                    {error.message}
                    <Show when={error.fieldPath}>
                      {(path) => <span class={styles.fieldPath}> ({path()})</span>}
                    </Show>
                  </li>
                )}
              </For>
              <For each={result().warnings}>{(warning) => <li class={styles.warn}>{warning.message}</li>}</For>
            </ul>
          )}
        </Show>
        <Show when={props.unverified?.has(row.rowId) && !props.pending?.has(row.rowId)}>
          <Button variant="secondary" size="sm" onClick={() => props.onSaveAnyway?.(row.rowId)}>
            Save anyway
          </Button>
        </Show>
      </div>
    );
  }

  return (
    <div class={styles.scroller}>
      <div role="table" aria-label="Batch rows" class={[styles.table, styles[props.mode]].join(" ")}>
        <div role="row" class={[styles.row, styles.header].join(" ")}>
          <Show when={selectable()}>
            <div role="columnheader" class={styles.cell}>
              <Checkbox
                checked={props.rows.length > 0 && selectedCount() === props.rows.length}
                indeterminate={selectedCount() > 0 && selectedCount() < props.rows.length}
                onChange={(checked) => props.onToggleAll(checked)}
              >
                <span class={styles.srOnly}>Select all rows</span>
              </Checkbox>
            </div>
          </Show>
          <For each={HEADERS[props.mode]}>
            {(header) => (
              <div role="columnheader" class={styles.cell}>
                {header}
              </div>
            )}
          </For>
        </div>

        <For each={props.rows}>
          {(row, index) => {
            const locked = () => isRowCreated(row);
            const pending = () => props.pending?.has(row.rowId) ?? false;
            // Connection input stays editable for rows not yet created, and
            // for a created-but-incomplete row that can still be reconnected
            // through its printerId.
            const connectionEditable = () =>
              !pending() &&
              (!locked() || (!!row.printerId && row.result?.outcome === "createdSetupIncomplete"));
            return (
              <div role="row" class={styles.row} data-testid={`row-${row.rowId}`}>
                <Show when={selectable()}>
                  <div role="cell" class={styles.cell}>
                    <Checkbox checked={row.selected} onChange={(checked) => props.onToggle(row.rowId, checked)}>
                      <span class={styles.srOnly}>Select {rowLabel(row, index())}</span>
                    </Checkbox>
                  </div>
                </Show>

                <Switch>
                  <Match when={props.mode === "edit"}>
                    {nameCell(row, index(), !locked())}
                    {locationCell(row, index(), !locked())}
                    {previewCell(row)}
                    <div role="cell" class={styles.cell}>
                      <Show when={!locked()}>
                        <Button
                          variant="ghost"
                          size="sm"
                          aria-label={`Remove ${rowLabel(row, index())}`}
                          onClick={() => props.onRemove(row.rowId)}
                        >
                          Remove
                        </Button>
                      </Show>
                    </div>
                  </Match>

                  <Match when={props.mode === "connect"}>
                    {nameCell(row, index(), false)}
                    {locationCell(row, index(), false)}
                    <div role="cell" class={styles.cell}>
                      <span class={styles.text}>{row.protocol}</span>
                    </div>
                    <div role="cell" class={styles.cell}>
                      <Show when={!locked()} fallback={<span class={styles.text}>{row.host}</span>}>
                        {hostInput(row, index())}
                      </Show>
                    </div>
                    <div role="cell" class={styles.cell}>
                      <Show when={!locked()} fallback={<span class={styles.text}>{row.port ?? ""}</span>}>
                        {portInput(row, index())}
                      </Show>
                    </div>
                    <div role="cell" class={styles.cell}>
                      <span class={styles.text}>{row.useTls ? "On" : "Off"}</span>
                    </div>
                    <div role="cell" class={styles.cell}>
                      <span class={styles.text}>{CREDENTIAL_LABELS[row.credential.source]}</span>
                      <Show when={!locked() && row.credential.source === "row"}>
                        <TextField
                          aria-label={`Credential for row ${index() + 1}`}
                          type="password"
                          value={row.credential.source === "row" ? row.credential.value : ""}
                          onChange={(value) => props.onChange(row.rowId, { credential: { source: "row", value } })}
                        />
                      </Show>
                    </div>
                    {previewCell(row)}
                  </Match>

                  <Match when={props.mode === "results"}>
                    {nameCell(row, index(), !locked() && !pending())}
                    {locationCell(row, index(), !locked() && !pending())}
                    <Show
                      when={connectionEditable()}
                      fallback={
                        <>
                          <div role="cell" class={styles.cell}>
                            <span class={styles.text}>{row.host}</span>
                          </div>
                          <div role="cell" class={styles.cell}>
                            <span class={styles.text}>{row.host ? (row.port ?? "") : ""}</span>
                          </div>
                        </>
                      }
                    >
                      <div role="cell" class={styles.cell}>
                        {hostInput(row, index())}
                      </div>
                      <div role="cell" class={styles.cell}>
                        {portInput(row, index())}
                      </div>
                    </Show>
                    {outcomeCell(row)}
                    {detailsCell(row)}
                  </Match>
                </Switch>
              </div>
            );
          }}
        </For>
      </div>
    </div>
  );
}
