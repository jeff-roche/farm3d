import type { PrinterTelemetry } from "../generated/contracts/domain/PrinterTelemetry";

export function formatTemperature(current: number | undefined, target: number | undefined): string {
  return `${formatValue(current)} / ${formatValue(target)}`;
}

function formatValue(value: number | undefined): string {
  return value === undefined ? "—" : `${Math.round(value)} °C`;
}

export interface LabelledReading {
  label: string;
  value: string;
}

/** The nozzle readings a view shows: one "Nozzle" for a single-tool
 *  printer, or "T0", "T1", … for each tool of a multi-tool one (A0.1, #9,
 *  decision B2). Absent readings stay "—". */
export function nozzleReadings(
  readings: Pick<PrinterTelemetry, "nozzleTempC" | "nozzleTargetC" | "tools">,
): LabelledReading[] {
  const tools = readings.tools ?? [];
  if (tools.length > 1) {
    return tools.map((tool) => ({ label: `T${tool.index}`, value: formatTemperature(tool.tempC, tool.targetC) }));
  }
  return [{ label: "Nozzle", value: formatTemperature(readings.nozzleTempC, readings.nozzleTargetC) }];
}
