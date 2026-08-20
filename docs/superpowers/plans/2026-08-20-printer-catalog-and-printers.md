# Printer catalog and Printer instances Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace farm3d's hardcoded mock `Printer` array with a real two-level
data model — a read-only Printer Model catalog derived from OrcaSlicer's
presets, and user-owned Printer instances that live-inherit from their catalog
variant with sparse overrides — persisted to `printers.json` and rendered as a
grouped, editable dashboard.

**Architecture:** A generator binary (`gen-catalog`) sparse-clones a pinned
OrcaSlicer git tag, walks its `inherits` chains, and emits an allowlisted,
committed `printer-catalog.json` snapshot (~1.1 MB pretty-printed; 369 models
/ 971 variants at `v2.4.2` — see Task 4's note below on why this is larger
than the plan's earlier estimate) — the *only* runtime catalog
source, loaded once into `tauri::State<Arc<Catalog>>`. Printers persist
separately in `printers.json` via hand-rolled `std::fs`/`serde_json`, mirroring
the existing `settings.rs` pattern. Override resolution happens in Rust;
Tauri commands return a fully pre-resolved `ResolvedPrinter` so the SolidJS
frontend (a `createStore`-backed module singleton, mirroring
`settings-store.ts`) never touches the catalog directly except in the
add-printer flow.

**Tech Stack:** Rust (`serde`, `serde_json`, `std::fs` — no new crates this
phase), SolidJS (`solid-js/store`, Kobalte `number-field` and `combobox`
primitives, newly wrapped), Vitest + `@solidjs/testing-library`.

**Spec:** `docs/superpowers/specs/2026-08-20-printer-catalog-and-printers-design.md`

## Global Constraints

- Persistence follows `src-tauri/src/settings.rs`'s exact shape: `std::fs` +
  `serde_json`, pure path-taking helpers, thin `#[tauri::command]` wrappers,
  `#[serde(rename_all = "camelCase", default)]`, `to_string_pretty` output.
- Never hardcode colors, font sizes, or radii — use `--f3d-color-*` /
  `--f3d-type-*` / `--f3d-radius-*` custom properties.
- Prefer a Kobalte primitive over hand-rolled behavior; check
  `node_modules/@kobalte/core/src/<name>/` for real prop names before wiring
  one up.
- CSS Modules per component (`Component.module.css`), never global CSS.
- Every new design-system component is added to
  `src/design-system/components/index.ts` **and** `src/design-system/Showcase.tsx`.
- Kobalte's `Select`/`Combobox`/`DropdownMenu` triggers open on `pointerdown`
  and items select on `pointerup` — tests use `fireEvent.pointerDown` /
  `fireEvent.pointerUp`, never `fireEvent.click`, for those.
- `just build` and `just test` must both pass before any task is considered
  done; `just test-rust` needs `source "$HOME/.cargo/env" &&` prefixed in
  non-interactive shells.
- The catalog is generated from a **pinned OrcaSlicer git tag**
  (`v2.4.2`) via sparse clone — never from a local OrcaSlicer install, never
  vendored in full. It is the **only** runtime catalog source; no local-install
  scanning in this phase.
- Nozzle diameter, nozzle type, and gcode flavor are **never** user-overridable
  fields — they are variant identity, changed only by rebinding to a sibling
  variant.
- `tauri.conf.json`'s `identifier` is `"farm3d"` — the app config dir is
  `~/.config/farm3d` (Linux). Don't use `com.jroche.farm3d` anywhere new.

---

## File Structure

**Generator-only (never runs in the shipped app):**
- `src-tauri/src/catalog/ingest/shape.rs` — `printable_area` string parsing → `BedShape`
- `src-tauri/src/catalog/ingest/inherits.rs` — the `inherits` chain walk over a vendor's raw machine-preset files
- `src-tauri/src/catalog/ingest/mod.rs` — allowlisted extraction: raw vendor JSON → `Catalog`
- `src-tauri/src/bin/gen-catalog.rs` — CLI: sparse-clone pinned tag → ingest → write snapshot JSON

**Runtime:**
- `src-tauri/src/catalog/mod.rs` — `Catalog`/`CatalogModel`/`CatalogVariant`/`BedShape` types (shared by generator and runtime) + `load_snapshot(&Path)`
- `src-tauri/src/catalog/resolve.rs` — `resolve_catalog_ref`, `merge_profile`, `resolve_printer` (pure, testable)
- `src-tauri/src/catalog/commands.rs` — `#[tauri::command]` catalog read endpoints
- `src-tauri/src/printers.rs` — `StoredPrinter` et al., path-taking persistence, `apply_override`, printer commands
- `src-tauri/src/lib.rs` — wire `CatalogState`, register all new commands *(modify)*

**Frontend:**
- `src/printers/types.ts` — TS mirrors of the Rust wire types
- `src/printers/printer-store.ts` — `createStore`-backed module singleton, `isTauri()`-guarded
- `src/printers/printer-catalog.ts` — catalog read wrappers used only by the add-printer flow
- `src/design-system/components/NumberField.tsx` (+ `.module.css`)
- `src/design-system/components/Combobox.tsx` (+ `.module.css`)
- `src/design-system/components/Field.tsx` (+ `.module.css`)
- `src/screens/PrinterAddDialog.tsx` (+ `.module.css`)
- `src/screens/PrinterProfilePanel.tsx` (+ `.module.css`)
- `src/screens/PrinterDashboard.tsx` — regroup + new prop surface *(modify)*
- `src/App.tsx` — delete mock data, wire the store *(modify)*

**Docs:**
- `docs/adr/0007-printer-catalog-is-derived-data.md`
- `docs/adr/0003-wrap-orcaslicer.md` — GPLv3 → AGPL-3.0 correction *(modify)*

## Interfaces summary (cross-task contract)

```ts
// src/printers/types.ts — the shape every later task codes against
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
  | "bedShape" | "printableHeightMm" | "bedExcludeAreas"
  | "defaultBedType" | "hasAuxiliaryFan" | "supportsAirFiltration";

export interface CatalogRef {
  vendor: string; model: string; variant: string; modelId: string; printerVariant: string;
}

export type CatalogStatus = "ok" | "rematched" | "variantMissing" | "modelMissing" | "vendorMissing";

export interface ResolvedPrinter {
  id: string; name: string; group: string; notes: string;
  catalogRef: CatalogRef;
  catalogStatus: CatalogStatus;
  modelLabel: string; variantLabel: string;
  profile: PrinterProfile;
  overriddenFields: OverridableField[];
  inherited: Partial<PrinterProfile>;
  profileDrift: { field: string; from: unknown; to: unknown }[];
  unknownOverrideKeys: string[];
  connection: unknown | null;
  /** Phase 3. Always undefined until live status polling lands. */
  runtimeStatus?: unknown;
}

export interface CatalogModelSummary { modelId: string; vendor: string; model: string; }
export interface CatalogVariantSummary { variant: string; printerVariant: string; }
export interface CatalogInfo { generatedAt: string; sourceTag: string; modelCount: number; variantCount: number; }
```

---

## Task 1: Bed shape parsing (`catalog/ingest/shape.rs`)

**Files:**
- Create: `src-tauri/src/catalog/ingest/shape.rs`
- Create: `src-tauri/src/catalog/ingest/mod.rs` (module declarations only this task)
- Modify: `src-tauri/src/lib.rs:1` (add `pub mod catalog;`)
- Create: `src-tauri/src/catalog/mod.rs` (module declarations only this task)

**Interfaces:**
- Consumes: nothing (first task)
- Produces: `pub fn parse_printable_area(points: &[String]) -> Result<BedShape, ShapeError>`,
  `pub enum BedShape { Rectangular { width_mm, depth_mm, origin_x_mm, origin_y_mm }, Polygon { points: Vec<PointMm> } }`,
  `pub struct PointMm { pub x_mm: f64, pub y_mm: f64 }` — all in `catalog::BedShape` /
  `catalog::PointMm` (re-exported from `catalog/mod.rs`), used by every later
  catalog task and by `printers.rs`'s override type.

- [ ] **Step 1: Create the module skeleton**

`src-tauri/src/catalog/mod.rs`:
```rust
pub mod ingest;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BedShape {
    Rectangular {
        width_mm: f64,
        depth_mm: f64,
        origin_x_mm: f64,
        origin_y_mm: f64,
    },
    Polygon {
        points: Vec<PointMm>,
    },
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PointMm {
    pub x_mm: f64,
    pub y_mm: f64,
}
```

`src-tauri/src/catalog/ingest/mod.rs`:
```rust
pub mod shape;
```

`src-tauri/src/lib.rs`, add as the first line:
```rust
pub mod catalog;
```

`pub`, not `mod` — `bin/gen-catalog.rs` (Task 4) is a separate crate target in
the same package and can only reach `catalog::ingest` through the library
crate's public surface (`farm3d_lib::catalog::...`).

- [ ] **Step 2: Write the failing tests**

`src-tauri/src/catalog/ingest/shape.rs`:
```rust
use crate::catalog::{BedShape, PointMm};

#[derive(Debug, PartialEq)]
pub struct ShapeError(pub String);

impl std::fmt::Display for ShapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Parses OrcaSlicer's `printable_area` list of "Xx Y" strings (e.g. "0x0",
/// "256x0", "256x256", "0x256") into a BedShape. Exactly four points that form
/// an axis-aligned bounding box become Rectangular; anything else (deltas,
/// circular beds, cut corners) becomes Polygon.
pub fn parse_printable_area(points: &[String]) -> Result<BedShape, ShapeError> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_point_box_becomes_rectangular() {
        let points = ["0x0", "256x0", "256x256", "0x256"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let shape = parse_printable_area(&points).unwrap();
        assert_eq!(
            shape,
            BedShape::Rectangular {
                width_mm: 256.0,
                depth_mm: 256.0,
                origin_x_mm: 0.0,
                origin_y_mm: 0.0,
            }
        );
    }

    #[test]
    fn offset_rectangle_captures_origin() {
        let points = ["10x0", "266x0", "266x256", "10x256"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let shape = parse_printable_area(&points).unwrap();
        assert_eq!(
            shape,
            BedShape::Rectangular {
                width_mm: 256.0,
                depth_mm: 256.0,
                origin_x_mm: 10.0,
                origin_y_mm: 0.0,
            }
        );
    }

    #[test]
    fn non_rectangular_shape_becomes_polygon() {
        // A coarse octagon approximation, as delta/round beds use.
        let raw = ["150x0", "106x44", "44x106", "0x150", "0x0"];
        let points = raw.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let shape = parse_printable_area(&points).unwrap();
        match shape {
            BedShape::Polygon { points: parsed } => assert_eq!(parsed.len(), 5),
            other => panic!("expected Polygon, got {other:?}"),
        }
    }

    #[test]
    fn malformed_point_is_an_error() {
        let points = vec!["not-a-point".to_string()];
        assert!(parse_printable_area(&points).is_err());
    }

    #[test]
    fn empty_list_is_an_error() {
        assert!(parse_printable_area(&[]).is_err());
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml catalog::ingest::shape`
Expected: compile error or panic from `unimplemented!()` on every test.

- [ ] **Step 4: Implement `parse_printable_area`**

Replace the `unimplemented!()` body in `src-tauri/src/catalog/ingest/shape.rs` with:
```rust
pub fn parse_printable_area(points: &[String]) -> Result<BedShape, ShapeError> {
    let parsed: Vec<PointMm> = points
        .iter()
        .map(|p| {
            let (x, y) = p
                .split_once('x')
                .ok_or_else(|| ShapeError(format!("malformed point: {p:?}")))?;
            Ok(PointMm {
                x_mm: x.parse().map_err(|_| ShapeError(format!("bad x in {p:?}")))?,
                y_mm: y.parse().map_err(|_| ShapeError(format!("bad y in {p:?}")))?,
            })
        })
        .collect::<Result<_, ShapeError>>()?;

    if parsed.is_empty() {
        return Err(ShapeError("printable_area has no points".to_string()));
    }
    if let Some(rect) = as_axis_aligned_rectangle(&parsed) {
        return Ok(rect);
    }
    Ok(BedShape::Polygon { points: parsed })
}

fn as_axis_aligned_rectangle(points: &[PointMm]) -> Option<BedShape> {
    if points.len() != 4 {
        return None;
    }
    let min_x = points.iter().map(|p| p.x_mm).fold(f64::INFINITY, f64::min);
    let max_x = points.iter().map(|p| p.x_mm).fold(f64::NEG_INFINITY, f64::max);
    let min_y = points.iter().map(|p| p.y_mm).fold(f64::INFINITY, f64::min);
    let max_y = points.iter().map(|p| p.y_mm).fold(f64::NEG_INFINITY, f64::max);

    let is_corner =
        |p: &PointMm| (p.x_mm == min_x || p.x_mm == max_x) && (p.y_mm == min_y || p.y_mm == max_y);
    if !points.iter().all(is_corner) {
        return None;
    }
    let mut corners: Vec<(i64, i64)> = points
        .iter()
        .map(|p| ((p.x_mm * 1000.0).round() as i64, (p.y_mm * 1000.0).round() as i64))
        .collect();
    corners.sort_unstable();
    corners.dedup();
    if corners.len() != 4 {
        return None;
    }

    Some(BedShape::Rectangular {
        width_mm: max_x - min_x,
        depth_mm: max_y - min_y,
        origin_x_mm: min_x,
        origin_y_mm: min_y,
    })
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml catalog::ingest::shape`
Expected: all 5 tests pass.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/lib.rs src-tauri/src/catalog/mod.rs src-tauri/src/catalog/ingest/mod.rs src-tauri/src/catalog/ingest/shape.rs
git commit -m "feat: add BedShape and printable_area parsing for the catalog generator"
```

---

## Task 2: The `inherits` chain walk (`catalog/ingest/inherits.rs`)

**Files:**
- Create: `src-tauri/src/catalog/ingest/inherits.rs`
- Modify: `src-tauri/src/catalog/ingest/mod.rs` (add `pub mod inherits;`)

**Interfaces:**
- Consumes: nothing beyond `serde_json::Value` / `std::collections::HashMap`
- Produces: `pub fn resolve_machine_preset(index: &HashMap<String, Value>, name: &str) -> Result<Value, InheritsError>` — used by Task 3's ingestion to fully resolve every leaf variant before extracting typed fields.

467 of a real vendor snapshot's leaf variant files lack `printable_area` —
resolving a variant means merging it with its ancestors, base to derived, with
a derived preset's own keys winning. This is why the walk operates on raw
`serde_json::Value` maps rather than a typed struct: at any point in the
chain, only a subset of fields may be present, and the merge must be
field-by-field, not struct-by-struct.

- [ ] **Step 1: Write the failing tests**

`src-tauri/src/catalog/ingest/inherits.rs`:
```rust
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, PartialEq)]
pub struct InheritsError(pub String);

