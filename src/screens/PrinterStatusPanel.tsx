import { For, onMount, Show } from "solid-js";
import { ColorSwatch } from "../design-system";
import { serializeNavigationTarget } from "../navigation/navigation-store";
import type { MaterialSlot, ResolvedPrinter } from "../printers/types";
import { materialLabel } from "../spools/materials";
import { ensureInventoryLoaded, spoolState } from "../spools/spool-store";
import { formatGrams } from "../spools/weight";
import { formatTemperature } from "./monitor-printer-presentation";
import styles from "./PrinterStatusPanel.module.css";

export interface PrinterStatusPanelProps {
  printer: ResolvedPrinter;
  syncState?: "syncing" | "current" | "uncertain";
}

const readinessLabels = {
  setupIncomplete: "Setup incomplete",
  connectionError: "Connection error",
  offline: "Offline",
  refreshing: "Refreshing status",
  staleTelemetry: "Stale telemetry",
  printerBusy: "Printer busy",
  unknownState: "Unknown state",
  archived: "Archived",
} as const;

function formatObservedAge(timestamp: string | undefined): string {
  if (!timestamp) return "Unavailable";
  const elapsed = Date.now() - Date.parse(timestamp);
  if (!Number.isFinite(elapsed)) return "Unavailable";
  if (elapsed < 60_000) return "just now";
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 60) return `${minutes} minute${minutes === 1 ? "" : "s"} ago`;
  const hours = Math.floor(minutes / 60);
  return `${hours} hour${hours === 1 ? "" : "s"} ago`;
}

export function PrinterStatusPanel(props: PrinterStatusPanelProps) {
  const status = () => props.printer.runtimeStatus;
  const readings = () => status()?.telemetry;
  const fields = () => [
    ["Operational state", status()?.operationalState ?? "Unavailable"],
    ["Readiness", status()?.readiness.state ?? "Unavailable"],
    ["Reason", status()?.readiness.reason ? readinessLabels[status()!.readiness.reason!] : "—"],
    ["Connection", status()?.connectionState ?? "Unavailable"],
    ["Host activity", readings()?.hostActivityName ?? readings()?.hostActivity ?? "—"],
    ["Progress", readings()?.progress === undefined ? "—" : `${Math.round(readings()!.progress! * 100)}%`],
    ["Nozzle", formatTemperature(readings()?.nozzleTempC, readings()?.nozzleTargetC)],
    ["Bed", formatTemperature(readings()?.bedTempC, readings()?.bedTargetC)],
    ["Freshness", status()?.freshness ?? "Unavailable"],
    ["Last observed", formatObservedAge(status()?.lastObservedAt)],
  ];

  onMount(() => void ensureInventoryLoaded());

  return (
    <div class={styles.panel}>
      <p class={styles.summary}>Operational status and retained adapter telemetry.</p>
      <dl class={styles.fields}>
        <For each={fields()}>{([label, value]) => <div class={styles.field}><dt>{label}</dt><dd>{value}</dd></div>}</For>
      </dl>
      <MaterialSlotsList slots={props.printer.materialSlots} />
      <p class={styles.sync} aria-live="polite">
        {props.syncState === "uncertain" ? "Live status is still reconciling." : ""}
      </p>
    </div>
  );
}

/** Opens the Spool in the Spools destination; App's `hashchange` listener
 *  does the navigating, exactly as for a pasted deep link. */
function openSpool(spoolId: string): void {
  window.location.hash = serializeNavigationTarget({
    version: 1, destination: "spools", selection: { kind: "spool", id: spoolId },
  }).slice(1);
}

/** Spec §Components "Printer Status tab": each slot's occupant (number,
 *  material, swatch and color name, remaining with its confidence marker,
 *  Low/Reserved) or "Empty". Facets come from Rust; none are re-derived. */
function MaterialSlotsList(props: { slots: MaterialSlot[] }) {
  const occupantOf = (slot: MaterialSlot) =>
    slot.occupantSpoolId ? spoolState.spools.find((s) => s.id === slot.occupantSpoolId) : undefined;

  return (
    <section class={styles.slots} aria-labelledby="printer-status-slots-title">
      <h3 id="printer-status-slots-title" class={styles.slotsTitle}>Material Slots</h3>
      <ul class={styles.slotList} aria-label="Material slots">
        <For each={props.slots}>
          {(slot) => (
            <li class={styles.slot}>
              <span class={styles.slotName}>
                {slot.name}
                <Show when={slot.feederLabel}>{(feeder) => <span class={styles.feeder}>{feeder()}</span>}</Show>
              </span>
              <Show
                when={occupantOf(slot)}
                fallback={<span class={styles.empty}>{slot.occupantSpoolId ? "Loaded" : "Empty"}</span>}
              >
                {(spool) => (
                  <span class={styles.occupantLine}>
                    <button type="button" class={styles.occupant} onClick={() => openSpool(spool().id)}>
                      <span class={styles.number}>#{spool().spoolNumber}</span>
                      <span>{materialLabel(spool().materialFamily, spool().materialOther)}</span>
                      <ColorSwatch hex={spool().colorHex ?? null} name={spool().colorName} size="sm" />
                      <span>{spool().colorName}</span>
                      <span class={styles.remaining}>
                        {formatGrams(spool().availability.currentMg, 0)}
                        <Show when={spool().facets.confidence === "estimated"}>
                          <span class={styles.estimated}> est.</span>
                        </Show>
                      </span>
                    </button>
                    <Show when={spool().facets.low}><span class={styles.chip}>Low</span></Show>
                    <Show when={spool().facets.reserved}><span class={styles.chip}>Reserved</span></Show>
                  </span>
                )}
              </Show>
            </li>
          )}
        </For>
      </ul>
    </section>
  );
}
