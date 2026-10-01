import { setHistoryKnownIds } from "./known-ids";

/** Ids accumulate (filtering must not drop a selected Job's id), bounded to
 *  the newest 5000. */
export const addHistoryKnownIds = (next: string[]) =>
  setHistoryKnownIds((current) => [...new Set([...current, ...next])].slice(-5000));

export const clearHistoryKnownIds = () => setHistoryKnownIds([]);
