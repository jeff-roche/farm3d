import { createEffect, createMemo, createResource, createSignal, Show } from "solid-js";
import { Button, Combobox, Dialog, Select, TextField } from "../design-system";
import { listCatalogModels, listCatalogVariants, previewProfile } from "../printers/printer-catalog";
import type {
  CatalogModelSummary,
  CatalogVariantSummary,
  PrinterDraft,
  PrinterProfile,
} from "../printers/types";
import styles from "./PrinterAddDialog.module.css";

/** If the model's own name already starts with its vendor's, drop that
 *  prefix in the Model dropdown — the Brand dropdown already said it. Falls
 *  back to the full name when the model doesn't literally start with the
 *  vendor string (true for ~80% of the catalog, e.g. not for Bambu Lab,
 *  whose vendor code is "BBL") or when stripping would leave nothing. */
function stripBrandPrefix(model: string, vendor: string): string {
  if (!model.toLowerCase().startsWith(vendor.toLowerCase())) return model;
  const rest = model.slice(vendor.length).trimStart();
  return rest || model;
}

/** `bedShape`'s discriminated-union narrowing only holds within a single
 *  function body — it doesn't carry across separate calls to the same
 *  accessor. Narrowing here, on a plain argument, keeps that local so the
 *  call site can still call the live `p()` accessor directly inside JSX
 *  (see the render-prop below) rather than freezing its value in a `const`,
 *  which would stop the preview from updating on a later nozzle change. */
function formatBedSummary(profile: PrinterProfile): string {
  const { bedShape } = profile;
  return bedShape.kind === "rectangular"
    ? `${bedShape.widthMm} × ${bedShape.depthMm} × ${profile.printableHeightMm} mm`
    : `${profile.printableHeightMm} mm tall, non-rectangular bed`;
}

export interface PrinterAddDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onAdd: (draft: PrinterDraft) => void;
}

export function PrinterAddDialog(props: PrinterAddDialogProps) {
  const [models] = createResource(listCatalogModels);
  const [vendorQuery, setVendorQuery] = createSignal("");
  const [selectedVendor, setSelectedVendor] = createSignal<string | null>(null);
  const [selectedModel, setSelectedModel] = createSignal<CatalogModelSummary | null>(null);
  const [selectedVariant, setSelectedVariant] = createSignal<CatalogVariantSummary | null>(null);
  const [name, setName] = createSignal("");
  const [nameTouched, setNameTouched] = createSignal(false);

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

  // Auto-select the sole variant, or default to 0.4mm, without clobbering a
  // manual choice the user already made.
  createEffect(() => {
    const list = variants();
    if (!list || list.length === 0 || selectedVariant()) return;
    setSelectedVariant(list.find((v) => v.printerVariant === "0.4") ?? list[0]);
  });

  const catalogRef = createMemo(() => {
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

  function onSelectVendor(vendor: string) {
    setSelectedVendor(vendor);
    setSelectedModel(null);
    setSelectedVariant(null);
  }

  function onSelectModel(model: CatalogModelSummary) {
    setSelectedModel(model);
    setSelectedVariant(null);
    if (!nameTouched()) setName(model.model);
  }

  const nameError = createMemo(() => (name().trim() === "" ? "Name is required" : undefined));
  const canAdd = createMemo(() => !!catalogRef() && !nameError());

  function submit() {
    const ref = catalogRef();
    if (!ref || nameError()) return;
    props.onAdd({ name: name(), catalogRef: ref });
    props.onOpenChange(false);
  }

  return (
    <Dialog
      title="Add printer"
      trigger="+ Add printer"
      open={props.open}
      onOpenChange={props.onOpenChange}
    >
      <div class={styles.form}>
        <Combobox
          label="Brand"
          options={filteredVendors()}
          value={selectedVendor() ?? undefined}
          onChange={onSelectVendor}
          onInputChange={setVendorQuery}
          placeholder={`Search ${vendors().length} brands...`}
        />
        <Show when={selectedVendor()}>
          <Select
            label="Model"
            options={modelsForVendor()}
            optionValue={(m: CatalogModelSummary) => m.model}
            optionLabel={(m: CatalogModelSummary) => stripBrandPrefix(m.model, m.vendor)}
            value={selectedModel() ?? undefined}
            onChange={onSelectModel}
          />
        </Show>
        <Show when={selectedModel()}>
          <Select
            label="Nozzle"
            options={variants() ?? []}
            optionValue={(v: CatalogVariantSummary) => v.variant}
            optionLabel={(v: CatalogVariantSummary) => `${v.printerVariant} mm`}
            value={selectedVariant() ?? undefined}
            onChange={setSelectedVariant}
          />
        </Show>
        <TextField
          label="Name"
          value={name()}
          onChange={(v) => {
            setName(v);
            setNameTouched(true);
          }}
          error={nameTouched() ? nameError() : undefined}
        />
        <Show when={preview()}>
          {(p) => (
            // Calling p() inside each JSX expression (not hoisted into a
            // `const` above) is required, not stylistic: PrinterDashboard's
            // pattern of a non-keyed <Show> applies here too — this
            // render-prop runs once for the whole time preview() stays
            // truthy, so only expressions that call the live accessor
            // directly stay reactive to a later nozzle/model change.
            <p class={styles.summary}>
              {formatBedSummary(p())}
              {" · "}
              {p().nozzleDiameterMm.join(", ")} mm nozzle
            </p>
          )}
        </Show>
      </div>
      <div class={styles.footer}>
        <Button variant="secondary" onClick={() => props.onOpenChange(false)}>
          Cancel
        </Button>
        <Button variant="primary" disabled={!canAdd()} onClick={submit}>
          Add printer
        </Button>
      </div>
    </Dialog>
  );
}
