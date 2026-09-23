import { Dialog as KDialog } from "@kobalte/core/dialog";
import { createEffect, createSignal, on, Show } from "solid-js";
import { Button, ColorSwatch, DropdownMenu, Timeline } from "../design-system";
import type { DropdownMenuEntry } from "../design-system";
import type { TimelineItem } from "../design-system";
import { isCommandError } from "../ipc/client";
import { materialLabel } from "../spools/materials";
import { formatGrams } from "../spools/weight";
import { loadHistory, moveSpool, setLifecycle } from "../spools/spool-store";
import { printers } from "../printers/printer-store";
import type { ResolvedPrinter } from "../printers/types";
import type { AmountEvent } from "../generated/contracts/domain/AmountEvent";
import type { AmountEventKind } from "../generated/contracts/domain/AmountEventKind";
import type { MovementReason } from "../generated/contracts/domain/MovementReason";
import type { SpoolHistory } from "../generated/contracts/command/SpoolHistory";
import type { SpoolLocationSnapshot } from "../generated/contracts/domain/SpoolLocationSnapshot";
import type { SpoolMovement } from "../generated/contracts/domain/SpoolMovement";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import { MoveSpoolDialog } from "./MoveSpoolDialog";
import { RecordAmountDialog } from "./RecordAmountDialog";
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

  createEffect(on(() => props.spool.id, (id) => {
    setHistory(null);
    void loadHistory(id).then(setHistory);
  }));

  const location = () => {
    const loc = props.spool.location;
    if (loc.kind === "storage") return loc.storageLabel ? `In storage — ${loc.storageLabel}` : "In storage";
    const printer = printers().find((p) => p.id === loc.printerId);
    const slot = printer?.materialSlots.find((s) => s.id === loc.slotId);
    return printer && slot ? `Loaded on ${printer.name} — ${slot.name}` : "Loaded";
  };

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
    }
  }

  const lifecycleItems = (): DropdownMenuEntry[] => {
    const lifecycle = props.spool.lifecycle;
    if (lifecycle === "active") {
      return [
        { label: "Mark empty (used up)", onSelect: () => void setLifecycle(props.spool.id, "markEmpty") },
        { label: "Archive", onSelect: () => void setLifecycle(props.spool.id, "archive") },
      ];
    }
    if (lifecycle === "empty") {
      return [
        { label: "Reactivate", onSelect: () => void setLifecycle(props.spool.id, "reactivate") },
        { label: "Archive", onSelect: () => void setLifecycle(props.spool.id, "archive") },
      ];
    }
    return [{ label: "Unarchive", onSelect: () => void setLifecycle(props.spool.id, "unarchive") }];
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
        </div>
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
    </div>
  );
}

