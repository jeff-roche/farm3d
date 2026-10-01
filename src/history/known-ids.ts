import { createSignal } from "solid-js";

/** The Job and Incident ids the History view or a Job timeline has shown.
 *  They can be older than the Queue and Attention stores hold, and each is
 *  still a valid deep-link target. Kept apart from `history-store` so the
 *  app shell can read them without pulling the store into the main chunk;
 *  writers live in `known-ids-actions.ts` for the same reason. */
export const [historyKnownIds, setHistoryKnownIds] = createSignal<string[]>([]);
