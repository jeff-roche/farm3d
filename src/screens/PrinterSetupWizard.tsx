import { createEffect, createMemo, createResource, createSignal, For, on, Show } from "solid-js";
import { Button, Dialog, RadioGroup, Select, Stepper, TextField } from "../design-system";
import { suggestUniqueName } from "../printers/printer-identity";
import { createPrinter, credentialStoreInfo, probeCandidate } from "../printers/printer-store";
import type {
  CatalogModelSummary,
  PrinterProfile,
  ProbeResult,
  ResolvedPrinter,
  StartSafety,
} from "../printers/types";
import { bedTypeLabel, bedTypeOptionsFor } from "./PrinterProfilePanel";
import {
  buildMismatches,
  ConnectionFields,
  connectionDraftChanged,
  toSubmission,
  type ConnectionDraft,
} from "./ConnectionFields";
import { CatalogPickerFields, createCatalogPicker } from "./CatalogPicker";
import styles from "./PrinterSetupWizard.module.css";

const STEP_ORDER = ["identify", "connect", "operate", "review"] as const;
type StepId = (typeof STEP_ORDER)[number];
const STEP_LABELS: Record<StepId, string> = {
  identify: "Identify",
  connect: "Connect",
  operate: "Operate",
  review: "Review",
};

export const START_SAFETY_OPTIONS = [
  { value: "confirmBedClear", label: "Confirm the bed is clear before each start" },
  { value: "unattended", label: "Allow unattended starts" },
];

const DEFAULT_CONNECTION_DRAFT: ConnectionDraft = {
  kind: "moonraker",
  host: "",
  port: 7125,
  useTls: false,
  credential: "",
};

/** `bedShape`'s discriminated-union narrowing only holds within a single
 *  function body — it doesn't carry across separate calls to the same
 *  accessor. Narrowing here, on a plain argument, keeps that local so the
 *  call site can still call the live `p()` accessor directly inside JSX
 *  rather than freezing its value in a `const`, which would stop the
 *  preview from updating on a later nozzle change. */
function formatBedSummary(profile: PrinterProfile): string {
  const { bedShape } = profile;
  return bedShape.kind === "rectangular"
    ? `${bedShape.widthMm} × ${bedShape.depthMm} × ${profile.printableHeightMm} mm`
    : `${profile.printableHeightMm} mm tall, non-rectangular bed`;
}

export interface PrinterSetupWizardProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Set by a group header's "add another" button to skip straight to a
   *  pre-selected model. Omitted for the toolbar's plain "+ Add printer"
   *  entry point, which starts blank. */
  prefillModel?: { vendor: string; model: string };
  /** Every printer already in the Farm, for suggesting a unique default
   *  name and warning on a typed duplicate. */
  existingPrinters: ResolvedPrinter[];
  /** Resolves with the newly created printer on a successful Save. */
  onCreated?: (printer: ResolvedPrinter) => void;
}

/** Single Printer setup: Identify, Connect, Operate, Review (spec §Frontend
 *  architecture → Components). Replaces `PrinterAddDialog`; only Save (on
 *  Review) persists anything — every earlier step is free to revisit or
 *  abandon. */
