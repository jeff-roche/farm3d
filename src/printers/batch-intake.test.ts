import { describe, expect, it } from "vitest";
import {
  defaultPort,
  generateRows,
  mapDiscovery,
  parseIntake,
  toBatchInput,
  type BatchRowDraft,
} from "./batch-intake";
import type { DiscoveredPrinter } from "./types";
import type { ResolvedPrinter } from "./types";
import type { BatchShared } from "../generated/contracts/command/BatchShared";

function idGen(prefix = "row"): () => string {
  let n = 0;
  return () => `${prefix}-${++n}`;
}

describe("parseIntake", () => {
  it("parses a TSV header with two rows", () => {
    const text = "name\tlocation\thost\nVoron A\tBay A\tvoron-a.local\nVoron B\tBay B\tvoron-b.local";
    const result = parseIntake(text, idGen());
    expect(result.errors).toEqual([]);
    expect(result.rows).toHaveLength(2);
    expect(result.rows[0]).toMatchObject({ name: "Voron A", location: "Bay A", host: "voron-a.local" });
    expect(result.rows[1]).toMatchObject({ name: "Voron B", location: "Bay B", host: "voron-b.local" });
  });

  it("parses CSV with a quoted comma", () => {
    const text = 'name,location,host\n"Voron, A",Bay A,v1.local';
    const result = parseIntake(text, idGen());
    expect(result.errors).toEqual([]);
    expect(result.rows).toHaveLength(1);
    expect(result.rows[0].name).toBe("Voron, A");
    expect(result.rows[0].host).toBe("v1.local");
  });

  it("errors when the name header is missing", () => {
    const text = "location,host\nBay A,v1.local";
    const result = parseIntake(text, idGen());
    expect(result.rows).toEqual([]);
    expect(result.errors.length).toBeGreaterThan(0);
    expect(result.errors[0].message).toMatch(/name/i);
  });

  it("rejects a row that asks for TLS, which no adapter supports yet", () => {
    // A0.1 (#9), decision B4.
    const text = "name,host,tls\nVoron A,v1.local,yes\nVoron B,v2.local,no";
    const result = parseIntake(text, idGen());
    expect(result.errors).toEqual([{ line: 2, message: "TLS connections are not supported yet." }]);
    expect(result.rows.map((row) => [row.name, row.useTls])).toEqual([["Voron B", false]]);
  });

  it("reports a bad port as a line error while other lines survive", () => {
    const text = "name,host,port\nVoron A,v1.local,not-a-port\nVoron B,v2.local,7125";
    const result = parseIntake(text, idGen());
    expect(result.errors).toHaveLength(1);
    expect(result.errors[0].line).toBe(2);
    expect(result.rows).toHaveLength(1);
    expect(result.rows[0].name).toBe("Voron B");
  });

  it("warns once about an unknown column", () => {
    const text = "name,host,color\nVoron A,v1.local,red";
    const result = parseIntake(text, idGen());
    expect(result.errors).toEqual([]);
    expect(result.warnings).toHaveLength(1);
    expect(result.rows).toHaveLength(1);
  });

  it("rejects an api_key column with an error and returns no rows", () => {
    const text = "name,host,api_key\nVoron A,v1.local,secret-value";
    const result = parseIntake(text, idGen());
    expect(result.rows).toEqual([]);
    expect(result.errors.length).toBeGreaterThan(0);
    expect(result.errors[0].message).toMatch(/api_key|credential/i);
  });

  it("skips blank lines", () => {
    const text = "name,host\nVoron A,v1.local\n\n   \nVoron B,v2.local";
    const result = parseIntake(text, idGen());
    expect(result.errors).toEqual([]);
    expect(result.rows).toHaveLength(2);
  });

  it("defaults protocol to moonraker and port per protocol when a host is set", () => {
    const text = "name,host\nVoron A,v1.local";
    const result = parseIntake(text, idGen());
    expect(result.rows[0].protocol).toBe("moonraker");
    expect(result.rows[0].port).toBe(7125);
  });

  it("leaves port null when no host is given", () => {
    const text = "name,host\nVoron A,";
    const result = parseIntake(text, idGen());
    expect(result.rows[0].host).toBe("");
    expect(result.rows[0].port).toBeNull();
  });
});

describe("generateRows", () => {
  it("generates rows per location, counting {n} across locations", () => {
    const rows = generateRows(
      { quantity: 2, pattern: "Voron {nn}", locations: ["Bay A", "Bay B"] },
      idGen(),
    );
    expect(rows.map((r) => r.name)).toEqual(["Voron 01", "Voron 02", "Voron 03", "Voron 04"]);
    expect(rows.map((r) => r.location)).toEqual(["Bay A", "Bay A", "Bay B", "Bay B"]);
  });

  it("appends ` {n}` to a pattern with no placeholder", () => {
    const rows = generateRows({ quantity: 1, pattern: "Voron", locations: ["Bay A"] }, idGen());
    expect(rows[0].name).toBe("Voron 1");
  });

  it("returns an empty array for quantity 0", () => {
    const rows = generateRows({ quantity: 0, pattern: "Voron {n}", locations: ["Bay A"] }, idGen());
    expect(rows).toEqual([]);
  });
});