impl std::fmt::Display for InheritsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Resolves a named machine preset within `index` (preset name -> raw JSON
/// object) by walking its `inherits` chain from base to derived, merging
/// fields so a derived preset's own keys win. Returns the fully merged object.
pub fn resolve_machine_preset(
    index: &HashMap<String, Value>,
    name: &str,
) -> Result<Value, InheritsError> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn index(entries: &[(&str, Value)]) -> HashMap<String, Value> {
        entries.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn leaf_with_no_own_fields_inherits_everything() {
        let idx = index(&[
            ("base", json!({ "printable_height": 250, "gcode_flavor": "klipper" })),
            ("leaf", json!({ "inherits": "base", "name": "leaf" })),
        ]);
        let resolved = resolve_machine_preset(&idx, "leaf").unwrap();
        assert_eq!(resolved["printable_height"], 250);
        assert_eq!(resolved["gcode_flavor"], "klipper");
        assert_eq!(resolved["name"], "leaf");
    }

    #[test]
    fn three_deep_chain_merges_base_to_derived_with_derived_winning() {
        let idx = index(&[
            ("fdm_machine_common", json!({ "printable_height": 200, "auxiliary_fan": 0 })),
            ("family_common", json!({ "inherits": "fdm_machine_common", "auxiliary_fan": 1 })),
            ("leaf", json!({ "inherits": "family_common", "printable_height": 256 })),
        ]);
        let resolved = resolve_machine_preset(&idx, "leaf").unwrap();
        // Derived (leaf) overrides the common ancestor's printable_height.
        assert_eq!(resolved["printable_height"], 256);
        // Mid-chain override (family_common) wins over the base.
        assert_eq!(resolved["auxiliary_fan"], 1);
    }

    #[test]
    fn dangling_inherits_is_an_error_not_a_panic() {
        let idx = index(&[("leaf", json!({ "inherits": "does_not_exist" }))]);
        let result = resolve_machine_preset(&idx, "leaf");
        assert!(result.is_err());
    }

    #[test]
    fn inherits_cycle_terminates_with_an_error() {
        let idx = index(&[
            ("a", json!({ "inherits": "b" })),
            ("b", json!({ "inherits": "a" })),
        ]);
        let result = resolve_machine_preset(&idx, "a");
        assert!(result.is_err());
    }

    #[test]
    fn unknown_root_name_is_an_error() {
        let idx: HashMap<String, Value> = HashMap::new();
        assert!(resolve_machine_preset(&idx, "nope").is_err());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml catalog::ingest::inherits`
Expected: panics from `unimplemented!()`.

- [ ] **Step 3: Implement `resolve_machine_preset`**

Replace the `unimplemented!()` body with:
```rust
pub fn resolve_machine_preset(
    index: &HashMap<String, Value>,
    name: &str,
) -> Result<Value, InheritsError> {
    resolve_inner(index, name, &mut Vec::new())
}

fn resolve_inner(
    index: &HashMap<String, Value>,
    name: &str,
    seen: &mut Vec<String>,
) -> Result<Value, InheritsError> {
    if seen.iter().any(|s| s == name) {
        return Err(InheritsError(format!(
            "inherits cycle detected at {name:?}: {seen:?}"
        )));
    }
    seen.push(name.to_string());

    let preset = index
        .get(name)
        .ok_or_else(|| InheritsError(format!("unknown preset {name:?} (dangling inherits)")))?;

    let mut merged = match preset.get("inherits").and_then(|v| v.as_str()) {
        Some(parent) => resolve_inner(index, parent, seen)?,
        None => Value::Object(serde_json::Map::new()),
    };

    if let (Value::Object(base), Value::Object(overlay)) = (&mut merged, preset) {
        for (k, v) in overlay {
            base.insert(k.clone(), v.clone());
        }
    }
    Ok(merged)
}
```

Add `use crate::catalog::ingest::inherits::InheritsError;`-style export by adding
`pub mod inherits;` to `src-tauri/src/catalog/ingest/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml catalog::ingest::inherits`
Expected: all 5 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/catalog/ingest/inherits.rs src-tauri/src/catalog/ingest/mod.rs
git commit -m "feat: add the inherits chain walk for OrcaSlicer machine presets"
```

---

## Task 3: Catalog types + allowlisted ingestion (`catalog/ingest/mod.rs`)

**Files:**
- Modify: `src-tauri/src/catalog/mod.rs` (add `CatalogVariant`, `CatalogModel`, `Catalog` types)
- Create: `src-tauri/src/catalog/ingest/mod.rs` (replace the module-declarations-only version from Task 1 with the real ingestion logic)
- Create: `src-tauri/tests/fixtures/profiles/**` (synthetic vendor fixture tree, listed below)

**Interfaces:**
- Consumes: `catalog::ingest::inherits::resolve_machine_preset` (Task 2),
  `catalog::ingest::shape::parse_printable_area` (Task 1)
- Produces: `pub fn ingest_profiles_dir(dir: &Path) -> Result<Vec<CatalogModel>, IngestError>`
  — consumed by `bin/gen-catalog.rs` in Task 4. Also produces the full
  `CatalogVariant`/`CatalogModel`/`Catalog` types in `catalog::mod`, which every
  later task (runtime loading, resolution, commands, frontend types) is built
  against.

### First, verify two real field types against a live OrcaSlicer install

Before writing fixtures, this was verified against
`~/.config/OrcaSlicer/system/Elegoo/machine/ECC/Elegoo Centauri Carbon 0.4 nozzle.json`:
`printable_height` is the **string** `"256"`, not a JSON number, and
`auxiliary_fan` / `support_air_filtration` / `support_multi_filament` are the
**strings** `"1"`/`"0"`, not booleans. The fixtures and `extract_variant` below
both encode this — get it wrong and every real vendor bundle fails to parse.

- [ ] **Step 1: Add the catalog types**

Add to `src-tauri/src/catalog/mod.rs` (after the existing `BedShape`/`PointMm`):
```rust
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CatalogVariant {
    pub variant: String,
    pub printer_variant: String,
    pub bed_shape: BedShape,
    pub printable_height_mm: f64,
    pub bed_exclude_areas: Vec<PointMm>,
    pub default_bed_type: String,
    pub nozzle_diameter_mm: Vec<f64>,
    pub nozzle_type: String,
    pub gcode_flavor: String,
    pub has_auxiliary_fan: bool,
    pub supports_air_filtration: bool,
    pub supports_multi_filament: bool,
    pub suggested_host_type: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub model_id: String,
    pub vendor: String,
    pub model: String,
    pub variants: Vec<CatalogVariant>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub generated_at: String,
    pub source_tag: String,
    pub notice: String,
    pub models: Vec<CatalogModel>,
}
```

- [ ] **Step 2: Create the fixture vendor bundle**

Create `src-tauri/tests/fixtures/profiles/TestVendor.json`:
```json
{
  "name": "TestVendor",
  "version": "1.0.0.0",
  "machine_model_list": [
    { "name": "Test Printer", "sub_path": "machine/TP/Test Printer.json" },
    { "name": "Test Delta", "sub_path": "machine/TD/Test Delta.json" },
    { "name": "Test Printer 5T", "sub_path": "machine/T5/Test Printer 5T.json" }
  ],
  "machine_list": [
    { "name": "Test Printer 0.4 nozzle", "sub_path": "machine/TP/Test Printer 0.4 nozzle.json" },
    { "name": "Test Delta 0.4 nozzle", "sub_path": "machine/TD/Test Delta 0.4 nozzle.json" },
    { "name": "Test Printer 5T 0.4 nozzle", "sub_path": "machine/T5/Test Printer 5T 0.4 nozzle.json" },
    { "name": "Ghost Printer 0.4 nozzle", "sub_path": "machine/nonexistent.json" }
  ]
}
```

The `"Ghost Printer 0.4 nozzle"` entry deliberately names a preset that exists
nowhere in the fixture tree — it exercises the "skip a broken entry, don't
fail the whole vendor" behavior real bundles occasionally need (a vendor
bundle referencing a preset OrcaSlicer itself later renamed or removed).

- [ ] **Step 3: Create the shared base preset (3-deep chain, root)**

Create `src-tauri/tests/fixtures/profiles/TestVendor/machine/fdm_machine_common.json`:
```json
{
  "type": "machine",
  "name": "fdm_machine_common",
  "printable_area": ["0x0", "220x0", "220x220", "0x220"],
  "printable_height": "250",
  "gcode_flavor": "marlin",
  "nozzle_type": "brass",
  "default_bed_type": "1",
  "auxiliary_fan": "0",
  "support_air_filtration": "0",
  "support_multi_filament": "0"
}
```

- [ ] **Step 4: Create "Test Printer" — needs the full 3-deep chain walk**

Create `src-tauri/tests/fixtures/profiles/TestVendor/machine/TP/Test Printer.json` (model file):
```json
{
  "type": "machine_model",
  "name": "Test Printer",
  "model_id": "TestVendor-TP",
  "nozzle_diameter": "0.4",
  "machine_tech": "FFF"
}
```

Create `src-tauri/tests/fixtures/profiles/TestVendor/machine/TP/fdm_testvendor_common.json` (mid-chain, overrides fan/filtration):
```json
{
  "type": "machine",
  "name": "fdm_testvendor_common",
  "inherits": "fdm_machine_common",
  "auxiliary_fan": "1",
  "support_air_filtration": "1"
}
```

Create `src-tauri/tests/fixtures/profiles/TestVendor/machine/TP/Test Printer 0.4 nozzle.json` (leaf — **no `printable_area` or `printable_height` of its own**, must resolve through two levels of `inherits`):
```json
{
  "type": "machine",
  "name": "Test Printer 0.4 nozzle",
  "inherits": "fdm_testvendor_common",
  "printer_model": "Test Printer",
  "printer_variant": "0.4",
  "nozzle_diameter": ["0.4"],
  "host_type": "moonraker"
}
```

- [ ] **Step 5: Create "Test Delta" — non-rectangular bed, own field overrides**

Create `src-tauri/tests/fixtures/profiles/TestVendor/machine/TD/Test Delta.json` (model file):
```json
{
  "type": "machine_model",
  "name": "Test Delta",
  "model_id": "TestVendor-TD",
  "nozzle_diameter": "0.4",
  "machine_tech": "FFF"
}
```

Create `src-tauri/tests/fixtures/profiles/TestVendor/machine/TD/Test Delta 0.4 nozzle.json`:
```json
{
  "type": "machine",
  "name": "Test Delta 0.4 nozzle",
  "inherits": "fdm_machine_common",
  "printer_model": "Test Delta",
  "printer_variant": "0.4",
  "nozzle_diameter": ["0.4"],
  "printable_area": ["150x0", "106x44", "44x106", "0x150", "0x0"],
  "gcode_flavor": "klipper",
  "host_type": "octoprint"
}
```

- [ ] **Step 6: Create "Test Printer 5T" — multi-toolhead, own rectangular bed**

Create `src-tauri/tests/fixtures/profiles/TestVendor/machine/T5/Test Printer 5T.json` (model file):
```json
{
  "type": "machine_model",
  "name": "Test Printer 5T",
  "model_id": "TestVendor-T5",
  "nozzle_diameter": "0.4",
  "machine_tech": "FFF"
}
```

Create `src-tauri/tests/fixtures/profiles/TestVendor/machine/T5/Test Printer 5T 0.4 nozzle.json`:
```json
{
  "type": "machine",
  "name": "Test Printer 5T 0.4 nozzle",
  "inherits": "fdm_machine_common",
  "printer_model": "Test Printer 5T",
  "printer_variant": "0.4",
  "nozzle_diameter": ["0.4", "0.4", "0.4", "0.4", "0.4"],
  "printable_area": ["0x0", "300x0", "300x300", "0x300"],
  "host_type": "octoprint"
}
```

- [ ] **Step 7: Write the failing ingestion test**

Replace `src-tauri/src/catalog/ingest/mod.rs` (which so far only declared
`pub mod shape;` / `pub mod inherits;`) with:
```rust
pub mod inherits;
pub mod shape;

use crate::catalog::{BedShape, CatalogModel, CatalogVariant, PointMm};
use inherits::{resolve_machine_preset, InheritsError};
use serde_json::Value;
use shape::{parse_printable_area, ShapeError};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum IngestError {
    Io(String),
    Json(String),
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IngestError::Io(e) => write!(f, "io error: {e}"),
            IngestError::Json(e) => write!(f, "json error: {e}"),
        }
    }
}

impl From<InheritsError> for IngestError {
    fn from(e: InheritsError) -> Self {
        IngestError::Json(e.0)
    }
}

impl From<ShapeError> for IngestError {
    fn from(e: ShapeError) -> Self {
        IngestError::Json(e.0)
    }
}

/// Ingests OrcaSlicer's `resources/profiles` directory into a list of
/// CatalogModels, allowlisting only the factual capability fields farm3d
/// reads — never g-code, bed/hotend model filenames, or vendor descriptions.
/// A broken individual model or variant (unresolvable inherits, missing
/// fields) is skipped rather than failing the whole vendor.
pub fn ingest_profiles_dir(dir: &Path) -> Result<Vec<CatalogModel>, IngestError> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/profiles")
    }

    #[test]
    fn ingests_all_three_valid_models_and_skips_the_ghost_entry() {
        let models = ingest_profiles_dir(&fixtures_dir()).unwrap();
        let names: Vec<&str> = models.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"Test Printer"));
        assert!(names.contains(&"Test Delta"));
        assert!(names.contains(&"Test Printer 5T"));
    }

    #[test]
    fn resolves_printable_area_and_fan_flags_through_a_three_deep_chain() {
        let models = ingest_profiles_dir(&fixtures_dir()).unwrap();
        let printer = models.iter().find(|m| m.model == "Test Printer").unwrap();
        let variant = &printer.variants[0];
        assert_eq!(
            variant.bed_shape,
            BedShape::Rectangular {
                width_mm: 220.0,
                depth_mm: 220.0,
                origin_x_mm: 0.0,
                origin_y_mm: 0.0,
            }
        );
        assert_eq!(variant.printable_height_mm, 250.0);
        // Overridden by the mid-chain preset, not the base.
        assert!(variant.has_auxiliary_fan);
        assert!(variant.supports_air_filtration);
    }

    #[test]
    fn non_rectangular_own_printable_area_becomes_polygon() {
        let models = ingest_profiles_dir(&fixtures_dir()).unwrap();
        let delta = models.iter().find(|m| m.model == "Test Delta").unwrap();
        match &delta.variants[0].bed_shape {
            BedShape::Polygon { points } => assert_eq!(points.len(), 5),
            other => panic!("expected Polygon, got {other:?}"),
        }
        assert_eq!(delta.variants[0].gcode_flavor, "klipper");
    }

    #[test]
    fn multi_toolhead_nozzle_diameter_is_preserved_as_an_array() {
        let models = ingest_profiles_dir(&fixtures_dir()).unwrap();
        let t5 = models.iter().find(|m| m.model == "Test Printer 5T").unwrap();
        assert_eq!(t5.variants[0].nozzle_diameter_mm, vec![0.4, 0.4, 0.4, 0.4, 0.4]);
    }

    #[test]
    fn model_id_and_vendor_are_captured() {
        let models = ingest_profiles_dir(&fixtures_dir()).unwrap();
        let printer = models.iter().find(|m| m.model == "Test Printer").unwrap();
        assert_eq!(printer.model_id, "TestVendor-TP");
        assert_eq!(printer.vendor, "TestVendor");
    }
}
```

- [ ] **Step 8: Run the tests to verify they fail**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml catalog::ingest::tests`
Expected: panics from `unimplemented!()`.

- [ ] **Step 9: Implement `ingest_profiles_dir`**

Replace the `unimplemented!()` body in `src-tauri/src/catalog/ingest/mod.rs` with:
```rust
pub fn ingest_profiles_dir(dir: &Path) -> Result<Vec<CatalogModel>, IngestError> {
    let mut models = Vec::new();
    let mut vendor_files: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| IngestError::Io(e.to_string()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .collect();
    vendor_files.sort();

    for vendor_file in vendor_files {
        let vendor_name = vendor_file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let bundle: Value = match fs::read_to_string(&vendor_file) {
            Ok(s) => match serde_json::from_str(&s) {
                Ok(v) => v,
                Err(_) => continue,
            },
            Err(_) => continue,
        };

        let machine_dir = dir.join(&vendor_name).join("machine");
        let index = index_machine_dir(&machine_dir)?;

        let model_list = bundle["machine_model_list"].as_array().cloned().unwrap_or_default();
        let machine_list = bundle["machine_list"].as_array().cloned().unwrap_or_default();

        for model_entry in &model_list {
            let sub_path = model_entry["sub_path"].as_str().unwrap_or_default();
            let model_json: Value = match fs::read_to_string(dir.join(&vendor_name).join(sub_path)) {
                Ok(s) => match serde_json::from_str(&s) {
                    Ok(v) => v,
                    Err(_) => continue,
                },
                Err(_) => continue,
            };
            if let Some(tech) = model_json.get("machine_tech").and_then(|v| v.as_str()) {
                if tech != "FFF" {
                    continue;
                }
            }
            let model_name = model_json["name"].as_str().unwrap_or_default().to_string();
            let model_id = model_json["model_id"].as_str().unwrap_or_default().to_string();

            let mut variants = Vec::new();
            for variant_entry in &machine_list {
                let variant_name = match variant_entry["name"].as_str() {
                    Some(n) => n,
                    None => continue,
                };
                let resolved = match resolve_machine_preset(&index, variant_name) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if resolved.get("printer_model").and_then(|v| v.as_str()) != Some(model_name.as_str()) {
                    continue;
                }
                if let Ok(variant) = extract_variant(variant_name, &resolved) {
                    variants.push(variant);
                }
            }
            if !variants.is_empty() {
                models.push(CatalogModel {
                    model_id,
                    vendor: vendor_name.clone(),
                    model: model_name,
                    variants,
                });
            }
        }
    }
    Ok(models)
}

fn index_machine_dir(machine_dir: &Path) -> Result<HashMap<String, Value>, IngestError> {
    let mut index = HashMap::new();
    if !machine_dir.exists() {
        return Ok(index);
    }
    for path in walk_json_files(machine_dir)? {
        let contents = fs::read_to_string(&path).map_err(|e| IngestError::Io(e.to_string()))?;
        let json: Value = match serde_json::from_str(&contents) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(name) = json.get("name").and_then(|v| v.as_str()) {
            index.insert(name.to_string(), json);
        }
    }
    Ok(index)
}

fn walk_json_files(dir: &Path) -> Result<Vec<PathBuf>, IngestError> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| IngestError::Io(e.to_string()))? {
        let path = entry.map_err(|e| IngestError::Io(e.to_string()))?.path();
        if path.is_dir() {
            out.extend(walk_json_files(&path)?);
        } else if path.extension().map(|x| x == "json").unwrap_or(false) {
            out.push(path);
        }
    }
    Ok(out)
}

/// Allowlist: only these fields are ever copied out of a resolved preset.
/// Never g-code, filenames, or descriptions — see ADR 0007.
fn extract_variant(name: &str, resolved: &Value) -> Result<CatalogVariant, IngestError> {
    let printable_area: Vec<String> = resolved["printable_area"]
        .as_array()
        .ok_or_else(|| IngestError::Json(format!("{name}: missing printable_area")))?
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    let bed_shape = parse_printable_area(&printable_area)?;

    let printable_height_mm = resolved["printable_height"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .ok_or_else(|| IngestError::Json(format!("{name}: missing/invalid printable_height")))?;

    let nozzle_diameter_mm: Vec<f64> = resolved["nozzle_diameter"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().and_then(|s| s.parse::<f64>().ok()))
                .collect()
        })
        .unwrap_or_default();

    let bed_exclude_areas: Vec<PointMm> = resolved
        .get("bed_exclude_area")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter_map(|s| s.split_once('x'))
                .filter_map(|(x, y)| {
                    Some(PointMm {
                        x_mm: x.parse().ok()?,
                        y_mm: y.parse().ok()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let flag = |key: &str| resolved.get(key).and_then(|v| v.as_str()) == Some("1");

    Ok(CatalogVariant {
        variant: name.to_string(),
        printer_variant: resolved["printer_variant"].as_str().unwrap_or_default().to_string(),
        bed_shape,
        printable_height_mm,
        bed_exclude_areas,
        default_bed_type: resolved["default_bed_type"].as_str().unwrap_or_default().to_string(),
        nozzle_diameter_mm,
        nozzle_type: resolved["nozzle_type"].as_str().unwrap_or_default().to_string(),
        gcode_flavor: resolved["gcode_flavor"].as_str().unwrap_or_default().to_string(),
        has_auxiliary_fan: flag("auxiliary_fan"),
        supports_air_filtration: flag("support_air_filtration"),
        supports_multi_filament: flag("support_multi_filament"),
        suggested_host_type: resolved["host_type"].as_str().map(|s| s.to_string()),
    })
}
```

- [ ] **Step 10: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml catalog::ingest::tests`
Expected: all 5 tests pass.

- [ ] **Step 11: Commit**

```bash
git add src-tauri/src/catalog/mod.rs src-tauri/src/catalog/ingest/mod.rs src-tauri/tests/fixtures
git commit -m "feat: add allowlisted catalog ingestion over a resolved preset tree"
```

---

## Task 4: The `gen-catalog` binary + regenerate the real snapshot

**Files:**
- Create: `src-tauri/src/bin/gen-catalog.rs`
- Modify: `justfile` (add the `gen-catalog` recipe)
- Create: `src-tauri/resources/printer-catalog.json` (generated by running the binary, not hand-written)

**Interfaces:**
- Consumes: `farm3d_lib::catalog::ingest::ingest_profiles_dir` (Task 3),
  `farm3d_lib::catalog::Catalog` (Task 3)
- Produces: the committed `src-tauri/resources/printer-catalog.json` snapshot —
  consumed by Task 6 (runtime loading) and Task 5's completeness tests.

This binary has no unit tests of its own — its logic (ingestion, shape
parsing, the inherits walk) is already covered by Tasks 1–3. What it adds is
orchestration: clone, ingest, serialize, write. Its correctness is verified by
actually running it and inspecting the result.

- [ ] **Step 1: Write the binary**

Create `src-tauri/src/bin/gen-catalog.rs`:
```rust
use farm3d_lib::catalog::ingest::ingest_profiles_dir;
use farm3d_lib::catalog::Catalog;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const DEFAULT_TAG: &str = "v2.4.2";
const REPO_URL: &str = "https://github.com/OrcaSlicer/OrcaSlicer.git";

fn main() {
    let tag = env::args().nth(1).unwrap_or_else(|| DEFAULT_TAG.to_string());
    let tmp = env::temp_dir().join(format!("farm3d-gen-catalog-{}", std::process::id()));

    println!("Sparse-cloning OrcaSlicer at {tag} into {}...", tmp.display());
    run(Command::new("git").args([
        "clone",
        "--depth",
        "1",
        "--filter=blob:none",
        "--sparse",
        "--branch",
        &tag,
        REPO_URL,
        tmp.to_str().expect("temp path is not valid UTF-8"),
    ]));
    run(Command::new("git").args([
        "-C",
        tmp.to_str().unwrap(),
        "sparse-checkout",
        "set",
        "resources/profiles",
    ]));

    let profiles_dir = tmp.join("resources/profiles");
    println!("Ingesting {}...", profiles_dir.display());
    let models = ingest_profiles_dir(&profiles_dir).expect("ingestion failed");
    let variant_count: usize = models.iter().map(|m| m.variants.len()).sum();
    println!("Ingested {} models / {variant_count} variants", models.len());

    let catalog = Catalog {
        generated_at: now_utc_rfc3339(),
        source_tag: tag,
        notice: "Derived from OrcaSlicer's bundled printer profile data \
                 (https://github.com/OrcaSlicer/OrcaSlicer, AGPL-3.0-or-later). \
                 farm3d extracts only factual machine specifications (build \
                 volumes, nozzle diameters, capability flags); no G-code, \
                 scripts, assets, or descriptive text are included."
            .to_string(),
        models,
    };

    let out_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/printer-catalog.json");
    fs::create_dir_all(out_path.parent().unwrap()).expect("could not create resources dir");
    let json = serde_json::to_string_pretty(&catalog).expect("could not serialize catalog");
    fs::write(&out_path, json).expect("could not write catalog");
    println!("Wrote {}", out_path.display());

    fs::remove_dir_all(&tmp).ok();
}

fn run(cmd: &mut Command) {
    let status = cmd.status().expect("failed to run command");
    if !status.success() {
        panic!("command failed: {cmd:?}");
    }
}

/// Shells out to `date` rather than adding a date/time crate for one
/// provenance timestamp — this binary is a dev-only generator, never part of
/// the shipped app, and already depends on `git` being on PATH.
fn now_utc_rfc3339() -> String {
    let output = Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .expect("failed to run `date`");
    String::from_utf8(output.stdout)
        .expect("date output was not UTF-8")
        .trim()
        .to_string()
}
```

- [ ] **Step 2: Add the justfile recipe**

Add to `justfile`, after the existing `test-rust` recipe:
```make
# Regenerate the bundled printer catalog from a pinned OrcaSlicer git tag
gen-catalog tag="v2.4.2":
    cargo run --manifest-path src-tauri/Cargo.toml --bin gen-catalog -- {{tag}}
```

- [ ] **Step 3: Run it to produce the real committed snapshot**

Run: `source "$HOME/.cargo/env" && just gen-catalog`
Expected: prints a model/variant count and writes
`src-tauri/resources/printer-catalog.json`. **Correction to this plan's
earlier estimate:** the design spec's planning-time verification (358 models
/ 927 variants) was produced by a throwaway Python prototype that indexed
machine presets by *filename* rather than by their own internal JSON `"name"`
field — 59 files in the real `v2.4.2` tree have a `"name"` that differs from
their filename, so that prototype under-resolved the `inherits` chain for
those presets and undercounted. This task's `index_machine_dir` (Task 3)
indexes by the internal `"name"` field, as specified — the semantically
correct approach — and the true count is **369 models / 971 variants**
(independently re-verified against the same `v2.4.2` clone during Task 4's
review). Expect a count in that neighborhood, not 358/927.

- [ ] **Step 4: Sanity-check the output**

Run: `python3 -c "import json; d=json.load(open('src-tauri/resources/printer-catalog.json')); print(d['sourceTag'], len(d['models']), sum(len(m['variants']) for m in d['models']))"`
Expected: `v2.4.2 369 971` (or close — OrcaSlicer's tag content is fixed, so
this should be stable, but don't hand-edit the file if the count drifts
slightly; re-run `just gen-catalog` instead).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/bin/gen-catalog.rs justfile src-tauri/resources/printer-catalog.json
git commit -m "feat: add gen-catalog binary and commit the generated printer catalog"
```

---

## Task 5: ADR 0007, ADR 0003 correction, snapshot tests, bundle wiring

**Files:**
- Create: `docs/adr/0007-printer-catalog-is-derived-data.md`
- Modify: `docs/adr/0003-wrap-orcaslicer.md` (GPLv3 → AGPL-3.0)
- Modify: `src-tauri/tauri.conf.json` (`bundle.resources`)
- Create: `src-tauri/tests/snapshot.rs`

**Interfaces:**
- Consumes: `farm3d_lib::catalog::Catalog` / `farm3d_lib::catalog::BedShape` (Task 3), the committed `src-tauri/resources/printer-catalog.json` (Task 4)
- Produces: nothing consumed by later tasks — this is documentation + a review gate for future catalog regenerations.

- [ ] **Step 1: Write ADR 0007**

Create `docs/adr/0007-printer-catalog-is-derived-data.md`:
```markdown
# The printer catalog is derived factual data from OrcaSlicer profiles

farm3d ships a printer catalog (`src-tauri/resources/printer-catalog.json`)
generated from OrcaSlicer's `resources/profiles` at a pinned git tag
(`OrcaSlicer/OrcaSlicer`, AGPL-3.0-or-later — see the correction to ADR-0003).
The generator (`src-tauri/src/bin/gen-catalog.rs`) allowlists exactly the
factual capability fields farm3d reads — build volume, nozzle diameters,
capability flags, suggested host type — and never copies g-code templates,
bed/hotend model filenames, or vendor descriptive text. Facts aren't
copyrightable and this extraction is mechanical and exhaustive, not creative,
so the catalog's own license note ships as data alongside the snapshot rather
than farm3d inheriting AGPL obligations for a value it derives, not copies.

This means: adding a new field to the catalog must add it to the generator's
explicit allowlist (`extract_variant` in `catalog/ingest/mod.rs`), never via a
blanket copy of the resolved preset object. `src-tauri/tests/snapshot.rs`
enforces this mechanically by scanning the committed snapshot for known
disallowed field markers.
```

- [ ] **Step 2: Correct ADR 0003's license claim**

In `docs/adr/0003-wrap-orcaslicer.md`, change:
```
OrcaSlicer is GPLv3; keeping it a separate invoked process rather than
```
to:
```
OrcaSlicer is AGPL-3.0 (upstream moved to `OrcaSlicer/OrcaSlicer` on GitHub);
keeping it a separate invoked process rather than
```

- [ ] **Step 3: Wire the snapshot into the app bundle**

In `src-tauri/tauri.conf.json`, add a `resources` array to the `bundle` object:
```json
  "bundle": {
    "active": true,
    "targets": "all",
    "resources": ["resources/printer-catalog.json"],
    "icon": [
```
(insert the `"resources"` line right after `"targets": "all",`, keeping the
existing `"icon"` array unchanged below it).

- [ ] **Step 4: Write the snapshot tests**

Create `src-tauri/tests/snapshot.rs`:
```rust
use farm3d_lib::catalog::{BedShape, Catalog};
use std::path::Path;

fn load_snapshot_raw() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/printer-catalog.json"),
    )
    .expect("resources/printer-catalog.json should be committed")
}

#[test]
fn snapshot_is_complete() {
    let raw = load_snapshot_raw();
    let catalog: Catalog = serde_json::from_str(&raw).expect("snapshot should be valid JSON");
    assert!(!catalog.models.is_empty(), "snapshot has no models");

    for model in &catalog.models {
        assert!(!model.variants.is_empty(), "{} has no variants", model.model);
        for variant in &model.variants {
            assert!(
                variant.printable_height_mm > 0.0,
                "{} has no printable height",
                variant.variant
            );
            match &variant.bed_shape {
                BedShape::Rectangular { width_mm, depth_mm, .. } => {
                    assert!(
                        *width_mm > 0.0 && *depth_mm > 0.0,
                        "{} has a zero-size rectangular bed",
                        variant.variant
                    );
                }
                BedShape::Polygon { points } => {
                    assert!(
                        points.len() >= 3,
                        "{} has a degenerate polygon bed",
                        variant.variant
                    );
                }
            }
        }
    }
}

/// The executable form of ADR 0007's allowlist rule: never let a future field
/// addition smuggle g-code or filenames into the shipped catalog.
#[test]
fn snapshot_carries_no_disallowed_fields() {
    let raw = load_snapshot_raw();
    let lower = raw.to_lowercase();
    let disallowed = [
        "start_gcode",
        "startgcode",
        "end_gcode",
        "endgcode",
        "change_filament_gcode",
        "changefilamentgcode",
        "bed_model",
        "bedmodel",
        "bed_texture",
        "bedtexture",
        "hotend_model",
        "hotendmodel",
    ];
    for marker in disallowed {
        assert!(
            !lower.contains(marker),
            "snapshot contains disallowed field marker: {marker}"
        );
    }
}
```

- [ ] **Step 5: Run the tests**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml --test snapshot`
Expected: both tests pass against the real committed catalog from Task 4.

- [ ] **Step 6: Commit**

```bash
git add docs/adr/0007-printer-catalog-is-derived-data.md docs/adr/0003-wrap-orcaslicer.md src-tauri/tauri.conf.json src-tauri/tests/snapshot.rs
git commit -m "docs: record the derived-catalog ADR, correct OrcaSlicer's license, wire bundle resources"
```

---

## Task 6: Runtime catalog loading + `CatalogState`

**Files:**
- Modify: `src-tauri/src/catalog/mod.rs` (add `load_snapshot`)
- Modify: `src-tauri/src/lib.rs` (wire `CatalogState` in `setup()`)

**Interfaces:**
- Consumes: `Catalog` (Task 3), the committed snapshot file (Task 4)
- Produces: `pub fn load_snapshot(path: &Path) -> Result<Catalog, String>`; a
  `tauri::State<Arc<Catalog>>` available to every command in Task 9.

Unlike `printers.json` (Task 7), a corrupt bundled catalog snapshot is a
**packaging bug**, not a runtime data-integrity concern — there's no user data
to protect and no reasonable fallback. `setup()` panics loudly on a bad
snapshot rather than silently degrading, deliberately the opposite policy from
`printers.json`'s fail-soft quarantine behavior.

- [ ] **Step 1: Write the failing test for `load_snapshot`**

Add to `src-tauri/src/catalog/mod.rs` (imports first, then a `#[cfg(test)]`
module at the bottom of the file):
```rust
use std::fs;
use std::path::{Path, PathBuf};

pub fn load_snapshot(path: &Path) -> Result<Catalog, String> {
    unimplemented!()
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_path() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "farm3d-catalog-test-{}-{}.json",
            std::process::id(),
            id
        ))
    }

    #[test]
    fn loads_a_valid_snapshot() {
        let path = temp_path();
        let catalog = Catalog {
            generated_at: "2026-08-20T00:00:00Z".to_string(),
            source_tag: "v2.4.2".to_string(),
            notice: "test".to_string(),
            models: vec![],
        };
        fs::write(&path, serde_json::to_string(&catalog).unwrap()).unwrap();

        let loaded = load_snapshot(&path).unwrap();
        assert_eq!(loaded, catalog);
        fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_file_is_an_error() {
        let path = temp_path();
        assert!(load_snapshot(&path).is_err());
    }

    #[test]
    fn corrupt_file_is_an_error() {
        let path = temp_path();
        fs::write(&path, "not valid json").unwrap();
        assert!(load_snapshot(&path).is_err());
        fs::remove_file(&path).ok();
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml catalog::snapshot_tests`
Expected: panics from `unimplemented!()`.

- [ ] **Step 3: Implement `load_snapshot`**

```rust
pub fn load_snapshot(path: &Path) -> Result<Catalog, String> {
    let contents = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&contents).map_err(|e| e.to_string())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml catalog::snapshot_tests`
Expected: all 3 tests pass.

- [ ] **Step 5: Wire `CatalogState` into `setup()`**

Replace `src-tauri/src/lib.rs` in full:
```rust
pub mod catalog;
mod settings;

use settings::{load_settings, open_settings_file, save_settings};
use std::sync::Arc;
use tauri::path::BaseDirectory;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let resource_path = app
                .path()
                .resolve("resources/printer-catalog.json", BaseDirectory::Resource)
                .expect("printer-catalog.json should be a bundled resource");
            let snapshot = catalog::load_snapshot(&resource_path)
                .expect("bundled printer-catalog.json should be valid — this is a packaging bug");
            app.manage(Arc::new(snapshot));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            open_settings_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

Later tasks add more commands to `generate_handler!` and more `mod` lines —
this step establishes the pattern each of them follows.

- [ ] **Step 6: Confirm the app still builds and starts**

Run: `source "$HOME/.cargo/env" && cargo build --manifest-path src-tauri/Cargo.toml`
Expected: builds clean. (A full `just dev` launch confirmation happens once
the UI has something to show, in the final verification task.)

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/catalog/mod.rs src-tauri/src/lib.rs
git commit -m "feat: load the bundled printer catalog into managed Tauri state"
```

---

## Task 7: `PrinterProfile` type + `printers.rs` persistence

**Files:**
- Modify: `src-tauri/src/catalog/mod.rs` (add `PrinterProfile` + its `From<&CatalogVariant>`)
- Create: `src-tauri/src/printers.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod printers;`)

**Interfaces:**
- Consumes: `BedShape`, `PointMm`, `CatalogVariant` (Task 3)
- Produces: `StoredPrinter`, `CatalogRef`, `PrinterProfileOverrides`,
  `PrintersFile`, `LastKnownGood`, `catalog::PrinterProfile` — the on-disk and
  in-memory shapes every later task (resolution, commands, frontend types)
  is built against. Also `pub fn load_printers_from(&Path) -> Result<PrintersFile, String>`,
  `pub fn write_printers_to(&Path, &PrintersFile) -> Result<(), String>`,
  `pub const OVERRIDABLE_FIELDS: &[&str]`,
  `pub fn apply_override(&PrinterProfileOverrides, &str, Option<Value>) -> Result<PrinterProfileOverrides, String>`.

- [ ] **Step 1: Add `PrinterProfile` to `catalog/mod.rs`**

Append to `src-tauri/src/catalog/mod.rs`:
```rust
/// The fully-resolved capability set for one Printer instance — every field
/// present, whether inherited from the catalog or overridden. Distinct from
/// `CatalogVariant`, which additionally carries `variant`/`printer_variant`
/// identity fields that describe the catalog entry, not a resolved instance.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PrinterProfile {
    pub bed_shape: BedShape,
    pub printable_height_mm: f64,
    pub bed_exclude_areas: Vec<PointMm>,
    pub default_bed_type: String,
    pub nozzle_diameter_mm: Vec<f64>,
    pub nozzle_type: String,
    pub gcode_flavor: String,
    pub has_auxiliary_fan: bool,
    pub supports_air_filtration: bool,
    pub supports_multi_filament: bool,
    pub suggested_host_type: Option<String>,
}

impl From<&CatalogVariant> for PrinterProfile {
    fn from(v: &CatalogVariant) -> Self {
        Self {
            bed_shape: v.bed_shape.clone(),
            printable_height_mm: v.printable_height_mm,
            bed_exclude_areas: v.bed_exclude_areas.clone(),
            default_bed_type: v.default_bed_type.clone(),
            nozzle_diameter_mm: v.nozzle_diameter_mm.clone(),
            nozzle_type: v.nozzle_type.clone(),
            gcode_flavor: v.gcode_flavor.clone(),
            has_auxiliary_fan: v.has_auxiliary_fan,
            supports_air_filtration: v.supports_air_filtration,
            supports_multi_filament: v.supports_multi_filament,
            suggested_host_type: v.suggested_host_type.clone(),
        }
    }
}
```

- [ ] **Step 2: Write the failing tests**

Create `src-tauri/src/printers.rs`:
```rust
use crate::catalog::{BedShape, PointMm, PrinterProfile};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const PRINTERS_FILE_NAME: &str = "printers.json";
const PRINTERS_SCHEMA_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PrintersFile {
    pub schema_version: u32,
    pub printers: Vec<StoredPrinter>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct StoredPrinter {
    pub id: String,
    pub name: String,
    pub catalog_ref: CatalogRef,
    pub group: String,
    pub notes: String,
    #[serde(skip_serializing_if = "PrinterProfileOverrides::is_empty")]
    pub overrides: PrinterProfileOverrides,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_known_good: Option<LastKnownGood>,
    /// Phase 2's Connection config. Opaque here — printers.rs never
    /// interprets it, only round-trips it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CatalogRef {
    pub vendor: String,
    pub model: String,
    pub variant: String,
    pub model_id: String,
    pub printer_variant: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PrinterProfileOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bed_shape: Option<BedShape>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub printable_height_mm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bed_exclude_areas: Option<Vec<PointMm>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_bed_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_auxiliary_fan: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_air_filtration: Option<bool>,
    /// Unknown/future keys, preserved verbatim through read-modify-write so
    /// an older farm3d never destroys a newer one's data.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl PrinterProfileOverrides {
    pub fn is_empty(&self) -> bool {
        self.bed_shape.is_none()
            && self.printable_height_mm.is_none()
            && self.bed_exclude_areas.is_none()
            && self.default_bed_type.is_none()
            && self.has_auxiliary_fan.is_none()
            && self.supports_air_filtration.is_none()
            && self.extra.is_empty()
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LastKnownGood {
    pub profile: PrinterProfile,
    pub catalog_version: String,
    pub resolved_at: String,
}

pub const OVERRIDABLE_FIELDS: &[&str] = &[
    "bedShape",
    "printableHeightMm",
    "bedExcludeAreas",
    "defaultBedType",
    "hasAuxiliaryFan",
    "supportsAirFiltration",
];

fn printers_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join(PRINTERS_FILE_NAME)
}

pub fn write_printers_to(config_dir: &Path, file: &PrintersFile) -> Result<(), String> {
    unimplemented!()
}

pub fn load_printers_from(config_dir: &Path) -> Result<PrintersFile, String> {
    unimplemented!()
}

pub fn apply_override(
    overrides: &PrinterProfileOverrides,
    field: &str,
    value: Option<serde_json::Value>,
) -> Result<PrinterProfileOverrides, String> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_dir() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("farm3d-printers-test-{}-{}", std::process::id(), id))
    }

    fn a_printer() -> StoredPrinter {
        StoredPrinter {
            id: "prn-1".to_string(),
            name: "Test Printer".to_string(),
            catalog_ref: CatalogRef {
                vendor: "TestVendor".to_string(),
                model: "Test Printer".to_string(),
                variant: "Test Printer 0.4 nozzle".to_string(),
                model_id: "TestVendor-TP".to_string(),
                printer_variant: "0.4".to_string(),
            },
            group: String::new(),
            notes: String::new(),
            overrides: PrinterProfileOverrides::default(),
            last_known_good: None,
            connection: None,
        }
    }

    #[test]
    fn load_creates_empty_file_when_missing() {
        let dir = temp_dir();
        let loaded = load_printers_from(&dir).unwrap();
        assert_eq!(loaded, PrintersFile { schema_version: 1, printers: vec![] });
        assert!(printers_file_path(&dir).exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir();
        let file = PrintersFile { schema_version: 1, printers: vec![a_printer()] };
        write_printers_to(&dir, &file).unwrap();
        let loaded = load_printers_from(&dir).unwrap();
        assert_eq!(loaded, file);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_file_is_quarantined_not_silently_defaulted() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(printers_file_path(&dir), "not valid json").unwrap();

        let result = load_printers_from(&dir);
        assert!(result.is_err(), "a corrupt printers.json must be an error, not a silent default");

        let quarantined: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("printers.json.corrupt-"))
            .collect();
        assert_eq!(quarantined.len(), 1, "corrupt file should be quarantined, not deleted");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn serialized_file_contains_only_overridden_override_keys() {
        let mut printer = a_printer();
        printer.overrides.supports_air_filtration = Some(false);
        let json = serde_json::to_string(&printer).unwrap();
        assert!(json.contains(r#""overrides":{"supportsAirFiltration":false}"#));
        assert!(!json.contains("printableHeightMm"));
        assert!(!json.contains("bedShape"));
    }

    #[test]
    fn unknown_override_keys_survive_a_round_trip() {
        let dir = temp_dir();
        let mut printer = a_printer();
        printer
            .overrides
            .extra
            .insert("printabelHeight".to_string(), serde_json::json!(300));
        let file = PrintersFile { schema_version: 1, printers: vec![printer] };
        write_printers_to(&dir, &file).unwrap();

        let loaded = load_printers_from(&dir).unwrap();
        assert_eq!(
            loaded.printers[0].overrides.extra.get("printabelHeight"),
            Some(&serde_json::json!(300))
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn apply_override_sets_a_field() {
        let overrides = PrinterProfileOverrides::default();
        let updated =
            apply_override(&overrides, "printableHeightMm", Some(serde_json::json!(240.0))).unwrap();
        assert_eq!(updated.printable_height_mm, Some(240.0));
    }

    #[test]
    fn apply_override_with_none_reverts_to_inherited() {
        let mut overrides = PrinterProfileOverrides::default();
        overrides.printable_height_mm = Some(240.0);
        let updated = apply_override(&overrides, "printableHeightMm", None).unwrap();
        assert_eq!(updated.printable_height_mm, None);
    }

    #[test]
    fn apply_override_rejects_a_non_overridable_field() {
        let overrides = PrinterProfileOverrides::default();
        let result = apply_override(&overrides, "nozzleDiameterMm", Some(serde_json::json!([0.6])));
        assert!(result.is_err());
    }

    #[test]
    fn apply_override_with_wrong_type_errors_and_leaves_struct_untouched() {
        let overrides = PrinterProfileOverrides::default();
        let result = apply_override(&overrides, "printableHeightMm", Some(serde_json::json!("not a number")));
        assert!(result.is_err());
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml printers::tests`
Expected: panics from the three `unimplemented!()` bodies.

- [ ] **Step 4: Implement the three functions**

Replace the three `unimplemented!()` bodies in `src-tauri/src/printers.rs`:
```rust
pub fn write_printers_to(config_dir: &Path, file: &PrintersFile) -> Result<(), String> {
    fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    fs::write(printers_file_path(config_dir), json).map_err(|e| e.to_string())
}

pub fn load_printers_from(config_dir: &Path) -> Result<PrintersFile, String> {
    let path = printers_file_path(config_dir);
    if !path.exists() {
        let defaults = PrintersFile {
            schema_version: PRINTERS_SCHEMA_VERSION,
            printers: vec![],
        };
        write_printers_to(config_dir, &defaults)?;
        return Ok(defaults);
    }
    let contents = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    match serde_json::from_str::<PrintersFile>(&contents) {
        Ok(file) => Ok(file),
        Err(e) => {
            // Quarantine, never silently default to empty — unlike settings.json,
            // the next save here would overwrite the user's Farm with `[]`.
            use std::time::{SystemTime, UNIX_EPOCH};
            let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            let quarantine_path = config_dir.join(format!("{PRINTERS_FILE_NAME}.corrupt-{ts}"));
            fs::rename(&path, &quarantine_path).map_err(|e| e.to_string())?;
            Err(format!(
                "printers.json was corrupt ({e}); original preserved at {}",
                quarantine_path.display()
            ))
        }
    }
}

pub fn apply_override(
    overrides: &PrinterProfileOverrides,
    field: &str,
    value: Option<serde_json::Value>,
) -> Result<PrinterProfileOverrides, String> {
    if !OVERRIDABLE_FIELDS.contains(&field) {
        return Err(format!("`{field}` is not an overridable Printer Profile field"));
    }
    let mut map = match serde_json::to_value(overrides).map_err(|e| e.to_string())? {
        serde_json::Value::Object(m) => m,
        _ => unreachable!("PrinterProfileOverrides always serializes to a JSON object"),
    };
    match value {
        Some(v) => {
            map.insert(field.to_string(), v);
        }
        None => {
            map.remove(field);
        }
    }
    serde_json::from_value(serde_json::Value::Object(map))
        .map_err(|e| format!("invalid value for `{field}`: {e}"))
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml printers::tests`
Expected: all 9 tests pass.

- [ ] **Step 6: Wire the module into `lib.rs`**

Add `mod printers;` to `src-tauri/src/lib.rs`, alongside the existing `mod settings;` line.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/catalog/mod.rs src-tauri/src/printers.rs src-tauri/src/lib.rs
git commit -m "feat: add printers.json persistence with quarantine-on-corrupt and typed overrides"
```

---

## Task 8: Override resolution core (`catalog/resolve.rs`)

**Files:**
- Create: `src-tauri/src/catalog/resolve.rs`
- Modify: `src-tauri/src/catalog/mod.rs` (add `pub mod resolve;`)

**Interfaces:**
- Consumes: `Catalog`, `CatalogModel`, `CatalogVariant`, `PrinterProfile`, `BedShape` (Task 3/7); `StoredPrinter`, `CatalogRef`, `PrinterProfileOverrides`, `LastKnownGood` (Task 7)
- Produces: `pub fn resolve_printer(&Catalog, &StoredPrinter) -> ResolvedPrinter`,
  `pub fn resolve_catalog_ref(&Catalog, &CatalogRef) -> (Option<&CatalogVariant>, CatalogStatus)`,
  `pub fn merge_profile(&PrinterProfile, &PrinterProfileOverrides) -> (PrinterProfile, Vec<&'static str>)`,
  `pub struct ResolvedPrinter`, `pub enum CatalogStatus` — consumed by Task 9's commands and mirrored by Task 10's TS types.

This is the highest-value test surface in the whole plan alongside Task 3's
ingestion tests — it's where the "pin identity four ways, resolve in a fixed
cascade" and "never drift silently" mitigations from the spec's Risks section
actually live.

- [ ] **Step 1: Write the failing tests**

Create `src-tauri/src/catalog/resolve.rs`:
```rust
use crate::catalog::{BedShape, Catalog, CatalogModel, CatalogVariant, PrinterProfile};
use crate::printers::{CatalogRef, PrinterProfileOverrides, StoredPrinter};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum CatalogStatus {
    Ok,
    Rematched,
    VariantMissing,
    ModelMissing,
    VendorMissing,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDrift {
    pub field: String,
    pub from: serde_json::Value,
    pub to: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPrinter {
    pub id: String,
    pub name: String,
    pub group: String,
    pub notes: String,
    pub catalog_ref: CatalogRef,
    pub catalog_status: CatalogStatus,
    pub model_label: String,
    pub variant_label: String,
    pub profile: PrinterProfile,
    pub overridden_fields: Vec<String>,
    pub inherited: serde_json::Map<String, serde_json::Value>,
    pub profile_drift: Vec<ProfileDrift>,
    pub unknown_override_keys: Vec<String>,
    pub connection: Option<serde_json::Value>,
}

pub fn resolve_catalog_ref<'a>(
    catalog: &'a Catalog,
    r: &CatalogRef,
) -> (Option<&'a CatalogVariant>, CatalogStatus) {
    unimplemented!()
}

pub fn merge_profile(
    base: &PrinterProfile,
    overrides: &PrinterProfileOverrides,
) -> (PrinterProfile, Vec<&'static str>) {
    unimplemented!()
}

pub fn resolve_printer(catalog: &Catalog, stored: &StoredPrinter) -> ResolvedPrinter {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::PointMm;
    use crate::printers::LastKnownGood;

    fn variant(name: &str, printer_variant: &str, height: f64) -> CatalogVariant {
        CatalogVariant {
            variant: name.to_string(),
            printer_variant: printer_variant.to_string(),
            bed_shape: BedShape::Rectangular {
                width_mm: 256.0, depth_mm: 256.0, origin_x_mm: 0.0, origin_y_mm: 0.0,
            },
            printable_height_mm: height,
            bed_exclude_areas: vec![],
            default_bed_type: "4".to_string(),
            nozzle_diameter_mm: vec![printer_variant.parse().unwrap()],
            nozzle_type: "hardened_steel".to_string(),
            gcode_flavor: "klipper".to_string(),
            has_auxiliary_fan: true,
            supports_air_filtration: true,
            supports_multi_filament: true,
            suggested_host_type: Some("elegoolink".to_string()),
        }
    }

    fn a_catalog() -> Catalog {
        Catalog {
            generated_at: "2026-08-20T00:00:00Z".to_string(),
            source_tag: "v2.4.2".to_string(),
            notice: "test".to_string(),
            models: vec![CatalogModel {
                model_id: "Elegoo-CC".to_string(),
                vendor: "Elegoo".to_string(),
                model: "Elegoo Centauri Carbon".to_string(),
                variants: vec![
                    variant("Elegoo Centauri Carbon 0.4 nozzle", "0.4", 256.0),
                    variant("Elegoo Centauri Carbon 0.2 nozzle", "0.2", 256.0),
                ],
            }],
        }
    }

    fn a_ref() -> CatalogRef {
        CatalogRef {
            vendor: "Elegoo".to_string(),
            model: "Elegoo Centauri Carbon".to_string(),
            variant: "Elegoo Centauri Carbon 0.4 nozzle".to_string(),
            model_id: "Elegoo-CC".to_string(),
            printer_variant: "0.4".to_string(),
        }
    }

    #[test]
    fn exact_match_resolves_ok() {
        let catalog = a_catalog();
        let (v, status) = resolve_catalog_ref(&catalog, &a_ref());
        assert_eq!(status, CatalogStatus::Ok);
        assert_eq!(v.unwrap().variant, "Elegoo Centauri Carbon 0.4 nozzle");
    }

    #[test]
    fn renamed_variant_within_the_same_model_rematches() {
        let catalog = a_catalog();
        let mut r = a_ref();
        r.variant = "Elegoo Centauri Carbon 0.4 nozzle (old name)".to_string();
        let (v, status) = resolve_catalog_ref(&catalog, &r);
        assert_eq!(status, CatalogStatus::Rematched);
        assert_eq!(v.unwrap().printer_variant, "0.4");
    }

    #[test]
    fn missing_variant_in_an_existing_model_is_variant_missing() {
        let catalog = a_catalog();
        let mut r = a_ref();
        r.variant = "does not exist".to_string();
        r.printer_variant = "0.8".to_string();
        let (v, status) = resolve_catalog_ref(&catalog, &r);
        assert_eq!(status, CatalogStatus::VariantMissing);
        assert!(v.is_none());
    }

    #[test]
    fn missing_model_in_an_existing_vendor_is_model_missing() {
        let catalog = a_catalog();
        let mut r = a_ref();
        r.model_id = "Elegoo-DoesNotExist".to_string();
        r.model = "Elegoo Nonexistent".to_string();
        let (v, status) = resolve_catalog_ref(&catalog, &r);
        assert_eq!(status, CatalogStatus::ModelMissing);
        assert!(v.is_none());
    }

    #[test]
    fn missing_vendor_is_vendor_missing() {
        let catalog = a_catalog();
        let mut r = a_ref();
        r.vendor = "NoSuchVendor".to_string();
        let (v, status) = resolve_catalog_ref(&catalog, &r);
        assert_eq!(status, CatalogStatus::VendorMissing);
        assert!(v.is_none());
    }

    #[test]
    fn never_rematches_across_vendors() {
        let mut catalog = a_catalog();
        catalog.models.push(CatalogModel {
            model_id: "Other-CC".to_string(),
            vendor: "OtherVendor".to_string(),
            model: "Other Centauri Carbon".to_string(),
            variants: vec![variant("Other Centauri Carbon 0.4 nozzle", "0.4", 256.0)],
        });
        let mut r = a_ref();
        r.model_id = "does-not-exist-anywhere".to_string();
        // Even though OtherVendor has a same-printerVariant match, vendor differs.
        let (v, status) = resolve_catalog_ref(&catalog, &r);
        assert_eq!(status, CatalogStatus::ModelMissing);
        assert!(v.is_none());
    }

    #[test]
    fn merge_profile_with_no_overrides_returns_the_base_unchanged() {
        let base = PrinterProfile::from(&variant("v", "0.4", 256.0));
        let (effective, overridden) = merge_profile(&base, &PrinterProfileOverrides::default());
        assert_eq!(effective, base);
        assert!(overridden.is_empty());
    }

    #[test]
    fn merge_profile_applies_a_single_override() {
        let base = PrinterProfile::from(&variant("v", "0.4", 256.0));
        let mut overrides = PrinterProfileOverrides::default();
        overrides.printable_height_mm = Some(240.0);
        let (effective, overridden) = merge_profile(&base, &overrides);
        assert_eq!(effective.printable_height_mm, 240.0);
        assert_eq!(overridden, vec!["printableHeightMm"]);
    }

    #[test]
    fn an_override_equal_to_the_inherited_value_still_counts_as_overridden() {
        let base = PrinterProfile::from(&variant("v", "0.4", 256.0));
        let mut overrides = PrinterProfileOverrides::default();
        overrides.printable_height_mm = Some(256.0); // same as base
        let (_, overridden) = merge_profile(&base, &overrides);
        assert_eq!(overridden, vec!["printableHeightMm"]);
    }

    #[test]
    fn resolve_printer_with_no_overrides_reports_the_catalog_profile_directly() {
        let catalog = a_catalog();
        let stored = StoredPrinter {
            id: "prn-1".to_string(),
            name: "My Centauri".to_string(),
            catalog_ref: a_ref(),
            group: String::new(),
            notes: String::new(),
            overrides: PrinterProfileOverrides::default(),
            last_known_good: None,
            connection: None,
        };
        let resolved = resolve_printer(&catalog, &stored);
        assert_eq!(resolved.catalog_status, CatalogStatus::Ok);
        assert_eq!(resolved.profile.printable_height_mm, 256.0);
        assert!(resolved.overridden_fields.is_empty());
        assert_eq!(resolved.model_label, "Elegoo Centauri Carbon");
    }

    #[test]
    fn resolve_printer_with_a_dangling_ref_falls_back_to_last_known_good() {
        let catalog = a_catalog();
        let mut r = a_ref();
        r.model_id = "gone".to_string();
        r.model = "Gone Printer".to_string();
        let fallback_profile = PrinterProfile::from(&variant("v", "0.4", 300.0));
        let stored = StoredPrinter {
            id: "prn-1".to_string(),
            name: "My Centauri".to_string(),
            catalog_ref: r,
            group: String::new(),
            notes: String::new(),
            overrides: PrinterProfileOverrides::default(),
            last_known_good: Some(LastKnownGood {
                profile: fallback_profile.clone(),
                catalog_version: "02.04.00.06".to_string(),
                resolved_at: "2026-08-01T00:00:00Z".to_string(),
            }),
            connection: None,
        };
        let resolved = resolve_printer(&catalog, &stored);
        assert_eq!(resolved.catalog_status, CatalogStatus::ModelMissing);
        assert_eq!(resolved.profile, fallback_profile);
    }

    #[test]
    fn drift_is_reported_only_for_non_overridden_fields() {
        let catalog = a_catalog();
        let mut overrides = PrinterProfileOverrides::default();
        overrides.printable_height_mm = Some(999.0); // user override, must never "drift"
        let stale_profile = PrinterProfile::from(&variant("v", "0.4", 100.0)); // stale height AND stale bed type
        let stored = StoredPrinter {
            id: "prn-1".to_string(),
            name: "My Centauri".to_string(),
            catalog_ref: a_ref(),
            group: String::new(),
            notes: String::new(),
            overrides,
            last_known_good: Some(LastKnownGood {
                profile: stale_profile,
                catalog_version: "02.04.00.05".to_string(),
                resolved_at: "2026-08-01T00:00:00Z".to_string(),
            }),
            connection: None,
        };
        let resolved = resolve_printer(&catalog, &stored);
        // printableHeightMm is overridden, so it must NOT appear as drift even
        // though the stale snapshot's value (100) differs from the catalog's (256).
        assert!(!resolved.profile_drift.iter().any(|d| d.field == "printableHeightMm"));
    }

    #[test]
    fn unknown_override_keys_are_surfaced() {
        let catalog = a_catalog();
        let mut overrides = PrinterProfileOverrides::default();
        overrides.extra.insert("printabelHeight".to_string(), serde_json::json!(300));
        let stored = StoredPrinter {
            id: "prn-1".to_string(),
            name: "My Centauri".to_string(),
            catalog_ref: a_ref(),
            group: String::new(),
            notes: String::new(),
            overrides,
            last_known_good: None,
            connection: None,
        };
        let resolved = resolve_printer(&catalog, &stored);
        assert_eq!(resolved.unknown_override_keys, vec!["printabelHeight".to_string()]);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml catalog::resolve::tests`
Expected: panics from `unimplemented!()`.

- [ ] **Step 3: Implement `resolve_catalog_ref` and `merge_profile`**

Replace the two `unimplemented!()` bodies:
```rust
pub fn resolve_catalog_ref<'a>(
    catalog: &'a Catalog,
    r: &CatalogRef,
) -> (Option<&'a CatalogVariant>, CatalogStatus) {
    if let Some(model) = catalog
        .models
        .iter()
        .find(|m| m.vendor == r.vendor && m.model_id == r.model_id)
    {
        if let Some(variant) = model.variants.iter().find(|v| v.variant == r.variant) {
            return (Some(variant), CatalogStatus::Ok);
        }
        if let Some(variant) = model
            .variants
            .iter()
            .find(|v| v.printer_variant == r.printer_variant)
        {
            return (Some(variant), CatalogStatus::Rematched);
        }
        return (None, CatalogStatus::VariantMissing);
    }

    if let Some(model) = catalog
        .models
        .iter()
        .find(|m| m.vendor == r.vendor && normalize(&m.model) == normalize(&r.model))
    {
        if let Some(variant) = model
            .variants
            .iter()
            .find(|v| v.printer_variant == r.printer_variant)
        {
            return (Some(variant), CatalogStatus::Rematched);
        }
        return (None, CatalogStatus::VariantMissing);
    }

    if catalog.models.iter().any(|m| m.vendor == r.vendor) {
        return (None, CatalogStatus::ModelMissing);
    }
    (None, CatalogStatus::VendorMissing)
}

fn normalize(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

pub fn merge_profile(
    base: &PrinterProfile,
    overrides: &PrinterProfileOverrides,
) -> (PrinterProfile, Vec<&'static str>) {
    let mut effective = base.clone();
    let mut overridden = Vec::new();

    if let Some(v) = &overrides.bed_shape {
        effective.bed_shape = v.clone();
        overridden.push("bedShape");
    }
    if let Some(v) = overrides.printable_height_mm {
        effective.printable_height_mm = v;
        overridden.push("printableHeightMm");
    }
    if let Some(v) = &overrides.bed_exclude_areas {
        effective.bed_exclude_areas = v.clone();
        overridden.push("bedExcludeAreas");
    }
    if let Some(v) = &overrides.default_bed_type {
        effective.default_bed_type = v.clone();
        overridden.push("defaultBedType");
    }
    if let Some(v) = overrides.has_auxiliary_fan {
        effective.has_auxiliary_fan = v;
        overridden.push("hasAuxiliaryFan");
    }
    if let Some(v) = overrides.supports_air_filtration {
        effective.supports_air_filtration = v;
        overridden.push("supportsAirFiltration");
    }

    (effective, overridden)
}
```

- [ ] **Step 4: Implement `resolve_printer`**

```rust
pub fn resolve_printer(catalog: &Catalog, stored: &StoredPrinter) -> ResolvedPrinter {
    let (variant, status) = resolve_catalog_ref(catalog, &stored.catalog_ref);

    let (base_profile, model_label, variant_label) = match variant {
        Some(v) => {
            let model_label = catalog
                .models
                .iter()
                .find(|m| m.variants.iter().any(|mv| mv.variant == v.variant))
                .map(|m| m.model.clone())
                .unwrap_or_else(|| stored.catalog_ref.model.clone());
            (PrinterProfile::from(v), model_label, v.variant.clone())
        }
        None => {
            let fallback = stored
                .last_known_good
                .as_ref()
                .map(|lkg| lkg.profile.clone())
                .unwrap_or_else(empty_profile);
            (
                fallback,
                stored.catalog_ref.model.clone(),
                stored.catalog_ref.variant.clone(),
            )
        }
    };

    let (profile, overridden) = merge_profile(&base_profile, &stored.overrides);
    let inherited = inherited_subset(&base_profile, &overridden);

    let profile_drift = stored
        .last_known_good
        .as_ref()
        .map(|lkg| diff_profile(&lkg.profile, &base_profile, &overridden))
        .unwrap_or_default();

    ResolvedPrinter {
        id: stored.id.clone(),
        name: stored.name.clone(),
        group: stored.group.clone(),
        notes: stored.notes.clone(),
        catalog_ref: stored.catalog_ref.clone(),
        catalog_status: status,
        model_label,
        variant_label,
        profile,
        overridden_fields: overridden.iter().map(|s| s.to_string()).collect(),
        inherited,
        profile_drift,
        unknown_override_keys: stored.overrides.extra.keys().cloned().collect(),
        connection: stored.connection.clone(),
    }
}

fn empty_profile() -> PrinterProfile {
    PrinterProfile {
        bed_shape: BedShape::Rectangular {
            width_mm: 0.0,
            depth_mm: 0.0,
            origin_x_mm: 0.0,
            origin_y_mm: 0.0,
        },
        printable_height_mm: 0.0,
        bed_exclude_areas: vec![],
        default_bed_type: String::new(),
        nozzle_diameter_mm: vec![],
        nozzle_type: String::new(),
        gcode_flavor: String::new(),
        has_auxiliary_fan: false,
        supports_air_filtration: false,
        supports_multi_filament: false,
        suggested_host_type: None,
    }
}

fn inherited_subset(
    base: &PrinterProfile,
    overridden_fields: &[&'static str],
) -> serde_json::Map<String, serde_json::Value> {
    let mut subset = serde_json::Map::new();
    if let serde_json::Value::Object(map) = serde_json::to_value(base).unwrap() {
        for field in overridden_fields {
            if let Some(v) = map.get(*field) {
                subset.insert(field.to_string(), v.clone());
            }
        }
    }
    subset
}

/// Compares the catalog's current values against the last user-confirmed
/// snapshot for fields the instance does NOT override — an overridden field
/// can't drift, since the user's value wins regardless of what the catalog says.
fn diff_profile(
    last: &PrinterProfile,
    current: &PrinterProfile,
    overridden: &[&'static str],
) -> Vec<ProfileDrift> {
    let last_json = serde_json::to_value(last).unwrap();
    let current_json = serde_json::to_value(current).unwrap();
    let (serde_json::Value::Object(last_map), serde_json::Value::Object(current_map)) =
        (&last_json, &current_json)
    else {
        return vec![];
    };
    let mut drift = vec![];
    for (key, current_value) in current_map {
        if overridden.contains(&key.as_str()) {
            continue;
        }
        if let Some(last_value) = last_map.get(key) {
            if last_value != current_value {
                drift.push(ProfileDrift {
                    field: key.clone(),
                    from: last_value.clone(),
                    to: current_value.clone(),
                });
            }
        }
    }
    drift
}
```

Add `pub mod resolve;` to `src-tauri/src/catalog/mod.rs`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml catalog::resolve::tests`
Expected: all 14 tests pass.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/catalog/resolve.rs src-tauri/src/catalog/mod.rs
git commit -m "feat: add override resolution with dangling-ref fallback and drift detection"
```

---

## Task 9: Tauri commands + `invoke_handler!` registration

**Files:**
- Create: `src-tauri/src/catalog/commands.rs`
- Modify: `src-tauri/src/printers.rs` (append the printer commands + a
  dependency-free RFC3339 timestamp helper)
- Modify: `src-tauri/src/catalog/mod.rs` (add `pub mod commands;`)
- Modify: `src-tauri/src/lib.rs` (register every new command)

**Interfaces:**
- Consumes: everything from Tasks 6–8
- Produces: the full Tauri command surface — `list_printers`, `create_printer`,
  `update_printer`, `delete_printer`, `set_printer_override`, `rebind_printer`,
  `resolve_profile_drift`, `open_printers_file`, `list_catalog_models`,
  `list_catalog_variants`, `preview_profile`, `catalog_info` — the exact
  `invoke()` names Task 11's frontend store calls by string.

These are thin wrappers, matching `settings.rs`'s existing commands — the
logic they call (`load_printers_from`, `apply_override`, `resolve_printer`,
...) is already unit-tested in Tasks 7–8. Consistent with that file's own
testing scope, these commands aren't unit-tested directly (a `tauri::AppHandle`
isn't practical to construct in a `#[test]`); they're verified by a successful
build here and end-to-end once the UI exists (Task 15's verification pass).

- [ ] **Step 1: Add an RFC3339 timestamp helper to `printers.rs`**

Append to `src-tauri/src/printers.rs`, before the `#[cfg(test)]` module:
```rust
/// A dependency-free RFC3339 UTC timestamp for `LastKnownGood.resolved_at` —
/// avoids adding a date/time crate for one field. Runs at runtime (unlike
/// gen-catalog's shell-out to `date`, which is fine for a dev-only tool but
/// wouldn't be portable inside the shipped app).
pub fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    rfc3339_from_unix_seconds(secs)
}

fn rfc3339_from_unix_seconds(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Howard Hinnant's days-from-civil algorithm, inverted (public domain) —
/// converts a day count since the Unix epoch into a (year, month, day) civil
/// date without a date/time crate dependency.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod rfc3339_tests {
    use super::*;

    #[test]
    fn epoch_zero_is_the_unix_epoch_date() {
        assert_eq!(rfc3339_from_unix_seconds(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn a_known_timestamp_round_trips_correctly() {
        // 946684800 is the well-known Unix timestamp for 2000-01-01T00:00:00Z.
        assert_eq!(rfc3339_from_unix_seconds(946_684_800), "2000-01-01T00:00:00Z");
    }
}
```

- [ ] **Step 2: Run the timestamp tests**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml printers::rfc3339_tests`
Expected: both tests pass immediately (this step has no red phase — the
implementation is written directly, matching how `gen-catalog`'s
orchestration code in Task 4 was verified by running it rather than by a
separate red/green cycle).

- [ ] **Step 3: Add the printer commands**

Append to `src-tauri/src/printers.rs`, above the `#[cfg(test)]` module:
```rust
use crate::catalog::resolve::{resolve_catalog_ref, resolve_printer, ResolvedPrinter};
use crate::catalog::Catalog;
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterDraft {
    pub name: String,
    pub catalog_ref: CatalogRef,
    pub group: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterPatch {
    pub name: Option<String>,
    pub group: Option<String>,
    pub notes: Option<String>,
}

fn app_config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map_err(|e| e.to_string())
}

fn find_printer_mut<'a>(
    file: &'a mut PrintersFile,
    id: &str,
) -> Result<&'a mut StoredPrinter, String> {
    file.printers
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("no printer with id {id:?}"))
}

fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    format!("prn-{:x}", nanos & 0xFFFF_FFFF)
}

#[tauri::command]
pub fn list_printers(
    app: AppHandle,
    catalog: tauri::State<Arc<Catalog>>,
) -> Result<Vec<ResolvedPrinter>, String> {
    let file = load_printers_from(&app_config_dir(&app)?)?;
    Ok(file.printers.iter().map(|p| resolve_printer(&catalog, p)).collect())
}

#[tauri::command]
pub fn create_printer(
    app: AppHandle,
    catalog: tauri::State<Arc<Catalog>>,
    draft: PrinterDraft,
) -> Result<ResolvedPrinter, String> {
    let config_dir = app_config_dir(&app)?;
    let mut file = load_printers_from(&config_dir)?;

    let (variant, status) = resolve_catalog_ref(&catalog, &draft.catalog_ref);
    let variant = variant.ok_or_else(|| {
        format!("cannot add a printer for an unresolvable catalog reference (status: {status:?})")
    })?;

    let stored = StoredPrinter {
        id: generate_id(),
        name: draft.name,
        catalog_ref: draft.catalog_ref,
        group: draft.group.unwrap_or_default(),
        notes: String::new(),
        overrides: PrinterProfileOverrides::default(),
        last_known_good: Some(LastKnownGood {
            profile: PrinterProfile::from(variant),
            catalog_version: catalog.source_tag.clone(),
            resolved_at: now_rfc3339(),
        }),
        connection: None,
    };

    let resolved = resolve_printer(&catalog, &stored);
    file.printers.push(stored);
    write_printers_to(&config_dir, &file)?;
    Ok(resolved)
}

#[tauri::command]
pub fn update_printer(
    app: AppHandle,
    catalog: tauri::State<Arc<Catalog>>,
    id: String,
    patch: PrinterPatch,
) -> Result<ResolvedPrinter, String> {
    let config_dir = app_config_dir(&app)?;
    let mut file = load_printers_from(&config_dir)?;
    {
        let stored = find_printer_mut(&mut file, &id)?;
        if let Some(name) = patch.name {
            stored.name = name;
        }
        if let Some(group) = patch.group {
            stored.group = group;
        }
        if let Some(notes) = patch.notes {
            stored.notes = notes;
        }
    }
    write_printers_to(&config_dir, &file)?;
    Ok(resolve_printer(&catalog, file.printers.iter().find(|p| p.id == id).unwrap()))
}

#[tauri::command]
pub fn delete_printer(app: AppHandle, id: String) -> Result<(), String> {
    let config_dir = app_config_dir(&app)?;
    let mut file = load_printers_from(&config_dir)?;
    file.printers.retain(|p| p.id != id);
    write_printers_to(&config_dir, &file)
}

#[tauri::command]
pub fn set_printer_override(
    app: AppHandle,
    catalog: tauri::State<Arc<Catalog>>,
    id: String,
    field: String,
    value: Option<serde_json::Value>,
) -> Result<ResolvedPrinter, String> {
    let config_dir = app_config_dir(&app)?;
    let mut file = load_printers_from(&config_dir)?;
    {
        let stored = find_printer_mut(&mut file, &id)?;
        stored.overrides = apply_override(&stored.overrides, &field, value)?;
    }
    write_printers_to(&config_dir, &file)?;
    Ok(resolve_printer(&catalog, file.printers.iter().find(|p| p.id == id).unwrap()))
}

#[tauri::command]
pub fn rebind_printer(
    app: AppHandle,
    catalog: tauri::State<Arc<Catalog>>,
    id: String,
    catalog_ref: CatalogRef,
) -> Result<ResolvedPrinter, String> {
    let config_dir = app_config_dir(&app)?;
    let mut file = load_printers_from(&config_dir)?;
    {
        let stored = find_printer_mut(&mut file, &id)?;
        stored.catalog_ref = catalog_ref;
        let (variant, status) = resolve_catalog_ref(&catalog, &stored.catalog_ref);
        let variant = variant.ok_or_else(|| {
            format!("cannot rebind to an unresolvable catalog reference (status: {status:?})")
        })?;
        // Rebinding invalidates any stale drift baseline — re-baseline immediately.
        stored.last_known_good = Some(LastKnownGood {
            profile: PrinterProfile::from(variant),
            catalog_version: catalog.source_tag.clone(),
            resolved_at: now_rfc3339(),
        });
    }
    write_printers_to(&config_dir, &file)?;
    Ok(resolve_printer(&catalog, file.printers.iter().find(|p| p.id == id).unwrap()))
}

#[tauri::command]
pub fn resolve_profile_drift(
    app: AppHandle,
    catalog: tauri::State<Arc<Catalog>>,
    id: String,
    action: String,
) -> Result<ResolvedPrinter, String> {
    let config_dir = app_config_dir(&app)?;
    let mut file = load_printers_from(&config_dir)?;
    {
        let stored_ref = find_printer_mut(&mut file, &id)?;
        let resolved = resolve_printer(&catalog, stored_ref);
        match action.as_str() {
            "accept" => {
                stored_ref.last_known_good = Some(LastKnownGood {
                    profile: resolved.profile.clone(),
                    catalog_version: catalog.source_tag.clone(),
                    resolved_at: now_rfc3339(),
                });
            }
            "pin" => {
                for drift in &resolved.profile_drift {
                    // Pin to the OLD (last-known-good) value — "keep my value".
                    stored_ref.overrides =
                        apply_override(&stored_ref.overrides, &drift.field, Some(drift.from.clone()))?;
                }
            }
            other => return Err(format!("unknown drift action: {other:?}")),
        }
    }
    write_printers_to(&config_dir, &file)?;
    Ok(resolve_printer(&catalog, file.printers.iter().find(|p| p.id == id).unwrap()))
}

#[tauri::command]
pub fn open_printers_file(app: AppHandle) -> Result<(), String> {
    let config_dir = app_config_dir(&app)?;
    let path = printers_file_path(&config_dir);
    if !path.exists() {
        write_printers_to(
            &config_dir,
            &PrintersFile { schema_version: PRINTERS_SCHEMA_VERSION, printers: vec![] },
        )?;
    }
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}
```

- [ ] **Step 4: Add the read-only catalog commands**

Create `src-tauri/src/catalog/commands.rs`:
```rust
use crate::catalog::resolve::resolve_catalog_ref;
use crate::catalog::{Catalog, PrinterProfile};
use crate::printers::CatalogRef;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModelSummary {
    pub model_id: String,
    pub vendor: String,
    pub model: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CatalogVariantSummary {
    pub variant: String,
    pub printer_variant: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CatalogInfo {
    pub generated_at: String,
    pub source_tag: String,
    pub model_count: usize,
    pub variant_count: usize,
}

#[tauri::command]
pub fn list_catalog_models(catalog: State<Arc<Catalog>>) -> Vec<CatalogModelSummary> {
    catalog
        .models
        .iter()
        .map(|m| CatalogModelSummary {
            model_id: m.model_id.clone(),
            vendor: m.vendor.clone(),
            model: m.model.clone(),
        })
        .collect()
}

#[tauri::command]
pub fn list_catalog_variants(
    catalog: State<Arc<Catalog>>,
    model_id: String,
) -> Vec<CatalogVariantSummary> {
    catalog
        .models
        .iter()
        .find(|m| m.model_id == model_id)
        .map(|m| {
            m.variants
                .iter()
                .map(|v| CatalogVariantSummary {
                    variant: v.variant.clone(),
                    printer_variant: v.printer_variant.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tauri::command]
pub fn preview_profile(
    catalog: State<Arc<Catalog>>,
    catalog_ref: CatalogRef,
) -> Result<PrinterProfile, String> {
    let (variant, status) = resolve_catalog_ref(&catalog, &catalog_ref);
    variant.map(PrinterProfile::from).ok_or_else(|| {
        format!("cannot preview an unresolvable catalog reference (status: {status:?})")
    })
}

#[tauri::command]
pub fn catalog_info(catalog: State<Arc<Catalog>>) -> CatalogInfo {
    CatalogInfo {
        generated_at: catalog.generated_at.clone(),
        source_tag: catalog.source_tag.clone(),
        model_count: catalog.models.len(),
        variant_count: catalog.models.iter().map(|m| m.variants.len()).sum(),
    }
}
```

Add `pub mod commands;` to `src-tauri/src/catalog/mod.rs`.

- [ ] **Step 5: Register every command in `lib.rs`**

Replace `src-tauri/src/lib.rs` in full:
```rust
pub mod catalog;
mod printers;
mod settings;

use catalog::commands::{catalog_info, list_catalog_models, list_catalog_variants, preview_profile};
use printers::{
    create_printer, delete_printer, list_printers, open_printers_file, rebind_printer,
    resolve_profile_drift, set_printer_override, update_printer,
};
use settings::{load_settings, open_settings_file, save_settings};
use std::sync::Arc;
use tauri::path::BaseDirectory;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let resource_path = app
                .path()
                .resolve("resources/printer-catalog.json", BaseDirectory::Resource)
                .expect("printer-catalog.json should be a bundled resource");
            let snapshot = catalog::load_snapshot(&resource_path)
                .expect("bundled printer-catalog.json should be valid — this is a packaging bug");
            app.manage(Arc::new(snapshot));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            open_settings_file,
            list_printers,
            create_printer,
            update_printer,
            delete_printer,
            set_printer_override,
            rebind_printer,
            resolve_profile_drift,
            open_printers_file,
            list_catalog_models,
            list_catalog_variants,
            preview_profile,
            catalog_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 6: Confirm the whole crate builds and all tests still pass**

Run: `source "$HOME/.cargo/env" && cargo build --manifest-path src-tauri/Cargo.toml && cargo test --manifest-path src-tauri/Cargo.toml`
Expected: builds clean; every test from Tasks 1–9 (shape, inherits, ingest,
snapshot, printers, resolve, rfc3339 — around 45 tests total) passes.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/printers.rs src-tauri/src/catalog/commands.rs src-tauri/src/catalog/mod.rs src-tauri/src/lib.rs
git commit -m "feat: add the full printer and catalog Tauri command surface"
```

---

## Task 10: `NumberField` design-system component

**Files:**
- Create: `src/design-system/components/NumberField.tsx`
- Create: `src/design-system/components/NumberField.module.css`
- Modify: `src/design-system/components/index.ts`
- Modify: `src/design-system/components/components.test.tsx`
- Modify: `src/design-system/Showcase.tsx`

**Interfaces:**
- Consumes: `@kobalte/core/number-field` (verified at
  `node_modules/@kobalte/core/src/number-field/number-field-root.tsx` — the
  numeric API is `rawValue`/`onRawValueChange`, **not** `value`/`onChange`,
  which are the *formatted string* API)
- Produces: `export function NumberField(props: NumberFieldProps)`,
  `export interface NumberFieldProps { label?, "aria-label"?, value?: number, defaultValue?: number, onChange?: (n: number) => void, minValue?, maxValue?, step?, suffix?, disabled?, error?, class? }`
  — used by Task 16's `PrinterProfilePanel` for every numeric profile field,
  which needs `"aria-label"` since its `NumberField`s sit inside `Field` rows
  that already render their own visible label.

- [ ] **Step 1: Write the component**

Create `src/design-system/components/NumberField.tsx`:
```tsx
import { NumberField as KNumberField } from "@kobalte/core/number-field";
import { splitProps } from "solid-js";
import styles from "./NumberField.module.css";

export interface NumberFieldProps {
  label?: string;
  /** An accessible name for the input when no visible `label` is rendered —
   *  e.g. when this NumberField sits inside a `Field` row that already shows
   *  its own visible label text. Kobalte's form-control primitives read
   *  `aria-label` from the *Input* subcomponent specifically, not the Root
   *  (verified at `node_modules/@kobalte/core/src/form-control/create-form-control-field.tsx`),
   *  so it's forwarded there rather than spread onto the root element. */
  "aria-label"?: string;
  value?: number;
  defaultValue?: number;
  onChange?: (value: number) => void;
  minValue?: number;
  maxValue?: number;
  step?: number;
  suffix?: string;
  disabled?: boolean;
  error?: string;
  class?: string;
}

export function NumberField(props: NumberFieldProps) {
  const [local, rest] = splitProps(props, [
    "label",
    "aria-label",
    "value",
    "defaultValue",
    "onChange",
    "suffix",
    "error",
    "class",
  ]);

  return (
    <KNumberField
      class={[styles.root, local.class].filter(Boolean).join(" ")}
      rawValue={local.value}
      defaultValue={local.defaultValue}
      onRawValueChange={local.onChange}
      validationState={local.error ? "invalid" : "valid"}
      {...rest}
    >
      {local.label && (
        <KNumberField.Label class={styles.label}>{local.label}</KNumberField.Label>
      )}
      <div class={styles.inputRow}>
        <KNumberField.Input class={styles.input} aria-label={local["aria-label"]} />
        {local.suffix && <span class={styles.suffix}>{local.suffix}</span>}
        <div class={styles.spinner}>
          <KNumberField.IncrementTrigger class={styles.spinButton} aria-label="Increment">
            <ChevronIcon direction="up" />
          </KNumberField.IncrementTrigger>
          <KNumberField.DecrementTrigger class={styles.spinButton} aria-label="Decrement">
            <ChevronIcon direction="down" />
          </KNumberField.DecrementTrigger>
        </div>
      </div>
      {local.error && (
        <KNumberField.ErrorMessage class={styles.errorMessage}>
          {local.error}
        </KNumberField.ErrorMessage>
      )}
    </KNumberField>
  );
}

function ChevronIcon(props: { direction: "up" | "down" }) {
  const d = props.direction === "up" ? "M2 6L5 3L8 6" : "M2 4L5 7L8 4";
  return (
    <svg width="8" height="8" viewBox="0 0 10 10" fill="none" aria-hidden="true">
      <path d={d} stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
    </svg>
  );
}
```

Create `src/design-system/components/NumberField.module.css`:
```css
.root {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
  min-width: 0;
}

.label {
  font-family: var(--f3d-type-label-font);
  font-size: var(--f3d-type-label-size);
  font-weight: var(--f3d-type-label-weight);
  letter-spacing: var(--f3d-type-label-tracking);
  color: var(--f3d-color-text-muted);
}

.inputRow {
  display: inline-flex;
  align-items: stretch;
  min-width: 0;
  border-radius: var(--f3d-radius-sm);
  border: 1px solid var(--f3d-color-border-strong);
  background-color: var(--f3d-color-surface-raised);
  overflow: hidden;
}

.inputRow:focus-within {
  outline: 2px solid var(--f3d-color-accent);
  outline-offset: -1px;
}

.root[data-invalid] .inputRow {
  border-color: var(--f3d-color-danger);
}

.input {
  flex: 1;
  min-width: 0;
  height: 1.75rem;
  padding: 0 0.5rem;
  border: none;
  background: none;
  color: var(--f3d-color-text);
  font-family: var(--f3d-type-body-font);
  font-size: var(--f3d-type-body-size);
}

.input:focus {
  outline: none;
}

.input[data-disabled] {
  cursor: not-allowed;
  opacity: 0.5;
}

.suffix {
  display: inline-flex;
  align-items: center;
  padding: 0 0.375rem;
  color: var(--f3d-color-text-muted);
  font-family: var(--f3d-type-body-small-font);
  font-size: var(--f3d-type-body-small-size);
}

.spinner {
  display: flex;
  flex-direction: column;
  border-left: 1px solid var(--f3d-color-border);
}

.spinButton {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 1.25rem;
  height: 0.875rem;
  background: none;
  border: none;
  color: var(--f3d-color-text-muted);
  cursor: pointer;
}

.spinButton:hover {
  background-color: var(--f3d-color-surface-hover);
  color: var(--f3d-color-text);
}

.spinButton[data-disabled] {
  cursor: not-allowed;
  opacity: 0.4;
}

.errorMessage {
  font-family: var(--f3d-type-body-small-font);
  font-size: var(--f3d-type-body-small-size);
  color: var(--f3d-color-danger);
}
```

- [ ] **Step 2: Register it in `index.ts`**

Add to `src/design-system/components/index.ts`:
```ts
export { NumberField, type NumberFieldProps } from "./NumberField";
```

- [ ] **Step 3: Write the failing component test**

Add to `src/design-system/components/components.test.tsx` (a new `describe`
block, following the file's existing pattern):
```tsx
describe("NumberField", () => {
  it("calls onChange with the incremented raw value when the increment trigger is clicked", async () => {
    const onChange = vi.fn();
    render(() => <NumberField label="Height" value={10} step={1} onChange={onChange} />);
    const increment = screen.getByLabelText("Increment");
    await fireEvent.click(increment);
    expect(onChange).toHaveBeenCalledWith(11);
  });

  it("does not exceed maxValue", async () => {
    const onChange = vi.fn();
    render(() => (
      <NumberField label="Height" value={10} maxValue={10} step={1} onChange={onChange} />
    ));
    const increment = screen.getByLabelText("Increment");
    await fireEvent.click(increment);
    expect(onChange).not.toHaveBeenCalledWith(11);
  });

  it("renders the suffix text", () => {
    render(() => <NumberField label="Height" value={10} suffix="mm" />);
    expect(screen.getByText("mm")).toBeInTheDocument();
  });
});
```

Add `NumberField` to the file's existing import line from `"../../design-system"`
or `"./index"` (match whatever the file's current import already uses for
`TextField`/`Select`/etc.).

- [ ] **Step 4: Run the test to verify it fails**

Run: `npx vitest run src/design-system/components/components.test.tsx -t NumberField`
Expected: fails with "NumberField is not defined" (import not yet resolvable
— this is expected to go green immediately once Step 1's file exists and is
imported; the "red" phase here is about catching a typo in the test/import,
not missing logic, since the component itself is already fully written in
Step 1).

- [ ] **Step 5: Run the test to verify it passes**

Run: `npx vitest run src/design-system/components/components.test.tsx -t NumberField`
Expected: all 3 tests pass. **If the increment-trigger click doesn't fire**
(Kobalte's spin-button primitive may use pointer events for press-and-hold
repeat rather than plain `click`), switch the test to
`fireEvent.pointerDown(increment, { pointerType: "mouse", button: 0 })` +
`fireEvent.pointerUp(increment, { button: 0 })`, matching the
`AGENTS.md`-documented convention for other Kobalte trigger primitives —
confirm which is correct by running the test, don't guess.

- [ ] **Step 6: Add it to the Showcase**

Add a new `<Panel title="NumberField">` block to `src/design-system/Showcase.tsx`,
following the file's existing `<Panel title="...">` pattern (see the `Select`/
`Slider` panels for the shape): a controlled example with a `createSignal`,
`label`, `suffix="mm"`, `minValue={0}`, `maxValue={500}`, `step={1}`.

- [ ] **Step 7: Run the full frontend build and test suite**

Run: `npm run build && npm test`
Expected: both pass.

- [ ] **Step 8: Commit**

```bash
git add src/design-system/components/NumberField.tsx src/design-system/components/NumberField.module.css src/design-system/components/index.ts src/design-system/components/components.test.tsx src/design-system/Showcase.tsx
git commit -m "feat: add the NumberField design-system component"
```

---

## Task 11: `Combobox` design-system component

**Files:**
- Create: `src/design-system/components/Combobox.tsx`
- Create: `src/design-system/components/Combobox.module.css`
- Modify: `src/design-system/components/index.ts`
- Modify: `src/design-system/components/components.test.tsx`
- Modify: `src/design-system/Showcase.tsx`

**Interfaces:**
- Consumes: `@kobalte/core/combobox` (verified at
  `node_modules/@kobalte/core/src/combobox/combobox-base.tsx` — filtering is
  **not** automatic; the consumer must narrow `options`/`groups` itself via
  `onInputChange`, which this component only forwards)
- Produces: `export function Combobox<T>(props: ComboboxProps<T>)`,
  `export interface ComboboxProps<T> { label?, options?: T[], groups?: ComboboxGroup<T>[], value?, onChange?, onInputChange?, optionValue?, optionLabel?, placeholder?, disabled?, class? }`
  — used by Task 15's `PrinterAddDialog` to search the ~370-model catalog.

- [ ] **Step 1: Write the component**

Create `src/design-system/components/Combobox.tsx`:
```tsx
import { Combobox as KCombobox } from "@kobalte/core/combobox";
import styles from "./Combobox.module.css";

export interface ComboboxGroup<T> {
  label: string;
  options: T[];
}

export interface ComboboxProps<T> {
  label?: string;
  /** Flat option list. Ignored if `groups` is passed. */
  options?: T[];
  /** Grouped option list, rendered under section headers. */
  groups?: ComboboxGroup<T>[];
  value?: T;
  onChange?: (value: T) => void;
  /** Fires as the user types — filtering `options`/`groups` is the caller's job. */
  onInputChange?: (query: string) => void;
  /** Defaults to the option itself (for T = string). */
  optionValue?: (option: T) => string;
  /** Defaults to the option itself (for T = string). */
  optionLabel?: (option: T) => string;
  placeholder?: string;
  disabled?: boolean;
  class?: string;
}

export function Combobox<T>(props: ComboboxProps<T>) {
  const toLabel = (option: T) =>
    props.optionLabel ? props.optionLabel(option) : String(option);
  const toValue = (option: T) =>
    props.optionValue ? props.optionValue(option) : String(option);

  return (
    <KCombobox
      class={[styles.root, props.class].filter(Boolean).join(" ")}
      options={(props.groups ?? props.options ?? []) as never[]}
      optionGroupChildren={props.groups ? ("options" as never) : undefined}
      optionValue={props.optionValue ? ((o: T) => toValue(o)) as never : undefined}
      optionTextValue={((o: T) => toLabel(o)) as never}
      optionLabel={((o: T) => toLabel(o)) as never}
      value={props.value as never}
      onChange={(v) => v !== null && props.onChange?.(v as T)}
      onInputChange={props.onInputChange}
      placeholder={props.placeholder}
      disabled={props.disabled}
      itemComponent={(itemProps) => (
        <KCombobox.Item item={itemProps.item} class={styles.item}>
          <KCombobox.ItemLabel>{toLabel(itemProps.item.rawValue as T)}</KCombobox.ItemLabel>
        </KCombobox.Item>
      )}
      sectionComponent={(sectionProps) => (
        <KCombobox.Section class={styles.section}>
          {(sectionProps.section.rawValue as ComboboxGroup<T>).label}
        </KCombobox.Section>
      )}
    >
      {props.label && <KCombobox.Label class={styles.label}>{props.label}</KCombobox.Label>}
      <KCombobox.Control class={styles.control}>
        <KCombobox.Input class={styles.input} />
        <KCombobox.Trigger class={styles.trigger}>
          <KCombobox.Icon>
            <ChevronIcon />
          </KCombobox.Icon>
        </KCombobox.Trigger>
      </KCombobox.Control>
      <KCombobox.Portal>
        <KCombobox.Content class={styles.content}>
          <KCombobox.Listbox class={styles.listbox} />
        </KCombobox.Content>
      </KCombobox.Portal>
    </KCombobox>
  );
}

function ChevronIcon() {
  return (
    <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
      <path
        d="M2 4L5 7L8 4"
        stroke="currentColor"
        stroke-width="1.5"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}
```

The `as never` casts sidestep Kobalte's `<Option, OptGroup>` dual-generic
signature, which doesn't infer cleanly through a wrapper this generic — the
same kind of light coercion `Select.tsx` already does for its `optionValue`
prop. If `npm run build`'s `tsc` pass (Step 5) surfaces a real type error
here rather than just an inference gap, fix the specific mismatch rather than
widening the casts further.

Create `src/design-system/components/Combobox.module.css`:
```css
.root {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
}

.label {
  font-family: var(--f3d-type-label-font);
  font-size: var(--f3d-type-label-size);
  font-weight: var(--f3d-type-label-weight);
  letter-spacing: var(--f3d-type-label-tracking);
  color: var(--f3d-color-text-muted);
}

.control {
  composes: focusRing from "./shared.module.css";
  display: inline-flex;
  align-items: center;
  gap: 0.25rem;
  height: 1.75rem;
  padding: 0 0.375rem 0 0.5rem;
  border-radius: var(--f3d-radius-sm);
  border: 1px solid var(--f3d-color-border-strong);
  background-color: var(--f3d-color-surface-raised);
}

.input {
  flex: 1;
  min-width: 0;
  height: 100%;
  border: none;
  background: none;
  color: var(--f3d-color-text);
  font-family: var(--f3d-type-body-font);
  font-size: var(--f3d-type-body-size);
}

.input:focus {
  outline: none;
}

.trigger {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--f3d-color-text-muted);
  cursor: pointer;
}

.content {
  min-width: var(--kb-combobox-trigger-width);
  border-radius: var(--f3d-radius-md);
  border: 1px solid var(--f3d-color-border);
  background-color: var(--f3d-color-surface-raised);
  overflow: hidden;
  z-index: 50;
}

.listbox {
  padding: 0.25rem;
  overflow-y: auto;
  max-height: 16rem;
}

.section {
  padding: 0.375rem 0.5rem 0.125rem;
  font-family: var(--f3d-type-label-font);
  font-size: var(--f3d-type-label-size);
  font-weight: var(--f3d-type-label-weight);
  letter-spacing: var(--f3d-type-label-tracking);
  color: var(--f3d-color-text-disabled);
  text-transform: uppercase;
}

.item {
  composes: focusRing from "./shared.module.css";
  display: flex;
  align-items: center;
  padding: 0.375rem 0.5rem;
  border-radius: var(--f3d-radius-sm);
  color: var(--f3d-color-text);
  font-family: var(--f3d-type-body-font);
  font-size: var(--f3d-type-body-size);
  cursor: pointer;
}

.item[data-disabled] {
  cursor: not-allowed;
  opacity: 0.5;
}

.item[data-highlighted] {
  background-color: var(--f3d-color-surface-hover);
}
```

- [ ] **Step 2: Register it in `index.ts`**

Add to `src/design-system/components/index.ts`:
```ts
export { Combobox, type ComboboxProps, type ComboboxGroup } from "./Combobox";
```

- [ ] **Step 3: Write the failing component test**

Add to `src/design-system/components/components.test.tsx`:
```tsx
describe("Combobox", () => {
  it("filters options via onInputChange and selects one on pointerup", async () => {
    const onChange = vi.fn();
    const onInputChange = vi.fn();
    render(() => (
      <Combobox
        label="Model"
        options={["Elegoo Centauri Carbon", "Prusa MK4", "Voron 2.4"]}
        onChange={onChange}
        onInputChange={onInputChange}
      />
    ));
    const input = screen.getByLabelText("Model") as HTMLInputElement;
    await fireEvent.pointerDown(input, { pointerType: "mouse", button: 0 });
    await fireEvent.input(input, { target: { value: "Prusa" } });
    expect(onInputChange).toHaveBeenCalledWith("Prusa");

    const item = await screen.findByText("Prusa MK4");
    await fireEvent.pointerUp(item, { button: 0 });
    expect(onChange).toHaveBeenCalledWith("Prusa MK4");
  });

  it("renders grouped options under section headers", async () => {
    render(() => (
      <Combobox
        label="Model"
        groups={[
          { label: "Elegoo", options: ["Centauri Carbon", "Neptune 4"] },
          { label: "Prusa", options: ["MK4", "CORE One"] },
        ]}
      />
    ));
    const input = screen.getByLabelText("Model");
    await fireEvent.pointerDown(input, { pointerType: "mouse", button: 0 });
    expect(await screen.findByText("Elegoo")).toBeInTheDocument();
    expect(await screen.findByText("Prusa")).toBeInTheDocument();
  });
});
```

Add `Combobox` to the file's existing component import line.

- [ ] **Step 4: Run the tests**

Run: `npx vitest run src/design-system/components/components.test.tsx -t Combobox`
Expected: both pass. Per `AGENTS.md`'s documented Kobalte convention, the
trigger/input opens on `pointerdown` and item selection fires on `pointerup`
— this test already uses that pattern; if it still doesn't fire, check
`vitest.setup.ts` for a missing polyfill (Kobalte's popper positioning may
need `IntersectionObserver`, which isn't currently stubbed there — the
existing stubs are `ResizeObserver` and pointer-capture methods) before
assuming the component itself is wrong.

- [ ] **Step 5: Run the full frontend build**

Run: `npm run build`
Expected: `tsc` typecheck and `vite build` both pass. This is the step that
validates or invalidates the `as never` casts from Step 1 — resolve any real
type error here before moving on.

- [ ] **Step 6: Add it to the Showcase**

Add a new `<Panel title="Combobox">` block to `src/design-system/Showcase.tsx`:
one flat example (`options={["Apple", "Banana", "Cherry"]}`) and one grouped
example mirroring the test's vendor/model shape.

- [ ] **Step 7: Run the full test suite**

Run: `npm test`
Expected: passes.

- [ ] **Step 8: Commit**

```bash
git add src/design-system/components/Combobox.tsx src/design-system/components/Combobox.module.css src/design-system/components/index.ts src/design-system/components/components.test.tsx src/design-system/Showcase.tsx
git commit -m "feat: add the Combobox design-system component"
```

---

## Task 12: `Field` design-system component

**Files:**
- Create: `src/design-system/components/Field.tsx`
- Create: `src/design-system/components/Field.module.css`
- Modify: `src/design-system/components/index.ts`
- Modify: `src/design-system/components/components.test.tsx`
- Modify: `src/design-system/Showcase.tsx`

**Interfaces:**
- Consumes: `IconButton` (existing), `IconArrowBackUp` from
  `@tabler/icons-solidjs` (verified present at
  `node_modules/@tabler/icons-solidjs/dist/types/icons/IconArrowBackUp.d.ts`)
- Produces: `export function Field(props: FieldProps)`,
  `export interface FieldProps { label, hint?, overridden?, onRevert?, children, class? }`
  — used by Task 16's `PrinterProfilePanel` for every profile field row.

Uses a native `title` attribute for the revert hint rather than the design
system's `Tooltip` component — `Tooltip.tsx`'s own doc comment says its
`trigger` slot should hold "text/icon content, not another button", and this
codebase already uses native `title` for exactly this hover-hint-on-a-button
case (`PrinterDashboard.tsx`'s disabled `+ Add printer` button). No existing
call site pairs `Tooltip` with a button, so there's no established pattern to
follow instead.

- [ ] **Step 1: Write the component**

Create `src/design-system/components/Field.tsx`:
```tsx
import { IconArrowBackUp } from "@tabler/icons-solidjs";
import { splitProps, type ParentProps } from "solid-js";
import { IconButton } from "./IconButton";
import styles from "./Field.module.css";

export interface FieldProps extends ParentProps {
  label: string;
  hint?: string;
  /** Whether this field's value diverges from the catalog. Draws the accent
   *  border and reveals the revert control. */
  overridden?: boolean;
  onRevert?: () => void;
  class?: string;
}

export function Field(props: FieldProps) {
  const [local, rest] = splitProps(props, [
    "label",
    "hint",
    "overridden",
    "onRevert",
    "children",
    "class",
  ]);

  return (
    <div
      class={[styles.field, local.overridden ? styles.overridden : "", local.class]
        .filter(Boolean)
        .join(" ")}
      {...rest}
    >
      <div class={styles.header}>
        <span class={styles.label}>{local.label}</span>
        {local.overridden && local.onRevert && (
          <IconButton
            aria-label={`Revert ${local.label} to inherited`}
            title={local.hint ?? "Revert to inherited"}
            onClick={local.onRevert}
          >
            <IconArrowBackUp size={14} />
          </IconButton>
        )}
      </div>
      <div class={styles.control}>{local.children}</div>
      {local.hint && <span class={styles.hint}>{local.hint}</span>}
    </div>
  );
}
```

Create `src/design-system/components/Field.module.css`:
```css
.field {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
  padding: 0.375rem 0 0.375rem 0.5rem;
  border-left: 2px solid transparent;
}

.overridden {
  border-left-color: var(--f3d-color-accent);
}

.header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
}

.label {
  font-family: var(--f3d-type-label-font);
  font-size: var(--f3d-type-label-size);
  font-weight: var(--f3d-type-label-weight);
  letter-spacing: var(--f3d-type-label-tracking);
  color: var(--f3d-color-text-muted);
  text-transform: uppercase;
}

.control {
  display: flex;
  align-items: center;
}

.hint {
  font-family: var(--f3d-type-body-small-font);
  font-size: var(--f3d-type-body-small-size);
  color: var(--f3d-color-text-disabled);
}
```

- [ ] **Step 2: Register it in `index.ts`**

Add to `src/design-system/components/index.ts`:
```ts
export { Field, type FieldProps } from "./Field";
```

- [ ] **Step 3: Write the failing test**

Add to `src/design-system/components/components.test.tsx`:
```tsx
describe("Field", () => {
  it("renders no revert control when not overridden", () => {
    render(() => <Field label="Printable height">240</Field>);
    expect(screen.queryByLabelText(/Revert/)).not.toBeInTheDocument();
  });

  it("renders a revert control when overridden and calls onRevert when clicked", async () => {
    const onRevert = vi.fn();
    render(() => (
      <Field label="Printable height" overridden onRevert={onRevert}>
        240
      </Field>
    ));
    const revert = screen.getByLabelText("Revert Printable height to inherited");
    await fireEvent.click(revert);
    expect(onRevert).toHaveBeenCalledTimes(1);
  });

  it("shows the hint text when not overridden", () => {
    render(() => (
      <Field label="Printable height" hint="inherited: 256">
        240
      </Field>
    ));
    expect(screen.getByText("inherited: 256")).toBeInTheDocument();
  });
});
```

Add `Field` to the file's existing component import line.

- [ ] **Step 4: Run the tests**

Run: `npx vitest run src/design-system/components/components.test.tsx -t Field`
Expected: all 3 tests pass (no red phase needed — `IconButton` is a plain
Kobalte button, which responds to ordinary `fireEvent.click`, unlike the
pointerdown/pointerup triggers `NumberField`/`Combobox` needed).

- [ ] **Step 5: Add it to the Showcase**

Add a `<Panel title="Field">` block to `src/design-system/Showcase.tsx` with
two examples side by side: one plain (`<Field label="Printable height">256 mm</Field>`)
and one overridden with a no-op `onRevert={() => {}}` and `hint="inherited: 256"`.

- [ ] **Step 6: Run the full build and test suite**

Run: `npm run build && npm test`
Expected: both pass.

- [ ] **Step 7: Commit**

```bash
git add src/design-system/components/Field.tsx src/design-system/components/Field.module.css src/design-system/components/index.ts src/design-system/components/components.test.tsx src/design-system/Showcase.tsx
git commit -m "feat: add the Field design-system component"
```

---

## Task 13: Frontend types + `printer-store.ts` + `printer-catalog.ts`

**Files:**
- Create: `src/printers/types.ts`
- Create: `src/printers/printer-store.ts`
- Create: `src/printers/printer-store.test.ts`
- Create: `src/printers/printer-catalog.ts`
- Create: `src/printers/printer-catalog.test.ts`

**Interfaces:**
- Consumes: Task 9's command surface (`list_printers`, `create_printer`,
  `update_printer`, `delete_printer`, `set_printer_override`,
  `rebind_printer`, `resolve_profile_drift`, `open_printers_file`,
  `list_catalog_models`, `list_catalog_variants`, `preview_profile`)
- Produces: `printers()`, `loadPrinters()`, `addPrinter()`, `updatePrinter()`,
  `removePrinter()`, `overrideField()`, `revertField()`, `rebindPrinter()`,
  `resolveDrift()`, `openPrintersFile()` from `printer-store.ts`;
  `listCatalogModels()`, `listCatalogVariants()`, `previewProfile()` from
  `printer-catalog.ts` — consumed by Task 14's `App.tsx` wiring and Task 15's
  `PrinterAddDialog`.

**A real uncertainty to confirm, not assume:** Tauri v2 auto-converts a
snake_case Rust command parameter name to a camelCase `invoke()` key (e.g.
`catalog_ref` → `{ catalogRef: ... }`). The existing `settings-store.ts` only
exercises this with single-word params (`settings`), which doesn't fully
prove the multi-word case. Step 4's test run is where this gets confirmed —
if `rebind_printer` fails with a missing-argument error, try the literal
snake_case key `catalogRef` → `catalog_ref` instead.

- [ ] **Step 1: Write `types.ts`**

Create `src/printers/types.ts`:
```ts
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
```

- [ ] **Step 2: Write `printer-store.ts`**

Create `src/printers/printer-store.ts`:
```ts
import { invoke, isTauri } from "@tauri-apps/api/core";
import { createStore } from "solid-js/store";
import type {
  CatalogRef,
  OverridableField,
  PrinterDraft,
  PrinterPatch,
  ResolvedPrinter,
} from "./types";

interface PrinterStoreState {
  printers: ResolvedPrinter[];
  status: "idle" | "loading" | "ready" | "error";
  error: string | null;
}

const [state, setState] = createStore<PrinterStoreState>({
  printers: [],
  status: "idle",
  error: null,
});

/** Reactive getter — read inside JSX/createMemo for Solid to track it. */
export const printers = () => state.printers;
export const printerStoreStatus = () => state.status;
export const printerStoreError = () => state.error;

const EMPTY_PROFILE = {
  bedShape: { kind: "rectangular", widthMm: 220, depthMm: 220, originXMm: 0, originYMm: 0 },
  printableHeightMm: 250,
  bedExcludeAreas: [],
  defaultBedType: "1",
  nozzleDiameterMm: [0.4],
  nozzleType: "brass",
  gcodeFlavor: "marlin",
  hasAuxiliaryFan: false,
  supportsAirFiltration: false,
  supportsMultiFilament: false,
  suggestedHostType: null,
} as const;

/** `just web` seed data — no Rust backend, so this stands in for both the
 *  Farm and the catalog. Shaped after the pre-catalog mock in App.tsx. */
const WEB_FALLBACK_PRINTERS: ResolvedPrinter[] = [
  {
    id: "prn-voron-1",
    name: "Voron 2.4 — Bay 1",
    group: "Bay 1",
    notes: "",
    catalogRef: {
      vendor: "Voron", model: "Voron 2.4", variant: "Voron 2.4 0.4 nozzle",
      modelId: "web-voron-24", printerVariant: "0.4",
    },
    catalogStatus: "ok",
    modelLabel: "Voron 2.4",
    variantLabel: "Voron 2.4 0.4 nozzle",
    profile: EMPTY_PROFILE,
    overriddenFields: [],
    inherited: {},
    profileDrift: [],
    unknownOverrideKeys: [],
    connection: null,
  },
  {
    id: "prn-prusa-1",
    name: "Prusa MK4 — Bay 2",
    group: "Bay 2",
    notes: "",
    catalogRef: {
      vendor: "Prusa", model: "Prusa MK4", variant: "Prusa MK4 0.4 nozzle",
      modelId: "web-prusa-mk4", printerVariant: "0.4",
    },
    catalogStatus: "ok",
    modelLabel: "Prusa MK4",
    variantLabel: "Prusa MK4 0.4 nozzle",
    profile: EMPTY_PROFILE,
    overriddenFields: [],
    inherited: {},
    profileDrift: [],
    unknownOverrideKeys: [],
    connection: null,
  },
];

export async function loadPrinters(): Promise<void> {
  setState("status", "loading");
  if (!isTauri()) {
    setState({ printers: WEB_FALLBACK_PRINTERS, status: "ready", error: null });
    return;
  }
  try {
    const loaded = await invoke<ResolvedPrinter[]>("list_printers");
    setState({ printers: loaded, status: "ready", error: null });
  } catch (e) {
    setState({ status: "error", error: String(e) });
  }
}

function spliceResolved(resolved: ResolvedPrinter): void {
  setState("printers", (p) => p.id === resolved.id, resolved);
}

function removeById(id: string): void {
  setState("printers", (list) => list.filter((p) => p.id !== id));
}

export async function addPrinter(draft: PrinterDraft): Promise<string> {
  if (!isTauri()) {
    const id = `prn-web-${state.printers.length + 1}`;
    setState("printers", (list) => [
      ...list,
      {
        id,
        name: draft.name,
        group: draft.group ?? "",
        notes: "",
        catalogRef: draft.catalogRef,
        catalogStatus: "ok",
        modelLabel: draft.catalogRef.model,
        variantLabel: draft.catalogRef.variant,
        profile: EMPTY_PROFILE,
        overriddenFields: [],
        inherited: {},
        profileDrift: [],
        unknownOverrideKeys: [],
        connection: null,
      },
    ]);
    return id;
  }
  const resolved = await invoke<ResolvedPrinter>("create_printer", { draft });
  setState("printers", (list) => [...list, resolved]);
  return resolved.id;
}

export async function updatePrinter(id: string, patch: PrinterPatch): Promise<void> {
  if (!isTauri()) {
    setState("printers", (p) => p.id === id, (p) => ({ ...p, ...patch }));
    return;
  }
  const resolved = await invoke<ResolvedPrinter>("update_printer", { id, patch });
  spliceResolved(resolved);
}

export async function removePrinter(id: string): Promise<void> {
  if (!isTauri()) {
    removeById(id);
    return;
  }
  await invoke("delete_printer", { id });
  removeById(id);
}

/** `value: undefined` is not valid here — pass a concrete value to override,
 *  or use `revertField()` to clear one. */
export async function overrideField(
  id: string,
  field: OverridableField,
  value: unknown,
): Promise<void> {
  if (!isTauri()) return; // web fallback has no catalog to resolve overrides against
  const resolved = await invoke<ResolvedPrinter>("set_printer_override", { id, field, value });
  spliceResolved(resolved);
}

export async function revertField(id: string, field: OverridableField): Promise<void> {
  if (!isTauri()) return;
  const resolved = await invoke<ResolvedPrinter>("set_printer_override", {
    id,
    field,
    value: null,
  });
  spliceResolved(resolved);
}

export async function rebindPrinter(id: string, catalogRef: CatalogRef): Promise<void> {
  if (!isTauri()) return;
  const resolved = await invoke<ResolvedPrinter>("rebind_printer", { id, catalogRef });
  spliceResolved(resolved);
}

export async function resolveDrift(id: string, action: "accept" | "pin"): Promise<void> {
  if (!isTauri()) return;
  const resolved = await invoke<ResolvedPrinter>("resolve_profile_drift", { id, action });
  spliceResolved(resolved);
}

export async function openPrintersFile(): Promise<void> {
  if (!isTauri()) return;
  await invoke("open_printers_file");
}
```

- [ ] **Step 3: Write `printer-catalog.ts`**

Create `src/printers/printer-catalog.ts`:
```ts
import { invoke, isTauri } from "@tauri-apps/api/core";
import type { CatalogModelSummary, CatalogRef, CatalogVariantSummary, PrinterProfile } from "./types";

let modelsCache: CatalogModelSummary[] | null = null;

/** The ~370-model list, cached after first load — used only by the add-printer flow. */
export async function listCatalogModels(): Promise<CatalogModelSummary[]> {
  if (!isTauri()) return [];
  if (modelsCache) return modelsCache;
  modelsCache = await invoke<CatalogModelSummary[]>("list_catalog_models");
  return modelsCache;
}

export async function listCatalogVariants(modelId: string): Promise<CatalogVariantSummary[]> {
  if (!isTauri()) return [];
  return invoke<CatalogVariantSummary[]>("list_catalog_variants", { modelId });
}

export async function previewProfile(catalogRef: CatalogRef): Promise<PrinterProfile> {
  return invoke<PrinterProfile>("preview_profile", { catalogRef });
}
```

- [ ] **Step 4: Write and run the store tests**

Create `src/printers/printer-store.test.ts`, following
`src/settings/settings-store.test.ts`'s exact mocking pattern:
```ts
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

beforeEach(() => {
  vi.resetModules();
  tauriMock.isTauri.mockReset();
  tauriMock.invoke.mockReset();
});

afterEach(() => {
  vi.restoreAllMocks();
});

const A_RESOLVED_PRINTER = {
  id: "prn-1",
  name: "Centauri Carbon — Bay 1",
  group: "Bay 1",
  notes: "",
  catalogRef: {
    vendor: "Elegoo", model: "Elegoo Centauri Carbon",
    variant: "Elegoo Centauri Carbon 0.4 nozzle", modelId: "Elegoo-CC", printerVariant: "0.4",
  },
  catalogStatus: "ok",
  modelLabel: "Elegoo Centauri Carbon",
  variantLabel: "Elegoo Centauri Carbon 0.4 nozzle",
  profile: {
    bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
    printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "4",
    nozzleDiameterMm: [0.4], nozzleType: "hardened_steel", gcodeFlavor: "klipper",
    hasAuxiliaryFan: true, supportsAirFiltration: true, supportsMultiFilament: true,
    suggestedHostType: "elegoolink",
  },
  overriddenFields: [],
  inherited: {},
  profileDrift: [],
  unknownOverrideKeys: [],
  connection: null,
};

describe("printer-store", () => {
  describe("under Tauri", () => {
    beforeEach(() => tauriMock.isTauri.mockReturnValue(true));

    it("loads printers via list_printers", async () => {
      tauriMock.invoke.mockResolvedValue([A_RESOLVED_PRINTER]);
      const { loadPrinters, printers } = await import("./printer-store");
      await loadPrinters();
      expect(tauriMock.invoke).toHaveBeenCalledWith("list_printers");
      expect(printers()).toEqual([A_RESOLVED_PRINTER]);
    });

    it("adds a printer via create_printer and appends the resolved result", async () => {
      tauriMock.invoke.mockResolvedValue([]);
      const { loadPrinters, addPrinter, printers } = await import("./printer-store");
      await loadPrinters();
      tauriMock.invoke.mockResolvedValue(A_RESOLVED_PRINTER);

      const id = await addPrinter({ name: "Centauri Carbon — Bay 1", catalogRef: A_RESOLVED_PRINTER.catalogRef });

      expect(tauriMock.invoke).toHaveBeenCalledWith("create_printer", {
        draft: { name: "Centauri Carbon — Bay 1", catalogRef: A_RESOLVED_PRINTER.catalogRef },
      });
      expect(id).toBe("prn-1");
      expect(printers()).toEqual([A_RESOLVED_PRINTER]);
    });

    it("revertField sends value: null", async () => {
      tauriMock.invoke.mockResolvedValue([A_RESOLVED_PRINTER]);
      const { loadPrinters, revertField } = await import("./printer-store");
      await loadPrinters();
      tauriMock.invoke.mockResolvedValue(A_RESOLVED_PRINTER);

      await revertField("prn-1", "printableHeightMm");

      expect(tauriMock.invoke).toHaveBeenCalledWith("set_printer_override", {
        id: "prn-1",
        field: "printableHeightMm",
        value: null,
      });
    });

    it("removePrinter invokes delete_printer and drops the row", async () => {
      tauriMock.invoke.mockResolvedValue([A_RESOLVED_PRINTER]);
      const { loadPrinters, removePrinter, printers } = await import("./printer-store");
      await loadPrinters();
      tauriMock.invoke.mockResolvedValue(undefined);

      await removePrinter("prn-1");

      expect(tauriMock.invoke).toHaveBeenCalledWith("delete_printer", { id: "prn-1" });
      expect(printers()).toEqual([]);
    });
  });

  describe("under just web (no Tauri backend)", () => {
    beforeEach(() => tauriMock.isTauri.mockReturnValue(false));

    it("seeds from the web fallback fixture without invoking any command", async () => {
      const { loadPrinters, printers } = await import("./printer-store");
      await loadPrinters();
      expect(printers().length).toBeGreaterThan(0);
      expect(tauriMock.invoke).not.toHaveBeenCalled();
    });

    it("overrideField and revertField are no-ops without a catalog to resolve against", async () => {
      const { loadPrinters, printers, overrideField, revertField } = await import("./printer-store");
      await loadPrinters();
      const id = printers()[0].id;

      await overrideField(id, "printableHeightMm", 240);
      await revertField(id, "printableHeightMm");

      expect(tauriMock.invoke).not.toHaveBeenCalled();
    });
  });
});
```

Run: `npx vitest run src/printers/printer-store.test.ts`
Expected: all 6 tests pass.

- [ ] **Step 5: Write and run the catalog module tests**

Create `src/printers/printer-catalog.test.ts`:
```ts
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

beforeEach(() => {
  vi.resetModules();
  tauriMock.isTauri.mockReset();
  tauriMock.invoke.mockReset();
});

afterEach(() => vi.restoreAllMocks());

describe("printer-catalog", () => {
  it("caches list_catalog_models after the first call", async () => {
    tauriMock.isTauri.mockReturnValue(true);
    tauriMock.invoke.mockResolvedValue([{ modelId: "Elegoo-CC", vendor: "Elegoo", model: "Elegoo Centauri Carbon" }]);
    const { listCatalogModels } = await import("./printer-catalog");

    await listCatalogModels();
    await listCatalogModels();

    expect(tauriMock.invoke).toHaveBeenCalledTimes(1);
  });

  it("resolves to an empty list under just web", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    const { listCatalogModels } = await import("./printer-catalog");
    expect(await listCatalogModels()).toEqual([]);
    expect(tauriMock.invoke).not.toHaveBeenCalled();
  });

  it("previewProfile forwards the catalogRef", async () => {
    tauriMock.isTauri.mockReturnValue(true);
    const ref = { vendor: "Elegoo", model: "Elegoo Centauri Carbon", variant: "Elegoo Centauri Carbon 0.4 nozzle", modelId: "Elegoo-CC", printerVariant: "0.4" };
    tauriMock.invoke.mockResolvedValue({});
    const { previewProfile } = await import("./printer-catalog");

    await previewProfile(ref);

    expect(tauriMock.invoke).toHaveBeenCalledWith("preview_profile", { catalogRef: ref });
  });
});
```

Run: `npx vitest run src/printers/printer-catalog.test.ts`
Expected: all 3 tests pass.

- [ ] **Step 6: Run the full frontend build and test suite**

Run: `npm run build && npm test`
Expected: both pass.

- [ ] **Step 7: Commit**

```bash
git add src/printers/types.ts src/printers/printer-store.ts src/printers/printer-store.test.ts src/printers/printer-catalog.ts src/printers/printer-catalog.test.ts
git commit -m "feat: add the printer store and catalog frontend modules"
```

---

## Task 14: `PrinterDashboard` regroup + `App.tsx` wiring

**Files:**
- Modify: `src/screens/PrinterDashboard.tsx` (full rewrite)
- Modify: `src/screens/PrinterDashboard.module.css` (full rewrite)
- Create: `src/screens/PrinterDashboard.test.tsx`
- Modify: `src/App.tsx` (full rewrite)

**Interfaces:**
- Consumes: `ResolvedPrinter` (Task 13's `types.ts`), `printers()` /
  `loadPrinters()` / `addPrinter()` / `removePrinter()` (Task 13's
  `printer-store.ts`)
- Produces: `export function groupPrintersByModel(printers: ResolvedPrinter[]): PrinterGroup[]`,
  updated `summarizePrinters(printers: ResolvedPrinter[]): string` — consumed
  by Task 15 (the add dialog opens from `PrinterDashboard`'s empty-state and
  toolbar buttons) and Task 16 (which fills in the detail aside's Profile
  tab, currently a placeholder here).

The old `Printer` interface (status/connectionType/currentJob/temps) and its
"Assign job"/"Cancel job" buttons are removed, not adapted — `ResolvedPrinter`
carries no runtime status in phase 1, and those controls return with real
behavior once phase 3 wires live polling. Faking them with permanently-disabled
buttons would be worse than not showing them.

- [ ] **Step 1: Rewrite `PrinterDashboard.tsx`**

Replace `src/screens/PrinterDashboard.tsx` in full:
```tsx
import { createMemo, createSignal, For, onCleanup, Show } from "solid-js";
import { Button } from "../design-system";
import type { ResolvedPrinter } from "../printers/types";
import styles from "./PrinterDashboard.module.css";

export interface PrinterGroup {
  modelKey: string;
  modelLabel: string;
  printers: ResolvedPrinter[];
}

const UNLINKED_KEY = "__unlinked__";

/** Groups by catalog model; a Printer whose catalog reference doesn't
 *  resolve (renamed/removed upstream preset) lands in a trailing "Unlinked"
 *  group instead of a normal model group. */
export function groupPrintersByModel(printers: ResolvedPrinter[]): PrinterGroup[] {
  const byModel = new Map<string, ResolvedPrinter[]>();
  for (const printer of printers) {
    const key =
      printer.catalogStatus === "ok" || printer.catalogStatus === "rematched"
        ? printer.catalogRef.modelId
        : UNLINKED_KEY;
    const list = byModel.get(key) ?? [];
    list.push(printer);
    byModel.set(key, list);
  }

  const groups: PrinterGroup[] = [];
  for (const [key, list] of byModel) {
    if (key === UNLINKED_KEY) continue;
    groups.push({
      modelKey: key,
      modelLabel: list[0].modelLabel,
      printers: [...list].sort((a, b) => a.name.localeCompare(b.name)),
    });
  }
  groups.sort((a, b) => a.modelLabel.localeCompare(b.modelLabel));

  const unlinked = byModel.get(UNLINKED_KEY);
  if (unlinked && unlinked.length > 0) {
    groups.push({
      modelKey: UNLINKED_KEY,
      modelLabel: "Unlinked",
      printers: [...unlinked].sort((a, b) => a.name.localeCompare(b.name)),
    });
  }
  return groups;
}

/** "3 printers" — for AppShell's status bar. Per-status counts return once
 *  phase 3 wires `runtimeStatus`; every Printer is status-less in phase 1. */
export function summarizePrinters(printers: ResolvedPrinter[]): string {
  if (printers.length === 0) return "No printers";
  return `${printers.length} printer${printers.length === 1 ? "" : "s"}`;
}

export interface PrinterDashboardProps {
  printers: ResolvedPrinter[];
  onAddPrinter?: () => void;
  onRemovePrinter?: (id: string) => void;
}

const DEFAULT_DETAIL_WIDTH = 320;
const MIN_DETAIL_WIDTH = 220;
const MAX_DETAIL_WIDTH = 560;

export function PrinterDashboard(props: PrinterDashboardProps) {
  const [selectedId, setSelectedId] = createSignal<string | null>(null);
  const selected = createMemo(() => props.printers.find((p) => p.id === selectedId()));
  const groups = createMemo(() => groupPrintersByModel(props.printers));
  const [detailWidth, setDetailWidth] = createSignal(DEFAULT_DETAIL_WIDTH);

  let dragStartX = 0;
  let dragStartWidth = 0;

  function onResizeMove(event: PointerEvent) {
    const delta = dragStartX - event.clientX;
    const next = Math.min(MAX_DETAIL_WIDTH, Math.max(MIN_DETAIL_WIDTH, dragStartWidth + delta));
    setDetailWidth(next);
  }

  function stopResizing() {
    window.removeEventListener("pointermove", onResizeMove);
    window.removeEventListener("pointerup", stopResizing);
    document.body.style.cursor = "";
    document.body.style.userSelect = "";
  }

  function onResizeStart(event: PointerEvent) {
    event.preventDefault();
    dragStartX = event.clientX;
    dragStartWidth = detailWidth();
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    window.addEventListener("pointermove", onResizeMove);
    window.addEventListener("pointerup", stopResizing);
  }

  onCleanup(stopResizing);

  return (
    <div class={styles.dashboard}>
      {/* `.main` is a column wrapper: Task 15 mounts a toolbar as its first
          child, stacked above `.groups`. `.dashboard` itself stays row-direction
          so the resizable detail aside remains a sibling, not nested here. */}
      <div class={styles.main}>
        <div class={styles.groups}>
          <Show
            when={props.printers.length > 0}
            fallback={
              <div class={styles.empty}>
                <p class={styles.emptyMessage}>No printers yet</p>
                <Button variant="secondary" onClick={() => props.onAddPrinter?.()}>
                  + Add printer
                </Button>
              </div>
            }
        >
          <For each={groups()}>
            {(group) => (
              <section class={styles.group}>
                <header class={styles.groupHeader}>
                  <span class={styles.groupTitle}>{group.modelLabel}</span>
                  <span class={styles.groupCount}>{group.printers.length}</span>
                </header>
                <div class={styles.grid}>
                  <For each={group.printers}>
                    {(printer) => (
                      <button
                        class={styles.card}
                        classList={{ [styles.cardSelected]: printer.id === selectedId() }}
                        onClick={() => setSelectedId(printer.id)}
                      >
                        <div class={styles.cardHeader}>
                          <span class={styles.cardName}>{printer.name}</span>
                        </div>
                        <div class={styles.badgeRow}>
                          <Show when={printer.catalogStatus !== "ok"}>
                            <span class={[styles.badge, styles.badgeWarning].join(" ")}>
                              Unlinked
                            </span>
                          </Show>
                          <Show when={printer.profileDrift.length > 0}>
                            <span class={[styles.badge, styles.badgeAccent].join(" ")}>
                              Profile updated
                            </span>
                          </Show>
                          <Show when={printer.overriddenFields.length > 0}>
                            <span class={[styles.badge, styles.badgeMuted].join(" ")}>
                              {printer.overriddenFields.length} override
                              {printer.overriddenFields.length === 1 ? "" : "s"}
                            </span>
                          </Show>
                          <Show when={printer.unknownOverrideKeys.length > 0}>
                            <span
                              class={[styles.badge, styles.badgeWarning].join(" ")}
                              title={`Unrecognized override keys: ${printer.unknownOverrideKeys.join(", ")}`}
                            >
                              {printer.unknownOverrideKeys.length} unknown key
                              {printer.unknownOverrideKeys.length === 1 ? "" : "s"}
                            </span>
                          </Show>
                        </div>
                        <div class={styles.cardFooter}>
                          <span class={styles.variant}>{printer.variantLabel}</span>
                        </div>
                      </button>
                    )}
                  </For>
                </div>
              </section>
            )}
          </For>
        </Show>
      </div>
      </div>

      <Show when={selected()}>
        {(printer) => (
          <>
            <div
              class={styles.resizeHandle}
              onPointerDown={onResizeStart}
              role="separator"
              aria-orientation="vertical"
              aria-label="Resize printer detail panel"
            />
            <aside
              class={styles.detail}
              style={{ width: `${detailWidth()}px` }}
              aria-label="Printer detail"
            >
              <div class={styles.detailHeader}>
                <span>{printer().name}</span>
                <Button variant="danger" onClick={() => props.onRemovePrinter?.(printer().id)}>
                  Remove
                </Button>
              </div>
              <div class={styles.detailBody}>
                {/* Task 16 replaces this placeholder with <PrinterProfilePanel>
                    wrapped in the Status/Profile/Connection Tabs. */}
                <p class={styles.detailMuted}>Profile — wired up in Task 16.</p>
              </div>
            </aside>
          </>
        )}
      </Show>
    </div>
  );
}
```

- [ ] **Step 2: Rewrite `PrinterDashboard.module.css`**

Replace `src/screens/PrinterDashboard.module.css` in full:
```css
.dashboard {
  display: flex;
  height: 100%;
  min-height: 0;
}

/* Column wrapper for the toolbar (Task 15) stacked above the scrollable
   printer grid. `.dashboard` stays row-direction so the resizable detail
   aside remains a sibling, not a child, of this column. */
.main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  min-height: 0;
}

.groups {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 0.75rem;
  display: flex;
  flex-direction: column;
  gap: 1rem;
}

.group {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.groupHeader {
  position: sticky;
  top: 0;
  z-index: 1;
  display: flex;
  align-items: baseline;
  gap: 0.5rem;
  padding: 0.25rem 0;
  background-color: var(--f3d-color-surface);
}

.groupTitle {
  font-family: var(--f3d-type-heading-font);
  font-size: var(--f3d-type-heading-size);
  font-weight: var(--f3d-type-heading-weight);
  color: var(--f3d-color-text);
}

.groupCount {
  font-family: var(--f3d-type-mono-font);
  font-size: var(--f3d-type-body-small-size);
  color: var(--f3d-color-text-muted);
}

.grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(15rem, 1fr));
  gap: 0.75rem;
}

.card {
  composes: focusRing from "../design-system/components/shared.module.css";
  composes: resetButton from "../design-system/components/shared.module.css";
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
  text-align: left;
}

button.card {
  padding: 0.75rem;
  background-color: var(--f3d-color-surface-raised);
  border: 1px solid var(--f3d-color-border);
  border-radius: var(--f3d-radius-md);
}

button.card:hover {
  border-color: var(--f3d-color-border-strong);
}

button.cardSelected,
button.cardSelected:hover {
  border-color: var(--f3d-color-accent);
}

.cardHeader {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
}

.cardName {
  font-family: var(--f3d-type-heading-font);
  font-size: var(--f3d-type-heading-size);
  font-weight: var(--f3d-type-heading-weight);
  color: var(--f3d-color-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.badgeRow {
  display: flex;
  flex-wrap: wrap;
  gap: 0.25rem;
}

.badge {
  padding: 0.125rem 0.5rem;
  border-radius: var(--f3d-radius-full);
  font-family: var(--f3d-type-label-font);
  font-size: var(--f3d-type-label-size);
  font-weight: var(--f3d-type-label-weight);
  letter-spacing: var(--f3d-type-label-tracking);
}

.badgeWarning {
  background-color: var(--f3d-color-warning);
  color: var(--f3d-color-on-warning);
}

.badgeAccent {
  background-color: var(--f3d-color-accent);
  color: var(--f3d-color-on-accent);
}

.badgeMuted {
  background-color: var(--f3d-color-surface-hover);
  color: var(--f3d-color-text-muted);
}

.cardFooter {
  font-family: var(--f3d-type-mono-font);
  font-size: var(--f3d-type-body-small-size);
  color: var(--f3d-color-text-muted);
}

.empty {
  grid-column: 1 / -1;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.75rem;
  padding: 3rem 1rem;
}

.emptyMessage {
  margin: 0;
  color: var(--f3d-color-text-muted);
  font-size: var(--f3d-type-body-size);
}

.resizeHandle {
  flex-shrink: 0;
  width: 5px;
  margin-left: -1px;
  cursor: col-resize;
  background-color: transparent;
  z-index: 1;
}

.resizeHandle:hover,
.resizeHandle:active {
  background-color: var(--f3d-color-accent);
}

.detail {
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  min-width: 0;
  overflow-y: auto;
  background-color: var(--f3d-color-surface);
  border-left: 1px solid var(--f3d-color-border);
}

.detailHeader {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
  padding: 0.5rem 0.75rem;
  border-bottom: 1px solid var(--f3d-color-border);
  font-family: var(--f3d-type-heading-font);
  font-size: var(--f3d-type-heading-size);
  font-weight: var(--f3d-type-heading-weight);
  letter-spacing: var(--f3d-type-heading-tracking);
  color: var(--f3d-color-text);
}

.detailBody {
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
  padding: 0.75rem;
}

.detailMuted {
  color: var(--f3d-color-text-disabled);
}
```

- [ ] **Step 3: Rewrite `App.tsx`**

Replace `src/App.tsx` in full:
```tsx
// src/App.tsx
import { createSignal, onMount, Show } from "solid-js";
import { AppShell } from "./screens/AppShell";
import type { ScreenId } from "./screens/ActivityBar";
import { PrinterDashboard, summarizePrinters } from "./screens/PrinterDashboard";
import { ModelLibrary, type Model } from "./screens/ModelLibrary";
import { loadPrinters, printers, removePrinter } from "./printers/printer-store";

const MODELS: Model[] = [
  { id: "benchy", name: "Benchy_v3.gcode", addedAt: "2 days ago" },
  { id: "bracket", name: "mount_bracket.stl", addedAt: "1 week ago" },
];

const SCREEN_TITLE: Record<ScreenId, string> = {
  printers: "Printers",
  library: "Library",
};

function App() {
  const [active, setActive] = createSignal<ScreenId>("printers");

  onMount(() => {
    void loadPrinters();
  });

  return (
    <AppShell
      active={active()}
      onSelect={setActive}
      title={SCREEN_TITLE[active()]}
      statusSummary={summarizePrinters(printers())}
    >
      <Show
        when={active() === "printers"}
        fallback={
          <ModelLibrary models={MODELS} compatiblePrinterNames={printers().map((p) => p.name)} />
        }
      >
        <PrinterDashboard
          printers={printers()}
          onRemovePrinter={(id) => void removePrinter(id)}
        />
      </Show>
    </AppShell>
  );
}

export default App;
```

`onAddPrinter` is deliberately left unwired here — Task 15 adds
`PrinterAddDialog` and wires it in, rather than this task reaching ahead into
a component that doesn't exist yet.

- [ ] **Step 4: Write the failing dashboard tests**

Create `src/screens/PrinterDashboard.test.tsx`:
```tsx
import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import { groupPrintersByModel, PrinterDashboard, summarizePrinters } from "./PrinterDashboard";
import type { ResolvedPrinter } from "../printers/types";

afterEach(() => {
  document.body.innerHTML = "";
});

const PROFILE = {
  bedShape: { kind: "rectangular" as const, widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
  printableHeightMm: 256,
  bedExcludeAreas: [],
  defaultBedType: "4",
  nozzleDiameterMm: [0.4],
  nozzleType: "hardened_steel",
  gcodeFlavor: "klipper",
  hasAuxiliaryFan: true,
  supportsAirFiltration: true,
  supportsMultiFilament: true,
  suggestedHostType: "elegoolink",
};

function printer(overrides: Partial<ResolvedPrinter>): ResolvedPrinter {
  return {
    id: "prn-1",
    name: "Printer",
    group: "",
    notes: "",
    catalogRef: {
      vendor: "Elegoo", model: "Elegoo Centauri Carbon",
      variant: "Elegoo Centauri Carbon 0.4 nozzle", modelId: "Elegoo-CC", printerVariant: "0.4",
    },
    catalogStatus: "ok",
    modelLabel: "Elegoo Centauri Carbon",
    variantLabel: "Elegoo Centauri Carbon 0.4 nozzle",
    profile: PROFILE,
    overriddenFields: [],
    inherited: {},
    profileDrift: [],
    unknownOverrideKeys: [],
    connection: null,
    ...overrides,
  };
}

describe("groupPrintersByModel", () => {
  it("groups printers under their catalog model, sorted alphabetically by group then name", () => {
    const printers = [
      printer({ id: "a", name: "Bay 2", catalogRef: { vendor: "Prusa", model: "Prusa MK4", variant: "v", modelId: "Prusa-MK4", printerVariant: "0.4" }, modelLabel: "Prusa MK4" }),
      printer({ id: "b", name: "Bay 1" }),
      printer({ id: "c", name: "Bay 3" }),
    ];
    const groups = groupPrintersByModel(printers);
    expect(groups.map((g) => g.modelLabel)).toEqual(["Elegoo Centauri Carbon", "Prusa MK4"]);
    expect(groups[0].printers.map((p) => p.name)).toEqual(["Bay 1", "Bay 3"]);
  });

  it("puts unresolved printers in a trailing Unlinked group", () => {
    const printers = [printer({ id: "a" }), printer({ id: "b", catalogStatus: "variantMissing" })];
    const groups = groupPrintersByModel(printers);
    expect(groups[groups.length - 1].modelLabel).toBe("Unlinked");
    expect(groups[groups.length - 1].printers.map((p) => p.id)).toEqual(["b"]);
  });
});

describe("summarizePrinters", () => {
  it("reports a count with correct pluralization", () => {
    expect(summarizePrinters([])).toBe("No printers");
    expect(summarizePrinters([printer({ id: "a" })])).toBe("1 printer");
    expect(summarizePrinters([printer({ id: "a" }), printer({ id: "b" })])).toBe("2 printers");
  });
});

describe("PrinterDashboard", () => {
  it("renders one group header per catalog model with the right counts", () => {
    render(() => (
      <PrinterDashboard
        printers={[
          printer({ id: "a", name: "Bay 1" }),
          printer({ id: "b", name: "Bay 2" }),
        ]}
      />
    ));
    expect(screen.getByText("Elegoo Centauri Carbon")).toBeInTheDocument();
    expect(screen.getByText("2")).toBeInTheDocument();
  });

  it("selecting a card opens the detail aside", async () => {
    render(() => <PrinterDashboard printers={[printer({ id: "a", name: "Bay 1" })]} />);
    await fireEvent.click(screen.getByText("Bay 1"));
    expect(screen.getByLabelText("Printer detail")).toBeInTheDocument();
  });
});
```

- [ ] **Step 5: Run the tests to verify they fail, then pass**

Run: `npx vitest run src/screens/PrinterDashboard.test.tsx`
Expected (first run, before Steps 1–3): fails — `PrinterDashboard` still has
the old props shape. After Steps 1–3 are in place: all 5 tests pass.

- [ ] **Step 6: Run the full build and test suite**

Run: `npm run build && npm test`
Expected: both pass. `npm run build`'s `tsc` pass is what confirms every
remaining `Printer`/old-shape reference elsewhere in the frontend has been
removed — check its output carefully rather than skimming for "0 errors".

- [ ] **Step 7: Commit**

```bash
git add src/screens/PrinterDashboard.tsx src/screens/PrinterDashboard.module.css src/screens/PrinterDashboard.test.tsx src/App.tsx
git commit -m "feat: regroup the printer dashboard by catalog model and wire the printer store"
```

---

## Task 15: `PrinterAddDialog`

**Files:**
- Create: `src/screens/PrinterAddDialog.tsx`
- Create: `src/screens/PrinterAddDialog.module.css`
- Create: `src/screens/PrinterAddDialog.test.tsx`
- Modify: `src/screens/PrinterDashboard.tsx` (add a toolbar, wire the dialog in; the `onAddPrinter` prop's signature changes from `() => void` to `(draft: PrinterDraft) => void`)
- Modify: `src/screens/PrinterDashboard.module.css` (add `.toolbar`)
- Modify: `src/App.tsx` (pass the real `addPrinter` callback)

**Interfaces:**
- Consumes: `listCatalogModels`, `listCatalogVariants`, `previewProfile`
  (Task 13's `printer-catalog.ts`), the `Combobox`/`Select`/`TextField`/
  `Dialog`/`Button` design-system components
- Produces: `export function PrinterAddDialog(props: { open, onOpenChange, onAdd(draft: PrinterDraft): void })` — wired into `PrinterDashboard`'s toolbar this task, and into `App.tsx`'s `addPrinter` call.

**Scope note — one trigger, not two:** the design spec describes both an
empty-state button and a toolbar button opening the same dialog. The design
system's `Dialog` wrapper (`src/design-system/components/Dialog.tsx`) takes a
single required `trigger: JSX.Element`, and Kobalte's dialog root supports one
bound trigger per instance — giving it two independent trigger elements isn't
a natural fit for that API. This task implements **one** trigger, in the
toolbar, and updates the empty-state message to reference it instead of
duplicating it. Flagging this rather than silently building two dialogs or
fighting the API for a marginal UX gain.

- [ ] **Step 1: Write `PrinterAddDialog.tsx`**

Create `src/screens/PrinterAddDialog.tsx`:
```tsx
import { createEffect, createMemo, createResource, createSignal, Show } from "solid-js";
import { Button, Combobox, Dialog, Select, TextField } from "../design-system";
import { listCatalogModels, listCatalogVariants, previewProfile } from "../printers/printer-catalog";
import type { CatalogModelSummary, CatalogVariantSummary, PrinterDraft } from "../printers/types";
import styles from "./PrinterAddDialog.module.css";

export interface PrinterAddDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onAdd: (draft: PrinterDraft) => void;
}

export function PrinterAddDialog(props: PrinterAddDialogProps) {
  const [models] = createResource(listCatalogModels);
  const [query, setQuery] = createSignal("");
  const [selectedModel, setSelectedModel] = createSignal<CatalogModelSummary | null>(null);
  const [selectedVariant, setSelectedVariant] = createSignal<CatalogVariantSummary | null>(null);
  const [name, setName] = createSignal("");
  const [nameTouched, setNameTouched] = createSignal(false);

  // Combobox's rendered option text is "<vendor> · <model>" (middot). After a
  // selection, Kobalte resets the input's displayed text to that string and
  // round-trips it through onInputChange -> setQuery, which would re-filter
  // against this memo mid-selection. If the corpus below were plain
  // "<vendor> <model>" (no middot), that reset text would match nothing,
  // collapsing filteredModels() to [] at exactly the moment Kobalte's
  // internal selection resolution needs the just-picked option to still be
  // present — silently dropping the selection (onChange fires with null).
  // Normalizing both sides to bare alphanumerics avoids the mismatch.
  const normalize = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, " ").trim();

  const filteredModels = createMemo(() => {
    const q = normalize(query());
    const all = models() ?? [];
    const matching = q
      ? all.filter((m) => normalize(`${m.vendor} ${m.model}`).includes(q))
      : all;
    return matching.slice(0, 50);
  });

  const [variants] = createResource(selectedModel, (model) =>
    model ? listCatalogVariants(model.modelId) : Promise.resolve([]),
  );

  // Auto-select the sole variant, or default to 0.4mm, without clobbering a
  // manual choice the user already made.
  createEffect(() => {
    const list = variants();
    if (!list || list.length === 0 || selectedVariant()) return;
    setSelectedVariant(list.find((v) => v.printerVariant === "0.4") ?? list[0]);
  });

  const catalogRef = createMemo(() => {
    const model = selectedModel();
    const variant = selectedVariant();
    return model && variant
      ? {
          vendor: model.vendor,
          model: model.model,
          variant: variant.variant,
          modelId: model.modelId,
          printerVariant: variant.printerVariant,
        }
      : null;
  });

  const [preview] = createResource(catalogRef, (ref) => (ref ? previewProfile(ref) : Promise.resolve(null)));

  function onSelectModel(model: CatalogModelSummary) {
    setSelectedModel(model);
    setSelectedVariant(null);
    if (!nameTouched()) setName(model.model);
  }

  const nameError = createMemo(() => (name().trim() === "" ? "Name is required" : undefined));
  const canAdd = createMemo(() => !!catalogRef() && !nameError());

  function submit() {
    const ref = catalogRef();
    if (!ref || nameError()) return;
    props.onAdd({ name: name(), catalogRef: ref });
    props.onOpenChange(false);
  }

  return (
    <Dialog
      title="Add printer"
      trigger="+ Add printer"
      open={props.open}
      onOpenChange={props.onOpenChange}
    >
      <div class={styles.form}>
        <Combobox
          label="Printer model"
          options={filteredModels()}
          optionValue={(m: CatalogModelSummary) => m.modelId}
          optionLabel={(m: CatalogModelSummary) => `${m.vendor} · ${m.model}`}
          value={selectedModel() ?? undefined}
          onChange={onSelectModel}
          onInputChange={setQuery}
          placeholder={`Search ${(models() ?? []).length} models...`}
        />
        <Show when={selectedModel()}>
          <Select
            label="Nozzle"
            options={variants() ?? []}
            optionValue={(v: CatalogVariantSummary) => v.variant}
            optionLabel={(v: CatalogVariantSummary) => `${v.printerVariant} mm`}
            value={selectedVariant() ?? undefined}
            onChange={setSelectedVariant}
          />
        </Show>
        <TextField
          label="Name"
          value={name()}
          onChange={(v) => {
            setName(v);
            setNameTouched(true);
          }}
          error={nameTouched() ? nameError() : undefined}
        />
        <Show when={preview()}>
          {(p) => {
            // Binding p()/bedShape to locals once, rather than calling p()
            // again per branch, is required: TS's discriminated-union
            // narrowing on `bedShape.kind` doesn't carry across separate
            // call expressions of the same accessor, even though it's
            // referentially the same function each time.
            const profile = p();
            const bedShape = profile.bedShape;
            return (
              <p class={styles.summary}>
                {bedShape.kind === "rectangular"
                  ? `${bedShape.widthMm} × ${bedShape.depthMm} × ${profile.printableHeightMm} mm`
                  : `${profile.printableHeightMm} mm tall, non-rectangular bed`}
                {" · "}
                {profile.nozzleDiameterMm.join(", ")} mm nozzle
              </p>
            );
          }}
        </Show>
      </div>
      <div class={styles.footer}>
        <Button variant="secondary" onClick={() => props.onOpenChange(false)}>
          Cancel
        </Button>
        <Button variant="primary" disabled={!canAdd()} onClick={submit}>
          Add printer
        </Button>
      </div>
    </Dialog>
  );
}
```

Create `src/screens/PrinterAddDialog.module.css`:
```css
.form {
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
  padding: 0 0.25rem;
}

.summary {
  margin: 0;
  color: var(--f3d-color-text-muted);
  font-family: var(--f3d-type-mono-font);
  font-size: var(--f3d-type-body-small-size);
}

.footer {
  display: flex;
  justify-content: flex-end;
  gap: 0.5rem;
  padding: 0.75rem 0.25rem 0;
}
```

- [ ] **Step 2: Wire the dialog into `PrinterDashboard`**

In `src/screens/PrinterDashboard.tsx`:
1. Add the import: `import { PrinterAddDialog } from "./PrinterAddDialog";` and
   `import type { PrinterDraft } from "../printers/types";`.
2. Add a signal: `const [addDialogOpen, setAddDialogOpen] = createSignal(false);`.
3. Change the prop type: `onAddPrinter?: (draft: PrinterDraft) => void;`.
4. Add a toolbar as the first child of `<div class={styles.main}>`, immediately
   before `<div class={styles.groups}>` (both are inside `.main`, which Task 14
   wraps `.groups` in specifically so this toolbar stacks *above* the grid
   rather than sitting beside it in `.dashboard`'s row layout):
   ```tsx
   <div class={styles.toolbar}>
     <PrinterAddDialog
       open={addDialogOpen()}
       onOpenChange={setAddDialogOpen}
       onAdd={(draft) => props.onAddPrinter?.(draft)}
     />
   </div>
   ```
5. Replace the empty-state's `<Button>` and its message with:
   ```tsx
   <p class={styles.emptyMessage}>No printers yet — add one with the button above.</p>
   ```
   (drop the empty-state's own `Button`, per this task's scope note).

Add to `src/screens/PrinterDashboard.module.css`:
```css
.toolbar {
  flex-shrink: 0;
  display: flex;
  justify-content: flex-end;
  padding: 0.5rem 0.75rem 0;
}
```

- [ ] **Step 3: Wire `App.tsx`'s `onAddPrinter`**

In `src/App.tsx`, add the import `addPrinter` from `"./printers/printer-store"`
and pass it through:
```tsx
<PrinterDashboard
  printers={printers()}
  onAddPrinter={(draft) => void addPrinter(draft)}
  onRemovePrinter={(id) => void removePrinter(id)}
/>
```

- [ ] **Step 4: Write the failing test**

Create `src/screens/PrinterAddDialog.test.tsx`:
```tsx
import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PrinterAddDialog } from "./PrinterAddDialog";

vi.mock("../printers/printer-catalog", () => ({
  listCatalogModels: vi.fn().mockResolvedValue([
    { modelId: "Elegoo-CC", vendor: "Elegoo", model: "Elegoo Centauri Carbon" },
    { modelId: "Prusa-MK4", vendor: "Prusa", model: "Prusa MK4" },
  ]),
  listCatalogVariants: vi.fn().mockResolvedValue([
    { variant: "Elegoo Centauri Carbon 0.4 nozzle", printerVariant: "0.4" },
  ]),
  previewProfile: vi.fn().mockResolvedValue({
    bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
    printableHeightMm: 256,
    nozzleDiameterMm: [0.4],
    bedExcludeAreas: [],
    defaultBedType: "4",
    nozzleType: "hardened_steel",
    gcodeFlavor: "klipper",
    hasAuxiliaryFan: true,
    supportsAirFiltration: true,
    supportsMultiFilament: true,
    suggestedHostType: "elegoolink",
  }),
}));

afterEach(() => {
  document.body.innerHTML = "";
});

describe("PrinterAddDialog", () => {
  it("searches, selects a model + auto-selected variant, and submits the draft", async () => {
    const onAdd = vi.fn();
    render(() => <PrinterAddDialog open onOpenChange={() => {}} onAdd={onAdd} />);

    // findByLabelText is ambiguous here — Kobalte's Combobox trigger button
    // also carries "Printer model" in its computed accessible name via
    // aria-labelledby, so it collides with the input under
    // @testing-library/dom's (non-recursive) label-matching heuristic.
    // getByRole("combobox", ...) is unambiguous: that role belongs only to
    // the <input> (verified against Kobalte's combobox source during Task 11).
    const input = await screen.findByRole("combobox", { name: "Printer model" });
    await fireEvent.pointerDown(input, { pointerType: "mouse", button: 0 });
    await fireEvent.input(input, { target: { value: "Centauri" } });

    const item = await screen.findByText("Elegoo · Elegoo Centauri Carbon");
    // pointerType: "mouse" is required — Kobalte's selectable-item handler
    // only selects on pointerup when pointerType is "mouse" and button is 0
    // (verified against createSelectableItem during Task 11; omitting this
    // makes the event a no-op and onChange never fires).
    await fireEvent.pointerUp(item, { pointerType: "mouse", button: 0 });

    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe(
      "Elegoo Centauri Carbon",
    );

    // getByText("Add printer") is ambiguous: Dialog's title="Add printer"
    // renders as an <h2> with the exact same text as this submit button —
    // same disambiguation technique already used above for the combobox
    // input (getByRole instead of findByLabelText).
    await fireEvent.click(screen.getByRole("button", { name: "Add printer" }));

    expect(onAdd).toHaveBeenCalledWith({
      name: "Elegoo Centauri Carbon",
      catalogRef: {
        vendor: "Elegoo",
        model: "Elegoo Centauri Carbon",
        variant: "Elegoo Centauri Carbon 0.4 nozzle",
        modelId: "Elegoo-CC",
        printerVariant: "0.4",
      },
    });
  });

  it("disables Add printer until a model is selected", () => {
    render(() => <PrinterAddDialog open onOpenChange={() => {}} onAdd={vi.fn()} />);
    const addButton = screen.getByRole("button", { name: "Add printer" }) as HTMLButtonElement;
    expect(addButton.disabled).toBe(true);
  });
});
```

- [ ] **Step 5: Run the tests**

Run: `npx vitest run src/screens/PrinterAddDialog.test.tsx`
Expected: both pass. **Update from Task 11:** `IntersectionObserver` was
never actually the issue there — no polyfill was needed once the event
simulation matched Kobalte's real wiring (see the query/event fixes already
baked into Step 4's test code above, both verified against Kobalte's source
during Task 11). If something still doesn't register here, it's more likely
this exact same class of issue recurring in a new spot (an accessible-name
collision, or a missing `pointerType`/`button` on a synthetic event) than a
missing observer polyfill — check the query/event first.

- [ ] **Step 6: Run the full build and test suite**

Run: `npm run build && npm test`
Expected: both pass.

- [ ] **Step 7: Commit**

```bash
git add src/screens/PrinterAddDialog.tsx src/screens/PrinterAddDialog.module.css src/screens/PrinterAddDialog.test.tsx src/screens/PrinterDashboard.tsx src/screens/PrinterDashboard.module.css src/App.tsx
git commit -m "feat: add the printer add-dialog and wire it into the dashboard"
```

---

## Task 16: `PrinterProfilePanel` + Status/Profile/Connection tabs

**Files:**
- Create: `src/screens/PrinterProfilePanel.tsx`
- Create: `src/screens/PrinterProfilePanel.module.css`
- Create: `src/screens/PrinterProfilePanel.test.tsx`
- Modify: `src/screens/PrinterDashboard.tsx` (detail aside gets `Tabs`, replacing Task 14's placeholder)
- Modify: `src/screens/PrinterDashboard.module.css` (restore `.detailField`/`.detailLabel` for the Status tab)

**Interfaces:**
- Consumes: `overrideField`, `revertField`, `resolveDrift` (Task 13's
  `printer-store.ts`); `Field`, `NumberField`, `Select`, `Switch`, `Tabs`,
  `Button` design-system components
- Produces: `export function PrinterProfilePanel(props: { printer: ResolvedPrinter })`
  — this is the last piece; after this task the dashboard is feature-complete
  for phase 1.

**Deliberately out of scope for this task's UI:** `bedExcludeAreas` has no
dedicated editor — a multi-rectangle exclusion-zone editor is a bigger UI
investment than the other five fields, and it's still fully supported by the
override/resolution machinery from Tasks 7–9. It's set by hand-editing
`printers.json`, consistent with that file being designed to be hand-editable
throughout this plan. Polygon `bedShape` values render read-only, per the spec.

- [ ] **Step 1: Write `PrinterProfilePanel.tsx`**

Create `src/screens/PrinterProfilePanel.tsx`:
```tsx
import { createEffect, createMemo, on, onCleanup, Show } from "solid-js";
import { Button, Field, NumberField, Select, Switch } from "../design-system";
import { overrideField, resolveDrift, revertField } from "../printers/printer-store";
import type { BedShape, OverridableField, ResolvedPrinter } from "../printers/types";
import styles from "./PrinterProfilePanel.module.css";

export interface PrinterProfilePanelProps {
  printer: ResolvedPrinter;
}

const DEBOUNCE_MS = 300;

function isOverridden(printer: ResolvedPrinter, field: OverridableField): boolean {
  return printer.overriddenFields.includes(field);
}

type RectangularBedShape = Extract<BedShape, { kind: "rectangular" }>;

export function PrinterProfilePanel(props: PrinterProfilePanelProps) {
  let heightTimer: ReturnType<typeof setTimeout> | undefined;
  let bedShapeTimer: ReturnType<typeof setTimeout> | undefined;
  // Accumulates in-flight bed-size edits (width/depth) so a second edit
  // within the debounce window merges onto the first instead of re-reading
  // a stale pre-edit `shape()` snapshot and silently dropping it.
  let pendingBedShapePatch: Partial<Pick<RectangularBedShape, "widthMm" | "depthMm">> | undefined;

  // PrinterDashboard's outer <Show when={selected()}> is non-keyed, so
  // switching the selected printer (a truthy -> truthy transition) does NOT
  // remount this component -- Solid's <Show> only re-invokes its child on a
  // falsy<->truthy transition. That means the `let` timer state above, and
  // any debounced closure reading props.printer.id live, would otherwise
  // persist across a printer switch: edit height on printer A, switch to
  // printer B within the debounce window, and the pending write would fire
  // against whichever printer is selected when the timer expires -- not
  // the one being edited when it was scheduled. Cancel in-flight writes
  // whenever the printer identity changes.
  createEffect(on(
    () => props.printer.id,
    (_id, prevId) => {
      if (prevId === undefined) return;
      clearTimeout(heightTimer);
      clearTimeout(bedShapeTimer);
      heightTimer = undefined;
      bedShapeTimer = undefined;
      pendingBedShapePatch = undefined;
    },
  ));

  onCleanup(() => {
    clearTimeout(heightTimer);
    clearTimeout(bedShapeTimer);
  });

  function debouncedOverride(
    field: OverridableField,
    value: unknown,
    timer: () => ReturnType<typeof setTimeout> | undefined,
    setTimer: (t: ReturnType<typeof setTimeout>) => void,
  ) {
    clearTimeout(timer());
    const printerId = props.printer.id;
    setTimer(setTimeout(() => void overrideField(printerId, field, value), DEBOUNCE_MS));
  }

  // Kobalte's NumberField root fires onRawValueChange unconditionally at
  // mount (node_modules/@kobalte/core/src/number-field/number-field-root.tsx:214),
  // and again whenever the reactive source feeding its `rawValue` prop
  // changes identity -- neither case is a real edit. Writing an override
  // for a same-value "change" would silently pin the field to the catalog
  // value it already has, permanently defeating drift detection for it
  // (profile_drift is only computed for non-overridden fields). Each
  // onChange below must compare against the field's current value and
  // no-op on equality before scheduling a write.
  function debouncedBedShapeOverride(shape: RectangularBedShape, patch: Partial<Pick<RectangularBedShape, "widthMm" | "depthMm">>) {
    pendingBedShapePatch = { ...pendingBedShapePatch, ...patch };
    clearTimeout(bedShapeTimer);
    const printerId = props.printer.id;
    const value = { ...shape, ...pendingBedShapePatch };
    bedShapeTimer = setTimeout(() => {
      void overrideField(printerId, "bedShape", value);
      pendingBedShapePatch = undefined;
    }, DEBOUNCE_MS);
  }

  const rectShape = createMemo(() => {
    const shape = props.printer.profile.bedShape;
    return shape.kind === "rectangular" ? shape : null;
  });

  return (
    <div class={styles.panel}>
      <Show when={props.printer.profileDrift.length > 0}>
        <div class={styles.driftBanner}>
          <p class={styles.driftMessage}>
            The catalog changed for {props.printer.profileDrift.length} field
            {props.printer.profileDrift.length === 1 ? "" : "s"} since this printer was last
            confirmed.
          </p>
          <div class={styles.driftActions}>
            <Button variant="secondary" onClick={() => void resolveDrift(props.printer.id, "pin")}>
              Keep my value
            </Button>
            <Button variant="primary" onClick={() => void resolveDrift(props.printer.id, "accept")}>
              Accept
            </Button>
          </div>
        </div>
      </Show>

      <Field
        label="Printable height"
        overridden={isOverridden(props.printer, "printableHeightMm")}
        hint={
          isOverridden(props.printer, "printableHeightMm")
            ? `inherited: ${props.printer.inherited.printableHeightMm}`
            : undefined
        }
        onRevert={() => void revertField(props.printer.id, "printableHeightMm")}
      >
        <NumberField
          aria-label="Printable height"
          value={props.printer.profile.printableHeightMm}
          suffix="mm"
          minValue={1}
          onChange={(v) => {
            if (v === props.printer.profile.printableHeightMm) return;
            debouncedOverride(
              "printableHeightMm", v,
              () => heightTimer, (t) => (heightTimer = t),
            );
          }}
        />
      </Field>

      <Show when={rectShape()}>
        {(shape) => (
          <Field
            label="Bed size"
            overridden={isOverridden(props.printer, "bedShape")}
            onRevert={() => void revertField(props.printer.id, "bedShape")}
          >
            <div class={styles.bedSizeRow}>
              <NumberField
                aria-label="Bed width"
                value={shape().widthMm}
                suffix="mm"
                minValue={1}
                onChange={(v) => {
                  if (v === shape().widthMm) return;
                  debouncedBedShapeOverride(shape(), { widthMm: v });
                }}
              />
              <NumberField
                aria-label="Bed depth"
                value={shape().depthMm}
                suffix="mm"
                minValue={1}
                onChange={(v) => {
                  if (v === shape().depthMm) return;
                  debouncedBedShapeOverride(shape(), { depthMm: v });
                }}
              />
            </div>
          </Field>
        )}
      </Show>

      <Show when={props.printer.profile.bedShape.kind === "polygon"}>
        <Field label="Bed shape">
          <p class={styles.readOnlyHint}>Non-rectangular bed — edit printers.json to change.</p>
        </Field>
      </Show>

      <Field
        label="Bed type"
        overridden={isOverridden(props.printer, "defaultBedType")}
        onRevert={() => void revertField(props.printer.id, "defaultBedType")}
      >
        {/* Raw catalog bed-type codes, not human labels — the generator
            doesn't emit a code->label map in phase 1. */}
        <Select
          options={["1", "2", "3", "4"]}
          value={props.printer.profile.defaultBedType}
          onChange={(v) => void overrideField(props.printer.id, "defaultBedType", v)}
        />
      </Field>

      <Field
        label="Auxiliary fan"
        overridden={isOverridden(props.printer, "hasAuxiliaryFan")}
        onRevert={() => void revertField(props.printer.id, "hasAuxiliaryFan")}
      >
        <Switch
          checked={props.printer.profile.hasAuxiliaryFan}
          onChange={(v) => void overrideField(props.printer.id, "hasAuxiliaryFan", v)}
        />
      </Field>

      <Field
        label="Air filtration"
        overridden={isOverridden(props.printer, "supportsAirFiltration")}
        onRevert={() => void revertField(props.printer.id, "supportsAirFiltration")}
      >
        <Switch
          checked={props.printer.profile.supportsAirFiltration}
          onChange={(v) => void overrideField(props.printer.id, "supportsAirFiltration", v)}
        />
      </Field>
    </div>
  );
}
```

Create `src/screens/PrinterProfilePanel.module.css`:
```css
.panel {
  display: flex;
  flex-direction: column;
}

.driftBanner {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
  padding: 0.5rem;
  margin-bottom: 0.5rem;
  border-radius: var(--f3d-radius-sm);
  background-color: var(--f3d-color-surface-hover);
  border: 1px solid var(--f3d-color-border);
}

.driftMessage {
  margin: 0;
  font-size: var(--f3d-type-body-small-size);
  color: var(--f3d-color-text);
}

.driftActions {
  display: flex;
  justify-content: flex-end;
  gap: 0.5rem;
}

.bedSizeRow {
  display: flex;
  gap: 0.5rem;
  min-width: 0;
}

.bedSizeRow > * {
  flex: 1;
  min-width: 0;
}

.readOnlyHint {
  margin: 0;
  color: var(--f3d-color-text-disabled);
  font-size: var(--f3d-type-body-small-size);
}
```

- [ ] **Step 2: Wire it into `PrinterDashboard`'s detail aside**

In `src/screens/PrinterDashboard.tsx`:
1. Add imports: `import { PrinterProfilePanel } from "./PrinterProfilePanel";` and `import { Tabs } from "../design-system";`.
2. Replace the placeholder `<div class={styles.detailBody}>` block with:
   ```tsx
   <div class={styles.detailBody}>
     <Tabs
       defaultValue="profile"
       items={[
         {
           value: "status",
           label: "Status",
           content: (
             <div class={styles.detailField}>
               <span class={styles.detailLabel}>Catalog</span>
               <span>{printer().modelLabel} — {printer().variantLabel}</span>
               <Show when={printer().catalogStatus !== "ok"}>
                 <span class={styles.detailMuted}>
                   Catalog status: {printer().catalogStatus}
                 </span>
               </Show>
             </div>
           ),
         },
         { value: "profile", label: "Profile", content: <PrinterProfilePanel printer={printer()} /> },
         {
           value: "connection",
           label: "Connection",
           content: <p class={styles.detailMuted}>Configured in a later phase.</p>,
         },
       ]}
     />
   </div>
   ```

Add back to `src/screens/PrinterDashboard.module.css` (removed in Task 14,
needed again for the Status tab's field row):
```css
.detailField {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
}

.detailLabel {
  font-family: var(--f3d-type-label-font);
  font-size: var(--f3d-type-label-size);
  font-weight: var(--f3d-type-label-weight);
  letter-spacing: var(--f3d-type-label-tracking);
  color: var(--f3d-color-text-muted);
  text-transform: uppercase;
}
```

- [ ] **Step 3: Write the failing test**

Create `src/screens/PrinterProfilePanel.test.tsx`:
```tsx
import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PrinterProfilePanel } from "./PrinterProfilePanel";
import type { ResolvedPrinter } from "../printers/types";

const overrideField = vi.fn();
const revertField = vi.fn();
const resolveDrift = vi.fn();
vi.mock("../printers/printer-store", () => ({
  overrideField: (...args: unknown[]) => overrideField(...args),
  revertField: (...args: unknown[]) => revertField(...args),
  resolveDrift: (...args: unknown[]) => resolveDrift(...args),
}));

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
  vi.useRealTimers();
});

const PRINTER: ResolvedPrinter = {
  id: "prn-1",
  name: "Centauri Carbon — Bay 1",
  group: "",
  notes: "",
  catalogRef: {
    vendor: "Elegoo", model: "Elegoo Centauri Carbon",
    variant: "Elegoo Centauri Carbon 0.4 nozzle", modelId: "Elegoo-CC", printerVariant: "0.4",
  },
  catalogStatus: "ok",
  modelLabel: "Elegoo Centauri Carbon",
  variantLabel: "Elegoo Centauri Carbon 0.4 nozzle",
  profile: {
    bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
    printableHeightMm: 256,
    bedExcludeAreas: [],
    defaultBedType: "4",
    nozzleDiameterMm: [0.4],
    nozzleType: "hardened_steel",
    gcodeFlavor: "klipper",
    hasAuxiliaryFan: true,
    supportsAirFiltration: true,
    supportsMultiFilament: true,
    suggestedHostType: "elegoolink",
  },
  overriddenFields: [],
  inherited: {},
  profileDrift: [],
  unknownOverrideKeys: [],
  connection: null,
};

describe("PrinterProfilePanel", () => {
  it("renders no revert control on an inherited field", () => {
    render(() => <PrinterProfilePanel printer={PRINTER} />);
    expect(screen.queryByLabelText("Revert Printable height to inherited")).not.toBeInTheDocument();
  });

  it("renders a revert control and inherited hint on an overridden field", () => {
    const overridden: ResolvedPrinter = {
      ...PRINTER,
      overriddenFields: ["printableHeightMm"],
      inherited: { printableHeightMm: 256 },
      profile: { ...PRINTER.profile, printableHeightMm: 240 },
    };
    render(() => <PrinterProfilePanel printer={overridden} />);
    expect(screen.getByLabelText("Revert Printable height to inherited")).toBeInTheDocument();
    expect(screen.getByText("inherited: 256")).toBeInTheDocument();
  });

  it("debounces a height edit before calling overrideField", async () => {
    vi.useFakeTimers();
    render(() => <PrinterProfilePanel printer={PRINTER} />);
    const input = screen.getByLabelText("Printable height") as HTMLInputElement;

    await fireEvent.input(input, { target: { value: "240" } });
    expect(overrideField).not.toHaveBeenCalled();

    vi.advanceTimersByTime(300);
    expect(overrideField).toHaveBeenCalledWith("prn-1", "printableHeightMm", 240);
  });

  it("shows a drift banner and calls resolveDrift on Accept", async () => {
    const drifted: ResolvedPrinter = {
      ...PRINTER,
      profileDrift: [{ field: "printableHeightMm", from: 250, to: 256 }],
    };
    render(() => <PrinterProfilePanel printer={drifted} />);
    await fireEvent.click(screen.getByText("Accept"));
    expect(resolveDrift).toHaveBeenCalledWith("prn-1", "accept");
  });
});
```

- [ ] **Step 4: Run the tests**

Run: `npx vitest run src/screens/PrinterProfilePanel.test.tsx`
Expected: all 4 pass. `getByLabelText("Printable height")` works because
`NumberField` was given `aria-label="Printable height"` in Step 1, forwarded
to Kobalte's `NumberField.Input` (Task 10's revised `NumberFieldProps` —
confirmed by reading `create-form-control-field.tsx` that the *Input*
subcomponent, not the Root, reads its own `aria-label` prop directly). `Field`
itself renders only a plain `<span>` for its visible label, not a real
`<label>` element, so it creates no programmatic association on its own —
don't rely on it for `getByLabelText` queries elsewhere in this file (e.g.
`Switch`/`Select` rows aren't queried by label text in this test suite for
that reason).

- [ ] **Step 5: Run the full build and test suite**

Run: `npm run build && npm test`
Expected: both pass.

- [ ] **Step 6: Commit**

```bash
git add src/screens/PrinterProfilePanel.tsx src/screens/PrinterProfilePanel.module.css src/screens/PrinterProfilePanel.test.tsx src/screens/PrinterDashboard.tsx src/screens/PrinterDashboard.module.css
git commit -m "feat: add the printer profile panel with override editing and drift resolution"
```

---

## Task 17: Full verification pass

**Files:** none (verification only)

**Interfaces:** none — this task confirms every prior task's contract still
holds together as a whole, the way the settings-system plan's own final task
did (`docs/superpowers/plans/2026-08-20-settings-system.md`'s "Task 5: Full
verification pass").

- [ ] **Step 1: Full Rust test suite**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml`
Expected: every test from Tasks 1, 2, 3, 5, 6, 7, 8 passes — roughly 45 tests
(5 shape + 5 inherits + 5 ingest + 2 snapshot + 9 printers + 2 rfc3339 + 14
resolve). Also run `cargo build --manifest-path src-tauri/Cargo.toml --bin
gen-catalog` to confirm the generator binary still compiles independently of
the app binary.

- [ ] **Step 2: Full frontend build and test suite**

Run: `just build && just test`
Expected: `tsc` typecheck + `vite build` succeed; every Vitest file passes —
`components.test.tsx` (with the three new component suites), `printer-store.test.ts`,
`printer-catalog.test.ts`, `PrinterDashboard.test.tsx`, `PrinterAddDialog.test.tsx`,
`PrinterProfilePanel.test.tsx`, plus the pre-existing `SettingsMenu.test.tsx`,
`ThemePopover.test.tsx`, `settings-store.test.ts`, `theme-engine.test.ts`
(untouched by this plan, but confirming nothing broke them).

- [ ] **Step 3: Grep for stale references**

Run: `grep -rn "PRINTERS\b\|interface Printer\b\|connectionType\|currentJob\|nozzleTempC\|bedTempC" src/ --include="*.tsx" --include="*.ts"`
Expected: no matches. Every reference to the old flat mock `Printer` shape
should be gone — if anything matches, it's a leftover from before Task 14's
rewrite that needs cleaning up.

- [ ] **Step 4: Regenerate the catalog once more and diff it**

Run: `just gen-catalog && git status --short src-tauri/resources/printer-catalog.json`
Expected: no diff (or a diff only if upstream `v2.4.2` content genuinely
changed between Task 4 and now, which shouldn't happen for a fixed tag) —
confirms the generator is deterministic given the same pinned tag.

- [ ] **Step 5: Visual confirmation via `just dev`**

Run: `source "$HOME/.cargo/env" && just dev` (requires a display; if none is
available, use `just web` instead and accept that Tauri commands won't be
exercised — note which path was used when reporting this step's result).

Confirm, in order:
1. The Printers screen loads with no printers and the empty-state message
   referencing the toolbar's "+ Add printer" button.
2. Click **+ Add printer**, search "centauri" in the model combobox, confirm
   it narrows to Elegoo Centauri Carbon (and Carbon 2, if present in the
   `v2.4.2` catalog), pick the 0.4mm nozzle variant, confirm the live
   build-volume/nozzle summary line appears, type a name, and add it. Repeat
   twice more with different names to get three Centauri Carbons.
3. Confirm all three group under one "Elegoo Centauri Carbon" header with a
   count of 3.
4. Select one, open the Profile tab, toggle "Air filtration" off. Confirm the
   accent-colored left border and revert control appear on that field, and
   that the other two printers' cards/detail are unaffected.
5. Click the revert control; confirm the value returns to on (inherited) and
   the accent border disappears.
6. Open `~/.config/farm3d/printers.json` (or the equivalent path if `just
   dev` resolved a different `app_config_dir` on this platform) and confirm
   it contains exactly one printer with a non-empty `overrides` — should be
   empty after the Step 5 revert, so confirm the file's `overrides: {}` (or
   the key omitted entirely, per `skip_serializing_if`) for that printer.
7. Hand-edit that printer's `catalogRef.variant` in the file to a bogus
   string, quit and relaunch the app, and confirm the printer now appears
   under an "Unlinked" group with its `lastKnownGood` profile still populating
   its fields (not blank) rather than vanishing from the list entirely.

Report the outcome of each of these seven checks explicitly — a passing build
and test suite does not by itself confirm the UI renders and behaves
correctly, per `AGENTS.md`.

- [ ] **Step 6: Update `CONTEXT.md` if warranted**

If Step 5's walkthrough surfaced vocabulary this plan introduced that isn't
yet in `CONTEXT.md` (candidates: **Printer Model**, **Printer Variant** —
review against the existing **Printer Profile** entry, which may already
cover this, or may need a one-line addition distinguishing "Profile" the
resolved capability set from "Model"/"Variant" the catalog reference), add
it, following the file's existing `**Term**: definition. _Avoid_: ...` format.
Skip this step if the existing vocabulary already covers it without confusion
— don't add entries defensively.

- [ ] **Step 7: Final commit (only if Step 6 changed anything)**

```bash
git add CONTEXT.md
git commit -m "docs: extend the domain vocabulary for Printer Models and variants"
```

If Step 6 made no changes, there is nothing to commit — Task 16's commit
remains the last one for this plan.

