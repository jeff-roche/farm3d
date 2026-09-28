import { Show } from "solid-js";
import { Button } from "../design-system";
import { openPrinterJob } from "../host-ops/open-printer-job";
import { serializeNavigationTarget, type NavigationTarget } from "../navigation/navigation-store";
import { recoveryCodeLabel } from "../queue/presentation";
import { queue } from "../queue/queue-store";
import type { Blocker } from "../queue/types";

/** Goes to a place in the app the way a pasted deep link does: App's
 *  `hashchange` listener does the navigating. */
export function goTo(target: NavigationTarget): void {
  window.location.hash = serializeNavigationTarget(target).slice(1);
}

/** Opens a Queue Entry (or a Job, by its `job-*` id) in the Queue. */
export function showQueueEntry(id: string): void {
  goTo({ version: 1, destination: "queue", selection: { kind: "job", id } });
}

function showPrinter(printerId: string): void {
  goTo({ version: 1, destination: "monitor", selection: { kind: "printer", id: printerId } });
}

interface RecoveryAction {
  label: string;
  run: () => void;
}

/** The action a blocker's Rust-provided `recovery` code offers (D5's
 *  "Recovery per blocker"), or `undefined` when there is none to take from
 *  here. `onAssignManually` is `ASSIGN_MANUALLY`'s "switch the entry to
 *  Manual and open Assign"; without it that recovery isn't offered. */
function recoveryAction(blocker: Blocker, onAssignManually: (() => void) | undefined): RecoveryAction | undefined {
  const printerId = blocker.printerIds[0];
  switch (blocker.recovery) {
    case "OPEN_JOB": {
      const job = printerId ? queue.activeJobFor(printerId) : undefined;
      return job ? { label: recoveryCodeLabel("OPEN_JOB"), run: () => showQueueEntry(job.id) } : undefined;
    }
    case "OPEN_PRINTER_JOB":
      return printerId ? { label: "Open the Printer's Job tab", run: () => openPrinterJob(printerId) } : undefined;
    case "OPEN_PRINTER_SETUP":
    case "UNARCHIVE_PRINTER":
      return printerId ? { label: recoveryCodeLabel(blocker.recovery), run: () => showPrinter(printerId) } : undefined;
    case "CHECK_CONNECTION":
      return printerId ? { label: "Check the Connection", run: () => showPrinter(printerId) } : undefined;
    case "LOAD_SPOOL":
      return { label: recoveryCodeLabel("LOAD_SPOOL"), run: () => goTo({ version: 1, destination: "spools" }) };
    case "ASSIGN_MANUALLY":
      return onAssignManually ? { label: recoveryCodeLabel("ASSIGN_MANUALLY"), run: onAssignManually } : undefined;
    default:
      return undefined;
  }
}

export interface QueueRecoveryButtonProps {
  blocker: Blocker;
  onAssignManually?: () => void;
}

/** A blocker's recovery as one small button, or nothing. */
export function QueueRecoveryButton(props: QueueRecoveryButtonProps) {
  const action = () => recoveryAction(props.blocker, props.onAssignManually);
  return (
    <Show when={action()}>
      {(held) => (
        <Button
          variant="secondary"
          size="sm"
          onClick={(event: MouseEvent) => {
            // A row's own click selects it; the recovery goes elsewhere.
            event.stopPropagation();
            held().run();
          }}
        >
          {held().label}
        </Button>
      )}
    </Show>
  );
}
