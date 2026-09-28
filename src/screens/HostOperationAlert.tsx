import { For, Show, type JSX } from "solid-js";
import { Button } from "../design-system";
import { isCommandError } from "../ipc/client";
import { refreshHostOperations } from "../host-ops/host-operations-store";
import { openPrinterJob } from "../host-ops/open-printer-job";
import { printers } from "../printers/printer-store";
import { recoveryCodeLabel } from "../queue/presentation";
import { queue } from "../queue/queue-store";
import { showQueueEntry } from "./QueueRecoveryButton";
import styles from "./HostOperationAlert.module.css";

export interface HostOperationAlertProps {
  /** What a command rejected with: a `CommandError`, or anything else (a
   *  transport failure), which shows `fallback`. */
  error: unknown;
  fallback: string;
  /** The Printer `OPEN_PRINTER_JOB` opens when the error's details don't
   *  name one. */
  printerId?: string;
  /** Called after `OPEN_PRINTER_JOB` or `OPEN_JOB` is followed, e.g. so a
   *  dialog the alert sits in can close. */
  onOpenJob?: () => void;
  /** Extra work for `RELOAD` beyond re-reading Host Operations. */
  onReload?: () => void;
  /** Offered as "Try again" for a retryable error. */
  onRetry?: () => void;
  /** Local actions that aren't a `RecoveryCode` (e.g. the Start dialog's
   *  **Stage again**). */
  children?: JSX.Element;
}

/** The Printers an error's `details` name: `CONNECTION_IN_USE` carries
 *  `printerId`, `HOST_OPERATION_PENDING` carries `printerIds` (several for
 *  a refused import). */
function detailsPrinterIds(error: unknown): string[] {
  if (!isCommandError(error)) return [];
  const details = error.details ?? {};
  if (typeof details.printerId === "string") return [details.printerId];
  const ids = details.printerIds;
  return Array.isArray(ids) ? ids.filter((id): id is string => typeof id === "string") : [];
}

/** P7's `OPEN_JOB` target: the Job the error's `details` name
 *  (`CONNECTION_IN_USE` for a Job-linked operation, `JOB_ACTIVE`), else the
 *  named Printer's active Job. */
function detailsJobId(error: unknown, printerIds: string[]): string | undefined {
  if (!isCommandError(error)) return undefined;
  const jobId = error.details?.jobId;
  if (typeof jobId === "string") return jobId;
  for (const printerId of printerIds) {
    const job = queue.activeJobFor(printerId);
    if (job) return job.id;
  }
  return undefined;
}

/** A Host Operation command's failure, inline (`role="alert"`), with its
 *  recovery actions (spec "Errors and recovery"): `OPEN_PRINTER_JOB` opens
 *  the Printer's Job tab, `OPEN_JOB` opens the Job in the Queue (P7),
 *  `RELOAD` re-reads Host Operations. Used for every
 *  place `HOST_OPERATION_PENDING` or `CONNECTION_IN_USE` can appear, so
 *  they render the same way everywhere. */
export function HostOperationAlert(props: HostOperationAlertProps) {
  const recovery = () => (isCommandError(props.error) ? props.error.recovery : []);
  const message = () => (isCommandError(props.error) ? props.error.message : props.fallback);
  const jobPrinterIds = () => {
    const named = detailsPrinterIds(props.error);
    return named.length > 0 ? named : props.printerId ? [props.printerId] : [];
  };
  /** One Printer: "Open the Job tab". Several: one link each, by name. */
  const jobLabel = (printerId: string) => {
    if (jobPrinterIds().length === 1) return "Open the Job tab";
    const name = printers().find((printer) => printer.id === printerId)?.name ?? printerId;
    return `Open ${name}'s Job tab`;
  };
  const retryable = () => !isCommandError(props.error) || props.error.retryable;
  const openJobId = () => (recovery().includes("OPEN_JOB") ? detailsJobId(props.error, jobPrinterIds()) : undefined);

  return (
    <div class={styles.alert} role="alert">
      <p class={styles.message}>{message()}</p>
      <div class={styles.actions}>
        <Show when={recovery().includes("OPEN_PRINTER_JOB")}>
          <For each={jobPrinterIds()}>
            {(printerId) => (
              <Button
                variant="secondary"
                size="sm"
                onClick={() => {
                  openPrinterJob(printerId);
                  props.onOpenJob?.();
                }}
              >
                {jobLabel(printerId)}
              </Button>
            )}
          </For>
        </Show>
        <Show when={openJobId()}>
          {(jobId) => (
            <Button
              variant="secondary"
              size="sm"
              onClick={() => {
                showQueueEntry(jobId());
                props.onOpenJob?.();
              }}
            >
              {recoveryCodeLabel("OPEN_JOB")}
            </Button>
          )}
        </Show>
        <Show when={recovery().includes("RELOAD")}>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => {
              refreshHostOperations();
              props.onReload?.();
            }}
          >
            Reload
          </Button>
        </Show>
        <Show when={props.onRetry && retryable()}>
          <Button variant="secondary" size="sm" onClick={() => props.onRetry?.()}>
            Try again
          </Button>
        </Show>
        {props.children}
      </div>
    </div>
  );
}
