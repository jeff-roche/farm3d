import { Dialog as KDialog } from "@kobalte/core/dialog";
import { createEffect, createSignal, on, Show, type JSX } from "solid-js";
import { Button, SeverityMarker } from "../design-system";
import { isCommandError } from "../ipc/client";
import {
  acknowledgeAttentionEvent,
  attention,
  markAttentionRead,
  resolveAttentionEvent,
} from "../attention/attention-store";
import { attentionSeverityLabel, conditionKindLabel } from "../attention/presentation";
import { incidentTarget, openTargetFor, sourceId } from "../attention/deep-link";
import type { AttentionEvent, AttentionSourceKind } from "../attention/types";
import { serializeNavigationTarget, type NavigationTarget } from "../navigation/navigation-store";
import { formatDateTime } from "../slicing/revision-presentation";
import { printers } from "../printers/printer-store";
import { queue } from "../queue/queue-store";
import { spoolState } from "../spools/spool-store";
import styles from "./AttentionEventDetail.module.css";

export interface AttentionEventDetailProps {
  event: AttentionEvent;
  mode: "inline" | "overlay";
  onClose: () => void;
}

const SOURCE_KIND_LABEL: Record<AttentionSourceKind, string> = {
  printer: "Printer",
  job: "Job",
  reconciliationRequirement: "Job",
  spool: "Spool",
};

const ORIGIN_LABEL = { live: "Live", backfill: "Found at startup" } satisfies Record<AttentionEvent["origin"], string>;

function subjectName(event: AttentionEvent): string {
  switch (event.source.kind) {
    case "printer":
      return event.subject.printerName ?? "Unknown Printer";
    case "job":
    case "reconciliationRequirement":
      return event.subject.jobLabel ?? "Unknown Job";
    case "spool":
      return event.subject.spoolLabel
        ?? (event.subject.spoolNumber !== null ? `Spool #${event.subject.spoolNumber}` : "Unknown Spool");
  }
}

/** Every Printer/Job/Spool id this frontend currently knows about, the
 *  same sets `App.tsx`'s `navigationContext` builds -- used only to route
 *  "Open source" and to detect a deleted source (never persisted,
 *  global constraint 4: Rust alone decides the Event itself). */
function knownSourceIds(): string[] {
  return [
    ...printers().map((printer) => printer.id),
    ...spoolState.spools.map((spool) => spool.id),
    ...queue.entries().flatMap((entry) => (entry.jobId ? [entry.id, entry.jobId] : [entry.id])),
    ...queue.history().flatMap((entry) => (entry.jobId ? [entry.id, entry.jobId] : [entry.id])),
  ];
}

function goTo(target: NavigationTarget): void {
  window.location.hash = serializeNavigationTarget(target).slice(1);
}

/** The Monitor dock's content for an `attention` selection (spec
 *  "Frontend architecture"): severity, summary, subject, source,
 *  timestamps, observation count, origin, the recurrence and Incident
 *  links, Acknowledge/Resolve (from Rust's own `allowedActions`, never
 *  re-derived here per global constraint 4), and "Open source". Self-wraps
 *  inline/overlay like `PrinterDetailDock`/`MonitorQueueDock`, since it
 *  takes over the same dock slot they do. */
