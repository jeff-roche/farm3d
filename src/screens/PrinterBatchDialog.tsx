import { createEffect, createMemo, createResource, createSignal, For, Match, on, Show, Switch as Branch } from "solid-js";
import { createStore } from "solid-js/store";
import {
  Button,
  Checkbox,
  Dialog,
  RadioGroup,
  Select,
  Stepper,
  Switch,
  TextField,
  Textarea,
} from "../design-system";
import type { BatchRowError } from "../generated/contracts/command/BatchRowError";
import type { BatchRowErrorCode } from "../generated/contracts/command/BatchRowErrorCode";
import type { BatchShared } from "../generated/contracts/command/BatchShared";
import type { ErrorCode } from "../generated/contracts/command/ErrorCode";
import { isCommandError } from "../ipc/client";
import {
  defaultPort,
  generateRows,
  isRowCreated,
  mapDiscovery,
  parseIntake,
  toBatchInput,
  type BatchRowDraft,
  type CandidateMatch,
  type IntakeIssue,
} from "../printers/batch-intake";
import {
  cancelBatch,
  createPrintersBatch,
  credentialStoreInfo,
  discoverPrinters,
  probeCandidate,
  setConnection,
} from "../printers/printer-store";
import type {
  ConnectionSubmission,
  DiscoveredPrinter,
  ProbeResult,
  ResolvedPrinter,
  StartSafety,
} from "../printers/types";
import { BatchRowsTable } from "./BatchRowsTable";
import { CatalogPickerFields, createCatalogPicker } from "./CatalogPicker";
import { KINDS, toSubmission } from "./ConnectionFields";
import { bedTypeLabel, bedTypeOptionsFor } from "./PrinterProfilePanel";
import { START_SAFETY_OPTIONS } from "./PrinterSetupWizard";
import styles from "./PrinterBatchDialog.module.css";

const STEP_ORDER = ["shared", "rows", "connect", "review"] as const;
type StepId = (typeof STEP_ORDER)[number];
const STEP_LABELS: Record<StepId, string> = {
  shared: "Shared",
  rows: "Rows",
  connect: "Connect",
  review: "Review",
};

type CredentialSource = BatchRowDraft["credential"]["source"];
const CREDENTIAL_SOURCES: { value: CredentialSource; label: string }[] = [
  { value: "none", label: "None" },
  { value: "shared", label: "Shared" },
  { value: "row", label: "Per row" },
];

const ROW_ERROR_CODES = new Set<string>([
  "VALIDATION",
  "DUPLICATE_HOST",
  "UNSUPPORTED_ADAPTER",
  "AUTHENTICATION_FAILED",
  "PRINTER_UNREACHABLE",
  "TIMEOUT",
  "PROTOCOL_ERROR",
  "CREDENTIAL_UNAVAILABLE",
  "PERSISTENCE_UNAVAILABLE",
  "CANCELLED",
] satisfies BatchRowErrorCode[]);

function errorMessage(e: unknown): string {
  return isCommandError(e) ? e.message : "The operation could not be completed.";
}

/** Command codes outside the row vocabulary that have a close row
 *  equivalent; anything else unmapped is reported as `VALIDATION`. */
const ROW_ERROR_FALLBACKS: Partial<Record<ErrorCode, BatchRowErrorCode>> = {
  CREDENTIAL_REQUIRED: "CREDENTIAL_UNAVAILABLE",
  CORRUPT_DATA: "PERSISTENCE_UNAVAILABLE",
  MIGRATION_FAILED: "PERSISTENCE_UNAVAILABLE",
  UNSUPPORTED_SCHEMA_VERSION: "PERSISTENCE_UNAVAILABLE",
};

/** The codes a Connection probe fails with -- the only failures "Save
 *  anyway" (`acceptUnverified`) can get past (D8). */
const PROBE_ERROR_CODES = new Set<string>(["PRINTER_UNREACHABLE", "TIMEOUT", "AUTHENTICATION_FAILED", "PROTOCOL_ERROR"]);

/** A reconnection (`probe_connection`/`set_printer_connection`) failure,
 *  shaped as a row error so the results grid shows it like any other. The
 *  message is always the command's own. Exported for its unit test. */
