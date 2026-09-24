import { For, Show, type JSX } from "solid-js";
import styles from "./Timeline.module.css";

export interface TimelineItem {
  id: string;
  /** ISO 8601 timestamp; used verbatim as `<time datetime>` and formatted
   *  for the visible label. */
  at: string;
  title: string;
  detail?: JSX.Element;
  /** Severity-free marker weight — `"muted"` for lower-signal entries
   *  (e.g. amount reads) next to `"default"` ones (e.g. movements). */
  marker?: "default" | "muted";
}

export interface TimelineProps {
  items: TimelineItem[];
  label: string;
}

function formatTimestamp(at: string): string {
  const date = new Date(at);
  if (Number.isNaN(date.getTime())) return at;
  return date.toLocaleString(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
}

/** A vertical event list: a timestamp, a severity-free marker, a title, and
 *  optional detail content. Renders an `<ol>` so reading order is explicit
 *  for assistive tech; used for Spool/Job/Incident history. */
export function Timeline(props: TimelineProps) {
  return (
    <ol class={styles.list} aria-label={props.label}>
      <For each={props.items}>
        {(item) => (
          <li class={styles.item}>
            <span
              class={[styles.marker, item.marker === "muted" ? styles.muted : undefined]
                .filter(Boolean)
                .join(" ")}
              aria-hidden="true"
            />
            <div class={styles.body}>
              <time class={styles.time} datetime={item.at}>
                {formatTimestamp(item.at)}
              </time>
              <div class={styles.title}>{item.title}</div>
              <Show when={item.detail}>
                <div class={styles.detail}>{item.detail}</div>
              </Show>
            </div>
          </li>
        )}
      </For>
    </ol>
  );
}
