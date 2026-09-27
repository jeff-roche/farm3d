import { createEffect, createSignal, For, on, onMount, Show, type JSX } from "solid-js";
import { AlertDialog, Button } from "../design-system";
import { capabilities, loadCapabilities } from "../host-ops/capabilities-store";
import {
  cancelHostPrint,
  hostOperations,
  pauseHostPrint,
  reconcileHostOperation,
  resumeHostPrint,
} from "../host-ops/host-operations-store";
import {
  capabilityRefusalText,
  hostOperationFailureText,
  hostOperationLabel,
  inconclusiveReasonText,
  NO_LONGER_PENDING_TEXT,
} from "../host-ops/presentation";
import {
  controlOffer,
  controlRefusalText,
  startOffer,
  startRefusalText,
  statusOrUnknown,
  type ControlVerb,
} from "../host-ops/start-rule";
import { canAbandon, canReconcile as reconcileSupported, reconcileCapability, supports } from "../host-ops/reconcile-rule";
import type { HostOperation, PrinterCapabilities } from "../host-ops/types";
import { operationalLabel } from "../monitor/monitor-store";
import type { ResolvedPrinter } from "../printers/types";
import { AbandonReconciliationDialog } from "./AbandonReconciliationDialog";
import { queue } from "../queue/queue-store";
import { HostOperationAlert } from "./HostOperationAlert";
import { JobPanel } from "./JobPanel";
import { formatTemperature, nozzleReadings } from "./monitor-printer-presentation";
import { SeverityLabel } from "./SeverityLabel";
import { StartStagedDialog } from "./StartStagedDialog";
import styles from "./PrinterJobPanel.module.css";

export interface PrinterJobPanelProps {
  printer: ResolvedPrinter;
}

const CONTROLS: { verb: ControlVerb; label: string }[] = [
  { verb: "pause", label: "Pause" },
  { verb: "resume", label: "Resume" },
  { verb: "cancel", label: "Cancel print…" },
];

const SEND: Record<ControlVerb, (printerId: string) => Promise<unknown>> = {
  pause: pauseHostPrint,
  resume: resumeHostPrint,
  cancel: cancelHostPrint,
};

/** The Printer's **Job** tab (spec "Components"): the host's current print
 *  from telemetry, Pause/Resume/Cancel print…, what is staged on the
 *  Printer with **Start…**, and its Host Operations with **Check again**
 *  and **Abandon check…**. While the Printer has an active P7 Job, the
 *  Job's own panel replaces the raw controls and Stage/Start (decision 9). An unsupported control is never an enabled
 *  button; a disabled one says why in visible text. */
export function PrinterJobPanel(props: PrinterJobPanelProps) {
  const [capabilitiesError, setCapabilitiesError] = createSignal<unknown>(null);
  const held = () => capabilities.forPrinter(props.printer.id);
  const activeJob = () => queue.activeJobFor(props.printer.id);

  const loadHeld = () => {
    setCapabilitiesError(null);
    loadCapabilities(props.printer.id).catch(setCapabilitiesError);
  };
  onMount(() => {
    if (props.printer.connection && !held()) loadHeld();
  });

  return (
    <div class={styles.panel}>
      <Show
        when={props.printer.connection}
        fallback={<p class={styles.note}>This Printer has no Connection. Add one in Setup to stage and control prints.</p>}
      >
        <CurrentPrint printer={props.printer} />
        <Show when={capabilitiesError()}>
          {(error) => (
            <HostOperationAlert error={error()} fallback="farm3d couldn't read what this printer supports." onRetry={loadHeld} />
          )}
        </Show>
        {/* Decision 9: while a Job is active its own controls replace P6's
            raw Stage/Start, Pause, Resume, and Cancel. The Job's panel never
            waits on the capability load; only P6's raw parts do. */}
        <Show when={activeJob()}>
          {(job) => <JobPanel job={job()} printer={props.printer} showHostOperation={false} />}
        </Show>
        <Show when={held()} fallback={<Show when={!capabilitiesError()}><p class={styles.note}>Checking what this printer supports…</p></Show>}>
          {(record) => (
            <>
              <Show when={!activeJob()}>
                <Controls printer={props.printer} record={record()} />
                <Staged printer={props.printer} record={record()} />
              </Show>
              <Operations printer={props.printer} record={record()} />
            </>
          )}
        </Show>
      </Show>
    </div>
  );
}

