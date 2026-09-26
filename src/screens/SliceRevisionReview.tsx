import { IconAlertTriangle, IconArrowLeft } from "@tabler/icons-solidjs";
import {
  createMemo,
  createResource,
  createSignal,
  createUniqueId,
  For,
  onCleanup,
  onMount,
  Show,
  type JSX,
} from "solid-js";
import { Button } from "../design-system";
import { isCommandError } from "../ipc/client";
import { printers } from "../printers/printer-store";
import {
  controlRows,
  estimateRows,
  FACT_KEYS,
  FACT_LABELS,
  factValueText,
  formatDateTime,
  NEEDS_MANUAL_PRINTER,
  profileRows,
  revisionKindLabel,
  revisionTitle,
  runtimeLine,
  runtimeVersionsDiffer,
  type FactRow,
} from "../slicing/revision-presentation";
import { logSegments } from "../slicing/slice-presentation";
import { loadSliceRevision, loadSliceRevisionLog, slicing } from "../slicing/slicing-store";
import type { SliceOperationLog, SliceRevisionRecord } from "../slicing/types";
import { DeleteSliceRevisionDialog } from "./DeleteSliceRevisionDialog";
import { StageOnPrinterDialog } from "./StageOnPrinterDialog";
import { ProvenanceBadge } from "./ProvenanceBadge";
import styles from "./SliceRevisionReview.module.css";

export interface SliceRevisionReviewProps {
  sliceRevisionId: string;
  /** **Back to Model details**. */
  onBack: () => void;
  /** The revision was deleted from here. */
  onDeleted: (sliceRevisionId: string) => void;
}

/** P12: the queue-handoff intent. Queueing arrives with P7. */
const QUEUE_LATER_REASON = "The Queue arrives in a later version.";

function Rows(props: { rows: FactRow[] }) {
  return (
    <dl class={styles.rows}>
      <For each={props.rows}>
        {(row) => (
          <>
            <dt>{row.label}</dt>
            <dd>{row.value}</dd>
          </>
        )}
      </For>
    </dl>
  );
}

function Section(props: { title: string; children: JSX.Element }) {
  const id = createUniqueId();
  return (
    <section class={styles.section} aria-labelledby={id}>
      <h4 id={id} class={styles.sectionHeading}>{props.title}</h4>
      {props.children}
    </section>
  );
}

/** D21's Slice Revision review, shown in the Library's details dock: what
 *  was sliced from what, for which target, the estimates, every fact with
 *  where it came from, the runtime, the log, the disabled **Add to Queue…**
 *  intent (P12) and **Delete…**. A revision never changes, so the record
 *  loads once. */
export function SliceRevisionReview(props: SliceRevisionReviewProps) {
  const [record] = createResource(() => props.sliceRevisionId, (id) => loadSliceRevision(id));
  // Reading an errored resource throws, so check the error first.
  const loaded = (): SliceRevisionRecord | undefined => (record.error ? undefined : record());
  // Deleted elsewhere (an event) after this opened.
  const seen = createMemo<boolean>((was) => was || slicing.revision(props.sliceRevisionId) !== undefined, false);
  const removed = () => seen() && slicing.revision(props.sliceRevisionId) === undefined;
  const [deleting, setDeleting] = createSignal(false);

  let heading: HTMLHeadingElement | undefined;
  onMount(() => heading?.focus());

  const loadError = () => {
    const error = record.error as unknown;
    if (!error) return undefined;
    return isCommandError(error) && error.code === "NOT_FOUND"
      ? "This Slice Revision was deleted."
      : "This Slice Revision couldn't be loaded.";
  };

  return (
    <div class={styles.review}>
      <div class={styles.toolbar}>
        <Button variant="ghost" size="sm" onClick={() => props.onBack()}>
          <IconArrowLeft size={14} aria-hidden="true" /> Model details
        </Button>
      </div>
      <h3 ref={heading} class={styles.heading} tabIndex={-1}>
        {loaded() ? revisionTitle(loaded()!) : "Slice Revision"}
      </h3>
      <Show when={!removed()} fallback={<p class={styles.note} role="status">This Slice Revision was deleted.</p>}>
        <Show when={!loadError()} fallback={<p class={styles.note} role="alert">{loadError()}</p>}>
          <Show when={loaded()} fallback={<p class={styles.note} role="status">Loading the Slice Revision…</p>}>
            {(revision) => (
              <RevisionBody revision={revision()} onDelete={() => setDeleting(true)} />
            )}
          </Show>
        </Show>
      </Show>
      <Show when={deleting() && slicing.revision(props.sliceRevisionId)}>
        {(summary) => (
          <DeleteSliceRevisionDialog
            revision={summary()}
            onClose={() => setDeleting(false)}
            onDeleted={(id) => {
              setDeleting(false);
              props.onDeleted(id);
            }}
          />
        )}
      </Show>
    </div>
  );
}

