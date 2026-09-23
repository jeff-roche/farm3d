import { Dialog as KDialog } from "@kobalte/core/dialog";
import { createEffect, createSignal, For, on, Show } from "solid-js";
import { Button, Tabs } from "../design-system";
import { isCommandError } from "../ipc/client";
import { archivePrinter, lifecycleEligibility, printers, unarchivePrinter } from "../printers/printer-store";
import type { LifecycleEligibility, ResolvedPrinter } from "../printers/types";
import { DeletePrinterDialog } from "./DeletePrinterDialog";
import { PrinterConnectionPanel } from "./PrinterConnectionPanel";
import { PrinterProfilePanel } from "./PrinterProfilePanel";
import { PrinterSetupPanel } from "./PrinterSetupPanel";
import { PrinterStatusPanel } from "./PrinterStatusPanel";
import styles from "./PrinterDetailDock.module.css";

export interface PrinterDetailDockProps {
  printer?: ResolvedPrinter;
  mode: "inline" | "overlay";
  onClose: () => void;
  /** Called once a Printer this dock was showing has been permanently
   *  deleted (through the guarded Archive → Delete… flow), after the dock
   *  has already asked to close. Lets a caller reconcile navigation/first-run
   *  state the way it did for P1's direct removal. */
  onDeleted?: (id: string) => void;
  syncState?: "syncing" | "current" | "uncertain";
}

export function PrinterDetailDock(props: PrinterDetailDockProps) {
  const content = () => (
    <DockContent
      printer={props.printer!}
      overlay={props.mode === "overlay"}
      onClose={props.onClose}
      onDeleted={props.onDeleted}
      syncState={props.syncState}
    />
  );

  return (
    <Show when={props.printer}>
      <Show
        when={props.mode === "overlay"}
        fallback={<aside class={styles.inline} aria-label={props.printer!.name}>{content()}</aside>}
      >
        <KDialog open onOpenChange={(open) => !open && props.onClose()}>
          <KDialog.Portal>
            <KDialog.Overlay class={styles.overlay} />
            <KDialog.Content
              class={styles.overlayContent}
              onCloseAutoFocus={(event) => event.preventDefault()}
            >
              {content()}
            </KDialog.Content>
          </KDialog.Portal>
        </KDialog>
      </Show>
    </Show>
  );
}

function DockContent(props: Omit<PrinterDetailDockProps, "mode"> & { printer: ResolvedPrinter; overlay: boolean }) {
  const [eligibility, setEligibility] = createSignal<LifecycleEligibility | null>(null);
  const [unarchiveError, setUnarchiveError] = createSignal<string | null>(null);
  const [deleteOpen, setDeleteOpen] = createSignal(false);

  const archived = () => Boolean(props.printer.archivedAt);

  const refreshEligibility = async (id: string) => {
    try {
      setEligibility(await lifecycleEligibility(id));
    } catch {
      setEligibility(null);
    }
  };

  createEffect(on(() => props.printer.id, (id) => {
    setUnarchiveError(null);
    setEligibility(null);
    void refreshEligibility(id);
  }));

  async function onArchive() {
    await archivePrinter(props.printer.id);
    await refreshEligibility(props.printer.id);
  }

  /** A `DUPLICATE_HOST` failure (spec D3/D6: an active Printer now owns the
   *  host) names the conflicting Printer inline rather than only reporting
   *  a generic message -- the whole reason `unarchivePrinter` rejects
   *  instead of routing to the banner. */
  async function onUnarchive() {
    setUnarchiveError(null);
    try {
      await unarchivePrinter(props.printer.id);
      await refreshEligibility(props.printer.id);
    } catch (e) {
      if (isCommandError(e) && e.code === "DUPLICATE_HOST") {
        const conflictId = e.details?.conflictingPrinterId;
        const conflictName = typeof conflictId === "string"
          ? printers().find((candidate) => candidate.id === conflictId)?.name
          : undefined;
        setUnarchiveError(
          conflictName
            ? `Another Printer, "${conflictName}", already uses this host.`
            : e.message,
        );
      } else {
        setUnarchiveError(isCommandError(e) ? e.message : "This Printer could not be unarchived.");
      }
    }
  }

  function handleDeleted() {
    props.onDeleted?.(props.printer.id);
    props.onClose();
  }

  return (
    <div class={styles.dock}>
      <header class={styles.header}>
        <div>
          <Show when={props.overlay} fallback={<h2 class={styles.title}>{props.printer.name}</h2>}>
            <KDialog.Title class={styles.title}>{props.printer.name}</KDialog.Title>
          </Show>
          <p class={styles.subtitle}>{props.printer.modelLabel}</p>
        </div>
        <Button variant="ghost" class={styles.close} onClick={props.onClose}>Close</Button>
      </header>
      <Tabs
        defaultValue="status"
        items={[
          {
            value: "status",
            label: "Status",
            content: <PrinterStatusPanel printer={props.printer} syncState={props.syncState} />,
          },
          {
            value: "setup",
            label: "Setup",
            content: (
              <div class={styles.setup}>
                <PrinterSetupPanel printer={props.printer} />
                <PrinterProfilePanel printer={props.printer} />
                <PrinterConnectionPanel printer={props.printer} />
                <div class={styles.lifecycle}>
                  <div class={styles.lifecycleActions}>
                    <Show when={!archived()}>
                      <Button
                        variant="ghost"
                        disabled={!eligibility()?.canArchive}
                        onClick={() => void onArchive()}
                      >
                        Archive
                      </Button>
                    </Show>
                    <Show when={archived()}>
                      <Button
                        variant="ghost"
                        disabled={!eligibility()?.canUnarchive}
                        onClick={() => void onUnarchive()}
                      >
                        Unarchive
                      </Button>
                      <Show when={eligibility()?.canDelete}>
                        <Button variant="danger" onClick={() => setDeleteOpen(true)}>
                          Delete…
                        </Button>
                      </Show>
                    </Show>
                  </div>
                  <Show when={unarchiveError()}>
                    {(message) => <p class={styles.error} role="alert">{message()}</p>}
                  </Show>
                  <For each={eligibility()?.blockers ?? []}>
                    {(blocker) => <p class={styles.blocker}>{blocker.message}</p>}
                  </For>
                </div>
              </div>
            ),
          },
        ]}
      />
      <DeletePrinterDialog
        open={deleteOpen()}
        onOpenChange={setDeleteOpen}
        printerId={props.printer.id}
        printerName={props.printer.name}
        onDeleted={handleDeleted}
      />
    </div>
  );
}