function Section(props: { id: string; title: string; children: JSX.Element }) {
  return (
    <section class={styles.section} aria-labelledby={props.id}>
      <h3 id={props.id} class={styles.title}>{props.title}</h3>
      {props.children}
    </section>
  );
}

function CurrentPrint(props: { printer: ResolvedPrinter }) {
  const status = () => props.printer.runtimeStatus;
  const telemetry = () => status()?.telemetry;
  const progress = () => {
    const value = telemetry()?.progress;
    return value === undefined ? "—" : `${Math.round(value * 100)}%`;
  };
  const rows = () => [
    { label: "File", value: telemetry()?.jobName ?? "—", mono: true },
    { label: "State", value: operationalLabel(status()) },
    { label: "Progress", value: progress() },
    ...nozzleReadings(telemetry() ?? {}).map((reading) => ({ label: reading.label, value: reading.value })),
    { label: "Bed", value: formatTemperature(telemetry()?.bedTempC, telemetry()?.bedTargetC) },
  ];
  return (
    <Section id={`job-current-${props.printer.id}`} title="Current print">
      <dl class={styles.fields}>
        <For each={rows()}>
          {(row) => (
            <div class={styles.field}>
              <dt>{row.label}</dt>
              <dd class={"mono" in row && row.mono ? styles.mono : undefined}>{row.value}</dd>
            </div>
          )}
        </For>
      </dl>
    </Section>
  );
}

