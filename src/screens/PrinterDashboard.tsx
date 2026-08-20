import { createMemo, createSignal, For, onCleanup, Show } from "solid-js";
import { Button } from "../design-system";
import type { ResolvedPrinter } from "../printers/types";
import styles from "./PrinterDashboard.module.css";

export interface PrinterGroup {
  modelKey: string;
  modelLabel: string;
  printers: ResolvedPrinter[];
}

const UNLINKED_KEY = "__unlinked__";

/** Groups by catalog model; a Printer whose catalog reference doesn't
 *  resolve (renamed/removed upstream preset) lands in a trailing "Unlinked"
 *  group instead of a normal model group. */
export function groupPrintersByModel(printers: ResolvedPrinter[]): PrinterGroup[] {
  const byModel = new Map<string, ResolvedPrinter[]>();
  for (const printer of printers) {
    const key =
      printer.catalogStatus === "ok" || printer.catalogStatus === "rematched"
        ? printer.catalogRef.modelId
        : UNLINKED_KEY;
    const list = byModel.get(key) ?? [];
    list.push(printer);
    byModel.set(key, list);
  }

  const groups: PrinterGroup[] = [];
  for (const [key, list] of byModel) {
    if (key === UNLINKED_KEY) continue;
    groups.push({
      modelKey: key,
      modelLabel: list[0].modelLabel,
      printers: [...list].sort((a, b) => a.name.localeCompare(b.name)),
    });
  }
  groups.sort((a, b) => a.modelLabel.localeCompare(b.modelLabel));

  const unlinked = byModel.get(UNLINKED_KEY);
  if (unlinked && unlinked.length > 0) {
    groups.push({
      modelKey: UNLINKED_KEY,
      modelLabel: "Unlinked",
      printers: [...unlinked].sort((a, b) => a.name.localeCompare(b.name)),
    });
  }
  return groups;
}

/** "3 printers" — for AppShell's status bar. Per-status counts return once
 *  phase 3 wires `runtimeStatus`; every Printer is status-less in phase 1. */
export function summarizePrinters(printers: ResolvedPrinter[]): string {
  if (printers.length === 0) return "No printers";
  return `${printers.length} printer${printers.length === 1 ? "" : "s"}`;
}

export interface PrinterDashboardProps {
  printers: ResolvedPrinter[];
  onAddPrinter?: () => void;
  onRemovePrinter?: (id: string) => void;
}

const DEFAULT_DETAIL_WIDTH = 320;
const MIN_DETAIL_WIDTH = 220;
const MAX_DETAIL_WIDTH = 560;

export function PrinterDashboard(props: PrinterDashboardProps) {
  const [selectedId, setSelectedId] = createSignal<string | null>(null);
  const selected = createMemo(() => props.printers.find((p) => p.id === selectedId()));
  const groups = createMemo(() => groupPrintersByModel(props.printers));
  const [detailWidth, setDetailWidth] = createSignal(DEFAULT_DETAIL_WIDTH);

  let dragStartX = 0;
  let dragStartWidth = 0;

  function onResizeMove(event: PointerEvent) {
    const delta = dragStartX - event.clientX;
    const next = Math.min(MAX_DETAIL_WIDTH, Math.max(MIN_DETAIL_WIDTH, dragStartWidth + delta));
    setDetailWidth(next);
  }

  function stopResizing() {
    window.removeEventListener("pointermove", onResizeMove);
    window.removeEventListener("pointerup", stopResizing);
    document.body.style.cursor = "";
    document.body.style.userSelect = "";
  }

  function onResizeStart(event: PointerEvent) {
    event.preventDefault();
    dragStartX = event.clientX;
    dragStartWidth = detailWidth();
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    window.addEventListener("pointermove", onResizeMove);
    window.addEventListener("pointerup", stopResizing);
  }

  onCleanup(stopResizing);

  return (
    <div class={styles.dashboard}>
      {/* `.main` is a column wrapper: Task 15 mounts a toolbar as its first
          child, stacked above `.groups`. `.dashboard` itself stays row-direction
          so the resizable detail aside remains a sibling, not nested here. */}
      <div class={styles.main}>
        <div class={styles.groups}>
          <Show
            when={props.printers.length > 0}
            fallback={
              <div class={styles.empty}>
                <p class={styles.emptyMessage}>No printers yet</p>
                <Button variant="secondary" onClick={() => props.onAddPrinter?.()}>
                  + Add printer
                </Button>
              </div>
            }
        >
          <For each={groups()}>
            {(group) => (
              <section class={styles.group}>
                <header class={styles.groupHeader}>
                  <span class={styles.groupTitle}>{group.modelLabel}</span>
                  <span class={styles.groupCount}>{group.printers.length}</span>
                </header>
                <div class={styles.grid}>
                  <For each={group.printers}>
                    {(printer) => (
                      <button
                        class={styles.card}
                        classList={{ [styles.cardSelected]: printer.id === selectedId() }}
                        onClick={() => setSelectedId(printer.id)}
                      >
                        <div class={styles.cardHeader}>
                          <span class={styles.cardName}>{printer.name}</span>
                        </div>
                        <div class={styles.badgeRow}>
                          <Show when={printer.catalogStatus !== "ok"}>
                            <span class={[styles.badge, styles.badgeWarning].join(" ")}>
                              Unlinked
                            </span>
                          </Show>
                          <Show when={printer.profileDrift.length > 0}>
                            <span class={[styles.badge, styles.badgeAccent].join(" ")}>
                              Profile updated
                            </span>
                          </Show>
                          <Show when={printer.overriddenFields.length > 0}>
                            <span class={[styles.badge, styles.badgeMuted].join(" ")}>
                              {printer.overriddenFields.length} override
                              {printer.overriddenFields.length === 1 ? "" : "s"}
                            </span>
                          </Show>
                          <Show when={printer.unknownOverrideKeys.length > 0}>
                            <span
                              class={[styles.badge, styles.badgeWarning].join(" ")}
                              title={`Unrecognized override keys: ${printer.unknownOverrideKeys.join(", ")}`}
                            >
                              {printer.unknownOverrideKeys.length} unknown key
                              {printer.unknownOverrideKeys.length === 1 ? "" : "s"}
                            </span>
                          </Show>
                        </div>
                        <div class={styles.cardFooter}>
                          <span class={styles.variant}>{printer.variantLabel}</span>
                        </div>
                      </button>
                    )}
                  </For>
                </div>
              </section>
            )}
          </For>
        </Show>
      </div>
      </div>

      <Show when={selected()}>
        {(printer) => (
          <>
            <div
              class={styles.resizeHandle}
              onPointerDown={onResizeStart}
              role="separator"
              aria-orientation="vertical"
              aria-label="Resize printer detail panel"
            />
            <aside
              class={styles.detail}
              style={{ width: `${detailWidth()}px` }}
              aria-label="Printer detail"
            >
              <div class={styles.detailHeader}>
                <span>{printer().name}</span>
                <Button variant="danger" onClick={() => props.onRemovePrinter?.(printer().id)}>
                  Remove
                </Button>
              </div>
              <div class={styles.detailBody}>
                {/* Task 16 replaces this placeholder with <PrinterProfilePanel>
                    wrapped in the Status/Profile/Connection Tabs. */}
                <p class={styles.detailMuted}>Profile — wired up in Task 16.</p>
              </div>
            </aside>
          </>
        )}
      </Show>
    </div>
  );
}
