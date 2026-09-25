import { createMemo, createResource, createSignal, createUniqueId, For, onCleanup, Show, type JSX } from "solid-js";
import { Button, Dialog, Select, TextField } from "../design-system";
import type { GcodeClaim } from "../generated/contracts/domain/GcodeClaim";
import { isCommandError } from "../ipc/client";
import { listCatalogModels, listCatalogVariants } from "../printers/printer-catalog";
import { MATERIAL_FAMILIES, materialFamilyLabel } from "../spools/materials";
import {
  absentFactCount,
  claimsFor,
  emptyFactsDraft,
  FACT_CLAIM_KEYS,
  FACT_FIELDS,
  factFieldAt,
  matchCatalogModel,
  matchCatalogVariant,
  parseDiameterClaim,
  parseMaterialClaim,
  validateFacts,
  type FactErrors,
  type FactField,
  type FactsDraft,
} from "../slicing/fact-parsing";
import { FACT_LABELS, type FactKey } from "../slicing/revision-presentation";
import { createExternalSliceRevision } from "../slicing/slicing-store";
import type { MaterialFamily, SliceRevisionRecord, SliceTarget } from "../slicing/types";
import { ProvenanceBadge } from "./ProvenanceBadge";
import { SliceTargetSelect } from "./SliceTargetSelect";
import styles from "./GcodeFactsDialog.module.css";

export interface GcodeFactsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The G-code Model Source Revision the Slice Revision wraps. */
  sourceRevisionId: string;
  sourceRevisionSequence: number;
  /** The file's allowlisted claims (P4 D11), untrusted. */
  claims: readonly GcodeClaim[];
  /** Whether `claims` has been read: the file's inspection loads with the
   *  Model's revision history, and can fail. */
  claimsState: "loading" | "ready" | "failed";
  /** The revision was created; the dialog closes. */
  onCreated: (record: SliceRevisionRecord) => void;
}

/** A claim read into a fact's type, or why it can't be. */
type FileValue<T> = { value: T } | { reason: string };

const NOT_IN_FILE = "Not in the file.";
const CLAIMS_LOADING = "Reading the file…";
const CLAIMS_FAILED = "The file's claims couldn't be read.";

/** D16/P8's **Create Slice Revision…** for imported G-code. Every fact
 *  starts empty: farm3d never infers one from the file. The file's claims
 *  sit beside each fact, unverified, and **Use the file's value** copies
 *  one into the field for the operator to check and confirm. A fact left
 *  empty is recorded as absent. */
export function GcodeFactsDialog(props: GcodeFactsDialogProps) {
  // No closing (the X, Escape, or outside) mid-request: the result would
  // land on a closed dialog.
  const [busy, setBusy] = createSignal(false);
  return (
    <Dialog
      title="Create Slice Revision"
      description={`Confirm what revision ${props.sourceRevisionSequence} of this G-code is for. Leave a fact empty if you don't know it.`}
      open={props.open}
      onOpenChange={(open) => {
        if (open || !busy()) props.onOpenChange(open);
      }}
    >
      {/* Mounted only while open, so every opening starts empty. */}
      <FactsForm {...props} busy={busy()} setBusy={setBusy} />
    </Dialog>
  );
}

