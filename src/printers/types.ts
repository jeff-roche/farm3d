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
  connection: unknown | null;
  /** Phase 3. Always undefined until live status polling lands. */
  runtimeStatus?: unknown;
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
