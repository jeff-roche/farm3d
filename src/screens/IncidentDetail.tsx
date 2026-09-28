import { Dialog as KDialog } from "@kobalte/core/dialog";
import { createEffect, createSignal, For, on, onCleanup, Show, type JSX } from "solid-js";
import { Button, SeverityMarker, Textarea, Timeline } from "../design-system";
import type { TimelineItem } from "../design-system";
import { isCommandError } from "../ipc/client";
import { addIncidentNote, getIncident, watchIncidentDetail } from "../incidents/incident-store";
import { useSnapshotImage } from "../cameras/useSnapshotImage";
import {
  attentionResolutionLabel,
  attentionSeverityLabel,
  conditionKindLabel,
  evidenceSkipReasonLabel,
  incidentEntryKindLabel,
  pruneReasonLabel,
  snapshotAltText,
  snapshotTriggerLabel,
} from "../attention/presentation";
import { jobEventKindLabel } from "../queue/presentation";
import type { CameraSnapshot, IncidentDetail as IncidentDetailRecord, IncidentEntry } from "../attention/types";
import { serializeNavigationTarget, type NavigationTarget } from "../navigation/navigation-store";
import { formatDateTime } from "../slicing/revision-presentation";
import { SnapshotViewerDialog } from "./SnapshotViewerDialog";
import styles from "./IncidentDetail.module.css";

export interface IncidentDetailProps {
  incidentId: string;
  mode: "inline" | "overlay";
  onClose: () => void;
}

function goTo(target: NavigationTarget): void {
  window.location.hash = serializeNavigationTarget(target).slice(1);
}

function eventTarget(eventId: string): NavigationTarget {
  return { version: 1, destination: "monitor", selection: { kind: "attention", id: eventId } };
}

/** One incident-side timeline entry, as text (spec "Incident detail":
 *  "every kind rendered as text") -- every `IncidentEntryKind` handled, no
 *  Rust decision re-derived (global constraint 4: the *kind*, *detail*, and
 *  *reason* all come straight from the row). */
function incidentEntryText(entry: IncidentEntry): { title: string; detail?: string } {
  const base = incidentEntryKindLabel(entry.kind);
  const d = entry.detail;
  switch (d.kind) {
    case "opened":
    case "eventLinked":
    case "reopened":
      return { title: base };
    case "eventAcknowledged":
      return { title: base, detail: d.by === "operator" ? "By the operator." : "By farm3d." };
    case "eventResolved":
      return { title: base, detail: attentionResolutionLabel(d.resolution) };
    case "evidenceCaptured":
      return { title: base, detail: `${snapshotTriggerLabel(d.trigger)} capture.` };
    case "evidenceSkipped":
      return {
        title: base,
        detail: `${evidenceSkipReasonLabel(d.reason)}${d.errorKind ? ` (${d.errorKind})` : ""}`,
      };
    case "evidencePruned":
      return { title: base, detail: pruneReasonLabel(d.reason) };
    case "evidencePinned":
    case "evidenceUnpinned":
      return { title: base };
    case "noteAdded":
      return { title: base, detail: d.text };
    case "closed":
      return { title: base };
  }
}

/** The Monitor dock's content for an `incident` selection (spec "Frontend
 *  architecture"): header, the merged timeline (`Timeline`), linked Events,
 *  notes, and evidence thumbnails. Self-wraps inline/overlay like
 *  `AttentionEventDetail`/`PrinterDetailDock`, since it takes over the same
 *  dock slot they do. */
