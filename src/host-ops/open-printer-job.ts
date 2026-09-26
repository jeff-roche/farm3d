import { createSignal } from "solid-js";
import { serializeNavigationTarget } from "../navigation/navigation-store";

/** `OPEN_PRINTER_JOB` (spec "Error codes"): open the Printer's Job tab.
 *  Navigates to the Printer in Monitor (App's `hashchange` listener does
 *  the navigating, as for a pasted deep link) and leaves a request the
 *  Printer's detail dock takes up once it shows that Printer, from
 *  anywhere in the app. The request lives only until the next navigation
 *  elsewhere, so an unanswered one can't switch tabs much later. */
const [request, setRequest] = createSignal<{ printerId: string } | undefined>();

let stopWatching: (() => void) | undefined;

export const printerJobRequest = request;

export function openPrinterJob(printerId: string): void {
  const hash = `#${serializeNavigationTarget({
    version: 1,
    destination: "monitor",
    selection: { kind: "printer", id: printerId },
  }).slice(1)}`;
  clearPrinterJobRequest();
  setRequest({ printerId });
  const onHashChange = () => {
    if (window.location.hash !== hash) clearPrinterJobRequest();
  };
  window.addEventListener("hashchange", onHashChange);
  stopWatching = () => window.removeEventListener("hashchange", onHashChange);
  window.location.hash = hash.slice(1);
}

/** The dock calls this once it has switched to the Job tab. */
export function clearPrinterJobRequest(): void {
  stopWatching?.();
  stopWatching = undefined;
  setRequest(undefined);
}
