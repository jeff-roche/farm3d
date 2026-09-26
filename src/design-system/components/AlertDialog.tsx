// Kobalte's own `@kobalte/core/alert-dialog` is deliberately NOT imported:
// in @kobalte/core 0.13.13 that module runs `Object.assign(DialogRoot,
// { Content: AlertDialogContent, ... })` on the SAME `DialogRoot` object
// `@kobalte/core/dialog` exports, so merely importing it turns every
// ordinary `Dialog.Content` in the app into `role="alertdialog"`. Its
// `AlertDialogContent` is exactly `<DialogContent role="alertdialog">`, so
// this renders that directly.
import { Dialog as KDialog } from "@kobalte/core/dialog";
import { Show, type ParentProps } from "solid-js";
import styles from "./AlertDialog.module.css";

export interface AlertDialogProps extends ParentProps {
  title: string;
  description?: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Where focus goes when the dialog closes. There is no trigger (the
   *  caller opens it through `open`), so without this Kobalte leaves focus
   *  wherever it lands. Called at close time. */
  returnFocus?: () => HTMLElement | null | undefined;
}

/** A confirmation that interrupts: Kobalte's alert dialog
 *  (`Dialog.Content` with `role="alertdialog"`, see the import note above),
 *  for a destructive or irreversible choice. No
 *  close button — the children carry the explicit choices (e.g. "Keep
 *  printing" / "Cancel print"); Escape still dismisses it. */
export function AlertDialog(props: AlertDialogProps) {
  return (
    <KDialog open={props.open} onOpenChange={props.onOpenChange}>
      <KDialog.Portal>
        <KDialog.Overlay class={styles.overlay} />
        <KDialog.Content
          role="alertdialog"
          class={styles.content}
          onCloseAutoFocus={(event) => {
            const element = props.returnFocus?.();
            if (!element) return;
            event.preventDefault();
            element.focus({ preventScroll: true });
          }}
        >
          <KDialog.Title class={styles.title}>{props.title}</KDialog.Title>
          <Show when={props.description}>
            <KDialog.Description class={styles.description}>{props.description}</KDialog.Description>
          </Show>
          {props.children}
        </KDialog.Content>
      </KDialog.Portal>
    </KDialog>
  );
}
