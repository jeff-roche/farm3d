import { Show } from "solid-js";
import { Button, Dialog } from "../design-system";
import type { CatalogRef } from "../printers/types";
import { CatalogPickerFields, createCatalogPicker } from "./CatalogPicker";
import styles from "./TargetProfileDialog.module.css";

export interface TargetProfileDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The chosen catalog profile; the dialog closes after. */
  onPick: (catalogRef: CatalogRef) => void;
}

/** D20's Target reaches any catalog printer profile, not only those of the
 *  Farm's Printers: Brand, Model and Nozzle, as when adding a Printer. */
export function TargetProfileDialog(props: TargetProfileDialogProps) {
  return (
    <Dialog
      title="Other printer profile"
      description="Slice for any printer profile in the catalog."
      open={props.open}
      onOpenChange={props.onOpenChange}
    >
      {/* Mounted only while open, so the catalog loads on first use. */}
      <ProfilePicker onPick={props.onPick} onCancel={() => props.onOpenChange(false)} />
    </Dialog>
  );
}

function ProfilePicker(props: { onPick: (catalogRef: CatalogRef) => void; onCancel: () => void }) {
  const picker = createCatalogPicker();
  return (
    <div class={styles.body}>
      <CatalogPickerFields picker={picker} />
      <Show when={picker.models.error}>
        <p class={styles.error} role="alert">The printer catalog couldn't be loaded.</p>
      </Show>
      <div class={styles.actions}>
        <Button variant="ghost" onClick={props.onCancel}>Cancel</Button>
        <Button
          variant="primary"
          disabled={!picker.catalogRef()}
          onClick={() => {
            const ref = picker.catalogRef();
            if (ref) props.onPick(ref);
          }}
        >
          Use this profile
        </Button>
      </div>
    </div>
  );
}
