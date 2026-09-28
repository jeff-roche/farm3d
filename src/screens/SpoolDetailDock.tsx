import { Dialog as KDialog } from "@kobalte/core/dialog";
import { createEffect, createSignal, For, on, Show } from "solid-js";
import { Button, ColorSwatch, DropdownMenu, Timeline } from "../design-system";
import type { DropdownMenuEntry } from "../design-system";
import type { TimelineItem } from "../design-system";
import { isCommandError } from "../ipc/client";
import { materialLabel } from "../spools/materials";
import { formatGrams } from "../spools/weight";
import { loadHistory, moveSpool, reportSpoolError, setLifecycle } from "../spools/spool-store";
import { printers } from "../printers/printer-store";
import type { ResolvedPrinter } from "../printers/types";
import type { AmountEvent } from "../generated/contracts/domain/AmountEvent";
import type { AmountEventKind } from "../generated/contracts/domain/AmountEventKind";
import type { MovementReason } from "../generated/contracts/domain/MovementReason";
import type { Reservation } from "../generated/contracts/domain/Reservation";
import type { ReservationState } from "../generated/contracts/domain/ReservationState";
import type { SpoolHistory } from "../generated/contracts/command/SpoolHistory";
import type { SpoolLocationSnapshot } from "../generated/contracts/domain/SpoolLocationSnapshot";
import type { SpoolMovement } from "../generated/contracts/domain/SpoolMovement";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import { requirementKindLabel, requirementStatusLabel } from "../queue/presentation";
import { queue } from "../queue/queue-store";
import { MoveSpoolDialog } from "./MoveSpoolDialog";
import { showQueueEntry } from "./QueueRecoveryButton";
import { RecordAmountDialog } from "./RecordAmountDialog";
import { SettleMaterialDialog } from "./SettleMaterialDialog";
import { SpoolFormDialog } from "./SpoolFormDialog";
import styles from "./SpoolDetailDock.module.css";

export interface SpoolDetailDockProps {
  spool?: SpoolRecord;
  mode: "inline" | "overlay";
  onClose: () => void;
}

const AMOUNT_KIND_LABEL: Record<AmountEventKind, string> = {
  initial: "Initial", measurement: "Measured", estimate: "Estimated",
  consumption: "Consumed", markedEmpty: "Marked empty",
};

const MOVEMENT_VERB: Record<MovementReason, string> = {
  load: "Loaded", unload: "Unloaded", displaced: "Displaced", relocate: "Moved",
  consumed: "Marked empty, unloaded", printerArchived: "Moved off an archived Printer",
};

/** P7 D8: `holder.kind` is opaque to P3 -- only `"job"` exists on the wire
 *  today (plus the debug-fixture-only `"debug"` seed), so this maps the
 *  one real case to its display label and falls back to the raw kind for
 *  anything else, rather than guessing at future holder kinds. */
const RESERVATION_HOLDER_LABEL: Record<string, string> = { job: "Job" };

const RESERVATION_STATE_LABEL: Record<ReservationState, string> = {
  active: "Reserved", unresolved: "Reserved (unresolved)", released: "Released", consumed: "Consumed",
};

function reservationItem(r: Reservation): TimelineItem {
  const holderLabel = RESERVATION_HOLDER_LABEL[r.holder.kind] ?? r.holder.kind;
  return {
    id: `reservation-${r.id}`,
    at: r.createdAt,
    title: `${RESERVATION_STATE_LABEL[r.state]} ${formatGrams(r.amountMg, 0)} for ${holderLabel}`,
    // P7: a Job's reservation links to that Job in the Queue.
    detail: r.holder.kind === "job"
      ? <Button variant="ghost" size="sm" onClick={() => showQueueEntry(r.holder.id)}>Open the Job</Button>
      : undefined,
    marker: "muted",
  };
}

