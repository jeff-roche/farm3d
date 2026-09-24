import { Dialog as KDialog } from "@kobalte/core/dialog";
import { createEffect, createSignal, For, on, Show } from "solid-js";
import { Button, Tabs } from "../design-system";
import { isCommandError } from "../ipc/client";
import { archivePrinter, lifecycleEligibility, printers, reportError, unarchivePrinter } from "../printers/printer-store";
import type { LifecycleEligibility, ResolvedPrinter } from "../printers/types";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import { ArchivePrinterDialog } from "./ArchivePrinterDialog";
import { DeletePrinterDialog } from "./DeletePrinterDialog";
import { MATERIAL_SLOTS_ANCHOR_ID, MaterialSlotsSection } from "./MaterialSlotsEditor";
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
  /** Opens the Setup tab at a section (batch Results' Equip). A new object
   *  is a new request; one for another Printer is ignored. */
  focusRequest?: DockFocusRequest;
}

export interface DockFocusRequest {
  printerId: string;
  section: "materialSlots";
}

export function PrinterDetailDock(props: PrinterDetailDockProps) {
  const content = () => (
    <DockContent
      printer={props.printer!}
      overlay={props.mode === "overlay"}
      onClose={props.onClose}
      onDeleted={props.onDeleted}
      syncState={props.syncState}
      focusRequest={props.focusRequest}
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
  const [archiveSpools, setArchiveSpools] = createSignal<SpoolRecord[] | null>(null);
  const [tab, setTab] = createSignal("status");

  createEffect(on(() => props.focusRequest, (request) => {
    if (!request || request.printerId !== props.printer.id) return;
    setTab("setup");
    setTimeout(() => {
      const section = document.getElementById(MATERIAL_SLOTS_ANCHOR_ID);
      section?.scrollIntoView?.({ block: "start" });
      section?.querySelector<HTMLElement>("h3")?.focus();
    });
  }));

  const archived = () => Boolean(props.printer.archivedAt);
  // Only explain actions the dock actually shows: Archive for an active
  // Printer, Unarchive for an archived one, Delete for either -- a blocker
  // for the hidden one ("not archived" / "already archived") is noise.
  // SPOOLS_LOADED isn't shown: Archive handles it by asking where each
  // loaded Spool goes.
  const visibleBlockers = () =>
    (eligibility()?.blockers ?? []).filter((blocker) =>
      blocker.code !== "SPOOLS_LOADED" &&
      (blocker.action === "delete" || blocker.action === (archived() ? "unarchive" : "archive")),
    );
  /** Archive is offered when the backend allows it outright, or when its
   *  only archive blocker is loaded Spools -- the dispositions dialog
   *  resolves those (D10). Reads the backend's blockers; derives none. */
  const archiveOffered = () => {
    const current = eligibility();
    if (!current) return false;
    if (current.canArchive) return true;
    const archiveBlockers = current.blockers.filter((blocker) => blocker.action === "archive");
    return current.loadedSpools.length > 0 && archiveBlockers.every((blocker) => blocker.code === "SPOOLS_LOADED");
  };

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

  // Keyed on the *pair* (id, archivedAt), not just id: `on()` doesn't dedupe
  // by value, so tracking `id` alone reruns on any push that replaces this
  // Printer's object for reasons that don't matter here (e.g. an unrelated
  // field edit whose reactive plumbing happens to swap the object identity)
  // -- refetching would be harmless there, except it can race an in-flight
  // refetch and flash the eligibility state to null. But archivedAt truly
  // *can* change out from under this dock while it's open, through a path
  // that isn't this dock's own Archive/Unarchive (e.g. `importPrinters()`
  // replacing the whole Printer list), and that case must still refetch --
  // otherwise Unarchive/Delete… stay stuck showing stale permissions until
  // the dock is reopened. Comparing both values, not just noticing *a*
  // rerun, gets both right: skip only when neither actually changed.
  createEffect(on(
    () => [props.printer.id, props.printer.archivedAt] as const,
    ([id, archivedAt], previous) => {
      const idChanged = !previous || previous[0] !== id;
      const archivedAtChanged = !previous || previous[1] !== archivedAt;
      if (!idChanged && !archivedAtChanged) return;
      if (idChanged) {
        // A genuine Printer-selection change: every bit of this dock's
        // lifecycle state belongs to the *previous* Printer and must not
        // leak into the next one. An archivedAt-only change (same
        // Printer) leaves `unarchiveError`/`eligibility` alone until the
        // refetch below actually resolves, per Fix round 2: the
        // eligibility error clears only on an id change or a successful
        // refetch, not just because a refetch started.
        setUnarchiveError(null);
        setEligibility(null);
        setEligibilityError(null);
      }
      void refreshEligibility(id);
    },
  ));

  /** Archiving/unarchiving doesn't call `refreshEligibility` itself on
   *  success -- the effect above already refetches once the store's
   *  `archivedAt` push for this Printer lands, and doing it here too would
   *  double the fetch for the same outcome (Fix round 2). It's only called
   *  explicitly here on failure paths that don't change `archivedAt` at
   *  all, where the effect has nothing to react to. */
  async function onArchive() {
    if (actionPending()) return;
    setActionPending(true);
    try {
      // Re-checked on click: a Spool may have been loaded or unloaded (e.g.
      // from Material Slots) since the dock last asked.
      const current = await lifecycleEligibility(props.printer.id);
      setEligibility(current);
      if (current.loadedSpools.length > 0) {
        setArchiveSpools(current.loadedSpools);
        return;
      }
      if (!current.canArchive) return;
      // No inline surface for the plain archive: failures go to the banner.
      await archivePrinter(props.printer.id);
    } catch (e) {
      reportError(e);
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
        value={tab()}
        onChange={setTab}
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
                <MaterialSlotsSection printer={props.printer} />
                <PrinterProfilePanel printer={props.printer} />
                <PrinterConnectionPanel printer={props.printer} />
                <div class={styles.lifecycle}>
                  <div class={styles.lifecycleActions}>
                    <Show when={!archived()}>
                      <Button
                        variant="ghost"
                        disabled={!archiveOffered() || actionPending()}
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
                  <For each={visibleBlockers()}>
                    {(blocker) => <p class={styles.blocker}>{blocker.message}</p>}
                  </For>
                </div>
              </div>
            ),
          },
        ]}
      />
      <Show when={archiveSpools()}>
        {(loaded) => (
          <ArchivePrinterDialog
            open
            onOpenChange={(open) => !open && setArchiveSpools(null)}
            printer={props.printer}
            loadedSpools={loaded()}
          />
        )}
      </Show>
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
