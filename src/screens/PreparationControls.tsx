import { Collapsible } from "@kobalte/core/collapsible";
import { IconChevronRight } from "@tabler/icons-solidjs";
import { createEffect, createMemo, createSignal, on, Show, type JSX } from "solid-js";
import { PrinterRoster, Select, type SelectGroup } from "../design-system";
import { printers } from "../printers/printer-store";
import {
  CONTROL_LIMITS,
  clampControl,
  layerHeightMax,
  type NumericControl,
  type PanelField,
} from "../slicing/slice-presentation";
import { operationalLabel } from "../monitor/monitor-store";
import type {
  BrimType,
  CatalogRef,
  FilamentPresetOption,
  InfillPattern,
  PreparationDocument,
  SliceControls,
  SliceTarget,
  SupportMode,
} from "../slicing/types";
import { CommitNumberField } from "./CommitNumberField";
import { TargetProfileDialog } from "./TargetProfileDialog";
import type { PreparationSession } from "./preparation-session";
import styles from "./PreparationPanel.module.css";

/** The controls folded under **Advanced** (D20: Strength and Support stay
 *  compact). A link to one of these opens the section first. */
export const ADVANCED_FIELDS: ReadonlySet<PanelField> = new Set<PanelField>([
  "topShellLayers",
  "bottomShellLayers",
  "brimWidthMm",
  "skirtLoops",
]);


/** Shown in an unset control: it takes the quality preset's value (D4). */
const PRESET_PLACEHOLDER = "Preset";

interface Choice<T> {
  value: T | undefined;
  label: string;
}

const fromPreset = <T,>(): Choice<T> => ({ value: undefined, label: "Preset's choice" });

const INFILL_PATTERNS: Choice<InfillPattern>[] = [
  fromPreset(),
  { value: "rectilinear", label: "Rectilinear" },
  { value: "grid", label: "Grid" },
  { value: "line", label: "Line" },
  { value: "cubic", label: "Cubic" },
  { value: "gyroid", label: "Gyroid" },
  { value: "honeycomb", label: "Honeycomb" },
  { value: "lightning", label: "Lightning" },
];

const SUPPORT_MODES: Choice<SupportMode>[] = [
  fromPreset(),
  { value: "off", label: "Off" },
  { value: "normal(auto)", label: "Normal (auto)" },
  { value: "tree(auto)", label: "Tree (auto)" },
];

const BRIM_TYPES: Choice<BrimType>[] = [
  fromPreset(),
  { value: "no_brim", label: "No brim" },
  { value: "outer_only", label: "Outer only" },
  { value: "auto_brim", label: "Auto" },
];

interface TargetEntry {
  key: string;
  label: string;
  target: SliceTarget;
}

/** The entry that opens the catalog, rather than being a target itself. */
const OTHER_PROFILE = "other-profile";

function profileKey(ref: CatalogRef): string {
  return `profile:${ref.vendor}|${ref.model}|${ref.variant}`;
}

function targetKey(target: SliceTarget): string {
  return target.kind === "printer" ? `printer:${target.printerId}` : profileKey(target.catalogRef);
}

function filamentLabel(option: FilamentPresetOption): string {
  const family = option.materialFamily ?? option.filamentType;
  return family ? `${option.name} · ${family}` : option.name;
}

export interface PreparationControlsProps {
  session: PreparationSession;
  document: PreparationDocument;
  advancedOpen: boolean;
  onAdvancedOpenChange: (open: boolean) => void;
}

/** D20's sections: Target (with the matching Printer count), Material,
 *  Quality, Strength, Support, and a folded Advanced. Every change is a
 *  Preparation edit, saved through the session's editor. */
