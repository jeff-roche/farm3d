import { describe, expect, it } from "vitest";
import { formatTemperature, nozzleReadings } from "./monitor-printer-presentation";

describe("formatTemperature", () => {
  it("shows an absent reading as a dash, never zero", () => {
    expect(formatTemperature(undefined, 0)).toBe("— / 0 °C");
    expect(formatTemperature(209.6, undefined)).toBe("210 °C / —");
  });
});

describe("nozzleReadings", () => {
  it("keeps a single-tool printer's one Nozzle reading", () => {
    expect(nozzleReadings({ nozzleTempC: 210, nozzleTargetC: 215 })).toEqual([
      { label: "Nozzle", value: "210 °C / 215 °C" },
    ]);
    expect(nozzleReadings({})).toEqual([{ label: "Nozzle", value: "— / —" }]);
  });

  it("lists every tool of a multi-tool printer as T0, T1, … with absent readings as dashes", () => {
    // A0.1 (#9), decision B2.
    expect(
      nozzleReadings({
        nozzleTempC: 24,
        nozzleTargetC: 0,
        tools: [
          { index: 0, tempC: 24, targetC: 0 },
          { index: 1, tempC: 25.5, targetC: 210 },
          { index: 2 },
          { index: 3, tempC: 23 },
        ],
      }),
    ).toEqual([
      { label: "T0", value: "24 °C / 0 °C" },
      { label: "T1", value: "26 °C / 210 °C" },
      { label: "T2", value: "— / —" },
      { label: "T3", value: "23 °C / —" },
    ]);
  });
});
