import { createEffect, createSignal, on, Show } from "solid-js";
import { Button, Dialog, Select } from "../design-system";
import { capabilities } from "../host-ops/capabilities-store";
import { hostOperations, stageSliceRevision } from "../host-ops/host-operations-store";
import { openPrinterJob } from "../host-ops/open-printer-job";
import { capabilityRefusalText } from "../host-ops/presentation";
import { printers } from "../printers/printer-store";
import type { ResolvedPrinter } from "../printers/types";
import { HostOperationAlert } from "./HostOperationAlert";
import styles from "./StageOnPrinterDialog.module.css";

export interface StageOnPrinterDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  sliceRevisionId: string;
  revisionTitle: string;
  returnFocus?: () => HTMLElement | null | undefined;
}

/** Why a Printer can't be staged on now, or `undefined` when it can. Each
 *  reason is distinct copy (spec "Components"): an unsupported upload says
 *  so with the capability's reason, an offline Printer says "Offline", and
 *  a Printer with an unresolved Host Operation says it is pending. */
function stageRefusal(printer: ResolvedPrinter): string | undefined {
  const record = capabilities.forPrinter(printer.id);
  if (!record) return printer.connection ? "Checking what this printer supports…" : "No Connection";
  const upload = record.capabilities.upload;
  if (upload.status !== "supported") return capabilityRefusalText("upload", upload, record.adapterKind);
  if (hostOperations.unresolvedFor(printer.id)) return "A printer operation is pending.";
  if (printer.runtimeStatus?.connectionState !== "online") return "Offline";
  return undefined;
}

/** **Stage on Printer…** from a Slice Revision's review (spec
 *  "Components"): uploads the revision's G-code to one Printer. Staging
 *  never starts a print; the Printer's Job tab shows the upload and offers
 *  **Start…** once the file is verified. */
export function StageOnPrinterDialog(props: StageOnPrinterDialogProps) {
  const [chosenId, setChosenId] = createSignal<string | null>(null);
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<unknown>(null);
  const [stagedOn, setStagedOn] = createSignal<ResolvedPrinter | null>(null);

  createEffect(on(() => props.open, (open) => {
    if (!open) return;
    setChosenId(null);
    setError(null);
    setStagedOn(null);
  }));

  const candidates = () => printers().filter((printer) => !printer.archivedAt);
  const chosen = () => candidates().find((printer) => printer.id === chosenId()) ?? null;
  const canStage = () => {
    const printer = chosen();
    return printer !== null && stageRefusal(printer) === undefined && !pending();
  };

  async function onStage() {
    const printer = chosen();
    if (!printer || !canStage()) return;
    setPending(true);
    setError(null);
    try {
      await stageSliceRevision(printer.id, props.sliceRevisionId);
      setStagedOn(printer);
    } catch (e) {
      setError(e);
    } finally {
      setPending(false);
    }
  }

  return (
    <Dialog title="Stage on Printer" open={props.open} onOpenChange={props.onOpenChange} returnFocus={props.returnFocus}>
      <div class={styles.body}>
        <Show
          when={stagedOn()}
          fallback={
            <>
              <p class={styles.text}>
                Upload <span class={styles.strong}>{props.revisionTitle}</span> to a Printer. Staging never starts a print.
              </p>
              <Select<ResolvedPrinter>
                label="Printer"
                placeholder="Choose a Printer"
                options={candidates()}
                value={chosen()}
                onChange={(printer) => {
                  setChosenId(printer.id);
                  setError(null);
                }}
                optionValue={(printer) => printer.id}
                optionLabel={(printer) => printer.name}
                optionDisabled={(printer) => stageRefusal(printer) !== undefined}
                optionDescription={stageRefusal}
                disabled={pending()}
              />
              <Show when={chosen() && stageRefusal(chosen()!)}>
                {(reason) => <p class={styles.muted}>{reason()}</p>}
              </Show>
              <Show when={error()}>
                {(held) => (
                  <HostOperationAlert error={held()} fallback="The file couldn't be staged." printerId={chosen()?.id} onOpenJob={() => props.onOpenChange(false)} />
                )}
              </Show>
              <div class={styles.actions}>
                <Button variant="secondary" onClick={() => props.onOpenChange(false)}>Cancel</Button>
                <Button variant="primary" disabled={!canStage()} onClick={() => void onStage()}>
                  Stage
                </Button>
              </div>
            </>
          }
        >
          {(printer) => (
            <>
              <p class={styles.text} role="status">
                Uploading to {printer().name}. Its Job tab shows when the file is staged.
              </p>
              <div class={styles.actions}>
                <Button
                  variant="secondary"
                  onClick={() => {
                    props.onOpenChange(false);
                    openPrinterJob(printer().id);
                  }}
                >
                  Open the Job tab
                </Button>
                <Button variant="primary" onClick={() => props.onOpenChange(false)}>Done</Button>
              </div>
            </>
          )}
        </Show>
      </div>
    </Dialog>
  );
}
