/**
 * Pure batch-setup helpers (spec D10-D12): paste/CSV intake, row
 * generation, and discovery-to-row mapping. No Solid imports — this module
 * is imported by both the batch dialog's component-local store and its
 * tests without dragging in Solid's reactivity.
 */
import type { BatchCredentialSource } from "../generated/contracts/command/BatchCredentialSource";
import type { BatchRowInput } from "../generated/contracts/command/BatchRowInput";
import type { BatchRowResult } from "../generated/contracts/command/BatchRowResult";
import type { BatchShared } from "../generated/contracts/command/BatchShared";
import type { CreatePrintersBatchInput } from "../generated/contracts/command/CreatePrintersBatchInput";
import { canonicalHostIdentity } from "./host-identity";
import type { DiscoveredPrinter, ResolvedPrinter } from "./types";

export interface BatchRowDraft {
  rowId: string;
  name: string;
  location: string;
  host: string;
  port: number | null;
  protocol: string;
  useTls: boolean;
  credential: BatchCredentialSource;
  selected: boolean;
  /** Set once created; retries target it rather than creating a duplicate. */
  printerId?: string;
  /** Latest result for this row, from a `create_printers_batch` response. */
  result?: BatchRowResult;
}

/** Whether this row already produced a Printer. Keyed on the latest
 *  outcome as well as `printerId`: a `created`/`createdSetupIncomplete`
 *  result without a `printer` record still means a Printer exists, so the
 *  row must never be re-sent in a batch (D1: a retry never creates a second
 *  Printer) and its identity is no longer editable. */
export function isRowCreated(row: BatchRowDraft): boolean {
  const outcome = row.result?.outcome;
  return !!row.printerId || outcome === "created" || outcome === "createdSetupIncomplete";
}

export interface IntakeIssue {
  line: number;
  message: string;
}

export interface IntakeResult {
  rows: BatchRowDraft[];
  errors: IntakeIssue[];
  warnings: IntakeIssue[];
}

/** Adapter kinds this build actually supports (mirrors
 *  `PrinterConnectionPanel`'s `KINDS` — only Moonraker today). */
const SUPPORTED_PROTOCOLS = new Set(["moonraker"]);

const KNOWN_COLUMNS = new Set(["name", "location", "host", "port", "protocol", "tls"]);

/** Column names that would leak a secret into a pasted/CSV file if read —
 *  D10 says these must never be read, and their presence rejects the whole
 *  input rather than silently ignoring the column. Compared against the
 *  header with punctuation/case stripped so `API_KEY`, `api-key`, and
 *  `apikey` are all caught. */
const CREDENTIAL_COLUMN_NAMES = new Set(["apikey", "password", "token", "secret"]);

function normalizeHeaderKey(header: string): string {
  return header.trim().toLowerCase().replace(/[^a-z0-9]/g, "");
}

export function defaultPort(protocol: string): number {
  return protocol.toLowerCase() === "octoprint" ? 80 : 7125;
}

/** Splits one line into fields for `delimiter`, honoring double-quoted
 *  fields with `""`-escaped quotes. Does not handle a quoted field that
 *  spans multiple lines — batch intake is a flat paste/CSV, not an
 *  arbitrary RFC 4180 document. */
function splitFields(line: string, delimiter: string): string[] {
  const fields: string[] = [];
  let current = "";
  let inQuotes = false;
  for (let i = 0; i < line.length; i += 1) {
    const char = line[i];
    if (inQuotes) {
      if (char === '"') {
        if (line[i + 1] === '"') {
          current += '"';
          i += 1;
        } else {
          inQuotes = false;
        }
      } else {
        current += char;
      }
    } else if (char === '"') {
      inQuotes = true;
    } else if (char === delimiter) {
      fields.push(current);
      current = "";
    } else {
      current += char;
    }
  }
  fields.push(current);
  return fields;
}

function parseTls(value: string): boolean {
  return ["true", "yes", "1"].includes(value.trim().toLowerCase());
}

