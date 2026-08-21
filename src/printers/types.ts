export type BedShape =
  | { kind: "rectangular"; widthMm: number; depthMm: number; originXMm: number; originYMm: number }
  | { kind: "polygon"; points: { xMm: number; yMm: number }[] };

export interface PrinterProfile {
  bedShape: BedShape;
  printableHeightMm: number;
  bedExcludeAreas: { xMm: number; yMm: number }[];
  defaultBedType: string;
  nozzleDiameterMm: number[];
  nozzleType: string;
  gcodeFlavor: string;
  hasAuxiliaryFan: boolean;
  supportsAirFiltration: boolean;
  supportsMultiFilament: boolean;
  suggestedHostType: string | null;
}

export type OverridableField =
  | "bedShape"
  | "printableHeightMm"
  | "bedExcludeAreas"
  | "defaultBedType"
  | "hasAuxiliaryFan"
  | "supportsAirFiltration";

export interface CatalogRef {
  vendor: string;
  model: string;
  variant: string;
  modelId: string;
  printerVariant: string;
}

export type CatalogStatus =
  | "ok"
  | "rematched"
  | "variantMissing"
  | "modelMissing"
  | "vendorMissing";

export interface ProfileDrift {
  field: string;
  from: unknown;
  to: unknown;
}

export interface ResolvedPrinter {
  id: string;
  name: string;
  group: string;
  notes: string;
  catalogRef: CatalogRef;
  catalogStatus: CatalogStatus;
  modelLabel: string;
  variantLabel: string;
  profile: PrinterProfile;
  overriddenFields: OverridableField[];
  inherited: Partial<PrinterProfile>;
  profileDrift: ProfileDrift[];
  unknownOverrideKeys: string[];
  connection: ConnectionConfig | null;
  runtimeStatus?: PrinterStatus;
}

export interface PrinterDraft {
  name: string;
  catalogRef: CatalogRef;
  group?: string;
}

export interface PrinterPatch {
  name?: string;
  group?: string;
  notes?: string;
}

export interface CatalogModelSummary {
  modelId: string;
  vendor: string;
  model: string;
}

export interface CatalogVariantSummary {
  variant: string;
  printerVariant: string;
}

export interface CatalogInfo {
  generatedAt: string;
  sourceTag: string;
  modelCount: number;
  variantCount: number;
}

export interface ConnectionConfig {
  kind: string;
  host: string;
  port: number;
  useTls: boolean;
  /** A key into the credential store — never the secret itself. */
  credentialRef?: string;
}

/** What the Connection tab submits. `apiKey: undefined` leaves the stored
 *  secret untouched (the UI never echoes it back); `apiKey: ""` clears it. */
export interface ConnectionSubmission {
  kind: string;
  host: string;
  port: number;
  useTls: boolean;
  apiKey?: string;
}

export type ConnectionState = "connecting" | "online" | "offline" | "error";

export interface PrinterStatus {
  connectionState: ConnectionState;
  error?: string;
  jobState?: string;
  jobName?: string;
  /** 0..1 */
  progress?: number;
  nozzleTempC?: number;
  nozzleTargetC?: number;
  bedTempC?: number;
  bedTargetC?: number;
  printDurationS?: number;
  updatedAt: string;
}

export interface ReportedCapabilities {
  bedWidthMm?: number;
  bedDepthMm?: number;
  printableHeightMm?: number;
}

export interface ProbeResult {
  kind: string;
  hostSoftware: string;
  firmware: string;
  reportedName: string;
  state: string;
  stateMessage: string;
  reported: ReportedCapabilities;
}

export interface DiscoveredPrinter {
  kind: string;
  name: string;
  host: string;
  port: number;
  addresses: string[];
}

export interface CredentialStoreInfo {
  kind: "keychain" | "file";
  reason?: string;
}
