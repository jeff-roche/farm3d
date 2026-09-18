import { Show } from "solid-js";
import { Button, SeverityMarker } from "../design-system";
import type { MonitorPrinterView } from "../monitor/monitor-store";
import { detailLabel, formatTemperature, freshnessLabel, operationalLabel, severityLabel } from "./monitor-printer-presentation";
import styles from "./PrinterCompactRow.module.css";

export interface PrinterCompactRowProps {
  printer: MonitorPrinterView;
  selected?: boolean;
  onSelect: (id: string) => void;
}

export function PrinterCompactRow(props: PrinterCompactRowProps) {
  const select = () => props.onSelect(props.printer.id);
  const selectFromKeyboard = (event: KeyboardEvent) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      select();
    }
  };
  const progress = () => props.printer.operationalState === "printing" && props.printer.freshness === "fresh"
    ? props.printer.readings.progress
    : undefined;

  return (
    <Button
      class={styles.row}
      classList={{ [styles.selected]: props.selected }}
      aria-label={props.printer.accessibleSummary}
      onClick={select}
      onKeyDown={selectFromKeyboard}
    >
      <span class={styles.name}>{props.printer.name}</span>
      <span class={styles.state}>{operationalLabel(props.printer)}</span>
      <span class={styles.detail}>{detailLabel(props.printer)}</span>
      <Show when={progress() !== undefined}><span class={styles.progress}>Host print {progress()}%</span></Show>
      <span class={styles.readings}>Nozzle {formatTemperature(props.printer.readings.nozzleTempC, props.printer.readings.nozzleTargetC)} · Bed {formatTemperature(props.printer.readings.bedTempC, props.printer.readings.bedTargetC)}</span>
      <Show when={freshnessLabel(props.printer)}>{(label) => <span class={styles.freshness}>{label()}</span>}</Show>
      <Show when={severityLabel(props.printer)}>{(label) => <SeverityMarker severity={props.printer.severity} label={label()} />}</Show>
    </Button>
  );
}
