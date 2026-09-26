import { createSignal } from "solid-js";
import { serializeNavigationTarget } from "../navigation/navigation-store";

/** `OPEN_PRINTER_JOB` (spec "Error codes"): open the Printer's Job tab.
 *  Navigates to the Printer in Monitor (App's `hashchange` listener does
 *  the navigating, as for a pasted deep link) and leaves a request the
 *  Printer's detail dock takes up once it shows that Printer, from
 *  anywhere in the app. */
const [request, setRequest] = createSignal<{ printerId: string } | undefined>();

export const printerJobRequest = request;

export function openPrinterJob(printerId: string): void {
  setRequest({ printerId });
  window.location.hash = serializeNavigationTarget({
    version: 1,
    destination: "monitor",
    selection: { kind: "printer", id: printerId },
  }).slice(1);
}

/** The dock calls this once it has switched to the Job tab. */
export function clearPrinterJobRequest(): void {
  setRequest(undefined);
}
