import { IconAlertTriangle } from "@tabler/icons-solidjs";
import { createResource, Show } from "solid-js";
import { Button, Timeline, type TimelineItem } from "../design-system";
import {
  filamentSummary,
  isPrerelease,
  NEEDS_MANUAL_PRINTER,
  revisionTitle,
} from "../slicing/revision-presentation";
import { loadSliceRevisions, slicing } from "../slicing/slicing-store";
import type { SliceRevisionSummary } from "../slicing/types";
import styles from "./SliceRevisionList.module.css";

export interface SliceRevisionListProps {
  modelId: string;
  /** G-code Models make external revisions; others are sliced. */
  external: boolean;
  onOpen: (sliceRevisionId: string) => void;
}

function formatDateTime(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

/** D21: a Model's Slice Revisions as a Timeline, newest first. Each entry
 *  says what it is (a plate, or external G-code), its target, its filament,
 *  whether a prerelease OrcaSlicer made it, and whether a Printer must be
 *  chosen by hand. **Open** shows its review. */
export function SliceRevisionList(props: SliceRevisionListProps) {
  // Merges the backend's list into the store; the list reads the store, so
  // revisions created or removed later show up without a reload.
  const [load] = createResource(() => props.modelId, (modelId) => loadSliceRevisions(modelId));
  const revisions = () => slicing.revisions(props.modelId);

  const detail = (revision: SliceRevisionSummary) => {
    const title = revisionTitle(revision);
    return (
      <span class={styles.detail}>
        <span>{[revision.targetLabel, filamentSummary(revision.estimates)].filter(Boolean).join(" · ")}</span>
        <Show when={isPrerelease(revision.runtime)}>
          <span class={styles.prerelease}>Prerelease OrcaSlicer</span>
        </Show>
        <Show when={revision.requiresManualPrinterSelection}>
          <span class={styles.manual}>
            <IconAlertTriangle size={12} aria-hidden="true" />
            {NEEDS_MANUAL_PRINTER}
          </span>
        </Show>
        <Button
          variant="ghost"
          size="sm"
          class={styles.open}
          data-revision-open={revision.id}
          aria-label={`Open ${title}, ${formatDateTime(revision.createdAt)}`}
          onClick={() => props.onOpen(revision.id)}
        >
          Open
        </Button>
      </span>
    );
  };

  const items = (): TimelineItem[] => revisions().map((revision) => ({
    id: revision.id,
    at: revision.createdAt,
    title: revisionTitle(revision),
    detail: detail(revision),
  }));

  return (
    <>
      <Show when={load.error}>
        <p class={styles.note}>The Slice Revisions couldn't be loaded.</p>
      </Show>
      <Show
        when={revisions().length > 0}
        fallback={
          <Show when={!load.loading && !load.error}>
            <p class={styles.note}>
              {props.external
                ? "None yet. Create one to confirm which Printer and material this G-code is for."
                : "None yet. Prepare… the Model and slice a plate to make one."}
            </p>
          </Show>
        }
      >
        <Timeline label="Slice Revisions" items={items()} />
      </Show>
    </>
  );
}
