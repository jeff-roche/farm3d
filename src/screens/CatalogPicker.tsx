import { createEffect, createMemo, createResource, createSignal, Show } from "solid-js";
import { Combobox, Select } from "../design-system";
import { listCatalogModels, listCatalogVariants, previewProfile } from "../printers/printer-catalog";
import { pickDefaultVariant, stripBrandPrefix } from "../printers/printer-identity";
import type { CatalogModelSummary, CatalogRef, CatalogVariantSummary } from "../printers/types";

/** Brand → Model → Nozzle catalog selection shared by `PrinterSetupWizard`'s
 *  Identify step and `PrinterBatchDialog`'s Shared step: catalog loading,
 *  the sole/0.4 mm variant default, the resulting `CatalogRef`, and the
 *  profile preview for it. Must be called inside a component (it creates
 *  resources and effects owned by the caller). */
export function createCatalogPicker() {
  const [models] = createResource(listCatalogModels);
  const [vendorQuery, setVendorQuery] = createSignal("");
  const [selectedVendor, setSelectedVendor] = createSignal<string | null>(null);
  const [selectedModel, setSelectedModel] = createSignal<CatalogModelSummary | null>(null);
  const [selectedVariant, setSelectedVariant] = createSignal<CatalogVariantSummary | null>(null);

  const vendors = createMemo(() => {
    const seen = new Set<string>();
    for (const m of models() ?? []) seen.add(m.vendor);
    return [...seen].sort((a, b) => a.localeCompare(b));
  });

  const filteredVendors = createMemo(() => {
    const q = vendorQuery().trim().toLowerCase();
    const all = vendors();
    return q ? all.filter((v) => v.toLowerCase().includes(q)) : all;
  });

  const modelsForVendor = createMemo(() => {
    const vendor = selectedVendor();
    return vendor ? (models() ?? []).filter((m) => m.vendor === vendor) : [];
  });

  const [variants] = createResource(selectedModel, (model) =>
    model ? listCatalogVariants(model.vendor, model.model) : Promise.resolve([]),
  );

  // Auto-selects the sole variant, or defaults to 0.4mm, without clobbering
  // a manual choice the user already made.
  createEffect(() => {
    const list = variants();
    if (!list || list.length === 0 || selectedVariant()) return;
    setSelectedVariant(pickDefaultVariant(list) ?? null);
  });

  const catalogRef = createMemo((): CatalogRef | null => {
    const model = selectedModel();
    const variant = selectedVariant();
    return model && variant
      ? {
          vendor: model.vendor,
          model: model.model,
          variant: variant.variant,
          modelId: model.modelId,
          printerVariant: variant.printerVariant,
        }
      : null;
  });

  const [preview] = createResource(catalogRef, (ref) => (ref ? previewProfile(ref) : Promise.resolve(null)));

  function selectVendor(vendor: string) {
    setSelectedVendor(vendor);
    setSelectedModel(null);
    setSelectedVariant(null);
  }

  function selectModel(model: CatalogModelSummary) {
    setSelectedModel(model);
    setSelectedVariant(null);
  }

  /** Pre-selects a model (and its vendor) without going through the UI. */
  function seed(model: CatalogModelSummary) {
    setSelectedVendor(model.vendor);
    setSelectedModel(model);
  }

  function reset() {
    setSelectedVendor(null);
    setSelectedModel(null);
    setSelectedVariant(null);
    setVendorQuery("");
  }

  return {
    models,
    vendors,
    filteredVendors,
    setVendorQuery,
    selectedVendor,
    modelsForVendor,
    selectedModel,
    variants,
    selectedVariant,
    setSelectedVariant,
    catalogRef,
    preview,
    selectVendor,
    selectModel,
    seed,
    reset,
  };
}

export type CatalogPicker = ReturnType<typeof createCatalogPicker>;

export interface CatalogPickerFieldsProps {
  picker: CatalogPicker;
  /** Called after a model is picked through the Model Select. */
  onModelSelected?: (model: CatalogModelSummary) => void;
}

/** The Brand Combobox, then Model and Nozzle Selects as each becomes
 *  choosable. */
export function CatalogPickerFields(props: CatalogPickerFieldsProps) {
  const picker = () => props.picker;
  return (
    <>
      <Combobox
        label="Brand"
        options={picker().filteredVendors()}
        value={picker().selectedVendor() ?? undefined}
        onChange={(vendor) => picker().selectVendor(vendor)}
        onInputChange={(query) => picker().setVendorQuery(query)}
        placeholder={`Search ${picker().vendors().length} brands...`}
      />
      <Show when={picker().selectedVendor()}>
        <Select
          label="Model"
          options={picker().modelsForVendor()}
          optionValue={(m: CatalogModelSummary) => m.model}
          optionLabel={(m: CatalogModelSummary) => stripBrandPrefix(m.model, m.vendor)}
          value={picker().selectedModel() ?? undefined}
          onChange={(model: CatalogModelSummary) => {
            picker().selectModel(model);
            props.onModelSelected?.(model);
          }}
        />
      </Show>
      <Show when={picker().selectedModel()}>
        <Select
          label="Nozzle"
          options={picker().variants() ?? []}
          optionValue={(v: CatalogVariantSummary) => v.variant}
          optionLabel={(v: CatalogVariantSummary) => `${v.printerVariant} mm`}
          value={picker().selectedVariant() ?? undefined}
          onChange={(variant: CatalogVariantSummary) => picker().setSelectedVariant(variant)}
        />
      </Show>
    </>
  );
}
