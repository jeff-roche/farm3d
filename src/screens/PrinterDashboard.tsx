import { createMemo, createSignal, For, onCleanup, Show } from "solid-js";
import { IconUsb, IconWifi } from "@tabler/icons-solidjs";
import { Button, Progress } from "../design-system";
import styles from "./PrinterDashboard.module.css";

export interface Printer {
  id: string;
  name: string;
  status: "idle" | "printing" | "paused" | "error" | "offline";
  connectionType: "network" | "serial";
  currentJob?: { modelName: string; progress: number };
  nozzleTempC?: number;
  bedTempC?: number;
}

export interface PrinterDashboardProps {
  printers: Printer[];
}

const STATUS_LABEL: Record<Printer["status"], string> = {
  idle: "Idle",
  printing: "Printing",
  paused: "Paused",
  error: "Error",
  offline: "Offline",
};

/** "3 printers · 1 printing · 2 idle" — for AppShell's status bar. */
export function summarizePrinters(printers: Printer[]): string {
  if (printers.length === 0) return "No printers";
  const printing = printers.filter((p) => p.status === "printing").length;
  const idle = printers.filter((p) => p.status === "idle").length;
  return `${printers.length} printer${printers.length === 1 ? "" : "s"} · ${printing} printing · ${idle} idle`;
}

const DEFAULT_DETAIL_WIDTH = 256;
const MIN_DETAIL_WIDTH = 220;
const MAX_DETAIL_WIDTH = 480;

export function PrinterDashboard(props: PrinterDashboardProps) {
  const [selectedId, setSelectedId] = createSignal<string | null>(null);
  const selected = createMemo(() => props.printers.find((p) => p.id === selectedId()));
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
      <div class={styles.grid}>
        <Show
          when={props.printers.length > 0}
          fallback={
            <div class={styles.empty}>
              <p class={styles.emptyMessage}>No printers yet</p>
              <Button variant="secondary" disabled title="Adding printers isn't wired up yet">
                + Add printer
              </Button>
            </div>
          }
        >
          <For each={props.printers}>
            {(printer) => (
              <button
                class={styles.card}
                classList={{ [styles.cardSelected]: printer.id === selectedId() }}
                onClick={() => setSelectedId(printer.id)}
              >
                <div class={styles.cardHeader}>
                  <span class={styles.cardName}>{printer.name}</span>
                  <span
                    class={styles.statusBadge}
                    classList={{ [styles[`statusBadge_${printer.status}`]]: true }}
                  >
                    {STATUS_LABEL[printer.status]}
                  </span>
                </div>

                {printer.currentJob && (
                  <div class={styles.jobRow}>
                    <span class={styles.jobName}>{printer.currentJob.modelName}</span>
                    <Progress value={printer.currentJob.progress * 100} showValue />
                  </div>
                )}

                <div class={styles.cardFooter}>
                  <span class={styles.temps}>
                    {printer.nozzleTempC != null && `${printer.nozzleTempC}°C nozzle`}
                    {printer.bedTempC != null && ` · ${printer.bedTempC}°C bed`}
                  </span>
                  <span class={styles.connection}>
                    {printer.connectionType === "network" ? (
                      <IconWifi size={12} />
                    ) : (
                      <IconUsb size={12} />
                    )}
                    {printer.connectionType}
                  </span>
                </div>
              </button>
            )}
          </For>
        </Show>
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
              <div class={styles.detailHeader}>{printer().name}</div>
              <div class={styles.detailBody}>
                <div class={styles.detailField}>
                  <span class={styles.detailLabel}>Status</span>
                  <span>{STATUS_LABEL[printer().status]}</span>
                </div>
                <div class={styles.detailField}>
                  <span class={styles.detailLabel}>Webcam</span>
                  <div class={styles.webcamPlaceholder}>No feed configured</div>
                </div>
                <div class={styles.detailField}>
                  <span class={styles.detailLabel}>Loaded material</span>
                  <span class={styles.detailMuted}>Not tracked yet</span>
                </div>
                <Button variant="secondary" disabled title="Job assignment isn't wired up yet">
                  Assign job
                </Button>
                <Button variant="danger" disabled title="Cancelling isn't wired up yet">
                  Cancel job
                </Button>
              </div>
            </aside>
          </>
        )}
      </Show>
    </div>
  );
}
