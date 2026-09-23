import { For, Match, Show, Switch, createSignal, onCleanup, onMount } from "solid-js";
import { Button, PrinterRoster } from "../design-system";
import type { MonitorStore } from "../monitor/monitor-store";
import type { ResolvedPrinter } from "../printers/types";
import { MonitorToolbar } from "./MonitorToolbar";
import { PrinterSetupWizard } from "./PrinterSetupWizard";
import { PrinterCard } from "./PrinterCard";
import { PrinterCompactRow } from "./PrinterCompactRow";
import { PrinterDetailDock } from "./PrinterDetailDock";
import styles from "./PrinterDashboard.module.css";

export interface PrinterDashboardProps {
  store: MonitorStore;
  loading?: boolean;
  isFirstRun?: boolean;
  syncState?: "syncing" | "current" | "uncertain";
  onSelectionChange?: (id: string | null) => void;
  /** Every printer already in the Farm, for the setup wizard's unique-name
   *  suggestion and duplicate-name warning. */
  existingPrinters?: ResolvedPrinter[];
  onPrinterCreated?: (printer: ResolvedPrinter) => void;
  onImport?: () => void;
  onExport?: () => void;
  onRemovePrinter?: (id: string) => void;
}

export function PrinterDashboard(props: PrinterDashboardProps) {
  const [wizardOpen, setWizardOpen] = createSignal(false);
  const sections = new Map<string, HTMLElement>();
  let workspace: HTMLDivElement | undefined;
  let selectionTrigger: HTMLButtonElement | undefined;
  let focusTimer: number | undefined;
  const [dockMode, setDockMode] = createSignal<"inline" | "overlay">("overlay");
  const selectPrinter = (id: string) => {
    props.store.setSelectedPrinterId(id);
    props.onSelectionChange?.(id);
  };
  const closeDock = () => {
    props.store.setSelectedPrinterId(null);
    props.onSelectionChange?.(null);
    queueMicrotask(() => selectionTrigger?.focus());
  };
  const clearFilters = () => {
    props.store.setSearch("");
    props.store.setFilter("all");
  };
  const loadingWithNoPrinters = () => Boolean(props.loading) && !props.store.hasPrinters();
  const isFirstRun = () => Boolean(props.isFirstRun) && !props.store.hasPrinters();
  const focusSection = (key: string) => {
    window.clearTimeout(focusTimer);
    focusTimer = window.setTimeout(() => {
      const section = sections.get(key);
      section?.scrollIntoView?.({ block: "start" });
      section?.querySelector<HTMLElement>("h2")?.focus();
    });
  };
  onCleanup(() => window.clearTimeout(focusTimer));
  onMount(() => {
    if (!workspace) return;
    const rem = Number.parseFloat(window.getComputedStyle(document.documentElement).fontSize) || 16;
    const minimumInlineWidth = 66 * rem; // 44rem workspace plus the 22rem dock.
    const updateMode = (width: number) => setDockMode(width >= minimumInlineWidth ? "inline" : "overlay");
    updateMode(workspace.clientWidth);
    const observer = new ResizeObserver((entries) => updateMode(entries[0]?.contentRect.width ?? workspace!.clientWidth));
    observer.observe(workspace);
    onCleanup(() => observer.disconnect());
  });

  return (
    <div class={styles.dashboard}>
      <MonitorToolbar
        store={props.store}
        onAddPrinter={() => setWizardOpen(true)}
        onImport={props.onImport}
        onExport={props.onExport}
      />
      <Show when={props.loading && props.store.hasPrinters()}>
        <p class={styles.loadingNotice} role="status">Loading persisted Printer updates…</p>
      </Show>
      <Show when={props.syncState === "uncertain"}>
        <p class={styles.syncNotice} role="status">Live status is still reconciling.</p>
      </Show>
      <div ref={workspace} class={styles.workspace}>
        <div class={styles.content}>
        <Switch>
          <Match when={loadingWithNoPrinters()}>
            <EmptyState message="Loading persisted Printers…" />
          </Match>
          <Match when={isFirstRun()}>
            <EmptyState message="Start your Farm by adding a Printer." onAdd={() => setWizardOpen(true)} />
          </Match>
          <Match when={props.store.isFilteredEmpty()}>
            <div class={styles.empty}>
              <p>No Printers match the current search and filters.</p>
              <Button variant="ghost" onClick={clearFilters}>Clear search and filters</Button>
            </div>
          </Match>
          <Match when={props.store.hasPrinters()}>
            <For each={props.store.sections()}>
              {(section) => (
                <section ref={(element) => sections.set(section.key, element)} class={styles.section}>
                  <Show when={section.label}>
                    <header class={styles.sectionHeader}>
                      <h2 class={styles.sectionTitle} tabIndex={-1}>{section.label}</h2>
                      <PrinterRoster
                        label={section.printers.length === 1 ? "Printer" : "Printers"}
                        count={section.printers.length}
                        printers={section.printers.map((printer) => ({
                          id: printer.id,
                          name: printer.name,
                          stateLabel: printer.operationalLabel,
                        }))}
                        onViewAll={() => focusSection(section.key)}
                      />
                    </header>
                  </Show>
                  <div
                    class={styles.printers}
                    classList={{ [styles.compact]: props.store.density() === "compact" }}
                  >
                    <div class={styles.cards}>
                      <For each={section.printers}>
                        {(printer) => (
                          <PrinterCard
                            printer={printer}
                            selected={props.store.selectedPrinterId() === printer.id}
                            onSelect={selectPrinter}
                            onSelectTrigger={(trigger) => (selectionTrigger = trigger)}
                          />
                        )}
                      </For>
                    </div>
                    <div class={styles.rows}>
                      <For each={section.printers}>
                        {(printer) => (
                          <PrinterCompactRow
                            printer={printer}
                            selected={props.store.selectedPrinterId() === printer.id}
                            onSelect={selectPrinter}
                            onSelectTrigger={(trigger) => (selectionTrigger = trigger)}
                          />
                        )}
                      </For>
                    </div>
                  </div>
                </section>
              )}
            </For>
          </Match>
          <Match when={true}>
            <EmptyState message="This Farm has no Printers." onAdd={() => setWizardOpen(true)} />
          </Match>
        </Switch>
        </div>
        <PrinterDetailDock
          printer={props.store.selectedPrinter()}
          mode={dockMode()}
          onClose={closeDock}
          onRemove={(id) => props.onRemovePrinter?.(id)}
          syncState={props.syncState}
        />
      </div>
      <PrinterSetupWizard
        open={wizardOpen()}
        onOpenChange={setWizardOpen}
        existingPrinters={props.existingPrinters ?? []}
        onCreated={props.onPrinterCreated}
      />
    </div>
  );
}

function EmptyState(props: { message: string; onAdd?: () => void }) {
  return (
    <div class={styles.empty}>
      <p>{props.message}</p>
      <Show when={props.onAdd}>
        <Button onClick={() => props.onAdd?.()}>Add Printer</Button>
      </Show>
    </div>
  );
}
