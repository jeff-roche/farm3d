import { createEffect, createResource, createSignal, For, Show } from "solid-js";
import { Button, Timeline } from "../design-system";
import type { TimelineItem } from "../design-system";
import { incidentTarget } from "../attention/deep-link";
import { getJobTimeline } from "../history/history-store";
import {
  isMaterialItem,
  timelineItemDetail,
  timelineItemLabel,
  timelineSourceLabel,
} from "../history/presentation";
import type { JobTimeline, JobTimelineItem } from "../history/types";
import { addHistoryKnownIds } from "../history/known-ids-actions";
import { formatDateTime } from "../slicing/revision-presentation";
import { goTo } from "./QueueRecoveryButton";
import { SnapshotViewerDialog } from "./SnapshotViewerDialog";
import styles from "./JobTimelinePanel.module.css";

export interface JobTimelinePanelProps {
  /** A settled Job (`get_job_timeline` refuses anything else). */
  jobId: string;
  /** Names the list for assistive tech: "Job timeline for <label>". */
  label: string;
  /** The Job's revision: a settled Job still gains events (a material
   *  settlement or correction), so a new revision reloads the timeline. */
  revision?: number;
}

/** The full, immutable timeline of a settled Job (D11): every item kind
 *  with a text label, its time, and a detail; a Material section for the
 *  reservation, deduction, and correction; links to the Incident and to
 *  each snapshot's viewer. Pruned evidence shows as text, never an image.
 *  Everything shown is what Rust returned. */
export function JobTimelinePanel(props: JobTimelinePanelProps) {
  const [timeline, { refetch }] = createResource(() => [props.jobId, props.revision] as const, ([jobId]) => getJobTimeline(jobId));
  const [viewing, setViewing] = createSignal<string | null>(null);
  const loaded = (): JobTimeline | undefined => (timeline.error ? undefined : timeline());
  // The Job and its Incident are valid deep-link targets even when neither
  // the Queue nor the Attention store holds them (the app shell's check).
  createEffect(() => {
    const held = loaded();
    if (held) addHistoryKnownIds(held.incident ? [held.job.id, held.incident.id] : [held.job.id]);
  });

  return (
    <div class={styles.panel}>
      <Show when={timeline.error}>
        <div role="alert" class={styles.error}>
          <p class={styles.errorText}>
            The Job's timeline couldn't be loaded.
          </p>
          <Button variant="ghost" size="sm" onClick={() => void refetch()}>Try again</Button>
        </div>
      </Show>
      <Show when={loaded()} fallback={
        <Show when={!timeline.error}><p class={styles.muted} role="status">Loading the Job's timeline…</p></Show>
      }>
        {(held) => <Loaded timeline={held()} label={props.label} viewing={viewing()} onView={setViewing} />}
      </Show>
    </div>
  );
}

function Loaded(props: { timeline: JobTimeline; label: string; viewing: string | null; onView: (id: string | null) => void }) {
  const material = () => props.timeline.items.filter(isMaterialItem);
  const incidentId = () => props.timeline.incident?.id;
  const snapshot = () => {
    const id = props.viewing;
    if (id === null) return undefined;
    for (const item of props.timeline.items) if (item.source === "snapshot" && item.snapshot.id === id) return item.snapshot;
    return undefined;
  };

  const entries = (): TimelineItem[] => props.timeline.items.map((item, index) => ({
    id: `${index}-${item.source}`,
    at: item.at,
    title: timelineItemLabel(item),
    marker: item.source === "job" && (item.event.kind === "assigned" || item.event.kind === "hostJobPinned") ? "muted" : "default",
    detail: (
      <span class={styles.detail}>
        <span class={styles.source}>{timelineSourceLabel(item.source)}</span>
        <span>{timelineItemDetail(item)}</span>
        <Show when={item.source === "incident" && incidentId()}>
          {(id) => (
            <Button variant="ghost" size="sm" onClick={() => goTo(incidentTarget(id()))}>Open Incident</Button>
          )}
        </Show>
        <Show when={item.source === "snapshot" && item.snapshot.prunedAt === null ? item.snapshot : undefined}>
          {(held) => (
            <Button variant="ghost" size="sm" onClick={() => props.onView(held().id)}>View snapshot</Button>
          )}
        </Show>
      </span>
    ),
  }));

  return (
    <>
      <Show when={props.timeline.incident}>
        {(incident) => (
          <p class={styles.incident}>
            <span>Incident {incident().state === "open" ? "open" : "closed"}, opened {formatDateTime(incident().openedAt)}</span>
            <Button variant="ghost" size="sm" onClick={() => goTo(incidentTarget(incident().id))}>Open Incident</Button>
          </p>
        )}
      </Show>
      <Show when={material().length > 0}>
        <section class={styles.material} aria-label="Material">
          <h5 class={styles.subheading}>Material</h5>
          <ul class={styles.materialList}>
            <For each={material()}>{(item) => <MaterialRow item={item} />}</For>
          </ul>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => goTo({ version: 1, destination: "spools", selection: { kind: "spool", id: props.timeline.spoolId } })}
          >
            Open Spool #{props.timeline.spoolNumber}
          </Button>
        </section>
      </Show>
      <Timeline label={`Job timeline for ${props.label}`} items={entries()} />
      <Show when={snapshot()}>
        {(held) => (
          <SnapshotViewerDialog
            snapshot={held()}
            printerName={props.timeline.printerSnapshot.name}
            open
            onOpenChange={(open) => !open && props.onView(null)}
          />
        )}
      </Show>
    </>
  );
}

function MaterialRow(props: { item: JobTimelineItem }) {
  return (
    <li class={styles.materialRow}>
      <span class={styles.materialLabel}>{timelineItemLabel(props.item)}</span>
      <time class={styles.time} datetime={props.item.at}>{formatDateTime(props.item.at)}</time>
      <span>{timelineItemDetail(props.item)}</span>
    </li>
  );
}