function destinationLabel(snapshot: SpoolLocationSnapshot, printerRecords: ResolvedPrinter[]): string {
  if (snapshot.slotId) {
    const printer = printerRecords.find((p) => p.id === snapshot.printerId);
    const slot = printer?.materialSlots.find((s) => s.id === snapshot.slotId);
    return printer && slot ? `${slot.name} on ${printer.name}` : "a Material Slot";
  }
  return snapshot.storageLabel ? `storage (${snapshot.storageLabel})` : "storage";
}

function movementItem(m: SpoolMovement, printerRecords: ResolvedPrinter[]): TimelineItem {
  return {
    id: `movement-${m.id}`,
    at: m.occurredAt,
    title: `${MOVEMENT_VERB[m.reason]} to ${destinationLabel(m.to, printerRecords)}`,
    marker: "default",
  };
}

/** D7: a `measurement`/`estimate` row that follows a `consumption` row is a
 *  correction -- `isCorrection` is computed server-side (`spool_history`),
 *  never re-derived here. Renders exactly the spec's wording: "Measured
 *  612 g (corrected +18 g from the estimate)". */
function amountItem(e: AmountEvent): TimelineItem {
  let title = e.kind === "markedEmpty" ? "Marked empty" : `${AMOUNT_KIND_LABEL[e.kind]} ${formatGrams(e.afterMg, 0)}`;
  if (e.isCorrection && e.beforeMg !== undefined) {
    const delta = e.afterMg - e.beforeMg;
    const sign = delta >= 0 ? "+" : "-";
    title += ` (corrected ${sign}${formatGrams(Math.abs(delta), 0)} from the estimate)`;
  }
  return {
    id: `amount-${e.id}`,
    at: e.occurredAt,
    title,
    detail: e.confidenceAfter === "estimated" ? "est." : undefined,
    marker: "muted",
  };
}

/** Merges movements and amount events newest first (spec §Components:
 *  "a `Timeline` that merges movements and amount events by time"). */
function historyTimelineItems(history: SpoolHistory | null, printerRecords: ResolvedPrinter[]): TimelineItem[] {
  if (!history) return [];
  const items = [
    ...history.movements.map((m) => movementItem(m, printerRecords)),
    ...history.amountEvents.map(amountItem),
    ...history.reservations.map(reservationItem),
  ];
  return items.sort((a, b) => b.at.localeCompare(a.at));
}

export function SpoolDetailDock(props: SpoolDetailDockProps) {
  const content = () => (
    <DockContent spool={props.spool!} overlay={props.mode === "overlay"} onClose={props.onClose} />
  );

  return (
    <Show when={props.spool}>
      <Show
        when={props.mode === "overlay"}
        fallback={<aside class={styles.inline} aria-label={`Spool ${props.spool!.spoolNumber}`}>{content()}</aside>}
      >
        <KDialog open onOpenChange={(open) => !open && props.onClose()}>
          <KDialog.Portal>
            <KDialog.Overlay class={styles.overlay} />
            <KDialog.Content class={styles.overlayContent} onCloseAutoFocus={(event) => event.preventDefault()}>
              {content()}
            </KDialog.Content>
          </KDialog.Portal>
        </KDialog>
      </Show>
    </Show>
  );
}

