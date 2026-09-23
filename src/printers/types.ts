import type { PrinterRecord } from "../generated/contracts/domain/PrinterRecord";
import type { CatalogRef } from "../generated/contracts/domain/CatalogRef";
import type { ConnectionSubmission } from "../generated/contracts/domain/ConnectionSubmission";
import type { PrinterStatus } from "../generated/contracts/domain/PrinterStatus";
import type { StartSafety } from "../generated/contracts/domain/StartSafety";

export type { BedShape } from "../generated/contracts/domain/BedShape";
export type { CatalogRef };
export type { CatalogStatus } from "../generated/contracts/domain/CatalogStatus";
export type { ConnectionConfig } from "../generated/contracts/domain/ConnectionConfig";
export type { ConnectionState } from "../generated/contracts/domain/ConnectionState";
export type { ConnectionSubmission };
export type { CredentialStoreInfo } from "../generated/contracts/domain/CredentialStoreInfo";
export type { DiscoveredPrinter } from "../generated/contracts/domain/DiscoveredPrinter";
export type { LifecycleAction } from "../generated/contracts/domain/LifecycleAction";
export type { LifecycleBlocker } from "../generated/contracts/domain/LifecycleBlocker";
export type { LifecycleBlockerCode } from "../generated/contracts/domain/LifecycleBlockerCode";
export type { LifecycleEligibility } from "../generated/contracts/domain/LifecycleEligibility";
export type { ProbeResult } from "../generated/contracts/domain/ProbeResult";
export type { ProfileDrift } from "../generated/contracts/domain/ProfileDrift";
export type { ReportedCapabilities } from "../generated/contracts/domain/ReportedCapabilities";
export type { PrinterProfile } from "../generated/contracts/domain/PrinterProfile";
export type { PrinterStatus } from "../generated/contracts/domain/PrinterStatus";
export type { PrinterStatusEventPayload } from "../generated/contracts/domain/PrinterStatusEventPayload";
export type { PrinterStatusEventType } from "../generated/contracts/domain/PrinterStatusEventType";
export type { PrinterPatch } from "../generated/contracts/command/PrinterPatch";
export type { SetupGap } from "../generated/contracts/domain/SetupGap";
export type { StartSafety };
export type { CatalogInfo } from "../generated/contracts/domain/CatalogInfo";
export type { CatalogModelSummary } from "../generated/contracts/domain/CatalogModelSummary";
export type { CatalogVariantSummary } from "../generated/contracts/domain/CatalogVariantSummary";

/** Options for `createPrinter` (spec D1/D5/D9) — a single-step create that
 *  may include a Connection, start safety, and a shared-bed-type override
 *  up front. */
export interface CreatePrinterOptions {
  name: string;
  catalogRef: CatalogRef;
  location?: string;
  startSafety?: StartSafety;
  defaultBedType?: string;
  connection?: ConnectionSubmission;
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