export function IncidentDetail(props: IncidentDetailProps) {
  const [detail, setDetail] = createSignal<IncidentDetailRecord | undefined>(undefined);
  const [loadError, setLoadError] = createSignal<unknown>(null);
  const [loading, setLoading] = createSignal(true);
  const [noteText, setNoteText] = createSignal("");
  const [notePending, setNotePending] = createSignal(false);
  const [noteError, setNoteError] = createSignal<unknown>(null);
  const [viewerSnapshotId, setViewerSnapshotId] = createSignal<string | null>(null);

  createEffect(on(() => props.incidentId, (id) => {
    setLoading(true);
    setLoadError(null);
    let cancelled = false;
    getIncident(id)
      .then((loaded) => { if (!cancelled) { setDetail(loaded); setLoading(false); } })
      .catch((e) => { if (!cancelled) { setLoadError(e); setLoading(false); } });
    const stop = watchIncidentDetail(id, (loaded) => setDetail(loaded));
    onCleanup(() => { cancelled = true; stop(); });
  }));

  async function submitNote(): Promise<void> {
    const text = noteText().trim();
    if (!text || notePending()) return;
    setNotePending(true);
    setNoteError(null);
    try {
      const updated = await addIncidentNote(props.incidentId, text);
      setDetail(updated);
      setNoteText("");
    } catch (e) {
      setNoteError(e);
    } finally {
      setNotePending(false);
    }
  }

  const timelineItems = (): TimelineItem[] => (detail()?.timeline ?? []).map((item) => {
    if (item.source === "incident") {
      const { title, detail: text } = incidentEntryText(item.entry);
      return {
        id: item.entry.id,
        at: item.entry.at,
        title,
        detail: text ? <span>{text}</span> : undefined,
        marker: item.entry.kind === "noteAdded" ? "muted" : "default",
      };
    }
    return { id: item.event.id, at: item.event.at, title: jobEventKindLabel(item.event.kind), marker: "muted" };
  });

  const viewerSnapshot = () => detail()?.snapshots.find((snapshot) => snapshot.id === viewerSnapshotId());

  const content = (title: (text: string) => JSX.Element) => (
    <div class={styles.detail}>
      <header class={styles.header}>
        <div class={styles.headerText}>
          {title(detail() ? conditionKindLabel(detail()!.incident.kind) : "Incident")}
          <Show when={detail()}>
            {(held) => <p class={styles.state}>{held().incident.state === "open" ? "Open" : "Closed"}</p>}
          </Show>
        </div>
        <Button variant="ghost" class={styles.close} onClick={props.onClose}>Close</Button>
      </header>

      <Show when={loading()}>
        <p class={styles.status} role="status">Loading the Incident…</p>
      </Show>
      <Show when={loadError()}>
        {(held) => {
          const err = held();
          return (
            <p class={styles.error} role="alert">
              {isCommandError(err) ? err.message : "This Incident couldn't be loaded."}
            </p>
          );
        }}
      </Show>

      <Show when={detail()}>
        {(held) => {
          const incident = () => held().incident;
          const printerName = () => incident().printerSnapshot.name;
          return (
            <>
              <dl class={styles.fields}>
                <div class={styles.field}>
                  <dt>Printer</dt>
                  <dd>
                    {printerName()}
                    <Show when={incident().printerSnapshot.location}>
                      {(location) => <span class={styles.location}> — {location()}</span>}
                    </Show>
                  </dd>
                </div>
                <div class={styles.field}>
                  <dt>Opened</dt>
                  <dd>{formatDateTime(incident().openedAt)}</dd>
                </div>
                <Show when={incident().closedAt}>
                  <div class={styles.field}>
                    <dt>Closed</dt>
                    <dd>{formatDateTime(incident().closedAt!)}</dd>
                  </div>
                </Show>
              </dl>

              <section class={styles.section} aria-labelledby="incident-events-title">
                <h3 id="incident-events-title" class={styles.sectionTitle}>Linked Events</h3>
                <Show when={held().events.length > 0} fallback={<p class={styles.empty}>No linked Events.</p>}>
                  <ul class={styles.eventList}>
                    <For each={held().events}>
                      {(event) => (
                        <li>
                          <button type="button" class={styles.eventLink} onClick={() => goTo(eventTarget(event.id))}>
                            <SeverityMarker severity={event.severity} label={attentionSeverityLabel(event.severity)} />
                            <span>{event.summary}</span>
                          </button>
                        </li>
                      )}
                    </For>
                  </ul>
                </Show>
              </section>

              <section class={styles.section} aria-labelledby="incident-timeline-title">
                <h3 id="incident-timeline-title" class={styles.sectionTitle}>Timeline</h3>
                <Timeline label={`Incident timeline for ${printerName()}`} items={timelineItems()} />
              </section>

              <section class={styles.section} aria-labelledby="incident-evidence-title">
                <h3 id="incident-evidence-title" class={styles.sectionTitle}>Evidence</h3>
                <Show when={held().snapshots.length > 0} fallback={<p class={styles.empty}>No evidence captured.</p>}>
                  <ul class={styles.evidenceList}>
                    <For each={held().snapshots}>
                      {(snapshot) => (
                        <li>
                          <EvidenceThumbnail
                            snapshot={snapshot}
                            printerName={printerName()}
                            onOpen={() => setViewerSnapshotId(snapshot.id)}
                          />
                        </li>
                      )}
                    </For>
                  </ul>
                </Show>
              </section>

              <section class={styles.section} aria-labelledby="incident-notes-title">
                <h3 id="incident-notes-title" class={styles.sectionTitle}>Notes</h3>
                <Textarea
                  label="Add a note"
                  value={noteText()}
                  onChange={setNoteText}
                  rows={3}
                  placeholder="What did you check or do?"
                />
                <Button
                  variant="secondary"
                  size="sm"
                  disabled={notePending() || noteText().trim().length === 0}
                  onClick={() => void submitNote()}
                >
                  Add note
                </Button>
                <Show when={noteError()}>
                  {(held2) => {
                    const err = held2();
                    return (
                      <p class={styles.error} role="alert">
                        {isCommandError(err) ? err.message : "This note couldn't be added."}
                      </p>
                    );
                  }}
                </Show>
              </section>
            </>
          );
        }}
      </Show>

      <Show when={viewerSnapshot()}>
        {(snapshot) => (
          <SnapshotViewerDialog
            snapshot={snapshot()}
            printerName={detail()?.incident.printerSnapshot.name ?? ""}
            open
            onOpenChange={(open) => !open && setViewerSnapshotId(null)}
          />
        )}
      </Show>
    </div>
  );

  return (
    <Show
      when={props.mode === "overlay"}
      fallback={<aside class={styles.inline} aria-label="Incident">{content((text) => <h2 class={styles.title}>{text}</h2>)}</aside>}
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

/** One evidence row: a clickable thumbnail (opens `SnapshotViewerDialog`)
 *  for a live snapshot, or "Evidence pruned (<reason>)" with no image for a
 *  pruned one (spec: "a pruned item renders 'Evidence pruned (age)' text
 *  and no <img>"). */
function EvidenceThumbnail(props: { snapshot: CameraSnapshot; printerName: string; onOpen: () => void }) {
  // The fetch error is ignored here (not `error()`): if the image can't be
  // loaded, the trigger/timestamp caption still renders on its own.
  const { url } = useSnapshotImage(() => props.snapshot);

  return (
    <Show
      when={props.snapshot.prunedAt === null}
      fallback={<p class={styles.pruned}>{`Evidence pruned (${pruneReasonLabel(props.snapshot.pruneReason!)})`}</p>}
    >
      <button type="button" class={styles.thumbnailButton} onClick={props.onOpen}>
        <Show when={url()}>
          {(src) => <img class={styles.thumbnail} src={src()} alt={snapshotAltText(props.snapshot, props.printerName)} />}
        </Show>
        <span class={styles.thumbnailMeta}>
          <span>{snapshotTriggerLabel(props.snapshot.trigger)}</span>
          <time datetime={props.snapshot.capturedAt}>{formatDateTime(props.snapshot.capturedAt)}</time>
        </span>
      </button>
    </Show>
  );
}
