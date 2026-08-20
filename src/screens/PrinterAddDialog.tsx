import { createEffect, createMemo, createResource, createSignal, Show } from "solid-js";
import { Button, Combobox, Dialog, Select, TextField } from "../design-system";
import { listCatalogModels, listCatalogVariants, previewProfile } from "../printers/printer-catalog";
import type { CatalogModelSummary, CatalogVariantSummary, PrinterDraft } from "../printers/types";
import styles from "./PrinterAddDialog.module.css";

export interface PrinterAddDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onAdd: (draft: PrinterDraft) => void;
}

export function PrinterAddDialog(props: PrinterAddDialogProps) {
  const [models] = createResource(listCatalogModels);
  const [query, setQuery] = createSignal("");
  const [selectedModel, setSelectedModel] = createSignal<CatalogModelSummary | null>(null);
  const [selectedVariant, setSelectedVariant] = createSignal<CatalogVariantSummary | null>(null);
  const [name, setName] = createSignal("");
  const [nameTouched, setNameTouched] = createSignal(false);

  // Punctuation-insensitive: Kobalte resets the input's displayed text to the
  // selected option's full label ("Vendor · Model") immediately after a
  // selection, which round-trips through onInputChange into `query`. Without
  // normalizing away the "·" separator, that reset text no longer matches the
  // plain "vendor model" search corpus below, `filteredModels` collapses to
  // empty, and the just-selected option vanishes from Kobalte's own option
  // list before it can resolve the selection — silently discarding it.
  const normalize = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, " ").trim();

  const filteredModels = createMemo(() => {
    const q = normalize(query());
    const all = models() ?? [];
    const matching = q
      ? all.filter((m) => normalize(`${m.vendor} ${m.model}`).includes(q))
      : all;
    return matching.slice(0, 50);
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
          label="Printer model"
          options={filteredModels()}
          // Composite key: modelId alone isn't unique in the real catalog, and
          // two options sharing a key makes Kobalte treat them as one option.
          optionValue={(m: CatalogModelSummary) => `${m.vendor}::${m.model}`}
          optionLabel={(m: CatalogModelSummary) => `${m.vendor} · ${m.model}`}
          value={selectedModel() ?? undefined}
          onChange={onSelectModel}
          onInputChange={setQuery}
          placeholder={`Search ${(models() ?? []).length} models...`}
        />
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
          {(p) => {
            // Bind once per render: TS's discriminated-union narrowing on
            // `bedShape.kind` doesn't carry across separate `p()` calls, since
            // each call is an independent (if referentially stable) accessor
            // invocation as far as the type-checker's control-flow analysis
            // is concerned.
            const profile = p();
            const bedShape = profile.bedShape;
            return (
              <p class={styles.summary}>
                {bedShape.kind === "rectangular"
                  ? `${bedShape.widthMm} × ${bedShape.depthMm} × ${profile.printableHeightMm} mm`
                  : `${profile.printableHeightMm} mm tall, non-rectangular bed`}
                {" · "}
                {profile.nozzleDiameterMm.join(", ")} mm nozzle
              </p>
            );
          }}
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
