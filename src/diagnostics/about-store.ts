/** `about_farm3d`, on its own so the shell can read the version without
 *  pulling the rest of the diagnostics store into the main chunk. */
import { createStore, reconcile } from "solid-js/store";
import { command, desktopAvailable, retryOnTransportFailure } from "../ipc/client";
import type { AboutInfo } from "./types";

const [state, setState] = createStore<{ about: AboutInfo | null }>({ about: null });

export const about = (): AboutInfo | null => state.about;

export async function loadAbout(): Promise<AboutInfo> {
  const info = desktopAvailable()
    ? await retryOnTransportFailure(() => command("about_farm3d"))
    : await import("./web-fixtures").then(({ webAboutInfo }) => webAboutInfo());
  setState("about", reconcile(info));
  return info;
}

/** Test seam. */
export function resetAboutStore(): void {
  setState("about", null);
}