function FactsForm(props: GcodeFactsDialogProps & { busy: boolean; setBusy: (busy: boolean) => void }) {
  const [draft, setDraft] = createSignal<FactsDraft>(emptyFactsDraft());
  const [errors, setErrors] = createSignal<FactErrors>({});
  const [formError, setFormError] = createSignal<{ message: string; retry: boolean } | undefined>();
  const busy = () => props.busy;
  const setBusy = props.setBusy;
  let form: HTMLFormElement | undefined;
  let disposed = false;
  onCleanup(() => { disposed = true; });

  const update = <K extends keyof FactsDraft>(field: K, value: FactsDraft[K]) => {
    setDraft((current) => ({ ...current, [field]: value }));
    setErrors((current) => ({ ...current, [field]: undefined }));
    setFormError(undefined);
  };

  const focusField = (field: FactField) => {
    queueMicrotask(() => {
      const wrapper = form?.querySelector<HTMLElement>(`[data-fact-field="${field}"]`);
      wrapper?.querySelector<HTMLElement>(":is(button, input):not(:disabled):not([tabindex='-1'])")?.focus();
    });
  };

  // --- What the file says ---------------------------------------------------------
  const claimsOf = (key: FactKey) => claimsFor(props.claims, FACT_CLAIM_KEYS[key]);
  const claimValue = (key: string) => props.claims.find((claim) => claim.key === key)?.value;

  /** Why no claim can be shown yet: the file's inspection is still
   *  loading, or failed. Never "Not in the file" then. */
  const claimsUnavailable = (): string | undefined => {
    if (props.claimsState === "loading") return CLAIMS_LOADING;
    if (props.claimsState === "failed") return CLAIMS_FAILED;
    return undefined;
  };

  const diameterFromFile = (key: "nozzleDiameterMm" | "filamentDiameterMm"): FileValue<number> => {
    const unavailable = claimsUnavailable();
    if (unavailable) return { reason: unavailable };
    const claim = claimsOf(key)[0];
    if (!claim) return { reason: NOT_IN_FILE };
    const value = parseDiameterClaim(claim.value);
    return value === undefined ? { reason: "farm3d can't read this as one diameter from 0 to 5 mm." } : { value };
  };
  const materialFromFile = (): FileValue<{ family: MaterialFamily; other?: string }> => {
    const unavailable = claimsUnavailable();
    if (unavailable) return { reason: unavailable };
    const claim = claimsOf("materialFamily")[0];
    if (!claim) return { reason: NOT_IN_FILE };
    const value = parseMaterialClaim(claim.value);
    return value ? { value } : { reason: "farm3d can't read this as one material." };
  };

  // The printer claim names a catalog model; finding its profile needs the
  // catalog. Keyed on the claims it reads, so claims that arrive after the
  // dialog opened are looked up too.
  const printerClaims = createMemo(
    () => (props.claimsState === "ready"
      ? {
          model: claimValue("printer_model"),
          settingsId: claimValue("printer_settings_id"),
          nozzle: claimValue("nozzle_diameter"),
        }
      : undefined),
    undefined,
    { equals: (a, b) => JSON.stringify(a) === JSON.stringify(b) },
  );
  const [printerLookup] = createResource(
    () => {
      const claims = printerClaims();
      return claims?.model === undefined ? false : { ...claims, model: claims.model };
    },
    async (claims): Promise<FileValue<SliceTarget>> => {
      const match = matchCatalogModel(claims.model, await listCatalogModels());
      if (!match) return { reason: "This printer isn't in farm3d's printer catalog." };
      const variant = matchCatalogVariant(await listCatalogVariants(match.vendor, match.model), {
        settingsId: claims.settingsId,
        nozzleMm: claims.nozzle === undefined ? undefined : parseDiameterClaim(claims.nozzle),
      });
      if (!variant) return { reason: "The file doesn't say which nozzle this printer has." };
      return {
        value: {
          kind: "profile",
          catalogRef: {
            vendor: match.vendor,
            model: match.model,
            variant: variant.variant,
            modelId: match.modelId,
            printerVariant: variant.printerVariant,
          },
        },
      };
    },
  );
  // Read by state, never by calling a pending resource, which would
  // suspend the dialog.
  const printerFromFile = (): FileValue<SliceTarget> => {
    const unavailable = claimsUnavailable();
    if (unavailable) return { reason: unavailable };
    if (claimValue("printer_model") === undefined) return { reason: NOT_IN_FILE };
    if (printerLookup.state === "ready") return printerLookup();
    if (printerLookup.state === "errored") return { reason: "The printer catalog couldn't be loaded." };
    return { reason: "Looking the printer up in the catalog…" };
  };

  // --- Submitting (D16: idempotent by operationId) -------------------------------
  // One operation id per attempt. A retry of the same facts after the
  // backend couldn't be reached reuses it, so a create that did land isn't
  // made twice; any change to the facts, or a refusal, starts a new one.
  let attempt: { operationId: string; key: string } | undefined;

  const submit = async () => {
    if (busy()) return;
    setFormError(undefined);
    const result = validateFacts(draft());
    if (!result.ok) {
      setErrors(result.errors);
      const first = FACT_FIELDS.find((field) => result.errors[field]);
      if (first) focusField(first);
      return;
    }
    setErrors({});
    const key = JSON.stringify(result.facts);
    if (attempt?.key !== key) attempt = { operationId: crypto.randomUUID(), key };
    const { operationId } = attempt;
    setBusy(true);
    try {
      const record = await createExternalSliceRevision(props.sourceRevisionId, result.facts, operationId);
      attempt = undefined;
      if (!disposed) props.onCreated(record);
    } catch (error) {
      if (disposed) return;
      if (!isCommandError(error)) {
        setFormError({ message: "farm3d couldn't reach its backend. Your answers are kept.", retry: true });
        return;
      }
      attempt = undefined;
      const field = error.code === "VALIDATION" ? factFieldAt(error.details?.fieldPath) : undefined;
      if (field) {
        setErrors({ [field]: error.message });
        focusField(field);
      } else {
        setFormError({ message: error.message, retry: false });
      }
    } finally {
      if (!disposed) setBusy(false);
    }
  };

  const absent = () => absentFactCount(draft());

  return (
    <form
      ref={form}
      class={styles.form}
      noValidate
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <div class={styles.columns} aria-hidden="true">
        <span>What the file says (not verified)</span>
        <span>Your value</span>
      </div>

      <FactRow
        label={FACT_LABELS.printerProfile}
        claims={claimsOf("printerProfile")}
        fileValue={printerFromFile()}
        filled={draft().printerProfile !== null}
        onUseFile={(target) => {
          update("printerProfile", target);
          focusField("printerProfile");
        }}
        onClear={() => {
          update("printerProfile", null);
          focusField("printerProfile");
        }}
      >
        <div data-fact-field="printerProfile">
          <SliceTargetSelect
            label="Printer profile"
            value={draft().printerProfile}
            placeholder="Not provided"
            error={errors().printerProfile}
            onChange={(target) => update("printerProfile", target)}
          />
        </div>
        <p class={styles.hint}>Choosing a Printer copies its profile as it is now.</p>
      </FactRow>

      <FactRow
        label={FACT_LABELS.nozzleDiameterMm}
        claims={claimsOf("nozzleDiameterMm")}
        fileValue={diameterFromFile("nozzleDiameterMm")}
        filled={draft().nozzleDiameterMm.trim() !== ""}
        onUseFile={(value) => {
          update("nozzleDiameterMm", String(value));
          focusField("nozzleDiameterMm");
        }}
        onClear={() => {
          update("nozzleDiameterMm", "");
          focusField("nozzleDiameterMm");
        }}
      >
        <div data-fact-field="nozzleDiameterMm">
          <TextField
            label="Nozzle diameter (mm)"
            placeholder="Not provided"
            value={draft().nozzleDiameterMm}
            error={errors().nozzleDiameterMm}
            onChange={(value) => update("nozzleDiameterMm", value)}
          />
        </div>
      </FactRow>

      <FactRow
        label={FACT_LABELS.materialFamily}
        claims={claimsOf("materialFamily")}
        fileValue={materialFromFile()}
        filled={draft().materialFamily !== null}
        onUseFile={(value) => {
          setDraft((current) => ({ ...current, materialFamily: value.family, materialOther: value.other ?? "" }));
          setErrors((current) => ({ ...current, materialFamily: undefined, materialOther: undefined }));
          setFormError(undefined);
          focusField(value.family === "OTHER" ? "materialOther" : "materialFamily");
        }}
        onClear={() => {
          setDraft((current) => ({ ...current, materialFamily: null, materialOther: "" }));
          setErrors((current) => ({ ...current, materialFamily: undefined, materialOther: undefined }));
          focusField("materialFamily");
        }}
      >
        <div data-fact-field="materialFamily">
          <Select<MaterialFamily>
            label="Material"
            options={MATERIAL_FAMILIES}
            optionLabel={materialFamilyLabel}
            value={draft().materialFamily}
            placeholder="Not provided"
            error={errors().materialFamily}
            onChange={(family) => update("materialFamily", family)}
          />
        </div>
        <Show when={draft().materialFamily === "OTHER"}>
          <div data-fact-field="materialOther">
            <TextField
              label="Material name"
              value={draft().materialOther}
              error={errors().materialOther}
              onChange={(value) => update("materialOther", value)}
            />
          </div>
        </Show>
      </FactRow>

      <FactRow
        label={FACT_LABELS.filamentDiameterMm}
        claims={claimsOf("filamentDiameterMm")}
        fileValue={diameterFromFile("filamentDiameterMm")}
        filled={draft().filamentDiameterMm.trim() !== ""}
        onUseFile={(value) => {
          update("filamentDiameterMm", String(value));
          focusField("filamentDiameterMm");
        }}
        onClear={() => {
          update("filamentDiameterMm", "");
          focusField("filamentDiameterMm");
        }}
      >
        <div data-fact-field="filamentDiameterMm">
          <TextField
            label="Filament diameter (mm)"
            placeholder="Not provided"
            value={draft().filamentDiameterMm}
            error={errors().filamentDiameterMm}
            onChange={(value) => update("filamentDiameterMm", value)}
          />
        </div>
      </FactRow>

      <p class={styles.summary} aria-live="polite">
        {absent() === 0
          ? "Every fact is confirmed."
          : `${absent()} ${absent() === 1 ? "fact" : "facts"} not provided: this Slice Revision will need a Printer chosen by hand when it is queued.`}
      </p>

      <Show when={formError()}>
        {(failure) => (
          <div class={styles.error} role="alert">
            <p>{failure().message}</p>
            <Show when={failure().retry}>
              <Button type="button" variant="secondary" size="sm" disabled={busy()} onClick={() => void submit()}>
                Try again
              </Button>
            </Show>
          </div>
        )}
      </Show>

      <div class={styles.actions}>
        <Button type="button" variant="ghost" disabled={busy()} onClick={() => props.onOpenChange(false)}>Cancel</Button>
        <Button type="submit" variant="primary" disabled={busy()}>
          {busy() ? "Creating…" : "Create Slice Revision"}
        </Button>
      </div>
    </form>
  );
}