export function toRowError(e: unknown): BatchRowError {
  if (!isCommandError(e)) return { code: "VALIDATION", message: errorMessage(e) };
  const code = ROW_ERROR_CODES.has(e.code)
    ? (e.code as BatchRowErrorCode)
    : (ROW_ERROR_FALLBACKS[e.code as ErrorCode] ?? "VALIDATION");
  return { code, message: e.message };
}

function candidateKey(candidate: DiscoveredPrinter): string {
  return `${candidate.host}:${candidate.port}`;
}

/** A row that failed and still needs the user: not created, cancelled, or
 *  created but its Connection didn't take. */
function isFailed(row: BatchRowDraft): boolean {
  const outcome = row.result?.outcome;
  if (outcome === "rejected" || outcome === "cancelled") return true;
  return outcome === "createdSetupIncomplete" && row.result!.errors.length > 0;
}

/** Created without a Connection, but has Connection input to retry via
 *  `set_printer_connection` (D1) rather than creating a second Printer. */
function needsReconnect(row: BatchRowDraft): boolean {
  return !!row.printerId && row.result?.outcome === "createdSetupIncomplete" && row.host.trim() !== "";
}

export interface PrinterBatchDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  existingPrinters: ResolvedPrinter[];
}

/** Batch Printer setup (spec "PrinterBatchDialog", D1, D9–D14): Shared →
 *  Rows → Connect → Review & results. Rows are component-local; results are
 *  merged back by `rowId`, and failed rows keep their input for Retry. */
