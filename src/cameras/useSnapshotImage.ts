import { createEffect, createSignal, on, onCleanup, type Accessor } from "solid-js";
import { snapshotImageUrl } from "./camera-store";

export interface SnapshotImageSource {
  id: string;
  prunedAt: string | null;
}

/** Fetches a Snapshot's full image as an object URL, revoking the previous
 *  one whenever `source()` changes (a different Snapshot, or the same one
 *  transitioning to pruned) or the caller unmounts. A pruned row
 *  (`prunedAt !== null`) is never fetched (spec: "a pruned item renders
 *  'Evidence pruned (<reason>)' text and no <img>"). Shared by
 *  `IncidentDetail`'s evidence thumbnails and `SnapshotViewerDialog`'s full
 *  view -- both wanted the identical fetch/object-URL/revoke-on-change
 *  dance, just with different consumption of the error (the thumbnail
 *  ignores it and falls back to caption-only; the viewer shows it). */
export function useSnapshotImage(source: Accessor<SnapshotImageSource>): { url: Accessor<string | null>; error: Accessor<unknown> } {
  const [url, setUrl] = createSignal<string | null>(null);
  const [error, setError] = createSignal<unknown>(null);

  createEffect(on(() => [source().id, source().prunedAt] as const, ([id, prunedAt]) => {
    // A new source starts clean: neither the last image nor its error.
    setUrl(null);
    setError(null);
    if (prunedAt !== null) return;
    let cancelled = false;
    let created: string | null = null;
    snapshotImageUrl(id).then((loaded) => {
      if (cancelled) return;
      created = loaded;
      setUrl(loaded);
    }).catch((e) => { if (!cancelled) setError(e); });
    onCleanup(() => {
      cancelled = true;
      if (created?.startsWith("blob:")) URL.revokeObjectURL(created);
    });
  }));

  return { url, error };
}