interface FactRowProps<T> {
  label: string;
  claims: GcodeClaim[];
  fileValue: FileValue<T>;
  /** The operator has entered a value. */
  filled: boolean;
  onUseFile: (value: T) => void;
  onClear: () => void;
  children: JSX.Element;
}

/** One fact: the file's claim on the left, the operator's value on the
 *  right, and where the value will come from. */
function FactRow<T>(props: FactRowProps<T>) {
  const labelId = createUniqueId();
  const claimId = createUniqueId();
  const usable = () => ("value" in props.fileValue ? props.fileValue : undefined);
  const reason = () => ("reason" in props.fileValue ? props.fileValue.reason : undefined);

  return (
    <div class={styles.row} role="group" aria-labelledby={labelId}>
      <div class={styles.rowHeader}>
        <span id={labelId} class={styles.factName}>{props.label}</span>
        <ProvenanceBadge provenance={props.filled ? "operatorConfirmed" : "absent"} />
      </div>
      <div class={styles.claim}>
        <div id={claimId}>
          <span class={styles.srOnly}>What the file says (not verified): </span>
          <Show when={props.claims.length > 0} fallback={<span class={styles.muted}>{reason() ?? NOT_IN_FILE}</span>}>
            <For each={props.claims}>
              {(claim) => (
                <span class={styles.claimValue}>
                  <code>{claim.key}</code> {claim.value}{" "}
                  <span class={styles.muted}>(line {claim.line.toLocaleString()})</span>
                </span>
              )}
            </For>
            <Show when={reason()}>{(text) => <span class={styles.muted}>{text()}</span>}</Show>
          </Show>
        </div>
        <Button
          type="button"
          variant="secondary"
          size="sm"
          disabled={!usable()}
          aria-label={`Use the file's value for ${props.label}`}
          aria-describedby={claimId}
          onClick={() => {
            const held = usable();
            if (held) props.onUseFile(held.value);
          }}
        >
          Use the file's value
        </Button>
      </div>
      <div class={styles.confirm}>
        {props.children}
        <Button
          type="button"
          variant="ghost"
          size="sm"
          class={styles.clear}
          disabled={!props.filled}
          aria-label={`Clear ${props.label}`}
          onClick={() => props.onClear()}
        >
          Clear
        </Button>
      </div>
    </div>
  );
}