export function PrinterBatchDialog(props: PrinterBatchDialogProps) {
  const picker = createCatalogPicker();
  const [step, setStep] = createSignal<StepId>("shared");

  // Shared
  const [startSafety, setStartSafety] = createSignal<StartSafety>("confirmBedClear");
  const [bedType, setBedType] = createSignal("");
  const [bedTypeTouched, setBedTypeTouched] = createSignal(false);
  createEffect(() => {
    const p = picker.preview();
    if (!p || bedTypeTouched()) return;
    setBedType(p.defaultBedType);
  });

  // Rows
  const [rows, setRows] = createStore<BatchRowDraft[]>([]);
  const newRowId = () => crypto.randomUUID();
  const [quantity, setQuantity] = createSignal("1");
  const [pattern, setPattern] = createSignal("");
  const [locationsText, setLocationsText] = createSignal("");
  const [pasteText, setPasteText] = createSignal("");
  const [intakeErrors, setIntakeErrors] = createSignal<IntakeIssue[]>([]);
  const [intakeWarnings, setIntakeWarnings] = createSignal<IntakeIssue[]>([]);

  // Connect. The shared credential lives only here, is never pre-filled,
  // and is cleared on close (D13).
  const [sharedCredential, setSharedCredential] = createSignal("");
  const [applyKind, setApplyKind] = createSignal(KINDS[0].value);
  const [applyPort, setApplyPort] = createSignal("");
  const [applyTls, setApplyTls] = createSignal(false);
  const [applyCredential, setApplyCredential] = createSignal<CredentialSource>("none");
  const [candidates, setCandidates] = createSignal<DiscoveredPrinter[]>([]);
  const [discovery, setDiscovery] = createSignal<"idle" | "scanning" | "done" | "failed">("idle");

  // Review
  const [probe, setProbe] = createSignal(true);
  const [pending, setPending] = createSignal<ReadonlySet<string>>(new Set());
  const [runningBatchId, setRunningBatchId] = createSignal<string | null>(null);
  const [batchError, setBatchError] = createSignal<string | null>(null);
  const [confirmOpen, setConfirmOpen] = createSignal(false);
  const [store] = createResource(credentialStoreInfo);
  // Created rows whose reconnection probe failed: each may be saved
  // unverified with its own "Save anyway". Cleared per row as soon as that
  // row's Connection input (or the shared credential) changes, so "Save
  // anyway" never saves something other than what was just probed.
  const [unverified, setUnverified] = createSignal<ReadonlySet<string>>(new Set());
  function markUnverified(rowId: string, value: boolean) {
    setUnverified((prev) => {
      if (prev.has(rowId) === value) return prev;
      const next = new Set(prev);
      if (value) next.add(rowId);
      else next.delete(rowId);
      return next;
    });
  }
  createEffect(on(sharedCredential, () => setUnverified(new Set<string>()), { defer: true }));

  function reset() {
    picker.reset();
    setStep("shared");
    setStartSafety("confirmBedClear");
    setBedType("");
    setBedTypeTouched(false);
    setRows([]);
    setQuantity("1");
    setPattern("");
    setLocationsText("");
    setPasteText("");
    setIntakeErrors([]);
    setIntakeWarnings([]);
    setSharedCredential("");
    setApplyKind(KINDS[0].value);
    setApplyPort("");
    setApplyTls(false);
    setApplyCredential("none");
    setCandidates([]);
    setDiscovery("idle");
    setProbe(true);
    setPending(new Set<string>());
    setRunningBatchId(null);
    setBatchError(null);
    setConfirmOpen(false);
    setUnverified(new Set<string>());
  }

  createEffect(
    on(
      () => props.open,
      (open) => {
        if (!open) reset();
      },
    ),
  );

  const existingNames = createMemo(() => props.existingPrinters.map((printer) => printer.name));
  const busy = () => pending().size > 0;
  const hasResults = () => rows.some((row) => row.result);
  const hasFailedRows = () => rows.some(isFailed);
  const canRetry = () => rows.some((row) => (!isRowCreated(row) && row.result) || needsReconnect(row));
  const rowsValid = () => rows.length > 0 && rows.every((row) => row.name.trim() !== "");
  const rowPreview = () => ({ defaultBedType: bedType(), startSafety: startSafety() });

  // ---- Rows step -------------------------------------------------------

  function appendRows(next: BatchRowDraft[]) {
    if (next.length > 0) setRows((prev) => [...prev, ...next]);
  }

  const quantityValue = () => {
    const n = Number(quantity().trim());
    return Number.isInteger(n) && n > 0 ? n : 0;
  };

  function onGenerate() {
    const locations = locationsText()
      .split(",")
      .map((location) => location.trim())
      .filter(Boolean);
    appendRows(
      generateRows(
        { quantity: quantityValue(), pattern: pattern().trim(), locations: locations.length ? locations : [""] },
        newRowId,
      ),
    );
  }

  function applyIntake(text: string): boolean {
    const intake = parseIntake(text, newRowId);
    setIntakeErrors(intake.errors);
    setIntakeWarnings(intake.warnings);
    appendRows(intake.rows);
    return intake.errors.length === 0;
  }

  function onAddPasted() {
    if (applyIntake(pasteText())) setPasteText("");
  }

  let fileInput: HTMLInputElement | undefined;

  async function onFileChosen(input: HTMLInputElement) {
    const file = input.files?.[0];
    if (!file) return;
    applyIntake(await file.text());
    input.value = "";
  }

  // ---- Connect step ----------------------------------------------------

  async function scan() {
    setDiscovery("scanning");
    try {
      setCandidates(await discoverPrinters());
      setDiscovery("done");
    } catch {
      setCandidates([]);
      setDiscovery("failed");
    }
  }

  createEffect(
    on(step, (current) => {
      if (current === "connect" && discovery() === "idle") void scan();
    }),
  );

  const mapping = createMemo(() => mapDiscovery(candidates(), rows, props.existingPrinters));
  const assignableRows = () => rows.filter((row) => !isRowCreated(row));
  const rowName = (rowId: string) => rows.find((row) => row.rowId === rowId)?.name ?? "";

  function assign(candidate: DiscoveredPrinter, rowId: string) {
    setRows((row) => row.rowId === rowId, { host: candidate.host, port: candidate.port, protocol: candidate.kind });
  }

  /** "Assign in order to selected rows" (D12): unclaimed assignable
   *  candidates go, in discovery order, to selected rows without a host. */
  function assignInOrder() {
    const targets = rows.filter((row) => row.selected && !isRowCreated(row) && row.host.trim() === "");
    const free = candidates().filter((candidate) => {
      const match = mapping().get(candidateKey(candidate));
      return match?.kind === "assignable" && !match.rowId;
    });
    targets.slice(0, free.length).forEach((row, i) => assign(free[i], row.rowId));
  }

  function applyToSelected() {
    const trimmed = applyPort().trim();
    const port = /^\d+$/.test(trimmed) ? Number(trimmed) : null;
    const source = applyCredential();
    setRows(
      (row) => row.selected && !isRowCreated(row),
      (row) => ({
        protocol: applyKind(),
        port,
        useTls: applyTls(),
        credential:
          source === "row"
            ? { source: "row" as const, value: row.credential.source === "row" ? row.credential.value : "" }
            : { source },
      }),
    );
  }

  // ---- Review & results ------------------------------------------------

  function sharedInput(): BatchShared | null {
    const catalogRef = picker.catalogRef();
    if (!catalogRef) return null;
    const catalogDefault = picker.preview()?.defaultBedType ?? "";
    return {
      catalogRef,
      startSafety: startSafety(),
      ...(bedType() !== catalogDefault ? { defaultBedType: bedType() } : {}),
    };
  }

  function mergeResults(results: NonNullable<BatchRowDraft["result"]>[]) {
    for (const result of results) {
      setRows((row) => row.rowId === result.rowId, {
        result,
        ...(result.printer ? { printerId: result.printer.id } : {}),
      });
    }
  }

  async function runBatch(batchRows: BatchRowDraft[], shared: BatchShared) {
    const batchId = crypto.randomUUID();
    const usesShared = batchRows.some((row) => row.credential.source === "shared");
    const credential = usesShared && sharedCredential() !== "" ? sharedCredential() : undefined;
    setRunningBatchId(batchId);
    try {
      const output = await createPrintersBatch(toBatchInput(batchRows, shared, credential, probe(), batchId));
      mergeResults(output.rows);
    } catch (e) {
      setBatchError(errorMessage(e));
    } finally {
      setRunningBatchId(null);
    }
  }

  function reconnectSubmission(row: BatchRowDraft): ConnectionSubmission {
    const credential =
      row.credential.source === "shared"
        ? sharedCredential()
        : row.credential.source === "row"
          ? row.credential.value
          : "";
    return toSubmission(
      {
        kind: row.protocol,
        host: row.host,
        port: row.port ?? defaultPort(row.protocol),
        useTls: row.useTls,
        credential,
      },
      "create",
    );
  }

  function setRowError(rowId: string, e: unknown) {
    setRows((r) => r.rowId === rowId, "result", (result) =>
      result ? { ...result, errors: [toRowError(e)] } : result,
    );
  }

  /** Saves a created row's Connection; a first-time set never probes on the
   *  backend (D8), so the caller decides whether it was verified first. */
  async function saveConnection(
    row: BatchRowDraft,
    submission: ConnectionSubmission,
    acceptUnverified: boolean,
    probed?: ProbeResult,
  ) {
    try {
      if (acceptUnverified) await setConnection(row.printerId!, submission, true);
      else await setConnection(row.printerId!, submission);
      setRows((r) => r.rowId === row.rowId, "result", (result) =>
        result
          ? {
              ...result,
              outcome: "created" as const,
              errors: [],
              credentialStored: submission.credential !== undefined,
              ...(probed ? { probe: probed } : {}),
            }
          : result,
      );
    } catch (e) {
      setRowError(row.rowId, e);
    }
  }

  /** Retry for a created-but-incomplete row. With "Test each Connection"
   *  on, the Connection is probed first and only saved if the probe passes;
   *  a probe failure keeps the row failed and offers "Save anyway". */
  async function reconnect(row: BatchRowDraft) {
    const submission = reconnectSubmission(row);
    markUnverified(row.rowId, false);
    let probed: ProbeResult | undefined;
    if (probe()) {
      try {
        probed = await probeCandidate(submission);
      } catch (e) {
        setRowError(row.rowId, e);
        if (isCommandError(e) && PROBE_ERROR_CODES.has(e.code)) markUnverified(row.rowId, true);
        return;
      }
    }
    await saveConnection(row, submission, false, probed);
  }

  async function saveAnyway(rowId: string) {
    const row = rows.find((r) => r.rowId === rowId);
    if (!row?.printerId || busy() || !unverified().has(rowId)) return;
    markUnverified(rowId, false);
    setPending(new Set([rowId]));
    try {
      await saveConnection(row, reconnectSubmission(row), true);
    } finally {
      setPending(new Set<string>());
    }
  }

  /** Create, and Retry failed: every row without a Printer goes out as a
   *  new batch (same `rowId`s, new `batchId`); a created-but-incomplete row
   *  with Connection input reconnects its existing Printer instead. A row
   *  created without a returned `printerId` is never re-sent and cannot be
   *  reconnected from here. */
  async function submit() {
    const shared = sharedInput();
    if (!shared || busy()) return;
    setBatchError(null);
    const batchRows = rows.filter((row) => !isRowCreated(row));
    const reconnectRows = rows.filter(needsReconnect);
    setPending(new Set([...batchRows, ...reconnectRows].map((row) => row.rowId)));
    try {
      await Promise.all([
        ...(batchRows.length > 0 ? [runBatch(batchRows, shared)] : []),
        ...reconnectRows.map(reconnect),
      ]);
    } finally {
      setPending(new Set<string>());
    }
  }

  function cancelRunning() {
    const batchId = runningBatchId();
    if (batchId) void cancelBatch(batchId);
  }

  // ---- Closing ---------------------------------------------------------

  function requestClose() {
    if (busy() || hasFailedRows()) setConfirmOpen(true);
    else props.onOpenChange(false);
  }

  function confirmClose() {
    cancelRunning();
    setConfirmOpen(false);
    props.onOpenChange(false);
  }

  // ---- Navigation ------------------------------------------------------

  const canLeave = (id: StepId) => {
    if (id === "shared") return !!picker.catalogRef();
    if (id === "rows") return rowsValid();
    return true;
  };

  function goNext() {
    const idx = STEP_ORDER.indexOf(step());
    if (idx < STEP_ORDER.length - 1 && canLeave(step())) setStep(STEP_ORDER[idx + 1]);
  }

  function goBack() {
    const idx = STEP_ORDER.indexOf(step());
    if (idx > 0) setStep(STEP_ORDER[idx - 1]);
  }

  const stepperSteps = createMemo(() => {
    const currentIndex = STEP_ORDER.indexOf(step());
    return STEP_ORDER.map((id, index) => ({
      id,
      label: STEP_LABELS[id],
      state: index < currentIndex ? ("complete" as const) : undefined,
    }));
  });

  const tableHandlers = {
    onChange: (rowId: string, patch: Partial<BatchRowDraft>) => {
      if (["host", "port", "protocol", "useTls", "credential"].some((key) => key in patch)) {
        markUnverified(rowId, false);
      }
      setRows((row) => row.rowId === rowId, patch);
    },
    onToggle: (rowId: string, selected: boolean) => setRows((row) => row.rowId === rowId, "selected", selected),
    onToggleAll: (selected: boolean) => setRows(() => true, "selected", selected),
    onRemove: (rowId: string) => setRows((prev) => prev.filter((row) => row.rowId !== rowId)),
  };

  const connectionCount = () => rows.filter((row) => row.host.trim() !== "").length;
  const startSafetyLabel = () =>
    START_SAFETY_OPTIONS.find((option) => option.value === startSafety())?.label ?? startSafety();

  return (
    <>
      <Dialog
        title="Add Printers"
        open={props.open}
        onOpenChange={(open) => (open ? props.onOpenChange(true) : requestClose())}
      >
        <div class={styles.body}>
          <Stepper
            steps={stepperSteps()}
            current={step()}
            onSelect={(id) => !busy() && setStep(id as StepId)}
            aria-label="Batch Printer setup"
          />

          <Show when={step() === "shared"}>
            <div class={styles.form}>
              <p class={styles.note}>Every Printer in this batch is the same model and receives these settings.</p>
              <CatalogPickerFields picker={picker} />
              <Select
                label="Bed type"
                options={bedTypeOptionsFor(bedType())}
                optionLabel={bedTypeLabel}
                value={bedType()}
                // Kobalte treats "" as no selection, so the catalog's blank bed type
                // needs the placeholder to read "Default" (as in PrinterProfilePanel).
                placeholder={bedTypeLabel("")}
                onChange={(value) => {
                  setBedTypeTouched(true);
                  setBedType(value);
                }}
              />
              <RadioGroup
                label="Start safety"
                options={START_SAFETY_OPTIONS}
                value={startSafety()}
                onChange={(value) => setStartSafety(value as StartSafety)}
              />
            </div>
          </Show>

          <Show when={step() === "rows"}>
            <div class={styles.intake}>
              <section class={styles.section} aria-label="Generate rows">
                <span class={styles.sectionTitle}>Generate</span>
                <div class={[styles.inline, styles.alignTop].join(" ")}>
                  <TextField
                    label="Quantity per location"
                    type="number"
                    value={quantity()}
                    onChange={setQuantity}
                    class={styles.narrow}
                  />
                  <TextField
                    label="Name pattern"
                    value={pattern()}
                    onChange={setPattern}
                    placeholder="Voron {nn}"
                    description="{n} counts up; {nn} pads to two digits"
                  />
                  <TextField
                    label="Locations"
                    value={locationsText()}
                    onChange={setLocationsText}
                    placeholder="Bay A, Bay B"
                    description="Comma-separated; optional"
                  />
                </div>
                <div>
                  <Button
                    variant="secondary"
                    disabled={pattern().trim() === "" || quantityValue() === 0}
                    onClick={onGenerate}
                  >
                    Generate rows
                  </Button>
                </div>
              </section>

              <section class={styles.section} aria-label="Paste or import rows">
                <span class={styles.sectionTitle}>Paste or import</span>
                <Textarea
                  label="Paste rows"
                  rows={3}
                  value={pasteText()}
                  onChange={setPasteText}
                  placeholder={"name,location,host,port,protocol,tls"}
                  description="A header row is required. Credentials are never imported."
                />
                <div class={styles.buttonRow}>
                  <Button variant="secondary" disabled={pasteText().trim() === ""} onClick={onAddPasted}>
                    Add pasted rows
                  </Button>
                  {/* The native file input can't take design-system styling, so
                      it stays hidden and a Button opens its picker. */}
                  <input
                    ref={fileInput}
                    type="file"
                    accept=".csv,.tsv,.txt,text/csv,text/tab-separated-values"
                    aria-label="Import CSV file"
                    class={styles.fileInput}
                    onChange={(e) => void onFileChosen(e.currentTarget)}
                  />
                  <Button variant="secondary" onClick={() => fileInput?.click()}>
                    Import CSV file…
                  </Button>
                </div>
                <Show when={intakeErrors().length > 0}>
                  <ul class={[styles.issues, styles.error].join(" ")} role="alert">
                    <For each={intakeErrors()}>{(issue) => <li>Line {issue.line}: {issue.message}</li>}</For>
                  </ul>
                </Show>
                <Show when={intakeWarnings().length > 0}>
                  <ul class={[styles.issues, styles.warn].join(" ")}>
                    <For each={intakeWarnings()}>{(issue) => <li>Line {issue.line}: {issue.message}</li>}</For>
                  </ul>
                </Show>
              </section>

              <Show when={rows.length > 0} fallback={<p class={styles.note}>No rows yet.</p>}>
                <BatchRowsTable
                  rows={rows}
                  {...tableHandlers}
                  mode="edit"
                  preview={rowPreview()}
                  existingNames={existingNames()}
                />
              </Show>
            </div>
          </Show>

          <Show when={step() === "connect"}>
            <div class={styles.intake}>
              <section class={styles.section} aria-label="Discovered Printers">
                <div class={styles.sectionHeader}>
                  <span class={styles.sectionTitle}>Discovered on this network</span>
                  <Button variant="ghost" size="sm" disabled={discovery() === "scanning"} onClick={() => void scan()}>
                    Rescan
                  </Button>
                  <Button variant="ghost" size="sm" onClick={assignInOrder}>
                    Assign in order to selected rows
                  </Button>
                </div>
                <Show when={discovery() === "scanning"}>
                  <p class={styles.note}>Scanning…</p>
                </Show>
                <Show when={discovery() === "failed"}>
                  <p class={styles.warn}>Discovery failed — enter hosts in the rows below.</p>
                </Show>
                <Show when={discovery() === "done" && candidates().length === 0}>
                  <p class={styles.note}>Nothing found — enter hosts in the rows below.</p>
                </Show>
                <ul class={styles.candidates}>
                  <For each={candidates()}>
                    {(candidate) => {
                      const match = () => mapping().get(candidateKey(candidate));
                      return (
                        <li class={styles.candidate} data-testid={`candidate-${candidateKey(candidate)}`}>
                          <span class={styles.candidateName}>{candidate.name}</span>
                          <span class={styles.mono}>{candidateKey(candidate)}</span>
                          <CandidateStatus
                            candidate={candidate}
                            match={match()}
                            rows={assignableRows()}
                            rowName={rowName}
                            onAssign={(rowId) => assign(candidate, rowId)}
                          />
                        </li>
                      );
                    }}
                  </For>
                </ul>
              </section>

              <section class={styles.section} aria-label="Apply to selected rows">
                <span class={styles.sectionTitle}>Apply to selected rows</span>
                <div class={styles.inline}>
                  <Select
                    label="Protocol"
                    options={KINDS}
                    optionValue={(kind: (typeof KINDS)[number]) => kind.value}
                    optionLabel={(kind: (typeof KINDS)[number]) => kind.label}
                    value={KINDS.find((kind) => kind.value === applyKind())}
                    onChange={(kind: (typeof KINDS)[number]) => setApplyKind(kind.value)}
                  />
                  <TextField
                    label="Port"
                    value={applyPort()}
                    onChange={setApplyPort}
                    placeholder={String(defaultPort(applyKind()))}
                    class={styles.narrow}
                  />
                  <Select
                    label="Credential source"
                    options={CREDENTIAL_SOURCES}
                    optionValue={(option: (typeof CREDENTIAL_SOURCES)[number]) => option.value}
                    optionLabel={(option: (typeof CREDENTIAL_SOURCES)[number]) => option.label}
                    value={CREDENTIAL_SOURCES.find((option) => option.value === applyCredential())}
                    onChange={(option: (typeof CREDENTIAL_SOURCES)[number]) => setApplyCredential(option.value)}
                  />
                  <Checkbox checked={applyTls()} onChange={setApplyTls} class={styles.checkboxField}>
                    Use TLS
                  </Checkbox>
                </div>
                <div>
                  <Button
                    variant="secondary"
                    disabled={!rows.some((row) => row.selected && !isRowCreated(row))}
                    onClick={applyToSelected}
                  >
                    Apply to selected
                  </Button>
                </div>
                <TextField
                  label="Shared credential"
                  type="password"
                  value={sharedCredential()}
                  onChange={setSharedCredential}
                  description="API key used by rows whose credential source is Shared. Never saved with the rows."
                />
              </section>

              <BatchRowsTable
                rows={rows}
                {...tableHandlers}
                mode="connect"
                preview={rowPreview()}
                existingNames={existingNames()}
              />
            </div>
          </Show>

          <Show when={step() === "review"}>
            <div class={styles.intake}>
              <div class={styles.summary}>
                <p>
                  {rows.length} Printer{rows.length === 1 ? "" : "s"} · {connectionCount()} with a Connection
                </p>
                <Show when={picker.selectedModel()}>
                  {(model) => (
                    <p>
                      Model: {model().model}
                      <Show when={picker.selectedVariant()}>{(variant) => ` · ${variant().printerVariant} mm nozzle`}</Show>
                    </p>
                  )}
                </Show>
                <p>Bed type: {bedTypeLabel(bedType())}</p>
                <p>Start safety: {startSafetyLabel()}</p>
              </div>
              <Show when={store()}>
                {(info) => (
                  <p class={styles.note}>
                    Credentials stored in: {info().kind === "keychain" ? "OS keychain" : "credentials.json"}
                  </p>
                )}
              </Show>
              <Switch checked={probe()} onChange={setProbe} disabled={busy()}>
                Test each Connection before saving it
              </Switch>
              <Show when={batchError()}>
                {(message) => (
                  <p class={styles.error} role="alert">
                    {message()}
                  </p>
                )}
              </Show>
              <BatchRowsTable
                rows={rows}
                {...tableHandlers}
                mode="results"
                preview={rowPreview()}
                existingNames={existingNames()}
                pending={pending()}
                unverified={unverified()}
                onSaveAnyway={(rowId) => void saveAnyway(rowId)}
              />
            </div>
          </Show>

          <div class={styles.footer}>
            <Show when={step() !== "shared"}>
              <Button variant="secondary" disabled={busy()} onClick={goBack}>
                Back
              </Button>
            </Show>
            <div class={styles.footerSpacer} />
            <Show when={step() === "shared"}>
              <Button variant="secondary" onClick={requestClose}>
                Cancel
              </Button>
            </Show>
            <Show
              when={step() === "review"}
              fallback={
                <Button variant="primary" disabled={!canLeave(step())} onClick={goNext}>
                  Next →
                </Button>
              }
            >
              <Show when={runningBatchId()}>
                <Button variant="secondary" onClick={cancelRunning}>
                  Cancel
                </Button>
              </Show>
              <Show when={hasResults() && !busy()}>
                <Button variant="secondary" onClick={requestClose}>
                  Done
                </Button>
              </Show>
              <Show
                when={hasResults()}
                fallback={
                  <Button variant="primary" disabled={busy() || !rowsValid()} onClick={() => void submit()}>
                    {busy() ? "Creating…" : "Create"}
                  </Button>
                }
              >
                <Button variant="primary" disabled={busy() || !canRetry()} onClick={() => void submit()}>
                  Retry failed
                </Button>
              </Show>
            </Show>
          </div>
        </div>
      </Dialog>

      <Dialog title="Close batch setup?" open={confirmOpen()} onOpenChange={setConfirmOpen}>
        <p class={styles.note}>
          {busy()
            ? "Rows that haven't been created yet will be cancelled, and their input discarded."
            : "Some rows weren't created or connected. Closing discards their input."}
        </p>
        <div class={styles.footer}>
          <div class={styles.footerSpacer} />
          <Button variant="secondary" onClick={() => setConfirmOpen(false)}>
            Keep editing
          </Button>
          <Button variant="danger" onClick={confirmClose}>
            Close anyway
          </Button>
        </div>
      </Dialog>
    </>
  );
}

