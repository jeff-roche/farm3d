import { createEffect, createMemo, createResource, createSignal, Show } from "solid-js";
import { Button, Combobox, Dialog, Select, TextField } from "../design-system";
import { listCatalogModels, listCatalogVariants, previewProfile } from "../printers/printer-catalog";
import { PrinterConnectionPanel } from "./PrinterConnectionPanel";
import type {
  CatalogModelSummary,
  CatalogVariantSummary,
  PrinterDraft,
  PrinterProfile,
  ResolvedPrinter,
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
  /** Resolves to the newly created printer on success, so this dialog can
   *  move on to setting up its Connection in the same session -- or to
   *  `undefined` on failure (the store's own error banner surfaces why),
   *  which keeps the details step open rather than advancing or closing. */
  onAdd: (draft: PrinterDraft) => Promise<ResolvedPrinter | undefined>;
  /** Set by a group header's "add another" button to skip straight to a
   *  pre-selected model (still shown, not hidden — Brand/Model stay
   *  visible and editable, just already filled in). `null`/omitted for the
   *  toolbar's plain "+ Add printer" entry point, which starts blank. */
  prefillModel?: CatalogModelSummary | null;
  /** Every printer already in the Farm, for suggesting a unique default
   *  name and warning on a typed duplicate. Omitted (treated as empty) by
   *  callers that don't care, e.g. existing tests of this component. */
  existingPrinters?: ResolvedPrinter[];
}

/** Base name is unique on its own if nothing already has it; otherwise
 *  appends " 2", " 3", … until one is. Prevents three Centauri Carbons from
 *  all defaulting to the literal same name with no warning. */
function suggestUniqueName(base: string, existingNames: Set<string>): string {
  if (!existingNames.has(base)) return base;
  let n = 2;
  while (existingNames.has(`${base} ${n}`)) n++;
  return `${base} ${n}`;
}

export function PrinterAddDialog(props: PrinterAddDialogProps) {
  const [models] = createResource(listCatalogModels);
  const [vendorQuery, setVendorQuery] = createSignal("");
  const [selectedVendor, setSelectedVendor] = createSignal<string | null>(null);
  const [selectedModel, setSelectedModel] = createSignal<CatalogModelSummary | null>(null);
  const [selectedVariant, setSelectedVariant] = createSignal<CatalogVariantSummary | null>(null);
  const [name, setName] = createSignal("");
  const [nameTouched, setNameTouched] = createSignal(false);
  const [step, setStep] = createSignal<"details" | "connection">("details");
  const [createdPrinter, setCreatedPrinter] = createSignal<ResolvedPrinter | null>(null);

  function resetForm() {
    setSelectedVendor(null);
    setSelectedModel(null);
    setSelectedVariant(null);
    setVendorQuery("");
    setName("");
    setNameTouched(false);
    setStep("details");
    setCreatedPrinter(null);
  }

  // Every open (however triggered) starts from a clean slate, then seeds a
  // prefill if one was given. Every close also resets -- otherwise a typed
  // but uncommitted name/selection from a previous, cancelled open lingers
  // into the next one, prefill or not.
  //
  // Deliberately a plain reactive effect, not `on(() => props.open, ...)`:
  // seeding `selectedModel` requires an entry with a matching `.model`
  // string to already be present in `models()` -- Kobalte's Select looks
  // up its `value` against `options` (by `optionValue`) to render a label,
  // and crashes (confirmed with Playwright against `just web`, whose mock
  // catalog is permanently empty) on a value not found there. So this
  // seeds from a freshly-looked-up entry in `models()` itself, not from
  // `props.prefillModel` directly, and simply leaves Model unselected if
  // no match is ever found -- reading `models()` here means the effect
  // re-runs and finishes the seed once it loads.
  let seededThisOpen = false;
  createEffect(() => {
    if (!props.open) {
      resetForm();
      seededThisOpen = false;
      return;
    }
    if (seededThisOpen) return;
    const prefill = props.prefillModel;
    if (!prefill) {
      resetForm();
      seededThisOpen = true;
      return;
    }
    const loadedModels = models();
    if (!loadedModels) return; // wait for it to load; re-runs when it does
    const match = loadedModels.find(
      (m) => m.vendor === prefill.vendor && m.model === prefill.model,
    );
    resetForm();
    if (match) {
      setSelectedVendor(match.vendor);
      setSelectedModel(match);
      const existingNames = new Set((props.existingPrinters ?? []).map((p) => p.name));
      setName(suggestUniqueName(match.model, existingNames));
    }
    seededThisOpen = true;
  });

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
  // A non-blocking companion to nameError -- duplicate names aren't actually
  // forbidden by the backend, so this warns without disabling Add.
  const nameWarning = createMemo(() => {
    const current = name().trim();
    if (current === "") return undefined;
    const clash = (props.existingPrinters ?? []).some((p) => p.name === current);
    return clash ? "Another printer is already named this" : undefined;
  });
  const canAdd = createMemo(() => !!catalogRef() && !nameError());

  async function submit() {
    const ref = catalogRef();
    if (!ref || nameError()) return;
    const created = await props.onAdd({ name: name(), catalogRef: ref });
    // A failed add leaves the details step open (the store's own error
    // banner explains why) rather than advancing to a Connection step for
    // a printer that doesn't exist, or closing over a lost draft.
    if (!created) return;
    setCreatedPrinter(created);
    setStep("connection");
  }

  return (
    <Dialog
      title={step() === "details" ? "Add printer" : "Set up connection"}
      trigger="+ Add printer"
      open={props.open}
      onOpenChange={props.onOpenChange}
    >
      <Show
        when={step() === "details"}
        fallback={
          // createdPrinter() is only ever null while step() === "details",
          // which this branch (the fallback of a Show on that same step
          // signal) can't render during -- non-null here is guaranteed by
          // submit() setting both together, not re-checked per read.
          <div class={styles.form}>
            <p class={styles.summary}>
              {createdPrinter()!.name} was added. Set up its connection now, or skip and do it
              later from its detail panel.
            </p>
            <PrinterConnectionPanel printer={createdPrinter()!} />
          </div>
        }
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
            // Unlike nameError (suppressed until touched, so "required"
            // doesn't flash on the blank initial state), this must surface
            // even when the collision comes from an auto-filled name the
            // user never typed themselves -- that's the case this warning
            // exists for (three same-model printers would otherwise all
            // silently default to one identical name).
            description={nameWarning()}
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
      </Show>
      <div class={styles.footer}>
        <Show
          when={step() === "details"}
          fallback={
            <Button variant="primary" onClick={() => props.onOpenChange(false)}>
              Done
            </Button>
          }
        >
          <Button variant="secondary" onClick={() => props.onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!canAdd()} onClick={() => void submit()}>
            Add printer
          </Button>
        </Show>
      </div>
    </Dialog>
  );
}
