import { createMemo, createSignal } from "solid-js";
import { Select, type SelectGroup } from "../design-system";
import { printers } from "../printers/printer-store";
import type { CatalogRef, SliceTarget } from "../slicing/types";
import { TargetProfileDialog } from "./TargetProfileDialog";

interface TargetEntry {
  key: string;
  label: string;
  target: SliceTarget | null;
}

/** The entry that opens the catalog, rather than being a target itself. */
const OTHER_PROFILE = "other-profile";

function profileKey(ref: CatalogRef): string {
  return `profile:${ref.vendor}|${ref.model}|${ref.variant}`;
}

export function sliceTargetKey(target: SliceTarget): string {
  return target.kind === "printer" ? `printer:${target.printerId}` : profileKey(target.catalogRef);
}

export interface SliceTargetSelectProps {
  label: string;
  /** `null` shows the placeholder: nothing chosen yet. */
  value: SliceTarget | null;
  /** A different target was chosen: a Printer, a profile, or one picked
   *  through **Other printer profile…**. */
  onChange: (target: SliceTarget) => void;
  placeholder?: string;
  error?: string;
}

/** A `SliceTarget` picker (D5, D16): the active Printers, then the printer
 *  profiles of those Printers (and the chosen one), then **Other printer
 *  profile…**, which reaches any catalog profile. Shared by the Preparation
 *  panel's **Slice for** and the G-code facts dialog's Printer profile. */
export function SliceTargetSelect(props: SliceTargetSelectProps) {
  const activePrinters = createMemo(() => printers().filter((printer) => !printer.archivedAt));
  const groups = createMemo((): SelectGroup<TargetEntry>[] => {
    const current = props.value;
    const printerEntries: TargetEntry[] = activePrinters().map((printer) => ({
      key: `printer:${printer.id}`,
      label: printer.name,
      target: { kind: "printer", printerId: printer.id },
    }));
    if (current?.kind === "printer" && !printerEntries.some((entry) => entry.key === sliceTargetKey(current))) {
      printerEntries.push({ key: sliceTargetKey(current), label: "A Printer that is no longer here", target: current });
    }
    const profiles = new Map<string, TargetEntry>();
    const addProfile = (ref: CatalogRef) => {
      const key = profileKey(ref);
      if (!profiles.has(key)) profiles.set(key, { key, label: ref.variant, target: { kind: "profile", catalogRef: { ...ref } } });
    };
    if (current?.kind === "profile") addProfile(current.catalogRef);
    for (const printer of activePrinters()) addProfile(printer.catalogRef);
    const result: SelectGroup<TargetEntry>[] = [];
    if (printerEntries.length > 0) result.push({ label: "Printers", options: printerEntries });
    result.push({
      label: "Printer profiles",
      // Any other catalog profile is one pick away.
      options: [...[...profiles.values()].sort((a, b) => a.label.localeCompare(b.label)), {
        key: OTHER_PROFILE, label: "Other printer profile…", target: null,
      }],
    });
    return result;
  });
  const chosen = () => {
    const current = props.value;
    if (!current) return null;
    const key = sliceTargetKey(current);
    return groups().flatMap((group) => group.options).find((entry) => entry.key === key) ?? null;
  };

  const [otherOpen, setOtherOpen] = createSignal(false);
  const choose = (entry: TargetEntry) => {
    if (entry.key === OTHER_PROFILE) {
      setOtherOpen(true);
      return;
    }
    if (entry.target && (!props.value || entry.key !== sliceTargetKey(props.value))) props.onChange(entry.target);
  };

  return (
    <>
      <Select<TargetEntry>
        label={props.label}
        groups={groups()}
        optionValue={(entry) => entry.key}
        optionLabel={(entry) => entry.label}
        value={chosen()}
        placeholder={props.placeholder}
        error={props.error}
        onChange={choose}
      />
      <TargetProfileDialog
        open={otherOpen()}
        onOpenChange={setOtherOpen}
        onPick={(catalogRef) => {
          setOtherOpen(false);
          choose({ key: profileKey(catalogRef), label: catalogRef.variant, target: { kind: "profile", catalogRef } });
        }}
      />
    </>
  );
}
