import { For, Show } from "solid-js";
import { SegmentedControl, Select, SeverityMarker } from "../design-system";
import { attention, loadMoreResolved } from "../attention/attention-store";
import {
  ATTENTION_SEVERITY_FILTERS,
  attentionFilterLabel,
  attentionSeverityFilterLabel,
  attentionSeverityLabel,
  matchesAttentionFilter,
  matchesSeverityFilter,
  type AttentionFilter,
  type AttentionSeverityFilter,
} from "../attention/presentation";
import type { AttentionEvent } from "../attention/types";
import { formatDateTime } from "../slicing/revision-presentation";
import styles from "./AttentionCenter.module.css";

export interface AttentionCenterProps {
  filter: AttentionFilter;
  onFilterChange: (filter: AttentionFilter) => void;
  severityFilter: AttentionSeverityFilter;
  onSeverityFilterChange: (filter: AttentionSeverityFilter) => void;
  /** A row was chosen (spec: navigates to `monitor/attention/<id>` and
   *  closes the popover -- both the caller's job, since this component
   *  neither navigates nor knows about the popover). */
  onSelect: (event: AttentionEvent) => void;
}

const FILTER_OPTIONS: AttentionFilter[] = ["actionable", "unread", "allOpen", "resolved"];

/** The row's own timestamp: when resolved, the resolution time; otherwise
 *  the Event's most recent observation. */
function rowTimestamp(event: AttentionEvent): string {
  return event.resolvedAt ?? event.lastObservedAt;
}

/** The filters (a `SegmentedControl` for Actionable/Unread/All open/
 *  Resolved, plus a severity `Select`), the list, and its two distinct
 *  empty states (spec "Frontend architecture"). Purely presentational --
 *  it reads the Attention store directly (like `PrinterStatusPanel` reads
 *  `spoolState`) but owns no filter state of its own, so a caller that
 *  never unmounts (`AttentionTrigger`) can keep the selection across the
 *  popover's own open/close cycles. */
export function AttentionCenter(props: AttentionCenterProps) {
  const source = () => (props.filter === "resolved" ? attention.resolved() : attention.open());
  const filtered = () =>
    source().filter((event) => matchesAttentionFilter(props.filter, event) && matchesSeverityFilter(props.severityFilter, event));
  const nothingAtAll = () => source().length === 0;

  return (
    <div class={styles.center}>
      <div class={styles.heading}>Attention</div>
      <div class={styles.filters}>
        <SegmentedControl
          label="Filter"
          value={props.filter}
          onChange={props.onFilterChange}
          options={FILTER_OPTIONS.map((value) => ({ value, label: attentionFilterLabel(value) }))}
        />
        <Select
          label="Severity"
          value={props.severityFilter}
          onChange={props.onSeverityFilterChange}
          options={ATTENTION_SEVERITY_FILTERS}
          optionLabel={attentionSeverityFilterLabel}
          class={styles.severity}
        />
      </div>
      <Show
        when={filtered().length > 0}
        fallback={
          <p class={styles.empty}>
            {nothingAtAll() ? "Nothing needs your attention." : "No Events match this filter."}
          </p>
        }
      >
        <ul class={styles.list} aria-label="Attention Events">
          <For each={filtered()}>
            {(event) => (
              <li>
                <button
                  type="button"
                  class={styles.row}
                  classList={{ [styles.unread]: event.readAt === null }}
                  onClick={() => props.onSelect(event)}
                >
                  <SeverityMarker severity={event.severity} label={attentionSeverityLabel(event.severity)} />
                  <span class={styles.summary}>{event.summary}</span>
                  <time class={styles.time} datetime={rowTimestamp(event)}>
                    {formatDateTime(rowTimestamp(event))}
                  </time>
                </button>
              </li>
            )}
          </For>
        </ul>
      </Show>
      <Show when={props.filter === "resolved" && attention.resolvedCursor() !== null}>
        <button type="button" class={styles.loadMore} onClick={() => void loadMoreResolved()}>
          Load older…
        </button>
      </Show>
    </div>
  );
}