function DockContent(props: { spool: SpoolRecord; overlay: boolean; onClose: () => void }) {
  const [history, setHistory] = createSignal<SpoolHistory | null>(null);
  const [recordOpen, setRecordOpen] = createSignal(false);
  const [moveOpen, setMoveOpen] = createSignal(false);
  const [editOpen, setEditOpen] = createSignal(false);
  const [actionError, setActionError] = createSignal<string | null>(null);
  /** The Job being settled, by id: the dialog reads it live from the
   *  store, so a settlement made elsewhere shows there, and a Job that
   *  leaves the store closes it. */
  const [settlingId, setSettlingId] = createSignal<string | null>(null);
  const settling = () => {
    const id = settlingId();
    return id ? queue.job(id) : undefined;
  };
  createEffect(() => {
    if (settlingId() && !settling()) setSettlingId(null);
  });
  let settleTrigger: HTMLButtonElement | undefined;

  /** P7: the open material Reconciliation Requirements on this Spool
   *  (the `reconciliation` facet's cause), each settled from here
   *  (`SETTLE_MATERIAL`). */
  const requirements = () => queue.requirements()
    .filter((requirement) => requirement.spoolId === props.spool.id && requirement.kind === "materialReconciliation");

  /** Re-fetched on every revision bump, not just a new id: a move, amount
   *  entry, or lifecycle change bumps the Spool's revision in place, and
   *  history must show it. A response for an id/revision that is no longer
   *  current is dropped, so an older fetch can never overwrite a newer one. */
  createEffect(on(() => [props.spool.id, props.spool.revision] as const, ([id, revision], previous) => {
    if (!previous || previous[0] !== id) setHistory(null);
    const isCurrent = () => props.spool.id === id && props.spool.revision === revision;
    loadHistory(id).then(
      (loaded) => { if (isCurrent()) setHistory(loaded); },
      (e) => { if (isCurrent()) reportSpoolError(e); },
    );
  }));

  const location = () => {
    const loc = props.spool.location;
    if (loc.kind === "storage") return loc.storageLabel ? `In storage — ${loc.storageLabel}` : "In storage";
    const printer = printers().find((p) => p.id === loc.printerId);
    const slot = printer?.materialSlots.find((s) => s.id === loc.slotId);
    return printer && slot ? `Loaded on ${printer.name} — ${slot.name}` : "Loaded";
  };

  /** Non-dialog caller (spec §Errors and recovery, fix round 1 ruling):
   *  `moveSpool` already rejects, so this both shows a local inline error
   *  (contextual, next to Unload) AND routes the same failure to the store
   *  banner via `reportSpoolError` -- belt and suspenders, so a failure is
   *  never silently unhandled even if a future refactor drops the local
   *  message. */
  async function onUnload() {
    setActionError(null);
    try {
      await moveSpool({
        spoolId: props.spool.id,
        expectedSpoolRevision: props.spool.revision,
        destination: { kind: "storage" },
      });
    } catch (e) {
      setActionError(isCommandError(e) ? e.message : "This Spool could not be unloaded.");
      reportSpoolError(e);
    }
  }

  /** Non-dialog caller (fix round 1 ruling): `setLifecycle` now rejects
   *  instead of reporting to the banner itself, so every call site must
   *  catch its own rejection -- this menu renders no inline error UI of
   *  its own, so it routes straight to `reportSpoolError` (the banner). */
  function runLifecycleAction(action: Parameters<typeof setLifecycle>[1]): void {
    setLifecycle(props.spool.id, action).catch(reportSpoolError);
  }

  const lifecycleItems = (): DropdownMenuEntry[] => {
    const lifecycle = props.spool.lifecycle;
    if (lifecycle === "active") {
      return [
        { label: "Mark empty (used up)", onSelect: () => runLifecycleAction("markEmpty") },
        { label: "Archive", onSelect: () => runLifecycleAction("archive") },
      ];
    }
    if (lifecycle === "empty") {
      return [
        { label: "Reactivate", onSelect: () => runLifecycleAction("reactivate") },
        { label: "Archive", onSelect: () => runLifecycleAction("archive") },
      ];
    }
    return [{ label: "Unarchive", onSelect: () => runLifecycleAction("unarchive") }];
  };

  return (
    <div class={styles.dock}>
      <header class={styles.header}>
        <div>
          <Show
            when={props.overlay}
            fallback={<h2 class={styles.title}>#{props.spool.spoolNumber} {materialLabel(props.spool.materialFamily, props.spool.materialOther)} {props.spool.colorName}</h2>}
          >
            <KDialog.Title class={styles.title}>
              #{props.spool.spoolNumber} {materialLabel(props.spool.materialFamily, props.spool.materialOther)} {props.spool.colorName}
            </KDialog.Title>
          </Show>
          <p class={styles.subtitle}>{props.spool.manufacturer}{props.spool.product ? ` — ${props.spool.product}` : ""}</p>
        </div>
        <Button variant="ghost" class={styles.close} onClick={props.onClose}>Close</Button>
      </header>

      <section class={styles.section}>
        <ColorSwatch hex={props.spool.colorHex ?? null} name={props.spool.colorName} />
        <span>{props.spool.colorName}</span>
        <span class={styles.diameter}>{props.spool.diameter} mm</span>
      </section>

      <section class={styles.section}>
        <p class={styles.amountLine}>
          Remaining: {formatGrams(props.spool.availability.currentMg, 1)}
          <Show when={props.spool.facets.confidence === "estimated"}>
            <span class={styles.estimated}>est.</span>
          </Show>
        </p>
        <Show when={props.spool.availability.reservedMg > 0}>
          <p class={styles.amountLine}>Available: {formatGrams(props.spool.availability.availableMg, 1)}</p>
        </Show>
        <div class={styles.facetChips}>
          <Show when={props.spool.facets.low}><span class={styles.chip}>Low</span></Show>
          <Show when={props.spool.facets.reserved}><span class={styles.chip}>Reserved</span></Show>
          <Show when={props.spool.facets.reconciliation}><span class={styles.chip}>Needs reconciliation</span></Show>
        </div>
        <For each={requirements()}>
          {(requirement) => {
            const job = () => queue.job(requirement.jobId);
            return (
              <div class={styles.requirement}>
                <span class={styles.requirementText}>
                  {requirementKindLabel(requirement.kind)}: {requirementStatusLabel(requirement.status)}
                  <Show when={job()}>{(held) => <> · Job on {held().printerSnapshot.name}</>}</Show>
                </span>
                <Show when={job()?.allowedActions.includes("settleMaterial") ? job() : undefined}>
                  {(held) => (
                    <Button
                      variant="secondary"
                      size="sm"
                      onClick={(event: MouseEvent) => {
                        settleTrigger = event.currentTarget as HTMLButtonElement;
                        setSettlingId(held().id);
                      }}
                    >
                      Settle…
                    </Button>
                  )}
                </Show>
                <Button variant="ghost" size="sm" onClick={() => showQueueEntry(requirement.jobId)}>Open the Job</Button>
              </div>
            );
          }}
        </For>
      </section>

      <section class={styles.section}>
        <p class={styles.locationLine}>{location()}</p>
      </section>

      <div class={styles.actions}>
        <Button variant="secondary" onClick={() => setRecordOpen(true)}>Record amount</Button>
        <Button variant="secondary" onClick={() => setMoveOpen(true)}>Move…</Button>
        <Show when={props.spool.facets.loaded}>
          <Button variant="ghost" onClick={() => void onUnload()}>Unload</Button>
        </Show>
        <Button variant="ghost" onClick={() => setEditOpen(true)}>Edit</Button>
        <DropdownMenu trigger={<Button variant="ghost">Lifecycle…</Button>} items={lifecycleItems()} />
      </div>
      <Show when={actionError()}>
        {(message) => <p class={styles.error} role="alert">{message()}</p>}
      </Show>

      <section class={styles.section}>
        <h3 class={styles.historyTitle}>History</h3>
        <Timeline label={`History for Spool ${props.spool.spoolNumber}`} items={historyTimelineItems(history(), printers())} />
      </section>

      <RecordAmountDialog open={recordOpen()} onOpenChange={setRecordOpen} spool={props.spool} />
      <MoveSpoolDialog open={moveOpen()} onOpenChange={setMoveOpen} spool={props.spool} />
      <SpoolFormDialog open={editOpen()} onOpenChange={setEditOpen} spool={props.spool} />
      <Show when={settling()}>
        {(job) => (
          <SettleMaterialDialog
            open
            onOpenChange={(open) => !open && setSettlingId(null)}
            job={job()}
            returnFocus={() => settleTrigger}
          />
        )}
      </Show>
    </div>
  );
}