export function AttentionEventDetail(props: AttentionEventDetailProps) {
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<unknown>(null);

  // Opening the detail marks the Event read (spec): once per distinct
  // Event shown, and only when it's actually unread.
  createEffect(on(() => props.event.id, (id) => {
    const current = attention.event(id) ?? props.event;
    if (current.readAt === null) void markAttentionRead([id]).catch(() => {});
  }));

  const sourceExists = () => {
    const id = sourceId(props.event);
    return id !== null && knownSourceIds().includes(id);
  };

  async function run(action: () => Promise<unknown>) {
    if (pending()) return;
    setPending(true);
    setError(null);
    try {
      await action();
    } catch (e) {
      setError(e);
    } finally {
      setPending(false);
    }
  }

  const content = (title: (text: string) => JSX.Element) => (
    <div class={styles.detail}>
      <header class={styles.header}>
        <div class={styles.headerText}>
          <SeverityMarker severity={props.event.severity} label={attentionSeverityLabel(props.event.severity)} />
          {title(props.event.summary)}
          <p class={styles.condition}>{conditionKindLabel(props.event.condition)}</p>
        </div>
        <Button variant="ghost" class={styles.close} onClick={props.onClose}>Close</Button>
      </header>

      <dl class={styles.fields}>
        <div class={styles.field}>
          <dt>Subject</dt>
          <dd>{subjectName(props.event)}</dd>
        </div>
        <div class={styles.field}>
          <dt>Source</dt>
          <dd>
            <span>{SOURCE_KIND_LABEL[props.event.source.kind]}</span>
            <Show when={!sourceExists()}>
              <span class={styles.deletedNote}>{`${SOURCE_KIND_LABEL[props.event.source.kind]} deleted`}</span>
            </Show>
          </dd>
        </div>
        <div class={styles.field}>
          <dt>First observed</dt>
          <dd>{formatDateTime(props.event.firstObservedAt)}</dd>
        </div>
        <div class={styles.field}>
          <dt>Origin</dt>
          <dd>{ORIGIN_LABEL[props.event.origin]}</dd>
        </div>
        <Show when={props.event.resolvedAt}>
          <div class={styles.field}>
            <dt>Resolved</dt>
            <dd>{formatDateTime(props.event.resolvedAt!)}</dd>
          </div>
        </Show>
      </dl>

      <Show when={props.event.recurrenceOf}>
        {(recurrenceOf) => (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => goTo({ version: 1, destination: "monitor", selection: { kind: "attention", id: recurrenceOf() } })}
          >
            View earlier occurrence
          </Button>
        )}
      </Show>

      <Show when={props.event.incidentId}>
        {(incidentId) => (
          <Button variant="ghost" size="sm" onClick={() => goTo(incidentTarget(incidentId()))}>
            View Incident
          </Button>
        )}
      </Show>

      <div class={styles.actions}>
        <Show when={props.event.allowedActions.includes("acknowledge")}>
          <Button
            variant="secondary"
            disabled={pending()}
            onClick={() => void run(() => acknowledgeAttentionEvent(props.event.id))}
          >
            Acknowledge
          </Button>
        </Show>
        <Show when={props.event.allowedActions.includes("resolve")}>
          <Button
            variant="primary"
            disabled={pending()}
            onClick={() => void run(() => resolveAttentionEvent(props.event.id))}
          >
            Resolve
          </Button>
        </Show>
        <Button
          variant="ghost"
          onClick={() => goTo(openTargetFor(props.event, knownSourceIds()))}
        >
          Open source
        </Button>
      </div>

      <Show when={error()}>
        {(held) => {
          const err = held();
          return (
            <p class={styles.error} role="alert">
              {isCommandError(err) ? err.message : "This Attention Event couldn't be updated."}
            </p>
          );
        }}
      </Show>
    </div>
  );

  return (
    <Show
      when={props.mode === "overlay"}
      fallback={<aside class={styles.inline} aria-label="Attention Event">{content((text) => <h2 class={styles.title}>{text}</h2>)}</aside>}
    >
      <KDialog open onOpenChange={(open) => !open && props.onClose()}>
        <KDialog.Portal>
          <KDialog.Overlay class={styles.overlay} />
          <KDialog.Content class={styles.overlayContent} onCloseAutoFocus={(event) => event.preventDefault()}>
            {content((text) => <KDialog.Title class={styles.title}>{text}</KDialog.Title>)}
          </KDialog.Content>
        </KDialog.Portal>
      </KDialog>
    </Show>
  );
}
