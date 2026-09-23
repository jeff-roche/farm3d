import { createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { Button, Chip, ColorSwatch, DataTable, DropdownMenu, Select, TextField } from "../design-system";
import type { DataTableColumn } from "../design-system";
import {
  applySpoolFilter,
  DEFAULT_SPOOL_FILTER,
  isFiltered,
  type SpoolFacetKey,
  type SpoolFilter,
} from "../spools/facets";
import { MATERIAL_FAMILIES, materialFamilyLabel, materialLabel } from "../spools/materials";
import { formatGrams } from "../spools/weight";
import { loadInventory, spoolState, spoolStoreError, dismissSpoolStoreError } from "../spools/spool-store";
import { printers } from "../printers/printer-store";
import type { ResolvedPrinter } from "../printers/types";
import { navigation, serializeNavigationTarget, type NavigationTarget } from "../navigation/navigation-store";
import type { MaterialFamily } from "../generated/contracts/domain/MaterialFamily";
import type { PrinterRecord } from "../generated/contracts/domain/PrinterRecord";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import { SpoolDetailDock } from "./SpoolDetailDock";
import { SpoolFormDialog } from "./SpoolFormDialog";
import { TareManagerDialog } from "./TareManagerDialog";
import styles from "./SpoolInventory.module.css";

const FACET_ORDER: SpoolFacetKey[] = ["loaded", "reserved", "low", "measured", "estimated"];
const FACET_LABELS: Record<SpoolFacetKey, string> = {
  loaded: "Loaded", reserved: "Reserved", low: "Low", measured: "Measured", estimated: "Estimated",
};

const LIFECYCLE_OPTIONS: SpoolFilter["lifecycle"][] = ["active", "empty", "archived", "all"];
const LIFECYCLE_LABELS: Record<SpoolFilter["lifecycle"], string> = {
  active: "Active", empty: "Empty", archived: "Archived", all: "All",
};

const ALL_MATERIALS = "__all__";
const ALL_PRINTERS = "__all__";

function locationLabel(spool: SpoolRecord, printerRecords: ResolvedPrinter[]): string {
  const location = spool.location;
  if (location.kind === "storage") {
    return location.storageLabel ? `Storage — ${location.storageLabel}` : "Storage";
  }
  const printer = printerRecords.find((p) => p.id === location.printerId);
  const slot = printer?.materialSlots.find((s) => s.id === location.slotId);
  return printer && slot ? `${printer.name} — ${slot.name}` : "Loaded";
}

/** §Components: `SpoolInventory` -- toolbar (search, facet chips, and
 *  lifecycle/Material/Printer filters), a `DataTable`, the two distinct
 *  empty states, and the deep-linked detail dock. Self-contained: it reads
 *  `spoolState`/`printers`/`navigation` directly rather than threading
 *  selection through App.tsx, the same way `PrinterDashboard` owns its own
 *  dock-mode measurement (mirrored below, same 66rem threshold). */
export function SpoolInventory() {
  const [filter, setFilter] = createSignal<SpoolFilter>(DEFAULT_SPOOL_FILTER);
  const [addOpen, setAddOpen] = createSignal(false);
  const [tareOpen, setTareOpen] = createSignal(false);
  const [dockMode, setDockMode] = createSignal<"inline" | "overlay">("overlay");
  let workspace: HTMLDivElement | undefined;

  onMount(() => {
    void loadInventory();
    if (!workspace) return;
    const rem = Number.parseFloat(window.getComputedStyle(document.documentElement).fontSize) || 16;
    const minimumInlineWidth = 66 * rem; // 44rem table plus the 22rem dock -- PrinterDashboard's rule.
    const updateMode = (width: number) => setDockMode(width >= minimumInlineWidth ? "inline" : "overlay");
    updateMode(workspace.clientWidth);
    const observer = new ResizeObserver((entries) => updateMode(entries[0]?.contentRect.width ?? workspace!.clientWidth));
    observer.observe(workspace);
    onCleanup(() => observer.disconnect());
  });

  // `applySpoolFilter` only reads `.id`/`.materialSlots` (see facets.ts's
  // `printerIdForSpool`), both present on `ResolvedPrinter` -- it's typed
  // against the wire `PrinterRecord` because that's what desktop event
  // payloads carry, so this narrows the resolved shape back to it.
  const filteredSpools = createMemo(() => applySpoolFilter(spoolState.spools, printers() as unknown as PrinterRecord[], filter()));

  const toggleFacet = (key: SpoolFacetKey) => setFilter((f) => {
    const next = new Set(f.facets);
    next.has(key) ? next.delete(key) : next.add(key);
    return { ...f, facets: next };
  });

  const materialValue = createMemo(() => {
    const materials = filter().materials;
    return materials.size === 1 ? [...materials][0] : ALL_MATERIALS;
  });
  const setMaterialValue = (v: string) => setFilter((f) => ({
    ...f, materials: v === ALL_MATERIALS ? new Set() : new Set([v as MaterialFamily]),
  }));

  const printerValue = createMemo(() => filter().printerId ?? ALL_PRINTERS);
  const setPrinterValue = (v: string) => setFilter((f) => ({ ...f, printerId: v === ALL_PRINTERS ? null : v }));

  const clearFilters = () => setFilter(DEFAULT_SPOOL_FILTER);

  const selectedSpoolId = createMemo<string | null>(() => {
    const target = navigation.target();
    return target.destination === "spools" && target.selection?.kind === "spool" ? target.selection.id : null;
  });
  const selectedSpool = createMemo(() => spoolState.spools.find((s) => s.id === selectedSpoolId()));

  function goTo(id: string | null) {
    const target: NavigationTarget = id
      ? { version: 1, destination: "spools", selection: { kind: "spool", id } }
      : { version: 1, destination: "spools" };
    navigation.navigate(target, {
      availableDestinations: ["spools"],
      availableIds: spoolState.spools.map((s) => s.id),
    });
    window.location.hash = serializeNavigationTarget(target).slice(1);
  }

  const columns: DataTableColumn<SpoolRecord>[] = [
    {
      id: "number", header: "#", width: "3rem",
      cell: (s) => `#${s.spoolNumber}`, sortValue: (s) => s.spoolNumber,
    },
    {
      id: "material", header: "Material",
      cell: (s) => materialLabel(s.materialFamily, s.materialOther),
      sortValue: (s) => materialLabel(s.materialFamily, s.materialOther),
    },
    {
      id: "color", header: "Color",
      cell: (s) => (
        <span class={styles.colorCell}>
          <ColorSwatch hex={s.colorHex ?? null} name={s.colorName} size="sm" />
          {s.colorName}
        </span>
      ),
      sortValue: (s) => s.colorName,
    },
    {
      id: "manufacturer", header: "Manufacturer and product",
      cell: (s) => `${s.manufacturer}${s.product ? ` ${s.product}` : ""}`,
      sortValue: (s) => s.manufacturer,
    },
    {
      id: "remaining", header: "Remaining", align: "end",
      cell: (s) => (
        <span>
          {formatGrams(s.availability.currentMg, 0)}
          <Show when={s.facets.confidence === "estimated"}>
            <span class={styles.est}> est.</span>
          </Show>
        </span>
      ),
      sortValue: (s) => s.availability.currentMg,
    },
    {
      id: "available", header: "Available", align: "end",
      cell: (s) => (s.availability.reservedMg > 0 ? formatGrams(s.availability.availableMg, 0) : ""),
      sortValue: (s) => s.availability.availableMg,
    },
    {
      id: "location", header: "Location",
      cell: (s) => locationLabel(s, printers()),
    },
    {
      id: "facets", header: "Facets",
      cell: (s) => (
        <span class={styles.facetChips}>
          <Show when={s.facets.loaded}><span class={styles.chip}>Loaded</span></Show>
          <Show when={s.facets.reserved}><span class={styles.chip}>Reserved</span></Show>
          <Show when={s.facets.low}><span class={styles.chip}>Low</span></Show>
        </span>
      ),
    },
  ];

  return (
    <div class={styles.inventory}>
      <Show when={spoolStoreError()}>
        {(message) => (
          <div class={styles.errorBanner} role="alert">
            <p class={styles.errorMessage}>{message()}</p>
            <Button variant="ghost" onClick={dismissSpoolStoreError}>Dismiss</Button>
          </div>
        )}
      </Show>
      <div class={styles.toolbar}>
        <TextField
          type="search"
          aria-label="Search Spools"
          placeholder="Search manufacturer, product, color, number, storage label"
          value={filter().query}
          onChange={(v) => setFilter((f) => ({ ...f, query: v }))}
        />
        <div class={styles.chips}>
          <For each={FACET_ORDER}>
            {(key) => (
              <Chip selected={filter().facets.has(key)} onSelectedChange={() => toggleFacet(key)}>
                {FACET_LABELS[key]}
              </Chip>
            )}
          </For>
        </div>
        <Select
          label="Lifecycle"
          options={LIFECYCLE_OPTIONS}
          value={filter().lifecycle}
          onChange={(v) => setFilter((f) => ({ ...f, lifecycle: v }))}
          optionLabel={(v) => LIFECYCLE_LABELS[v]}
        />
        <Select
          label="Material"
          options={[ALL_MATERIALS, ...MATERIAL_FAMILIES]}
          value={materialValue()}
          onChange={setMaterialValue}
          optionLabel={(v) => (v === ALL_MATERIALS ? "All materials" : materialFamilyLabel(v as MaterialFamily))}
        />
        <Select
          label="Printer"
          options={[ALL_PRINTERS, ...printers().map((p) => p.id)]}
          value={printerValue()}
          onChange={setPrinterValue}
          optionLabel={(v) => (v === ALL_PRINTERS ? "All printers" : printers().find((p) => p.id === v)?.name ?? v)}
        />
        <Show when={isFiltered(filter())}>
          <Button variant="ghost" onClick={clearFilters}>Clear filters</Button>
        </Show>
        <Button onClick={() => setAddOpen(true)}>Add Spool</Button>
        <DropdownMenu
          trigger={<Button variant="ghost">More…</Button>}
          items={[{ label: "Manage tares…", onSelect: () => setTareOpen(true) }]}
        />
      </div>
      <Show
        when={spoolState.loaded}
        fallback={<p class={styles.loadingNotice} role="status">Loading Spools…</p>}
      >
        <div ref={workspace} class={styles.workspace}>
          <div class={styles.content}>
            <DataTable
              label="Spools"
              rows={filteredSpools()}
              rowId={(s) => s.id}
              columns={columns}
              selectedId={selectedSpoolId()}
              onSelect={goTo}
              onActivate={goTo}
              empty={
                spoolState.spools.length === 0 ? (
                  <div class={styles.empty}>
                    <p>No Spools yet</p>
                    <Button onClick={() => setAddOpen(true)}>Add Spool</Button>
                  </div>
                ) : (
                  <div class={styles.empty}>
                    <p>No Spools match</p>
                    <Button variant="ghost" onClick={clearFilters}>Clear filters</Button>
                  </div>
                )
              }
            />
          </div>
          <SpoolDetailDock spool={selectedSpool()} mode={dockMode()} onClose={() => goTo(null)} />
        </div>
      </Show>
      <SpoolFormDialog open={addOpen()} onOpenChange={setAddOpen} />
      <TareManagerDialog open={tareOpen()} onOpenChange={setTareOpen} />
    </div>
  );
}