export function parseIntake(text: string, newRowId: () => string): IntakeResult {
  const lines = text.split(/\r\n|\r|\n/);
  const headerLine = lines[0] ?? "";
  const delimiter = headerLine.includes("\t") ? "\t" : ",";
  const headers = splitFields(headerLine, delimiter).map((h) => h.trim());
  const normalizedHeaders = headers.map(normalizeHeaderKey);

  const credentialColumn = headers.find((header) =>
    CREDENTIAL_COLUMN_NAMES.has(normalizeHeaderKey(header)),
  );
  if (credentialColumn) {
    return {
      rows: [],
      errors: [
        {
          line: 1,
          message: `Column "${credentialColumn}" looks like a credential and cannot be imported. Remove it and set credentials per row after import.`,
        },
      ],
      warnings: [],
    };
  }

  const nameIndex = normalizedHeaders.indexOf("name");
  if (nameIndex === -1) {
    return {
      rows: [],
      errors: [{ line: 1, message: 'A "name" column is required.' }],
      warnings: [],
    };
  }
  const locationIndex = normalizedHeaders.indexOf("location");
  const hostIndex = normalizedHeaders.indexOf("host");
  const portIndex = normalizedHeaders.indexOf("port");
  const protocolIndex = normalizedHeaders.indexOf("protocol");
  const tlsIndex = normalizedHeaders.indexOf("tls");

  const warnings: IntakeIssue[] = [];
  const unknownColumns = headers.filter((_, index) => !KNOWN_COLUMNS.has(normalizedHeaders[index]));
  if (unknownColumns.length > 0) {
    warnings.push({
      line: 1,
      message: `Unknown column${unknownColumns.length > 1 ? "s" : ""} ignored: ${unknownColumns.join(", ")}.`,
    });
  }

  const rows: BatchRowDraft[] = [];
  const errors: IntakeIssue[] = [];

  for (let lineNumber = 2; lineNumber <= lines.length; lineNumber += 1) {
    const line = lines[lineNumber - 1];
    if (line === undefined || line.trim() === "") continue;

    const fields = splitFields(line, delimiter);
    const name = (fields[nameIndex] ?? "").trim();
    if (name === "") {
      errors.push({ line: lineNumber, message: "A name is required." });
      continue;
    }
    const location = locationIndex === -1 ? "" : (fields[locationIndex] ?? "").trim();
    const host = hostIndex === -1 ? "" : (fields[hostIndex] ?? "").trim();
    const protocolRaw = protocolIndex === -1 ? "" : (fields[protocolIndex] ?? "").trim();
    const protocol = protocolRaw === "" ? "moonraker" : protocolRaw.toLowerCase();
    const useTls = tlsIndex === -1 ? false : parseTls(fields[tlsIndex] ?? "");

    let port: number | null = null;
    const portRaw = portIndex === -1 ? "" : (fields[portIndex] ?? "").trim();
    if (portRaw !== "") {
      const parsed = Number(portRaw);
      if (!Number.isInteger(parsed) || parsed < 1 || parsed > 65535) {
        errors.push({ line: lineNumber, message: `Invalid port "${portRaw}".` });
        continue;
      }
      port = parsed;
    } else if (host !== "") {
      port = defaultPort(protocol);
    }

    rows.push({
      rowId: newRowId(),
      name,
      location,
      host,
      port,
      protocol,
      useTls,
      credential: { source: "none" },
      selected: true,
    });
  }

  return { rows, errors, warnings };
}

export function generateRows(
  opts: { quantity: number; pattern: string; locations: string[]; startAt?: number },
  newRowId: () => string,
): BatchRowDraft[] {
  if (opts.quantity <= 0) return [];
  const pattern = /\{n+\}/i.test(opts.pattern) ? opts.pattern : `${opts.pattern} {n}`;
  const rows: BatchRowDraft[] = [];
  let counter = opts.startAt ?? 1;

  for (const location of opts.locations) {
    for (let i = 0; i < opts.quantity; i += 1) {
      const name = pattern.replace(/\{(n+)\}/i, (_match, ns: string) =>
        String(counter).padStart(ns.length, "0"),
      );
      rows.push({
        rowId: newRowId(),
        name,
        location,
        host: "",
        port: null,
        protocol: "moonraker",
        useTls: false,
        credential: { source: "none" },
        selected: true,
      });
      counter += 1;
    }
  }
  return rows;
}

