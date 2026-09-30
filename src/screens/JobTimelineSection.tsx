import { createResource, Show } from "solid-js";
import { Timeline } from "../design-system";
import type { TimelineItem } from "../design-system";
import { jobEventKindLabel } from "../queue/presentation";
import { getJobHistory } from "../queue/queue-store";
import type { Job } from "../queue/types";
import { JobTimelinePanel } from "./JobTimelinePanel";
import styles from "./JobPanel.module.css";

const SETTLED_STATES = new Set<Job["state"]>(["completed", "failed", "cancelled", "outcomeUnknown"]);

/** `JobPanel`'s Timeline. A live Job's is `get_job_history`'s events; a
 *  settled Job's is `get_job_timeline` (D11), read by `JobTimelinePanel`. */
export function JobTimelineSection(props: { job: Job }) {
  const settled = () => SETTLED_STATES.has(props.job.state);
  const [history] = createResource(
    () => (settled() ? false : `${props.job.id}|${props.job.revision}`),
    () => getJobHistory(props.job.id),
  );
  const timeline = (): TimelineItem[] => (history.error ? [] : history()?.events ?? []).map((event) => ({
    id: event.id,
    at: event.at,
    title: jobEventKindLabel(event.kind),
    marker: event.kind === "assigned" || event.kind === "hostJobPinned" ? "muted" : "default",
  }));

  return (
    <div class={styles.group}>
      <h4 class={styles.subheading}>Timeline</h4>
      <Show
        when={settled()}
        fallback={
          <>
            <Show when={history.error}>
              <p class={styles.muted}>The Job's history couldn't be loaded.</p>
            </Show>
            <Timeline label={`Job timeline on ${props.job.printerSnapshot.name}`} items={timeline()} />
          </>
        }
      >
        <JobTimelinePanel jobId={props.job.id} revision={props.job.revision} label={props.job.printerSnapshot.name} />
      </Show>
    </div>
  );
}
