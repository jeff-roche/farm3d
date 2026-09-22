export function formatTemperature(current: number | undefined, target: number | undefined): string {
  return `${formatValue(current)} / ${formatValue(target)}`;
}

function formatValue(value: number | undefined): string {
  return value === undefined ? "—" : `${Math.round(value)} °C`;
}