export type CandidateMatch =
  | { kind: "assignable"; rowId?: string }
  | { kind: "alreadyConfigured"; printerId: string }
  | { kind: "ambiguous"; rowIds: string[] }
  | { kind: "unsupported" };

function rowIdentity(row: BatchRowDraft): string | null {
  if (row.host.trim() === "") return null;
  return canonicalHostIdentity(row.host, row.port ?? defaultPort(row.protocol));
}

/** Maps discovery candidates to batch rows and existing Printers by
 *  canonical host identity (spec D12). Keyed by the candidate's own
 *  `host:port` (not its canonical form) so callers can look a result up by
 *  what they rendered. */
export function mapDiscovery(
  candidates: DiscoveredPrinter[],
  rows: BatchRowDraft[],
  existing: ResolvedPrinter[],
): Map<string, CandidateMatch> {
  const existingIdentities = new Map<string, string>();
  for (const printer of existing) {
    if (printer.archivedAt) continue;
    const connection = printer.connection;
    if (!connection) continue;
    const identity = canonicalHostIdentity(connection.host, connection.port);
    if (identity) existingIdentities.set(identity, printer.id);
  }

  const rowIdentities = rows
    .map((row) => ({ row, identity: rowIdentity(row) }))
    .filter((entry): entry is { row: BatchRowDraft; identity: string } => entry.identity !== null);

  const result = new Map<string, CandidateMatch>();
  // Tracks which candidate keys ended up assignable to exactly one row, so a
  // second pass can catch a row matched by more than one candidate (D12:
  // ambiguous applies both when a candidate matches >1 row and when a row
  // matches >1 candidate).
  const singularClaims = new Map<string, string[]>(); // rowId -> candidate keys

  for (const candidate of candidates) {
    const key = `${candidate.host}:${candidate.port}`;
    if (!SUPPORTED_PROTOCOLS.has(candidate.kind.toLowerCase())) {
      result.set(key, { kind: "unsupported" });
      continue;
    }

    const addresses = [candidate.host, ...candidate.addresses];
    const identities = new Set(
      addresses
        .map((address) => canonicalHostIdentity(address, candidate.port))
        .filter((identity): identity is string => identity !== null),
    );

    const alreadyConfiguredId = [...identities]
      .map((identity) => existingIdentities.get(identity))
      .find((id): id is string => id !== undefined);
    if (alreadyConfiguredId) {
      result.set(key, { kind: "alreadyConfigured", printerId: alreadyConfiguredId });
      continue;
    }

    const matchingRowIds = [
      ...new Set(
        rowIdentities.filter((entry) => identities.has(entry.identity)).map((entry) => entry.row.rowId),
      ),
    ];

    if (matchingRowIds.length === 0) {
      result.set(key, { kind: "assignable" });
    } else if (matchingRowIds.length > 1) {
      result.set(key, { kind: "ambiguous", rowIds: matchingRowIds });
    } else {
      result.set(key, { kind: "assignable", rowId: matchingRowIds[0] });
      const claims = singularClaims.get(matchingRowIds[0]) ?? [];
      claims.push(key);
      singularClaims.set(matchingRowIds[0], claims);
    }
  }

  // A row singularly claimed by more than one candidate is ambiguous from
  // both candidates' sides.
  for (const [rowId, keys] of singularClaims) {
    if (keys.length <= 1) continue;
    for (const key of keys) {
      result.set(key, { kind: "ambiguous", rowIds: [rowId] });
    }
  }

  return result;
}

export function toBatchInput(
  rows: BatchRowDraft[],
  shared: BatchShared,
  sharedCredential: string | undefined,
  probe: boolean,
  batchId: string,
): CreatePrintersBatchInput {
  return {
    batchId,
    shared,
    ...(sharedCredential !== undefined ? { sharedCredential } : {}),
    probe,
    rows: rows.map((row): BatchRowInput => ({
      rowId: row.rowId,
      name: row.name,
      ...(row.location.trim() !== "" ? { location: row.location } : {}),
      ...(row.host.trim() !== ""
        ? {
            connection: {
              kind: row.protocol,
              host: row.host,
              port: row.port ?? defaultPort(row.protocol),
              useTls: row.useTls,
              credential: row.credential,
            },
          }
        : {}),
    })),
  };
}
