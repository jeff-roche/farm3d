import { createMemo, createSignal, For, onCleanup, Show } from "solid-js";
import { Button, Tabs } from "../design-system";
import type { PrinterDraft, ResolvedPrinter } from "../printers/types";
import { openPrintersFile } from "../printers/printer-store";
import { PrinterAddDialog } from "./PrinterAddDialog";
import { PrinterConnectionPanel } from "./PrinterConnectionPanel";
import { PrinterProfilePanel } from "./PrinterProfilePanel";
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

/** "3 printers — 1 online, 1 offline" for AppShell's status bar. Printers
 *  that have never reported are counted in the total only: a printer with no
 *  Connection configured is not "offline", it is simply not connected. */
export function summarizePrinters(printers: ResolvedPrinter[]): string {
  if (printers.length === 0) return "No printers";
  const total = `${printers.length} printer${printers.length === 1 ? "" : "s"}`;
  const counts = { online: 0, offline: 0, error: 0, connecting: 0 };
  for (const printer of printers) {
    const state = printer.runtimeStatus?.connectionState;
    if (state) counts[state] += 1;
  }
  const parts = (["online", "connecting", "offline", "error"] as const)
    .filter((state) => counts[state] > 0)
    .map((state) => `${counts[state]} ${state}`);
  return parts.length > 0 ? `${total} — ${parts.join(", ")}` : total;
}

function formatTemp(value: number | undefined): string {
  return value === undefined ? "—" : `${Math.round(value)} °C`;
}

export interface PrinterDashboardProps {
  printers: ResolvedPrinter[];
  onAddPrinter?: (draft: PrinterDraft) => void;
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
  const [addDialogOpen, setAddDialogOpen] = createSignal(false);

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
        <div class={styles.toolbar}>
          {/* An escape hatch for hand-editing — notably the only way to
              recover a printer whose catalog ref no longer resolves. */}
          <Button variant="ghost" onClick={() => void openPrintersFile()}>
            Open printers.json
          </Button>
          <PrinterAddDialog
            open={addDialogOpen()}
            onOpenChange={setAddDialogOpen}
            onAdd={(draft) => props.onAddPrinter?.(draft)}
          />
        </div>
        <div class={styles.groups}>
          <Show
            when={props.printers.length > 0}
            fallback={
              <div class={styles.empty}>
                <p class={styles.emptyMessage}>No printers yet — add one with the button above.</p>
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
                          <Show when={printer.runtimeStatus}>
                            {(status) => (
                              <span
                                class={[styles.badge, styles[`state_${status().connectionState}`]].join(" ")}
                                title={status().error ?? `Updated ${status().updatedAt}`}
                              >
                                {status().connectionState}
                              </span>
                            )}
                          </Show>
                          <Show
                            when={
                              printer.catalogStatus !== "ok" &&
                              printer.catalogStatus !== "rematched"
                            }
                          >
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
                          <span>{printer.variantLabel}</span>
                          <Show when={printer.runtimeStatus}>
                            {(status) => (
                              // An unreported reading is an em dash, never a
                              // zero — "0 °C" reads as a real measurement.
                              <span class={styles.readings}>
                                {formatTemp(status().nozzleTempC)} / {formatTemp(status().bedTempC)}
                              </span>
                            )}
                          </Show>
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
                <Tabs
                  defaultValue="profile"
                  items={[
                    {
                      value: "status",
                      label: "Status",
                      content: (
                        <div class={styles.detailField}>
                          <span class={styles.detailLabel}>Catalog</span>
                          <span>{printer().modelLabel} — {printer().variantLabel}</span>
                          <Show when={printer().catalogStatus !== "ok"}>
                            <span class={styles.detailMuted}>
                              Catalog status: {printer().catalogStatus}
                            </span>
                          </Show>
                        </div>
                      ),
                    },
                    { value: "profile", label: "Profile", content: <PrinterProfilePanel printer={printer()} /> },
                    {
                      value: "connection",
                      label: "Connection",
                      content: <PrinterConnectionPanel printer={printer()} />,
                    },
                  ]}
                />
              </div>
            </aside>
          </>
        )}
      </Show>
    </div>
  );
}