export function PreparationControls(props: PreparationControlsProps) {
  const session = props.session;
  const controls = () => props.document.controls;

  const edit = (change: (document: PreparationDocument) => PreparationDocument) => session.editor.edit(change);
  const setControl = <K extends keyof SliceControls>(key: K, value: SliceControls[K] | undefined) => edit((document) => {
    if (document.controls[key] === value) return document;
    const next = { ...document.controls };
    if (value === undefined) delete next[key];
    else next[key] = value;
    return { ...document, controls: next };
  });

  // --- Target ------------------------------------------------------------------
  const activePrinters = createMemo(() => printers().filter((printer) => !printer.archivedAt));
  const targetGroups = createMemo((): SelectGroup<TargetEntry>[] => {
    const current = props.document.target;
    const printerEntries: TargetEntry[] = activePrinters().map((printer) => ({
      key: `printer:${printer.id}`,
      label: printer.name,
      target: { kind: "printer", printerId: printer.id },
    }));
    if (current.kind === "printer" && !printerEntries.some((entry) => entry.key === targetKey(current))) {
      printerEntries.push({ key: targetKey(current), label: "A Printer that is no longer here", target: current });
    }
    const profiles = new Map<string, TargetEntry>();
    const addProfile = (ref: CatalogRef) => {
      const key = profileKey(ref);
      if (!profiles.has(key)) profiles.set(key, { key, label: ref.variant, target: { kind: "profile", catalogRef: { ...ref } } });
    };
    if (current.kind === "profile") addProfile(current.catalogRef);
    for (const printer of activePrinters()) addProfile(printer.catalogRef);
    const groups: SelectGroup<TargetEntry>[] = [];
    if (printerEntries.length > 0) groups.push({ label: "Printers", options: printerEntries });
    groups.push({
      label: "Printer profiles",
      // Any other catalog profile is one pick away.
      options: [...[...profiles.values()].sort((a, b) => a.label.localeCompare(b.label)), {
        key: OTHER_PROFILE, label: "Other printer profile…", target: current,
      }],
    });
    return groups;
  });
  const chosenTarget = () => {
    const key = targetKey(props.document.target);
    return targetGroups().flatMap((group) => group.options).find((entry) => entry.key === key) ?? null;
  };

  // A new target may not offer the chosen presets: once its options load,
  // presets it doesn't offer become its defaults (D3). Only after the user
  // changed the target here, never behind their back.
  let retargeted = false;
  const [otherOpen, setOtherOpen] = createSignal(false);
  const chooseTarget = (entry: TargetEntry) => {
    if (entry.key === OTHER_PROFILE) {
      setOtherOpen(true);
      return;
    }
    if (entry.key === targetKey(props.document.target)) return;
    retargeted = true;
    edit((document) => ({ ...document, target: entry.target }));
  };
  createEffect(on(() => session.options(), (options) => {
    if (!options || !retargeted) return;
    retargeted = false;
    edit((document) => {
      const processOffered = options.processPresets.some((preset) => preset.name === document.processPreset);
      const filamentOffered = options.filamentPresets.some((preset) => preset.name === document.filamentPreset);
      if (processOffered && filamentOffered) return document;
      return {
        ...document,
        processPreset: processOffered ? document.processPreset : options.defaults.processPreset ?? undefined,
        filamentPreset: filamentOffered ? document.filamentPreset : options.defaults.filamentPreset ?? undefined,
      };
    });
  }, { defer: true }));

  const matching = createMemo(() => {
    const ids = new Set(session.options()?.matchingPrinterIds ?? []);
    return activePrinters()
      .filter((printer) => ids.has(printer.id))
      .map((printer) => ({
        id: printer.id,
        name: printer.name,
        detail: printer.location,
        stateLabel: operationalLabel(printer.runtimeStatus),
      }));
  });

  // --- Presets -------------------------------------------------------------------
  const optionsNote = (): string | undefined => {
    const error = session.optionsError();
    if (error) return `The presets didn't load: ${error}`;
    if (!session.options()) return "Loading the presets for this target…";
    return undefined;
  };
  const processNames = () => (session.options()?.processPresets ?? []).map((preset) => preset.name);
  const filaments = () => session.options()?.filamentPresets ?? [];
  const chosenFilament = () => filaments().find((option) => option.name === props.document.filamentPreset) ?? null;
  const chosenProcess = () => processNames().find((name) => name === props.document.processPreset) ?? null;

  // --- Layer height limit (D4: at most 80% of the nozzle) ----------------------
  const nozzleMm = createMemo((): number | undefined => {
    const target = props.document.target;
    if (target.kind === "printer") {
      return printers().find((printer) => printer.id === target.printerId)?.profile.nozzleDiameterMm[0];
    }
    const parsed = Number.parseFloat(target.catalogRef.printerVariant);
    return Number.isFinite(parsed) && parsed > 0 ? parsed : undefined;
  });
  const maxFor = (control: NumericControl) => {
    if (control !== "layerHeightMm") return CONTROL_LIMITS[control].max;
    const nozzle = nozzleMm();
    return nozzle === undefined ? undefined : layerHeightMax(nozzle);
  };

  const numberField = (control: NumericControl, label: string, suffix?: string): JSX.Element => {
    const limits = CONTROL_LIMITS[control];
    const unknownLimit = () => control === "layerHeightMm" && maxFor(control) === undefined;
    return (
      <div class={styles.control} data-panel-field={control} tabIndex={-1}>
        <CommitNumberField
          label={label}
          value={controls()[control]}
          placeholder={PRESET_PLACEHOLDER}
          minValue={limits.min}
          maxValue={maxFor(control)}
          step={limits.step}
          suffix={suffix}
          disabled={unknownLimit()}
          onCommit={(value) => setControl(control, clampControl(control, value, maxFor(control)))}
          onClear={() => setControl(control, undefined)}
        />
        <Show when={unknownLimit()}>
          <p class={styles.note}>The nozzle size of this target isn't known, so its layer height can't be set.</p>
        </Show>
      </div>
    );
  };

  const choiceField = <T extends string>(
    control: "infillPattern" | "supports" | "brimType",
    label: string,
    choices: Choice<T>[],
  ): JSX.Element => (
    <div class={styles.control} data-panel-field={control} tabIndex={-1}>
      <Select<Choice<T>>
        label={label}
        options={choices}
        optionValue={(choice) => choice.value ?? "preset"}
        optionLabel={(choice) => choice.label}
        value={choices.find((choice) => choice.value === controls()[control]) ?? choices[0]}
        onChange={(choice) => setControl(control, choice.value as SliceControls[typeof control])}
      />
    </div>
  );

  return (
    <div class={styles.controls}>
      <section class={styles.section} aria-labelledby="preparation-target">
        <h3 id="preparation-target" class={styles.heading}>Target</h3>
        <div data-panel-field="target" tabIndex={-1}>
          <Select<TargetEntry>
            label="Slice for"
            groups={targetGroups()}
            optionValue={(entry) => entry.key}
            optionLabel={(entry) => entry.label}
            value={chosenTarget()}
            placeholder="Choose a Printer or profile"
            onChange={chooseTarget}
          />
        </div>
        <TargetProfileDialog
          open={otherOpen()}
          onOpenChange={setOtherOpen}
          onPick={(catalogRef) => {
            setOtherOpen(false);
            chooseTarget({ key: profileKey(catalogRef), label: catalogRef.variant, target: { kind: "profile", catalogRef } });
          }}
        />
        <Show when={session.options()}>
          {(options) => (
            <div class={styles.targetFacts}>
              <p class={styles.note}>OrcaSlicer printer: {options().machinePreset}</p>
              <PrinterRoster label="matching Printers" count={options().matchingPrinterIds.length} printers={matching()} />
            </div>
          )}
        </Show>
      </section>

      <section class={styles.section} aria-labelledby="preparation-material">
        <h3 id="preparation-material" class={styles.heading}>Material</h3>
        <div data-panel-field="material" tabIndex={-1}>
          <Select<FilamentPresetOption>
            label="Filament preset"
            options={filaments()}
            optionValue={(option) => option.name}
            optionLabel={filamentLabel}
            value={chosenFilament()}
            placeholder={props.document.filamentPreset ?? "Choose a filament"}
            disabled={!session.options()}
            onChange={(option) => {
              if (option.name !== props.document.filamentPreset) edit((document) => ({ ...document, filamentPreset: option.name }));
            }}
          />
        </div>
        <Show when={optionsNote()}>{(note) => <p class={styles.note}>{note()}</p>}</Show>
      </section>

      <section class={styles.section} aria-labelledby="preparation-quality">
        <h3 id="preparation-quality" class={styles.heading}>Quality</h3>
        <div data-panel-field="quality" tabIndex={-1}>
          <Select<string>
            label="Process preset"
            options={processNames()}
            value={chosenProcess()}
            placeholder={props.document.processPreset ?? "Choose a quality"}
            disabled={!session.options()}
            onChange={(name) => {
              if (name !== props.document.processPreset) edit((document) => ({ ...document, processPreset: name }));
            }}
          />
        </div>
        <p class={styles.note}>
          Only presets made for this printer are offered; presets that name their printers by a condition aren't.
        </p>
        {numberField("layerHeightMm", "Layer height", "mm")}
      </section>

      <section class={styles.section} aria-labelledby="preparation-strength">
        <h3 id="preparation-strength" class={styles.heading}>Strength</h3>
        <div class={styles.grid}>
          {numberField("wallLoops", "Walls")}
          {numberField("infillDensityPercent", "Infill density", "%")}
        </div>
        {choiceField("infillPattern", "Infill pattern", INFILL_PATTERNS)}
      </section>

      <section class={styles.section} aria-labelledby="preparation-support">
        <h3 id="preparation-support" class={styles.heading}>Support</h3>
        {choiceField("supports", "Supports", SUPPORT_MODES)}
        <div class={styles.grid}>
          {numberField("supportThresholdAngleDeg", "Overhang angle", "°")}
          {choiceField("brimType", "Brim", BRIM_TYPES)}
        </div>
      </section>

      <Collapsible class={styles.section} open={props.advancedOpen} onOpenChange={props.onAdvancedOpenChange}>
        <Collapsible.Trigger class={styles.disclosure}>
          <IconChevronRight class={styles.chevron} size={14} aria-hidden="true" />
          Advanced
        </Collapsible.Trigger>
        <Collapsible.Content class={styles.advanced}>
          <div class={styles.grid}>
            {numberField("topShellLayers", "Top shells")}
            {numberField("bottomShellLayers", "Bottom shells")}
            {numberField("brimWidthMm", "Brim width", "mm")}
            {numberField("skirtLoops", "Skirt loops")}
          </div>
        </Collapsible.Content>
      </Collapsible>
      <p class={styles.note}>Empty fields use the quality preset's value.</p>
    </div>
  );
}
