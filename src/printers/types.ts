import type { PrinterRecord } from "../generated/contracts/domain/PrinterRecord";
import type { CatalogRef } from "../generated/contracts/domain/CatalogRef";
import type { PrinterStatus } from "../generated/contracts/domain/PrinterStatus";

export type { BedShape } from "../generated/contracts/domain/BedShape";
export type { CatalogRef };
export type { CatalogStatus } from "../generated/contracts/domain/CatalogStatus";
export type { ConnectionConfig } from "../generated/contracts/domain/ConnectionConfig";
export type { ConnectionState } from "../generated/contracts/domain/ConnectionState";
export type { ConnectionSubmission } from "../generated/contracts/domain/ConnectionSubmission";
export type { CredentialStoreInfo } from "../generated/contracts/domain/CredentialStoreInfo";
export type { DiscoveredPrinter } from "../generated/contracts/domain/DiscoveredPrinter";
export type { ProbeResult } from "../generated/contracts/domain/ProbeResult";
export type { ProfileDrift } from "../generated/contracts/domain/ProfileDrift";
export type { ReportedCapabilities } from "../generated/contracts/domain/ReportedCapabilities";
export type { PrinterProfile } from "../generated/contracts/domain/PrinterProfile";
export type { PrinterStatus } from "../generated/contracts/domain/PrinterStatus";
export type { PrinterStatusEventPayload } from "../generated/contracts/domain/PrinterStatusEventPayload";
export type { PrinterStatusEventType } from "../generated/contracts/domain/PrinterStatusEventType";
export type { PrinterPatch } from "../generated/contracts/command/PrinterPatch";
export type { CatalogInfo } from "../generated/contracts/domain/CatalogInfo";
export type { CatalogModelSummary } from "../generated/contracts/domain/CatalogModelSummary";
export type { CatalogVariantSummary } from "../generated/contracts/domain/CatalogVariantSummary";

export interface PrinterDraft {
  name: string;
  catalogRef: CatalogRef;
}

/** Presentation-only live state layered over the generated durable wire record. */
export type ResolvedPrinter = Omit<PrinterRecord, "profileResolution"> &
  PrinterRecord["profileResolution"] & {
  runtimeStatus?: PrinterStatus;
};

export function resolvePrinterRecord(record: PrinterRecord): ResolvedPrinter {
  const { profileResolution, ...printer } = record;
  return { ...printer, ...profileResolution };
}

export type OverridableField =
  | "bedShape"
  | "printableHeightMm"
  | "bedExcludeAreas"
  | "defaultBedType"
  | "hasAuxiliaryFan"
  | "supportsAirFiltration";
