import { For, Show } from "solid-js";
import { Button, Chip, Select, TextField } from "../design-system";
import type { MonitorFilter, MonitorStore } from "../monitor/monitor-store";
import styles from "./MonitorToolbar.module.css";

export interface MonitorToolbarProps {
  store: MonitorStore;
  onAddPrinter: () => void;
  onAddPrinters?: () => void;
  onImport?: () => void;
  onExport?: () => void;
}

const filters: readonly { value: MonitorFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "attention", label: "Attention" },
  { value: "printing", label: "Printing" },
  { value: "ready", label: "Ready" },
  { value: "offline", label: "Offline" },
  { value: "setupIncomplete", label: "Setup incomplete" },
  { value: "archived", label: "Archived" },
];

const sections = [
  { value: "printerModel", label: "Printer model" },
  { value: "location", label: "Location" },
  { value: "operationalState", label: "Operational state" },
  { value: "none", label: "No section" },
] as const;

const densities = [
  { value: "comfortable", label: "Comfortable" },
  { value: "compact", label: "Compact" },
] as const;

export function MonitorToolbar(props: MonitorToolbarProps) {
  return (
    <div class={styles.toolbar}>
      <TextField
        class={styles.search}
        type="search"
        aria-label="Search Printers"
        placeholder="Search Printers"
        value={props.store.search()}
        onChange={props.store.setSearch}
      />
      <div class={styles.filters} aria-label="Monitor filters">
        <For each={filters}>
          {(filter) => (
            <Chip
              selected={props.store.filter() === filter.value}
              onSelectedChange={(selected) => selected && props.store.setFilter(filter.value)}
            >
              {filter.label}
            </Chip>
          )}
        </For>
      </div>
      <div class={styles.preferences}>
        <Select
          label="Monitor section"
          options={[...sections]}
          value={sections.find((option) => option.value === props.store.section()) ?? sections[0]}
          optionValue={(option) => option.value}
          optionLabel={(option) => option.label}
          onChange={(option) => props.store.setSection(option.value)}
        />
        <Select
          label="Monitor density"
          options={[...densities]}
          value={densities.find((option) => option.value === props.store.density()) ?? densities[0]}
          optionValue={(option) => option.value}
          optionLabel={(option) => option.label}
          onChange={(option) => props.store.setDensity(option.value)}
        />
      </div>
      <div class={styles.actions}>
        <Show when={props.onImport}>
          <Button variant="ghost" size="sm" onClick={() => props.onImport?.()}>Import</Button>
        </Show>
        <Show when={props.onExport}>
          <Button variant="ghost" size="sm" onClick={() => props.onExport?.()}>Export</Button>
        </Show>
        <Show when={props.onAddPrinters}>
          <Button variant="secondary" size="sm" onClick={() => props.onAddPrinters?.()}>Add Printers…</Button>
        </Show>
        <Button size="sm" onClick={props.onAddPrinter}>Add Printer</Button>
      </div>
      <Show when={props.store.preferenceError()}>
        {(error) => <p class={styles.error} role="alert">{error()}</p>}
      </Show>
    </div>
  );
}
