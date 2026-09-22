import { Dialog as KDialog } from "@kobalte/core/dialog";
import { Show } from "solid-js";
import { Button, Tabs } from "../design-system";
import type { ResolvedPrinter } from "../printers/types";
import { PrinterConnectionPanel } from "./PrinterConnectionPanel";
import { PrinterProfilePanel } from "./PrinterProfilePanel";
import { PrinterSetupPanel } from "./PrinterSetupPanel";
import { PrinterStatusPanel } from "./PrinterStatusPanel";
import styles from "./PrinterDetailDock.module.css";

export interface PrinterDetailDockProps {
  printer?: ResolvedPrinter;
  mode: "inline" | "overlay";
  onClose: () => void;
  onRemove: (id: string) => void;
  syncState?: "syncing" | "current" | "uncertain";
}

export function PrinterDetailDock(props: PrinterDetailDockProps) {
  const content = () => (
    <DockContent
      printer={props.printer!}
      overlay={props.mode === "overlay"}
      onClose={props.onClose}
      onRemove={props.onRemove}
      syncState={props.syncState}
    />
  );

  return (
    <Show when={props.printer}>
      <Show
        when={props.mode === "overlay"}
        fallback={<aside class={styles.inline} aria-label={props.printer!.name}>{content()}</aside>}
      >
        <KDialog open onOpenChange={(open) => !open && props.onClose()}>
          <KDialog.Portal>
            <KDialog.Overlay class={styles.overlay} />
            <KDialog.Content
              class={styles.overlayContent}
              onCloseAutoFocus={(event) => event.preventDefault()}
            >
              {content()}
            </KDialog.Content>
          </KDialog.Portal>
        </KDialog>
      </Show>
    </Show>
  );
}

function DockContent(props: Omit<PrinterDetailDockProps, "mode"> & { printer: ResolvedPrinter; overlay: boolean }) {
  return (
    <div class={styles.dock}>
      <header class={styles.header}>
        <div>
          <Show when={props.overlay} fallback={<h2 class={styles.title}>{props.printer.name}</h2>}>
            <KDialog.Title class={styles.title}>{props.printer.name}</KDialog.Title>
          </Show>
          <p class={styles.subtitle}>{props.printer.modelLabel}</p>
        </div>
        <Button variant="ghost" class={styles.close} onClick={props.onClose}>Close</Button>
      </header>
      <Tabs
        defaultValue="status"
        items={[
          {
            value: "status",
            label: "Status",
            content: <PrinterStatusPanel printer={props.printer} syncState={props.syncState} />,
          },
          {
            value: "setup",
            label: "Setup",
            content: (
              <div class={styles.setup}>
                <PrinterSetupPanel printer={props.printer} />
                <PrinterProfilePanel printer={props.printer} />
                <PrinterConnectionPanel printer={props.printer} />
                <Button variant="danger" onClick={() => props.onRemove(props.printer.id)}>Remove Printer</Button>
              </div>
            ),
          },
        ]}
      />
    </div>
  );
}
