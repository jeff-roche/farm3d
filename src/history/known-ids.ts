import { createSignal } from "solid-js";

/** The Job and Incident ids the History view has listed. They can be older
 *  than the Queue and Attention stores hold, and a row is still a valid
 *  deep-link target. Kept apart from `history-store` so the app shell can
 *  read them without pulling the store into the main chunk. */
export const [historyKnownIds, setHistoryKnownIds] = createSignal<string[]>([]);