function Controls(props: { printer: ResolvedPrinter; record: PrinterCapabilities }) {
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<unknown>(null);
  const [confirmingCancel, setConfirmingCancel] = createSignal(false);
  let cancelTrigger: HTMLButtonElement | undefined;

  const hasUnresolved = () => hostOperations.unresolvedFor(props.printer.id) !== undefined;
  const shown = () => CONTROLS.filter((control) => supports(props.record, control.verb));
  const offerFor = (verb: ControlVerb) => controlOffer(statusOrUnknown(props.printer.runtimeStatus), verb, hasUnresolved());
  /** One line per distinct reason, so an unresolved row's sentence isn't
   *  repeated under all three buttons. */
  const reasons = () => [...new Set(shown().flatMap((control) => {
    const offer = offerFor(control.verb);
    return offer.offered || !offer.reason ? [] : [controlRefusalText(control.verb, offer.reason)];
  }))];

  /** Re-checked while the Cancel confirmation is open: if the print ends
   *  or a row becomes unresolved meanwhile, the confirmation can't send. */
  const cancelRefusal = () => {
    const offer = offerFor("cancel");
    return offer.offered ? undefined : controlRefusalText("cancel", offer.reason ?? "");
  };

  async function send(verb: ControlVerb) {
    if (pending()) return;
    setPending(true);
    setError(null);
    try {
      await SEND[verb](props.printer.id);
    } catch (e) {
      setError(e);
    } finally {
      setPending(false);
    }
  }

  return (
    <Show
      when={shown().length > 0}
      fallback={<p class={styles.note}>farm3d can't pause, resume, or cancel prints on this printer. Setup lists what it supports.</p>}
    >
      <div class={styles.controls}>
        <div class={styles.buttons}>
          <For each={shown()}>
            {(control) => (
              <Button
                ref={control.verb === "cancel" ? (element: HTMLButtonElement) => (cancelTrigger = element) : undefined}
                variant={control.verb === "cancel" ? "danger" : "secondary"}
                size="sm"
                disabled={!offerFor(control.verb).offered || pending()}
                onClick={() => (control.verb === "cancel" ? setConfirmingCancel(true) : void send(control.verb))}
              >
                {control.label}
              </Button>
            )}
          </For>
        </div>
        <For each={reasons()}>{(reason) => <p class={styles.note}>{reason}</p>}</For>
        <Show when={error()}>
          {(held) => <HostOperationAlert error={held()} fallback="The printer couldn't be reached." printerId={props.printer.id} />}
        </Show>
      </div>
      <AlertDialog
        title="Cancel this print?"
        description={`${props.printer.name} stops printing ${props.printer.runtimeStatus?.telemetry.jobName ?? "its current file"} now. A cancelled print can't be resumed.`}
        open={confirmingCancel()}
        onOpenChange={setConfirmingCancel}
        returnFocus={() => cancelTrigger}
      >
        <Show when={cancelRefusal()}>{(reason) => <p class={styles.note}>{reason()}</p>}</Show>
        <div class={styles.dialogActions}>
          <Button variant="secondary" onClick={() => setConfirmingCancel(false)}>Keep printing</Button>
          <Button
            variant="danger"
            disabled={cancelRefusal() !== undefined || pending()}
            onClick={() => {
              if (cancelRefusal() !== undefined) return;
              setConfirmingCancel(false);
              void send("cancel");
            }}
          >
            Cancel print
          </Button>
        </div>
      </AlertDialog>
    </Show>
  );
}

function Staged(props: { printer: ResolvedPrinter; record: PrinterCapabilities }) {
  const [starting, setStarting] = createSignal<HostOperation | null>(null);
  let lastTrigger: HTMLButtonElement | undefined;
  const staged = () => hostOperations.stagedFor(props.printer.id);
  const startState = () => props.record.capabilities.start;
  const offer = () => startOffer(
    statusOrUnknown(props.printer.runtimeStatus),
    hostOperations.unresolvedFor(props.printer.id) !== undefined,
  );
  const refusal = () => {
    const capability = startState();
    if (capability.status !== "supported") {
      return capabilityRefusalText("start", capability, props.record.adapterKind);
    }
    const current = offer();
    return current.offered ? undefined : startRefusalText(current.reason);
  };

  return (
    <Section id={`job-staged-${props.printer.id}`} title="Staged on this Printer">
      <Show when={staged().length > 0} fallback={<p class={styles.note}>Nothing is staged on this Printer.</p>}>
        <ul class={styles.list}>
          <For each={staged()}>
            {(operation) => (
              <li class={styles.row}>
                <span class={styles.mono}>{operation.hostPath}</span>
                <Button
                  size="sm"
                  variant="primary"
                  disabled={refusal() !== undefined}
                  onClick={(event) => {
                    lastTrigger = event.currentTarget;
                    setStarting(operation);
                  }}
                >
                  Start…
                </Button>
              </li>
            )}
          </For>
        </ul>
        <Show when={refusal()}>{(reason) => <p class={styles.note}>{reason()}</p>}</Show>
      </Show>
      <Show when={starting()}>
        {(operation) => (
          <StartStagedDialog
            open
            onOpenChange={(open) => !open && setStarting(null)}
            printer={props.printer}
            staged={operation()}
            returnFocus={() => lastTrigger}
          />
        )}
      </Show>
    </Section>
  );
}

function Operations(props: { printer: ResolvedPrinter; record: PrinterCapabilities }) {
  const [abandoning, setAbandoning] = createSignal<HostOperation | null>(null);
  const [checking, setChecking] = createSignal<string | null>(null);
  const [error, setError] = createSignal<unknown>(null);
  const [announcement, setAnnouncement] = createSignal("");
  let lastTrigger: HTMLButtonElement | undefined;

  const unresolved = () => hostOperations.unresolvedFor(props.printer.id);
  const rows = () => {
    const pending = unresolved();
    return [...(pending ? [pending] : []), ...hostOperations.recentFor(props.printer.id)];
  };

  // One polite announcement per state change (spec "Accessibility"). The
  // first pass only records what is there; nothing is announced on open.
  createEffect(on(
    () => rows().map((operation) => [operation.id, operation.state, operation.kind, operation.hostPath, operation.resolution] as const),
    (current, previous) => {
      if (!previous) return;
      const before = new Map(previous.map(([id, state]) => [id, state]));
      const changed = current.filter(([id, state]) => before.get(id) !== state);
      if (changed.length === 0) return;
      setAnnouncement(changed.map(([, state, kind, hostPath, resolution]) =>
        `${hostOperationLabel({ kind, state, resolution }).text}: ${hostPath}`).join(". "));
    },
  ));

  const canReconcile = (operation: HostOperation) => reconcileSupported(operation, props.record);

  async function checkAgain(operation: HostOperation) {
    if (checking()) return;
    setChecking(operation.id);
    setError(null);
    try {
      await reconcileHostOperation(operation.id);
    } catch (e) {
      setError(e);
    } finally {
      setChecking(null);
    }
  }

  return (
    <Section id={`job-operations-${props.printer.id}`} title="Printer operations">
      <Show when={rows().length > 0} fallback={<p class={styles.note}>farm3d hasn't sent anything to this Printer yet.</p>}>
        <ul class={styles.list}>
          <For each={rows()}>
            {(operation) => {
              const label = () => hostOperationLabel(operation);
              const uncertain = () => operation.state === "uncertain";
              const abandonable = () => canAbandon(operation, props.record);
              return (
                <li class={styles.operation}>
                  <div class={styles.row}>
                    <SeverityLabel severity={label().severity} text={label().text} />
                    <span class={styles.mono}>{operation.hostPath}</span>
                  </div>
                  <Show when={operation.failure}>{(failure) => <p class={styles.detail}>{hostOperationFailureText(failure().code)}</p>}</Show>
                  <Show when={!operation.failure && operation.state !== "succeeded" && operation.lastAttempt}>
                    {(attempt) => <p class={styles.detail}>{inconclusiveReasonText(attempt().reason)}</p>}
                  </Show>
                  <Show when={operation.noLongerPending && operation.state !== "succeeded"}>
                    <p class={styles.detail}>{NO_LONGER_PENDING_TEXT}</p>
                  </Show>
                  <Show when={operation.abandonNote}>{(note) => <p class={styles.detail}>Note: {note()}</p>}</Show>
                  <Show when={uncertain()}>
                    <div class={styles.buttons}>
                      <Button
                        size="sm"
                        variant="secondary"
                        disabled={!canReconcile(operation) || checking() !== null}
                        onClick={() => void checkAgain(operation)}
                      >
                        Check again
                      </Button>
                      <Button
                        size="sm"
                        variant="secondary"
                        disabled={!abandonable()}
                        onClick={(event) => {
                          lastTrigger = event.currentTarget;
                          setAbandoning(operation);
                        }}
                      >
                        Abandon check…
                      </Button>
                    </div>
                    <Show when={!canReconcile(operation)}>
                      <p class={styles.detail}>
                        farm3d can't check this printer.{" "}
                        {capabilityRefusalText(
                          reconcileCapability(operation),
                          props.record.capabilities[reconcileCapability(operation)],
                          props.record.adapterKind,
                        )}
                      </p>
                    </Show>
                    <Show when={!abandonable()}>
                      <p class={styles.detail}>farm3d can stop checking once it has checked at least once.</p>
                    </Show>
                  </Show>
                </li>
              );
            }}
          </For>
        </ul>
      </Show>
      <Show when={error()}>
        {(held) => <HostOperationAlert error={held()} fallback="farm3d couldn't check the printer." printerId={props.printer.id} />}
      </Show>
      <p class={styles.live} aria-live="polite">{announcement()}</p>
      <Show when={abandoning()}>
        {(operation) => (
          <AbandonReconciliationDialog
            open
            onOpenChange={(open) => !open && setAbandoning(null)}
            operation={operation()}
            printerName={props.printer.name}
            returnFocus={() => lastTrigger}
          />
        )}
      </Show>
    </Section>
  );
}