describe("mapDiscovery", () => {
  const existingPrinter = (id: string, host: string, port: number, archivedAt?: string): ResolvedPrinter =>
    ({
      id,
      revision: 1,
      name: id,
      notes: "",
      overrides: {},
      catalogRef: { vendor: "V", model: "M", variant: "V0.4", modelId: "V-M", printerVariant: "0.4" },
      catalogStatus: "ok",
      modelLabel: "M",
      variantLabel: "V0.4",
      profile: {} as ResolvedPrinter["profile"],
      overriddenFields: [],
      inherited: {},
      profileDrift: [],
      unknownOverrideKeys: [],
      startSafety: "confirmBedClear",
      setupGaps: [],
      createdAt: "",
      updatedAt: "",
      connection: { kind: "moonraker", host, port, useTls: false },
      archivedAt,
    }) as unknown as ResolvedPrinter;

  const row = (overrides: Partial<BatchRowDraft>): BatchRowDraft => ({
    rowId: "row-1",
    name: "Voron A",
    location: "",
    host: "",
    port: null,
    protocol: "moonraker",
    useTls: false,
    credential: { source: "none" },
    selected: true,
    ...overrides,
  });

  const candidate = (overrides: Partial<DiscoveredPrinter>): DiscoveredPrinter => ({
    kind: "moonraker",
    name: "Voron",
    host: "voron-a.local",
    port: 7125,
    addresses: [],
    ...overrides,
  });

  it("flags a candidate matching an existing printer as alreadyConfigured", () => {
    const candidates = [candidate({ host: "voron-a.local", port: 7125 })];
    const existing = [existingPrinter("prn-1", "voron-a.local", 7125)];
    const result = mapDiscovery(candidates, [], existing);
    expect(result.get("voron-a.local:7125")).toEqual({ kind: "alreadyConfigured", printerId: "prn-1" });
  });

  it("flags a candidate matching two rows as ambiguous", () => {
    const candidates = [candidate({ host: "voron-a.local", port: 7125 })];
    const rows = [
      row({ rowId: "row-1", host: "voron-a.local", port: 7125 }),
      row({ rowId: "row-2", host: "voron-a.local", port: 7125 }),
    ];
    const result = mapDiscovery(candidates, rows, []);
    const match = result.get("voron-a.local:7125");
    expect(match?.kind).toBe("ambiguous");
    expect((match as { rowIds: string[] }).rowIds.sort()).toEqual(["row-1", "row-2"]);
  });

  it("flags an unsupported kind as unsupported", () => {
    const candidates = [candidate({ kind: "elegoolink", host: "cc.local", port: 3030 })];
    const result = mapDiscovery(candidates, [], []);
    expect(result.get("cc.local:3030")).toEqual({ kind: "unsupported" });
  });

  it("treats a discovered OctoPrint instance as supported", () => {
    const candidates = [candidate({ kind: "octoprint", host: "octopi.local", port: 80 })];
    const rows = [row({ rowId: "row-1", host: "octopi.local", port: 80, protocol: "octoprint" })];
    const result = mapDiscovery(candidates, rows, []);
    expect(result.get("octopi.local:80")).toEqual({ kind: "assignable", rowId: "row-1" });
  });

  it("matches a row whose host is candidate's addresses[1]", () => {
    const candidates = [
      candidate({ host: "voron-a.local", port: 7125, addresses: ["192.168.1.5", "10.0.0.5"] }),
    ];
    const rows = [row({ rowId: "row-7", host: "10.0.0.5", port: 7125 })];
    const result = mapDiscovery(candidates, rows, []);
    expect(result.get("voron-a.local:7125")).toEqual({ kind: "assignable", rowId: "row-7" });
  });
});

describe("defaultPort", () => {
  it("returns 7125 for moonraker", () => {
    expect(defaultPort("moonraker")).toBe(7125);
  });
  it("returns 80 for octoprint", () => {
    expect(defaultPort("octoprint")).toBe(80);
  });
});

describe("toBatchInput", () => {
  const shared: BatchShared = { catalogRef: { vendor: "V", model: "M", variant: "V0.4", modelId: "V-M", printerVariant: "0.4" }, startSafety: "confirmBedClear" };

  const row = (overrides: Partial<BatchRowDraft>): BatchRowDraft => ({
    rowId: "row-1",
    name: "Voron A",
    location: "",
    host: "",
    port: null,
    protocol: "moonraker",
    useTls: false,
    credential: { source: "none" },
    selected: true,
    ...overrides,
  });

  it("omits connection for a row with an empty host", () => {
    const input = toBatchInput([row({})], shared, undefined, true, "batch-1");
    expect(input.rows[0].connection).toBeUndefined();
  });

  it("passes a row credential value through", () => {
    const input = toBatchInput(
      [row({ host: "v1.local", port: 7125, credential: { source: "row", value: "secret" } })],
      shared,
      undefined,
      true,
      "batch-1",
    );
    expect(input.rows[0].connection?.credential).toEqual({ source: "row", value: "secret" });
  });

  it("includes the shared credential only when provided", () => {
    const withCred = toBatchInput([row({})], shared, "shared-secret", true, "batch-1");
    expect(withCred.sharedCredential).toBe("shared-secret");
    const withoutCred = toBatchInput([row({})], shared, undefined, true, "batch-1");
    expect(withoutCred.sharedCredential).toBeUndefined();
  });
});
