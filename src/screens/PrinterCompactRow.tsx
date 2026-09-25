import { Show } from "solid-js";
import { Button, SeverityMarker } from "../design-system";
import type { MonitorPrinterView } from "../monitor/monitor-store";
import { formatTemperature, nozzleReadings } from "./monitor-printer-presentation";
import styles from "./PrinterCompactRow.module.css";

export interface PrinterCompactRowProps {
  printer: MonitorPrinterView;
  selected?: boolean;
  onSelect: (id: string) => void;
  onSelectTrigger?: (trigger: HTMLButtonElement) => void;
}

export function PrinterCompactRow(props: PrinterCompactRowProps) {
  return (
    <Button
      class={styles.row}
      classList={{ [styles.selected]: props.selected }}
      aria-label={props.printer.accessibleSummary}
      onClick={(event) => {
        props.onSelectTrigger?.(event.currentTarget);
        props.onSelect(props.printer.id);
      }}
    >
      <span class={styles.name}>{props.printer.name}</span>
      <span class={styles.state}>{props.printer.operationalLabel}</span>
      <span class={styles.detail}>{props.printer.statusSummary}</span>
      <span class={styles.readings}>
        {[
          ...nozzleReadings(props.printer.readings),
          { label: "Bed", value: formatTemperature(props.printer.readings.bedTempC, props.printer.readings.bedTargetC) },
        ].map((reading) => `${reading.label} ${reading.value}`).join(" · ")}
      </span>
      <Show when={props.printer.freshnessLabel}>{(label) => <span class={styles.freshness}>{label()}</span>}</Show>
      <Show when={props.printer.hasMissingReadings}><span class={styles.missing}>Readings unavailable</span></Show>
      <Show when={props.printer.severityLabel}>{(label) => <SeverityMarker severity={props.printer.severity} label={label()} />}</Show>
    </Button>
  );
}
