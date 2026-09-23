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
  // Distinct from `unarchiveError`: this is a failure to even *check* what's
  // allowed (the `lifecycleEligibility` query itself), not a failure of an
  // action the user asked for. Left unexplained, it would otherwise just
  // read as every lifecycle button being permanently, mysteriously disabled.
  const [eligibilityError, setEligibilityError] = createSignal<string | null>(null);
  const [unarchiveError, setUnarchiveError] = createSignal<string | null>(null);
  const [actionPending, setActionPending] = createSignal(false);
  const [deleteOpen, setDeleteOpen] = createSignal(false);

  const archived = () => Boolean(props.printer.archivedAt);

  const refreshEligibility = async (id: string) => {
    try {
      const result = await lifecycleEligibility(id);
      setEligibility(result);
      setEligibilityError(null);
    } catch (e) {
      setEligibility(null);
      setEligibilityError(
        isCommandError(e)
          ? `Couldn't check what you can do with this Printer. ${e.message}`
          : "Couldn't check what you can do with this Printer.",
      );
    }
  };

  createEffect(on(() => props.printer.id, (id, previousId) => {
    // `on()` doesn't dedupe by value -- it reruns whenever the tracked
    // expression's *dependencies* invalidate, which includes every store
    // push that replaces this Printer's object (e.g. Archive's own
    // `refreshEligibility`, a Connection save, a rename), not only an
    // actual Printer-selection change. Without this guard, any such push
    // while this dock is open would blow away the eligibility state (and
    // any in-flight `onArchive`/`onUnarchive` refetch's result) with a
    // fresh, momentarily-null one for the *same* id.
    if (id === previousId) return;
    setUnarchiveError(null);
    setEligibility(null);
    setEligibilityError(null);
    void refreshEligibility(id);
  }));

  async function onArchive() {
    if (actionPending()) return;
    setActionPending(true);
    try {
      await archivePrinter(props.printer.id);
      await refreshEligibility(props.printer.id);
    } finally {
      setActionPending(false);
    }
  }

  /** A `DUPLICATE_HOST` failure (spec D3/D6: an active Printer now owns the
   *  host) names the conflicting Printer inline rather than only reporting
   *  a generic message -- the whole reason `unarchivePrinter` rejects
   *  instead of routing to the banner. */
  async function onUnarchive() {
    if (actionPending()) return;
    setActionPending(true);
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
    } finally {
      setActionPending(false);
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
                        disabled={!eligibility()?.canArchive || actionPending()}
                        onClick={() => void onArchive()}
                      >
                        Archive
                      </Button>
                    </Show>
                    <Show when={archived()}>
                      <Button
                        variant="ghost"
                        disabled={!eligibility()?.canUnarchive || actionPending()}
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
                  <Show when={eligibilityError()}>
                    {(message) => (
                      <div class={styles.eligibilityError}>
                        <p class={styles.error} role="alert">{message()}</p>
                        <Button
                          variant="secondary"
                          onClick={() => void refreshEligibility(props.printer.id)}
                        >
                          Retry
                        </Button>
                      </div>
                    )}
                  </Show>
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