function RevisionBody(props: { revision: SliceRevisionRecord; onDelete: () => void }) {
  const revision = () => props.revision;
  const queueReasonId = createUniqueId();
  const [staging, setStaging] = createSignal(false);
  let stageTrigger: HTMLButtonElement | undefined;
  const absentCount = () => FACT_KEYS.filter((key) => revision().facts[key].provenance === "absent").length;

  const targetRows = (): FactRow[] => {
    const target = revision().target;
    if (!target) return [];
    const rows: FactRow[] = [];
    const chosen = target.target;
    if (chosen.kind === "printer") {
      const printer = printers().find((candidate) => candidate.id === chosen.printerId);
      rows.push({ label: "Printer", value: printer?.name ?? "A Printer that is no longer here" });
    }
    rows.push(...profileRows(target.profile));
    rows.push(
      { label: "Machine preset", value: target.machinePreset },
      { label: "Process preset", value: target.processPreset },
      { label: "Filament preset", value: target.filamentPreset },
    );
    return rows;
  };

  return (
    <>
      <dl class={styles.rows}>
        <dt>Kind</dt>
        <dd>{revisionKindLabel(revision().kind)}</dd>
        <Show when={revision().plate}>
          {(plate) => (
            <>
              <dt>Plate</dt>
              <dd>{plate().plateName ? `${plate().plateIndex}: ${plate().plateName}` : String(plate().plateIndex)}</dd>
            </>
          )}
        </Show>
        <dt>Source</dt>
        <dd>Revision {revision().sourceRevisionSequence}</dd>
        <dt>Created</dt>
        <dd><time datetime={revision().createdAt}>{formatDateTime(revision().createdAt)}</time></dd>
        <dt>Target</dt>
        <dd>{revision().targetLabel}</dd>
      </dl>

      <div class={styles.actions}>
        <Button variant="primary" disabled aria-describedby={queueReasonId}>Add to Queue…</Button>
        <Button ref={stageTrigger} variant="secondary" onClick={() => setStaging(true)}>Stage on Printer…</Button>
        <Button variant="secondary" onClick={() => props.onDelete()}>Delete…</Button>
      </div>
      <p id={queueReasonId} class={styles.note}>{QUEUE_LATER_REASON}</p>
      <StageOnPrinterDialog
        open={staging()}
        onOpenChange={setStaging}
        sliceRevisionId={revision().id}
        revisionTitle={revisionTitle(revision())}
        returnFocus={() => stageTrigger}
      />

      <Section title="Facts">
        <Show when={revision().requiresManualPrinterSelection}>
          <p class={styles.manual}>
            <IconAlertTriangle size={14} aria-hidden="true" />
            <span>
              {NEEDS_MANUAL_PRINTER}: {absentCount()} {absentCount() === 1 ? "fact" : "facts"} not provided, so a
              Printer will be chosen by hand when this is queued.
            </span>
          </p>
        </Show>
        <ul class={styles.facts}>
          <For each={FACT_KEYS}>
            {(key) => (
              <li class={styles.fact} data-fact={key}>
                <span class={styles.factLabel}>{FACT_LABELS[key]}</span>
                <span class={styles.factValue}>{factValueText(revision().facts, key) ?? "—"}</span>
                <span class={styles.factBadge}>
                  <ProvenanceBadge provenance={revision().facts[key].provenance} />
                </span>
              </li>
            )}
          </For>
        </ul>
        <Show when={revision().facts.printerProfile.value}>
          {(profile) => (
            <Show when={revision().kind === "external"}>
              <Rows rows={profileRows(profile(), { withProfileName: false })} />
            </Show>
          )}
        </Show>
      </Section>

      <Show when={revision().target}>
        {(target) => (
          <Section title="Target">
            <Rows rows={targetRows()} />
            <Show
              when={controlRows(target().controls).length > 0}
              fallback={<p class={styles.note}>Every setting came from the process preset.</p>}
            >
              <Rows rows={controlRows(target().controls)} />
              <p class={styles.note}>Other settings came from the process preset.</p>
            </Show>
          </Section>
        )}
      </Show>

      <Section title="Estimates">
        <Show
          when={revision().estimates}
          fallback={<p class={styles.note}>farm3d didn't slice this file, so it has no estimates of its own.</p>}
        >
          {(estimates) => (
            <Show when={estimateRows(estimates()).length > 0} fallback={<p class={styles.note}>OrcaSlicer gave no estimates.</p>}>
              <Rows rows={estimateRows(estimates())} />
            </Show>
          )}
        </Show>
        <Show when={revision().kind === "external"}>
          <ClaimedEstimates revision={revision()} />
        </Show>
      </Section>

      <Show when={revision().runtime}>
        {(runtime) => (
          <Section title="Runtime">
            <p class={styles.text}>{runtimeLine(runtime())}</p>
            <Show when={runtimeVersionsDiffer(runtime())}>
              <p class={styles.manual}>
                <IconAlertTriangle size={14} aria-hidden="true" />
                <span>The engine and its presets were different versions when this was sliced.</span>
              </p>
            </Show>
          </Section>
        )}
      </Show>

      <Section title="Log">
        <RevisionLog revision={revision()} />
      </Section>
    </>
  );
}

function ClaimedEstimates(props: { revision: SliceRevisionRecord }) {
  const id = createUniqueId();
  const rows = () => (props.revision.claimedEstimates ? estimateRows(props.revision.claimedEstimates) : []);
  return (
    <div class={styles.claims} role="group" aria-labelledby={id}>
      <h5 id={id} class={styles.claimsHeading}>What the file says (not verified)</h5>
      <Show when={props.revision.producer}>
        {(producer) => <p class={styles.text}>Made by {producer().name} {producer().version}</p>}
      </Show>
      <Show when={rows().length > 0} fallback={<p class={styles.note}>The file gives no estimates.</p>}>
        <Rows rows={rows()} />
      </Show>
    </div>
  );
}

type LogState =
  | { status: "idle" | "loading" }
  | { status: "loaded"; log: SliceOperationLog }
  | { status: "failed"; message: string };

/** The read-only log of the slice that made this revision (farm3d
 *  revisions), read by the revision's own id so it outlives its
 *  operation. External G-code has none. */
function RevisionLog(props: { revision: SliceRevisionRecord }) {
  const [open, setOpen] = createSignal(false);
  const [log, setLog] = createSignal<LogState>({ status: "idle" });
  const logId = createUniqueId();
  let disposed = false;
  onCleanup(() => { disposed = true; });

  const toggle = async () => {
    const next = !open();
    setOpen(next);
    if (!next || log().status === "loaded" || log().status === "loading") return;
    setLog({ status: "loading" });
    try {
      const loaded = (await loadSliceRevisionLog(props.revision.id)).log;
      if (disposed) return;
      setLog(loaded ? { status: "loaded", log: loaded } : { status: "failed", message: "This revision has no log." });
    } catch (error) {
      if (!disposed) setLog({ status: "failed", message: isCommandError(error) ? error.message : "The log couldn't be loaded." });
    }
  };

  const loadedLog = () => {
    const held = log();
    return held.status === "loaded" ? held.log : undefined;
  };
  const logFailure = () => {
    const held = log();
    return held.status === "failed" ? held.message : undefined;
  };
  const segments = () => {
    const held = loadedLog();
    return held ? logSegments(held.text, held.noiseLines) : [];
  };

  return (
    <Show
      when={props.revision.kind === "farm3d"}
      fallback={<p class={styles.note}>External G-code has no slicing log: farm3d didn't slice it.</p>}
    >
      <div class={styles.actions}>
        <Button
          variant="ghost"
          size="sm"
          aria-expanded={open()}
          aria-controls={open() ? logId : undefined}
          onClick={() => void toggle()}
        >
          {open() ? "Hide log" : "Show log"}
        </Button>
      </div>
      <Show when={open()}>
        <Show when={logFailure()}>{(message) => <p class={styles.note}>{message()}</p>}</Show>
        <Show when={loadedLog()?.text === ""}>
          <p class={styles.note}>This slice left no log.</p>
        </Show>
        <Show when={loadedLog()?.truncated}>
          <p class={styles.note}>The middle of this log was left out because it was too long.</p>
        </Show>
        <Show when={(loadedLog()?.noiseLines.length ?? 0) > 0}>
          <p class={styles.note}>Dimmed lines are known, harmless messages.</p>
        </Show>
        <pre
          id={logId}
          class={styles.log}
          tabIndex={0}
          aria-label={`Log for ${revisionTitle(props.revision)}`}
          aria-busy={log().status === "loading"}
        >
          <Show when={log().status === "loaded"} fallback={log().status === "loading" ? "Loading the log…" : ""}>
            <For each={segments()}>
              {(segment) => (segment.noise
                ? <span class={styles.noise} data-noise="">{segment.text}</span>
                : segment.text)}
            </For>
          </Show>
        </pre>
      </Show>
    </Show>
  );
}
