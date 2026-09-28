import { Popover as KPopover } from "@kobalte/core/popover";
import { createEffect, createSignal, on, Show } from "solid-js";
import { SeverityMarker } from "../design-system";
import { attention, attentionCenterRequest } from "../attention/attention-store";
import { attentionSeverityLabel, DEFAULT_ATTENTION_FILTER, type AttentionFilter, type AttentionSeverityFilter } from "../attention/presentation";
import type { AttentionEvent } from "../attention/types";
import { serializeNavigationTarget } from "../navigation/navigation-store";
import { AttentionCenter } from "./AttentionCenter";
import styles from "./AttentionTrigger.module.css";

/** "Attention: N actionable, highest <severity word>" or "Attention:
 *  nothing needs action" (spec "Accessibility and adaptation"). */
function accessibleName(): string {
  const count = attention.actionableCount();
  if (count === 0) return "Attention: nothing needs action";
  const severity = attention.highestOpenSeverity();
  return `Attention: ${count} actionable, highest ${severity ? attentionSeverityLabel(severity) : "unknown"}`;
}

function eventTarget(id: string) {
  return { version: 1 as const, destination: "monitor" as const, selection: { kind: "attention" as const, id } };
}

/** The top-bar Attention trigger (spec "Frontend architecture", planner
 *  default 9): a count + the highest open severity, opening
 *  `AttentionCenter` in a non-modal popover. Owns the filter state so it
 *  survives the popover closing (`AttentionCenter` itself unmounts with
 *  the portalled content) and the one polite live region that announces a
 *  newly arrived live `fatal`/`warning` Event (spec "Accessibility and
 *  adaptation"). */
export function AttentionTrigger() {
  const [open, setOpen] = createSignal(false);
  const [filter, setFilter] = createSignal<AttentionFilter>(DEFAULT_ATTENTION_FILTER);
  const [severityFilter, setSeverityFilter] = createSignal<AttentionSeverityFilter>("all");
  const [announcement, setAnnouncement] = createSignal("");

  // `requestAttentionCenterOpen()`'s seam (a notification click, or
  // another future caller): opens the center. `defer: true` skips the
  // initial subscription so mounting never opens it.
  createEffect(on(attentionCenterRequest, () => setOpen(true), { defer: true }));

  // One polite live region per component (spec): a newly arrived *live*
  // (never backfilled) fatal or warning Event is announced. The first run
  // only captures the baseline -- nothing already open on mount is "new".
  let seen: Set<string> | undefined;
  createEffect(() => {
    const openEvents = attention.open();
    const nextSeen = new Set(openEvents.map((event) => event.id));
    if (seen === undefined) {
      seen = nextSeen;
      return;
    }
    const previouslySeen = seen;
    for (const event of openEvents) {
      if (!previouslySeen.has(event.id) && event.origin === "live" && (event.severity === "fatal" || event.severity === "warning")) {
        setAnnouncement(`New ${event.severity} Attention Event: ${event.summary}`);
      }
    }
    seen = nextSeen;
  });

  function selectEvent(event: AttentionEvent): void {
    setOpen(false);
    window.location.hash = serializeNavigationTarget(eventTarget(event.id)).slice(1);
  }

  return (
    <>
      <KPopover open={open()} onOpenChange={setOpen} placement="bottom-end">
        <KPopover.Trigger class={styles.trigger} aria-label={accessibleName()}>
          <Show
            when={attention.actionableCount() > 0}
            fallback={<span class={styles.idle}>Attention</span>}
          >
            <SeverityMarker
              severity={attention.highestOpenSeverity() ?? "info"}
              label={attentionSeverityLabel(attention.highestOpenSeverity() ?? "info")}
            />
            <span class={styles.count}>{attention.actionableCount()}</span>
          </Show>
        </KPopover.Trigger>
        <KPopover.Portal>
          <KPopover.Content class={styles.content}>
            <AttentionCenter
              filter={filter()}
              onFilterChange={setFilter}
              severityFilter={severityFilter()}
              onSeverityFilterChange={setSeverityFilter}
              onSelect={selectEvent}
            />
          </KPopover.Content>
        </KPopover.Portal>
      </KPopover>
      <p class={styles.live} aria-live="polite">{announcement()}</p>
    </>
  );
}