export function PrinterSetupWizard(props: PrinterSetupWizardProps) {
  const picker = createCatalogPicker();
  const { models, catalogRef, preview } = picker;
  const [name, setName] = createSignal("");
  const [nameTouched, setNameTouched] = createSignal(false);
  const [location, setLocation] = createSignal("");

  const [step, setStep] = createSignal<StepId>("identify");

  const [connectionDraft, setConnectionDraft] = createSignal<ConnectionDraft>(DEFAULT_CONNECTION_DRAFT);
  const [lastProbe, setLastProbe] = createSignal<ProbeResult | null>(null);

  // Mirrors `ConnectionFields`' own probe invalidation (shared
  // `connectionDraftChanged` rule): Review's mismatch list reads
  // `lastProbe`, which must not keep describing a submission the user has
  // since materially edited (e.g. tested host A, then edited to host B
  // before reaching Review). `previous === undefined` skips the run `on()`
  // always does at mount.
  createEffect(
    on(connectionDraft, (current, previous) => {
      if (previous === undefined || !connectionDraftChanged(previous, current)) return;
      setLastProbe(null);
    }),
  );

  const [startSafety, setStartSafety] = createSignal<StartSafety>("confirmBedClear");
  const [bedType, setBedType] = createSignal("");
  const [bedTypeTouched, setBedTypeTouched] = createSignal(false);

  const [saving, setSaving] = createSignal(false);

  const [store] = createResource(credentialStoreInfo);

  const existingNames = () => props.existingPrinters.map((printer) => printer.name);

  function resetForm() {
    picker.reset();
    setName("");
    setNameTouched(false);
    setLocation("");
    setStep("identify");
    setConnectionDraft(DEFAULT_CONNECTION_DRAFT);
    setLastProbe(null);
    setStartSafety("confirmBedClear");
    setBedType("");
    setBedTypeTouched(false);
  }

  // Every open (however triggered) starts from a clean slate, then seeds a
  // prefill if one was given. Every close also resets -- otherwise a typed
  // but uncommitted name/selection from a previous, cancelled open lingers
  // into the next one, prefill or not. See `PrinterAddDialog`'s identical
  // reasoning for why this seeds from a freshly-looked-up `models()` entry
  // rather than `props.prefillModel` directly.
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
    const match = loadedModels.find((m) => m.vendor === prefill.vendor && m.model === prefill.model);
    resetForm();
    if (match) {
      picker.seed(match);
      setName(suggestUniqueName(match.model, new Set(existingNames())));
    }
    seededThisOpen = true;
  });

  // Seeds the bed-type default from the variant's own catalog value (spec
  // D9) until the user picks one themselves, and re-seeds on a later
  // model/nozzle change -- mirrors `ConnectionFields`' `suggestedKind`.
  createEffect(() => {
    const p = preview();
    if (!p || bedTypeTouched()) return;
    setBedType(p.defaultBedType);
  });

  function onSelectModel(model: CatalogModelSummary) {
    if (!nameTouched()) setName(model.model);
  }

  const nameError = createMemo(() => (name().trim() === "" ? "Name is required" : undefined));
  const nameWarning = createMemo(() => {
    const current = name().trim();
    if (current === "") return undefined;
    return existingNames().includes(current) ? "Another printer is already named this" : undefined;
  });
  const canLeaveIdentify = createMemo(() => !!catalogRef() && !nameError());

  const hasConnection = createMemo(() => connectionDraft().host.trim() !== "");

  const mismatches = createMemo(() => {
    const p = preview();
    const result = lastProbe();
    return p && result ? buildMismatches(p, result.reported) : [];
  });

  function goNext() {
    const idx = STEP_ORDER.indexOf(step());
    if (idx < STEP_ORDER.length - 1) setStep(STEP_ORDER[idx + 1]);
  }

  function goBack() {
    const idx = STEP_ORDER.indexOf(step());
    if (idx > 0) setStep(STEP_ORDER[idx - 1]);
  }

  function skipConnection() {
    setConnectionDraft(DEFAULT_CONNECTION_DRAFT);
    setLastProbe(null);
    goNext();
  }

  async function handleTest(): Promise<ProbeResult> {
    try {
      const result = await probeCandidate(toSubmission(connectionDraft(), "create"));
      setLastProbe(result);
      return result;
    } catch (e) {
      setLastProbe(null);
      throw e;
    }
  }

  async function handleSave() {
    const ref = catalogRef();
    if (!ref) return;
    setSaving(true);
    try {
      const created = await createPrinter({
        name: name().trim(),
        catalogRef: ref,
        location: location().trim() || undefined,
        startSafety: startSafety(),
        defaultBedType: bedType() !== (preview()?.defaultBedType ?? "") ? bedType() : undefined,
        connection: hasConnection() ? toSubmission(connectionDraft(), "create") : undefined,
      });
      // A failed save leaves Review open (the store's own error banner
      // explains why) rather than closing over a lost draft.
      if (!created) return;
      props.onCreated?.(created);
      props.onOpenChange(false);
    } finally {
      setSaving(false);
    }
  }

  const stepperSteps = createMemo(() => {
    const currentIndex = STEP_ORDER.indexOf(step());
    return STEP_ORDER.map((id, index) => ({
      id,
      label: STEP_LABELS[id],
      state: index < currentIndex ? ("complete" as const) : undefined,
    }));
  });

  function onStepKeyDown(e: KeyboardEvent) {
    if (e.key !== "Enter" || e.defaultPrevented) return;
    const target = e.target as HTMLElement;
    if (target.tagName === "TEXTAREA") return;
    if (step() === "identify" && !canLeaveIdentify()) return;
    if (step() === "review") return; // Save is an explicit click, not an Enter side effect.
    e.preventDefault();
    goNext();
  }

  return (
    <Dialog
      title="Add printer"
      open={props.open}
      onOpenChange={props.onOpenChange}
    >
      <div class={styles.body} onKeyDown={onStepKeyDown}>
        <Stepper steps={stepperSteps()} current={step()} onSelect={(id) => setStep(id as StepId)} aria-label="Printer setup" />

        <Show when={step() === "identify"}>
          <div class={styles.form}>
            <CatalogPickerFields picker={picker} onModelSelected={onSelectModel} />
            <TextField
              label="Name"
              value={name()}
              onChange={(v) => {
                setName(v);
                setNameTouched(true);
              }}
              error={nameTouched() ? nameError() : undefined}
              description={nameWarning()}
            />
            <TextField label="Location" value={location()} onChange={setLocation} placeholder="Bay 1" />
            <Show when={preview()}>
              {(p) => (
                <div class={styles.summary}>
                  <p>Build volume: {formatBedSummary(p())}</p>
                  <p>Nozzle: {p().nozzleDiameterMm.join(", ")} mm</p>
                </div>
              )}
            </Show>
          </div>
        </Show>

        <Show when={step() === "connect"}>
          <div class={styles.form}>
            <ConnectionFields
              value={connectionDraft()}
              onChange={setConnectionDraft}
              suggestedKind={preview()?.suggestedHostType ?? undefined}
              onTest={handleTest}
              profile={preview() ?? undefined}
            />
          </div>
        </Show>

        <Show when={step() === "operate"}>
          <div class={styles.form}>
            <RadioGroup
              label="Start safety"
              options={START_SAFETY_OPTIONS}
              value={startSafety()}
              onChange={(v) => setStartSafety(v as StartSafety)}
            />
            <Select
              label="Bed type"
              options={bedTypeOptionsFor(bedType())}
              optionLabel={bedTypeLabel}
              value={bedType()}
              // Kobalte treats "" as no selection, so the catalog's blank bed type
              // needs the placeholder to read "Default" (as in PrinterProfilePanel).
              placeholder={bedTypeLabel("")}
              onChange={(v) => {
                setBedTypeTouched(true);
                setBedType(v);
              }}
            />
          </div>
        </Show>

        <Show when={step() === "review"}>
          <div class={styles.form}>
            <div class={styles.summary}>
              <p>{name().trim()}</p>
              <Show when={location().trim()}>{(loc) => <p>Location: {loc()}</p>}</Show>
              <Show when={preview()}>
                {(p) => (
                  <>
                    <p>Build volume: {formatBedSummary(p())}</p>
                    <p>Nozzle: {p().nozzleDiameterMm.join(", ")} mm</p>
                  </>
                )}
              </Show>
              <p>Bed type: {bedType() || "(catalog default)"}</p>
              <p>
                Start safety:{" "}
                {START_SAFETY_OPTIONS.find((o) => o.value === startSafety())?.label ?? startSafety()}
              </p>
            </div>

            <Show
              when={hasConnection()}
              fallback={<p class={styles.notice}>This Printer will be Setup incomplete until it has a Connection</p>}
            >
              <p class={styles.note}>
                Connection: {connectionDraft().kind} — {connectionDraft().host}:{connectionDraft().port}
              </p>
            </Show>

            <Show when={mismatches().length > 0}>
              <ul class={styles.mismatches}>
                <For each={mismatches()}>{(text) => <li>{text}</li>}</For>
              </ul>
            </Show>

            <Show when={store()}>
              {(info) => (
                <p class={styles.note}>
                  Credentials stored in: {info().kind === "keychain" ? "OS keychain" : "credentials.json"}
                </p>
              )}
            </Show>
          </div>
        </Show>

        <div class={styles.footer}>
          <Show when={step() !== "identify"}>
            <Button variant="secondary" onClick={goBack}>
              Back
            </Button>
          </Show>
          <div class={styles.footerSpacer} />
          <Show when={step() === "identify"}>
            <Button variant="secondary" onClick={() => props.onOpenChange(false)}>
              Cancel
            </Button>
          </Show>
          <Show when={step() === "connect"}>
            <Button variant="ghost" onClick={skipConnection}>
              Skip — save Profile-only
            </Button>
          </Show>
          <Show
            when={step() === "review"}
            fallback={
              <Button
                variant="primary"
                disabled={step() === "identify" && !canLeaveIdentify()}
                onClick={goNext}
              >
                Next →
              </Button>
            }
          >
            <Button variant="primary" disabled={saving()} onClick={() => void handleSave()}>
              {saving() ? "Saving…" : "Save"}
            </Button>
          </Show>
        </div>
      </div>
    </Dialog>
  );
}
