import { Show } from "solid-js";
import { Button, SeverityMarker } from "../design-system";
import type { MonitorPrinterView } from "../monitor/monitor-store";
import { formatTemperature } from "./monitor-printer-presentation";
import styles from "./PrinterCard.module.css";

export interface PrinterCardProps {
  printer: MonitorPrinterView;
  selected?: boolean;
  onSelect: (id: string) => void;
  onSelectTrigger?: (trigger: HTMLButtonElement) => void;
}

export function PrinterCard(props: PrinterCardProps) {
  return (
    <Button
      class={styles.card}
      classList={{ [styles.selected]: props.selected }}
      aria-label={props.printer.accessibleSummary}
      onClick={(event) => {
        props.onSelectTrigger?.(event.currentTarget);
        props.onSelect(props.printer.id);
      }}
    >
      <div class={styles.header}>
        <span class={styles.name}>{props.printer.name}</span>
        <span class={styles.state}>{props.printer.operationalLabel}</span>
      </div>
      <span class={styles.detail}>{props.printer.statusSummary}</span>
      <div class={styles.readings}>
        <span>Nozzle {formatTemperature(props.printer.readings.nozzleTempC, props.printer.readings.nozzleTargetC)}</span>
        <span>Bed {formatTemperature(props.printer.readings.bedTempC, props.printer.readings.bedTargetC)}</span>
      </div>
      <Show when={props.printer.freshnessLabel}>
        {(label) => <span class={styles.freshness}>{label()}</span>}
      </Show>
      <Show when={props.printer.severityLabel}>
        {(label) => <SeverityMarker severity={props.printer.severity} label={label()} />}
      </Show>
    </Button>
  );
}