interface CandidateStatusProps {
  candidate: DiscoveredPrinter;
  match: CandidateMatch | undefined;
  rows: BatchRowDraft[];
  rowName: (rowId: string) => string;
  onAssign: (rowId: string) => void;
}

/** One discovery candidate's D12 mapping: already configured, ambiguous,
 *  and unsupported candidates are shown but never assignable. */
function CandidateStatus(props: CandidateStatusProps) {
  const assignedRowId = () => (props.match?.kind === "assignable" ? props.match.rowId : undefined);
  return (
    <Branch>
      <Match when={props.match?.kind === "alreadyConfigured"}>
        <span class={styles.note}>Already configured</span>
      </Match>
      <Match when={props.match?.kind === "unsupported"}>
        <span class={styles.note}>Unsupported protocol</span>
      </Match>
      <Match when={props.match?.kind === "ambiguous"}>
        <span class={styles.warn}>Ambiguous — not assigned automatically</span>
      </Match>
      <Match when={assignedRowId()}>
        {(rowId) => <span class={styles.note}>Assigned to {props.rowName(rowId())}</span>}
      </Match>
      <Match when={props.match?.kind === "assignable"}>
        <Select
          label={`Assign ${props.candidate.name}`}
          placeholder="Choose a row"
          options={props.rows}
          optionValue={(row: BatchRowDraft) => row.rowId}
          optionLabel={(row: BatchRowDraft) => row.name}
          onChange={(row: BatchRowDraft) => props.onAssign(row.rowId)}
          class={styles.assign}
        />
      </Match>
    </Branch>
  );
}
