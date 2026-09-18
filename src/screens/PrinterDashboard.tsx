import { For, Match, Show, Switch, createSignal } from "solid-js";
import { Button, PrinterRoster } from "../design-system";
import type { MonitorStore } from "../monitor/monitor-store";
import type { PrinterDraft, ResolvedPrinter } from "../printers/types";
import { MonitorToolbar } from "./MonitorToolbar";
import { PrinterAddDialog } from "./PrinterAddDialog";
import { PrinterCard } from "./PrinterCard";
import { PrinterCompactRow } from "./PrinterCompactRow";
import { operationalLabel } from "./monitor-printer-presentation";
import styles from "./PrinterDashboard.module.css";

export interface PrinterDashboardProps {
  store: MonitorStore;
  loading?: boolean;
  isFirstRun?: boolean;
  syncState?: "syncing" | "current" | "uncertain";
  onSelectionChange?: (id: string | null) => void;
  onAddPrinter?: (draft: PrinterDraft) => Promise<ResolvedPrinter | undefined>;
  onImport?: () => void;
  onExport?: () => void;
}

export function PrinterDashboard(props: PrinterDashboardProps) {
  const [addDialogOpen, setAddDialogOpen] = createSignal(false);
  const selectPrinter = (id: string) => {
    props.store.setSelectedPrinterId(id);
    props.onSelectionChange?.(id);
  };
  const clearFilters = () => {
    props.store.setSearch("");
    props.store.setFilter("all");
  };
  const loadingWithNoPrinters = () => Boolean(props.loading) && !props.store.hasPrinters();
  const isFirstRun = () => Boolean(props.isFirstRun) && !props.store.hasPrinters();

  return (
    <div class={styles.dashboard}>
      <MonitorToolbar
        store={props.store}
        onAddPrinter={() => setAddDialogOpen(true)}
        onImport={props.onImport}
        onExport={props.onExport}
      />
      <Show when={props.loading && props.store.hasPrinters()}>
        <p class={styles.loadingNotice} role="status">Loading persisted Printer updates…</p>
      </Show>
      <Show when={props.syncState === "uncertain"}>
        <p class={styles.syncNotice} role="status">Live status is still reconciling.</p>
      </Show>
      <div class={styles.content}>
        <Switch>
          <Match when={loadingWithNoPrinters()}>
            <EmptyState message="Loading persisted Printers…" />
          </Match>
          <Match when={isFirstRun()}>
            <EmptyState message="Start your Farm by adding a Printer." onAdd={() => setAddDialogOpen(true)} />
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
                <section class={styles.section}>
                  <Show when={section.label}>
                    <header class={styles.sectionHeader}>
                      <h2 class={styles.sectionTitle}>{section.label}</h2>
                      <PrinterRoster
                        label={section.printers.length === 1 ? "Printer" : "Printers"}
                        count={section.printers.length}
                        printers={section.printers.map((printer) => ({
                          id: printer.id,
                          name: printer.name,
                          stateLabel: operationalLabel(printer),
                        }))}
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
            <EmptyState message="This Farm has no Printers." onAdd={() => setAddDialogOpen(true)} />
          </Match>
        </Switch>
      </div>
      <PrinterAddDialog
        open={addDialogOpen()}
        onOpenChange={setAddDialogOpen}
        onAdd={(draft) => props.onAddPrinter?.(draft) ?? Promise.resolve(undefined)}
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
