import { Dialog as KDialog } from "@kobalte/core/dialog";
import { Show, type JSX } from "solid-js";
import { Button } from "../design-system";
import { copyLabel } from "../queue/presentation";
import { queue } from "../queue/queue-store";
import type { Job } from "../queue/types";
import { JobPanel } from "./JobPanel";
import { QueuePreview } from "./QueuePreview";
import styles from "./MonitorQueueDock.module.css";

export interface MonitorQueueDockProps {
  mode: "inline" | "overlay";
  /** The Job chosen from the preview; without one the dock shows
   *  `QueuePreview`. */
  job: Job | undefined;
  onSelectJob: (jobId: string) => void;
  /** Back from a Job to the preview. */
  onBack: () => void;
  /** Overlay only: dismisses the dock. */
  onClose: () => void;
}

function jobTitle(job: Job): string {
  const entry = queue.entry(job.queueEntryId);
  if (!entry) return job.printerSnapshot.name;
  const name = entry.display.plateLabel ? `${entry.display.modelName} — ${entry.display.plateLabel}` : entry.display.modelName;
  return entry.copyCount > 1 ? `${name} (${copyLabel(entry)})` : name;
}

/** The Monitor dock when no Printer is selected (spec "Frontend
 *  architecture"): `QueuePreview` by default, and a Job's `JobPanel` once
 *  one is chosen there. Inline beside the Printers, or a modal overlay at
 *  narrow widths, like `PrinterDetailDock`. */
export function MonitorQueueDock(props: MonitorQueueDockProps) {
  // One stable wrapper: swapping the preview for a Job directly under
  // Kobalte's `Dialog.Content` (beside its focus-trap sentinels) left the
  // preview behind.
  const overlay = () => props.mode === "overlay";
  const content = (title: (text: string) => JSX.Element) => (
    <div>
      <Show
        when={props.job}
        fallback={<QueuePreview onSelectJob={props.onSelectJob} onClose={overlay() ? props.onClose : undefined} />}
      >
        {(job) => (
          <div class={styles.job}>
            <header class={styles.header}>
              <div>
                {title(jobTitle(job()))}
                <p class={styles.subtitle}>{job().printerSnapshot.name}</p>
              </div>
              <div class={styles.headerActions}>
                <Button variant="ghost" size="sm" onClick={props.onBack}>Back to Queue</Button>
                <Show when={overlay()}>
                  <Button variant="ghost" size="sm" onClick={props.onClose}>Close</Button>
                </Show>
              </div>
            </header>
            <JobPanel job={job()} />
          </div>
        )}
      </Show>
    </div>
  );

  return (
    <Show
      when={props.mode === "overlay"}
      fallback={
        <aside class={styles.inline} aria-label={props.job ? "Job" : "Queue preview"}>
          {content((text) => <h2 class={styles.title}>{text}</h2>)}
        </aside>
      }
    >
      <KDialog open onOpenChange={(open) => !open && props.onClose()}>
        <KDialog.Portal>
          <KDialog.Overlay class={styles.overlay} />
          <KDialog.Content class={styles.overlayContent} aria-label={props.job ? undefined : "Queue preview"}>
            {content((text) => <KDialog.Title class={styles.title}>{text}</KDialog.Title>)}
          </KDialog.Content>
        </KDialog.Portal>
      </KDialog>
    </Show>
  );
}
