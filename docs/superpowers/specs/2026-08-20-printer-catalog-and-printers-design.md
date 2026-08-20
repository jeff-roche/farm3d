# Printer catalog and Printer instances

## Context

farm3d's Printers are fiction today. `src/screens/PrinterDashboard.tsx:6` defines
a single flat `interface Printer` (id, name, status, connectionType, currentJob,
temps), `src/App.tsx:8` hardcodes three of them in a module-level `const
PRINTERS`, and every mutating control is `disabled` with an "isn't wired up yet"
tooltip. There is no printer code in `src-tauri/` at all.

A real Farm isn't a flat list. A user owns *three Elegoo Centauri Carbons* — one
kind of Printer, three physical units at three addresses, each with its own
nozzle, bed surface, and filtration state. The flat list has nowhere to put the
shared capability data and nowhere to put the per-unit divergence. Both are
needed before Slicing can check compatibility (`CONTEXT.md`'s **Printer
Profile**) or a Job can be dispatched to a specific unit.

This spec introduces that two-level structure, sourced from OrcaSlicer's machine
presets — which the app already depends on for Slicing (ADR-0003) and which
already models exactly this shape.

This is phase 1 of a three-phase arc. Phase 2 (Connection config + a
`PrinterConnection` trait proved against Moonraker) and phase 3 (OctoPrint +
ElegooLink adapters) are recorded separately in
`2026-08-20-printer-connections-design.md` and
`2026-08-20-printer-adapters-design.md`. This spec is implementable on its own;
it reserves a typed `connection` slot so phase 2 is additive rather than a
reshape.

## Why OrcaSlicer's preset tree is the right source

Verified against a real OrcaSlicer install:

- `<profiles>/<Vendor>.json` lists `machine_model_list[]` (the *kinds*) and
  `machine_list[]` (the *variants*, one per nozzle diameter).
- A machine-model file is the group: `{name: "Elegoo Centauri Carbon", model_id:
  "Elegoo-CC", nozzle_diameter: "0.4;0.2;0.6;0.8", family: "Elegoo"}`.
- A variant file is the capability sheet: `printer_model`, `printer_variant`,
  `printable_area`, `printable_height`, `nozzle_type`, `support_air_filtration`,
  `auxiliary_fan`, `support_multi_filament`, `default_bed_type`, `gcode_flavor`,
  `host_type`.
- OrcaSlicer's *own* user presets are already sparse override layers:
  `user/default/machine/*.json` is `{from: "User", inherits: "<system preset>",
  name, print_host, host_type}` and nothing else.

That last point matters most: live inheritance isn't being invented here, it's
the model OrcaSlicer already uses — so farm3d instances stay translatable into
presets when Slicing lands.

**Four verified facts that shape the code:**

| Fact | Measured | Consequence |
|---|---|---|
| Leaf variants often lack `printable_area` | 467 / 1170 in one vendor snapshot | Walking the `inherits` chain to `fdm_machine_common` is **mandatory**, not an optimization |
| Beds aren't all rectangles | 43 presets non-4-point (Artillery M1 Pro: 239 points) | `BedShape` needs a first-class `Polygon` variant |
| Multi-toolhead is common | 176 / 1173 presets have >1 `nozzle_diameter` entry (Prusa XL 5T: `printer_variant "0.25"`, `nozzle_diameter ["0.25"×5]`) | `nozzleDiameterMm` must be an array; a scalar breaks silently |
| App identifier is `farm3d` | `src-tauri/tauri.conf.json:5` | Config dir is `~/.config/farm3d`. The settings-system spec's `com.jroche.farm3d` is stale — don't propagate it |

## Settled decisions

- Printer *types* derive from OrcaSlicer's machine preset catalog, not from a
  hand-authored list.
- An instance uses **live inheritance + overrides**: a Printer stores only the
  fields it overrides; everything else resolves through its catalog variant.
  Editing the catalog propagates to non-overriding instances.
- The catalog snapshot is generated from a **pinned upstream tag** and is the
  **only runtime source** — no scanning of a local OrcaSlicer install. See
  below.
- Printers persist in a **separate `printers.json`**, not inside `settings.json`.
- Override resolution happens in **Rust**; the frontend receives a pre-resolved
  profile.

## Data model

- **Catalog** (read-only, derived): `PrinterModel` → `PrinterVariant[]`. Never
  user-edited.
- **Printer** (user-owned, persisted): identity (`id`, `name`), a catalog
  reference, and a *sparse* `overrides` object. No capability data is stored —
  it resolves through the catalog.

Three Centauri Carbons are three `Printer` records sharing a `modelId`. Grouping
in the UI is `groupBy(modelId)`, not a stored group entity.

## Catalog snapshot: derived from a pinned upstream tag

Vendoring `resources/profiles/` verbatim is **78 MB** of upstream project data
including g-code templates and vendor prose. Instead a generator resolves the
`inherits` chains and emits only the factual capability fields farm3d reads.

**Source: OrcaSlicer's git repo at a pinned tag.** Verified working end to end:

```sh
git clone --depth 1 --filter=blob:none --sparse --branch v2.4.2 \
  https://github.com/OrcaSlicer/OrcaSlicer.git <tmp>
git -C <tmp> sparse-checkout set resources/profiles
```

~1 second, 66 vendor bundles, discarded after generation. Running the ingestion
over it produced **358 models / 927 variants / 278 KB raw / 14 KB gzipped**,
essentially matching a full local install's 361/939 (the small delta is a
newer-than-tag local install, which is exactly why pinning is worth doing).

`just gen-catalog [tag]` defaults to a pinned tag constant in the justfile;
bumping the catalog is a one-line, reviewable diff.

**Two upstream corrections found while verifying this:**

- The repo has moved: `SoftFever/OrcaSlicer` → **`OrcaSlicer/OrcaSlicer`** (the
  old URL 301s).
- The license is **AGPL-3.0**, not GPL-3.0. `docs/adr/0003-wrap-orcaslicer.md:6`
  says "OrcaSlicer is GPLv3" and is stale — corrected in the same commit as ADR
  0007 (below). This doesn't change ADR-0003's conclusion — invoking a separate
  process remains the right mitigation, and AGPL's network clause is moot for a
  local desktop app — but the stated fact was wrong.

**The generator must allowlist fields, never blocklist.** Copy exactly the
enumerated `CatalogVariant` fields and nothing else — never
`machine_start_gcode`/`machine_end_gcode`/`change_filament_gcode` (creative
expression), never `bed_model`/`bed_texture`/`hotend_model` (filenames pointing
at AGPL'd binaries), never vendor descriptions. This allowlist rule is what makes
"derived factual data" true rather than aspirational. It's recorded in a new
**ADR 0007 — the printer catalog is derived factual data from OrcaSlicer
profiles**. Ship the attribution notice inside the JSON (upstream URL, license,
pinned tag, per-vendor `version` strings) and echo one line in `README.md`.

### The bundled snapshot is the only runtime source

No scanning of local OrcaSlicer installs. The catalog is exactly what shipped
with the running farm3d build, and changes only when farm3d releases a new one.

- **The catalog is immutable at runtime** — `tauri::State<Arc<Catalog>>`, parsed
  once in `setup()` (278 KB, single-digit ms). No `RwLock`, no background thread,
  no hot-swap, no event, no store re-fetch listener.
- **No platform path discovery** — no flatpak/`/usr/share`/`.app`/`%APPDATA%`
  matrix, no AppImage gap, no `orcaslicerDataDir` setting.
- **No merge logic** — no per-vendor version comparison, no reconciliation.
- **Resolution is deterministic.** The same `printers.json` resolves identically
  on every machine running the same farm3d version — `printers.json` is
  explicitly a portable, hand-editable document, so this matters.
- **Catalog change is release-gated and reviewable** — bounded, correlated with
  a farm3d release, mentionable in release notes.

The cost: a user who pulls a vendor profile update inside OrcaSlicer won't see
it in farm3d until farm3d ships a snapshot bump. Given vendor bundles move on
the order of months and the bump is a one-line diff, that's the right trade —
and **regenerating the catalog should be its own commit**, reviewed via the two
snapshot tests (below) plus a model/variant count diff.

The `inherits` walk, `printable_area` normalization, and field allowlisting live
**only** on the generator path (`bin/gen-catalog.rs`) — the runtime just
deserializes a flat, already-resolved JSON document. They still get the full
fixture test suite; they just never run in the shipped app.

## Persistence: a separate `printers.json`

Not inside `settings.json`, for three reasons:

1. `updateSettings()` rewrites the whole file on every theme toggle
   (`src/settings/settings-store.ts:44`) — a theme flip should not rewrite the
   Farm.
2. **Failure semantics must differ.** `load_settings_from` silently falls back
   to defaults on a corrupt file (`src-tauri/src/settings.rs:41`); worst case is
   the wrong theme. Doing that to Printers means "your Farm vanished" *and the
   next save overwrites it with `[]`*. Printers must **quarantine** to
   `printers.json.corrupt-<ts>`, start empty, and surface an error banner.
3. Settings are preferences; Printers are documents that get backed up and
   copied between machines.

Everything else follows `settings.rs` exactly: `std::fs` + `serde_json`, pure
path-taking helpers (`load_printers_from(&Path)`) with thin `#[tauri::command]`
wrappers resolving `app.path().app_config_dir()`,
`serde(rename_all = "camelCase", default)`, `to_string_pretty`.

### Rust types (`src-tauri/src/printers.rs`)

```rust
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct StoredPrinter {
    pub id: String,
    pub name: String,
    pub catalog_ref: CatalogRef,
    pub group: String,                    // bay/room label; "" = ungrouped
    pub notes: String,
    #[serde(skip_serializing_if = "PrinterProfileOverrides::is_empty")]
    pub overrides: PrinterProfileOverrides,
    /// Resolved profile as of last user confirmation. NOT consulted while
    /// `catalog_ref` resolves — it is the dangling-ref fallback and drift baseline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_known_good: Option<LastKnownGood>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<ConnectionConfig>,   // phase 2; unused here
}

/// Identity recorded four ways so a renamed preset can be re-matched.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CatalogRef {
    pub vendor: String,          // "Elegoo"
    pub model: String,           // "Elegoo Centauri Carbon"
    pub variant: String,         // "Elegoo Centauri Carbon 0.4 nozzle"
    pub model_id: String,        // "Elegoo-CC"
    pub printer_variant: String, // "0.4"
}
```

### Overrides: a typed struct with all-`Option` fields

```rust
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PrinterProfileOverrides {
    #[serde(skip_serializing_if = "Option::is_none")] pub bed_shape: Option<BedShape>,
    #[serde(skip_serializing_if = "Option::is_none")] pub printable_height_mm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")] pub bed_exclude_areas: Option<Vec<PointMm>>,
    #[serde(skip_serializing_if = "Option::is_none")] pub default_bed_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub has_auxiliary_fan: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")] pub supports_air_filtration: Option<bool>,
    /// Unknown/future keys preserved verbatim so an older farm3d never destroys
    /// a newer one's data. Surfaced as `unknownOverrideKeys`, never as typed TS.
    #[serde(flatten)] pub extra: serde_json::Map<String, serde_json::Value>,
}
```

`HashMap<String, Value>` was rejected: it's stringly-typed end to end, a
hand-edit typo silently no-ops, and the UI still needs a separate registry to
know what's overridable — so you pay the typed cost anyway plus the untyped
one. The typed struct gives, for free: `skip_serializing_if` ⇒ the file contains
only what's overridden; `Option::is_some` ⇒ the overridden-key set; a TS mirror
that is exactly `Partial<Pick<PrinterProfile, OverridableField>>`. The `flatten
extra` bucket recovers the HashMap's one real advantage.

**Ship 6 overridable fields, not everything** — only what a user has a physical
reason to change (bed width/depth, printable height, bed exclusion, bed type,
aux fan, air filtration). Exposing every catalog field invites bug reports about
fields with no observable effect until Slicing exists.

**Nozzle diameter, nozzle type, and gcode flavor are deliberately NOT
overridable.** In OrcaSlicer these are *variant identity*, and phase 2/3 will
invoke the OrcaSlicer CLI with a **preset name**, not a field bag. A Printer
whose ref says `0.4` and whose override says `0.6` is unsliceable-correctly.
Changing nozzle is a **rebind** that re-points `catalogRef.variant` at the
sibling variant.

### On-disk shape

```json
{
  "schemaVersion": 1,
  "printers": [{
    "id": "prn-8f2a",
    "name": "Centauri Carbon — Bay 1",
    "catalogRef": { "vendor": "Elegoo", "model": "Elegoo Centauri Carbon",
                    "variant": "Elegoo Centauri Carbon 0.4 nozzle",
                    "modelId": "Elegoo-CC", "printerVariant": "0.4" },
    "group": "Bay 1",
    "overrides": { "supportsAirFiltration": false },
    "lastKnownGood": { "profile": { "…": "…" }, "catalogVersion": "02.04.00.06",
                       "resolvedAt": "2026-08-20T14:02:11Z" }
  }]
}
```

## Override resolution: in Rust, pre-resolved to the frontend

The catalog only exists in Rust; resolving in TS means shipping the catalog to
TS. Phase 2/3 will resolve the same profile in Rust to build the OrcaSlicer
invocation — two implementations would diverge. And the `inherits` walk is
exactly the logic most worth fixture-testing.

The frontend gets everything the UI needs per field in one payload:

```ts
export interface ResolvedPrinter {
  id: string; name: string; group: string; notes: string;
  catalogRef: CatalogRef;
  catalogStatus: "ok" | "rematched" | "variantMissing" | "modelMissing" | "vendorMissing";
  modelLabel: string; variantLabel: string;
  profile: PrinterProfile;                 // effective: catalog resolved, then overrides
  overriddenFields: OverridableField[];
  inherited: Partial<PrinterProfile>;      // catalog value for overridden fields — powers revert
  profileDrift: { field: string; from: unknown; to: unknown }[];
  unknownOverrideKeys: string[];
  connection: ConnectionConfig | null;     // always null in phase 1
}
```

Pure, testable core:

```rust
pub fn resolve_printer(catalog: &Catalog, stored: &StoredPrinter) -> ResolvedPrinter;
pub fn resolve_catalog_ref(catalog: &Catalog, r: &CatalogRef) -> (Option<&CatalogVariant>, CatalogStatus);
pub fn merge_profile(base: &PrinterProfile, ov: &PrinterProfileOverrides)
    -> (PrinterProfile, Vec<&'static str>);
```

`set_printer_override` avoids ten match arms by round-tripping the typed struct
through `serde_json::Value` — the re-deserialize *is* the type validation, and
`value: None` means revert-to-inherited. Adding a field in phase 2 touches only
the struct and `OVERRIDABLE_FIELDS`.

### Commands (registered in `lib.rs`'s `invoke_handler!`)

`list_printers`, `create_printer`, `update_printer`, `delete_printer`,
`set_printer_override`, `rebind_printer`, `resolve_profile_drift`,
`open_printers_file`; read-only catalog: `list_catalog_models`,
`list_catalog_variants`, `preview_profile`, `catalog_info`.

Every mutation returns the freshly-resolved `ResolvedPrinter` so the store
splices one row instead of re-fetching — resolution stays authoritative in
Rust, one round trip.

## Frontend state

`src/printers/printer-store.ts` keeps `settings-store.ts`'s shape — module-scope
singleton, `isTauri()` guard, no context provider — but backs it with
`createStore` from `solid-js/store` (already a dependency; zero new packages).
Settings could get away with plain functions because `theme-engine.ts` carries
its own subscriber list; Printers need reactive reads in three places (grid,
detail aside, status bar), and hand-rolling a third subscriber list is strictly
worse.

**`PrinterDashboard` stays a pure props component** — it must not import the
store. That keeps it mountable with a fixture array and zero mocks, and keeps
`src/screens/` store-agnostic. `App.tsx` is the single wiring point:

```tsx
onMount(() => { void loadPrinters(); });
// …
<PrinterDashboard printers={printers()} onAddPrinter={addPrinter} … />
```

Solid compiles JSX props to getters, so `printers={printers()}` is already
reactive — no `createResource`, no provider, no component-tree refactor.

Under `just web` the store seeds from a fixture (today's `PRINTERS` array
promoted to `ResolvedPrinter` shape) and mutations stay in-memory, so the whole
UI is buildable without the Rust backend. No event listener is needed — the
catalog can't change while the app is running.

**One deliberate exception:** the add dialog imports `printer-catalog.ts`
directly — threading 361 models through props is silly. It's `vi.mock`-ed in
its test.

## UI

**Grouping** — restructure `PrinterDashboard.module.css`'s single `.grid` into
sections, keeping `repeat(auto-fill, minmax(15rem, 1fr))` *inside* each. Sticky
group header (outliner convention, matches the Blender/Godot aesthetic; not
Kobalte `Accordion` — always-expanded is simpler and denser). Groups sorted
alphabetically, `catalogStatus !== "ok"` printers in an "Unlinked" group last.
Export `groupPrintersByModel(printers)` as a pure function alongside the
existing `summarizePrinters` precedent. Cards gain small badges for Unlinked /
Profile updated / *n* overrides.

**Add-printer flow** — `PrinterAddDialog.tsx`, one `Dialog`, three stacked
fields, no wizard chrome: a **Combobox** over the 361 models labelled
`"<Vendor> · <Model>"`; a plain `Select` for the nozzle variant (1–8 per model,
auto-selected when there's one, defaulting to `0.4`); a `TextField` for the
name. Plus a live read-only summary from `preview_profile`
(`"256 × 256 × 256 mm · 0.4 mm nozzle · textured PEI"`). Kobalte's Combobox does
**not** filter for you — handle `onInputChange` and filter in TS over 361 short
strings, capping the rendered list at ~50 rather than virtualizing.

**Per-instance config editor: the existing resizable detail aside** — not a
Dialog (blocks the grid you're comparing against), not a new screen (would
force a router the repo doesn't have). ADR-0006's inspector framing extends
naturally. Wrap the aside body in the existing `Tabs`: **Status | Profile |
Connection** (the last a phase-2 placeholder). Bump `DEFAULT_DETAIL_WIDTH`
256→320 and `MAX_DETAIL_WIDTH` 480→560.

`PrinterProfilePanel.tsx` renders one row per overridable field:

```
│ Printable height                     [ 240 ▲▼ ] mm  ⟲
  inherited: 256
```

Overridden rows get `border-left: 2px solid var(--f3d-color-accent)` and a
revert `IconButton` in a `Tooltip` reading `Revert to inherited (256)` —
Blender's modified-property convention, pure CSS plus existing components.
Debounced (~300 ms) live apply, no Save button, matching the theme/settings
precedent. Polygon bed shapes render **read-only** with an "edit printers.json
to change" hint — no polygon editor in phase 1.

**Three new design-system components** (Kobalte has both primitives unwrapped
today at `node_modules/@kobalte/core/src/{number-field,combobox}`):

| Component | Primitive | Note |
|---|---|---|
| `NumberField` | `number-field` | **Wire `rawValue`/`onRawValueChange` (numeric), not `value`/`onChange` (formatted strings).** Props: `label`, `value?: number`, `onChange?`, `minValue`, `maxValue`, `step`, `suffix?`, `error` |
| `Combobox` | `combobox` | Mirror `SelectProps<T>` + `onInputChange` + `groups`. Reuse `Select.module.css` via CSS Modules `composes` |
| `Field` | none (plain, like `Panel`) | The inherited/overridden row: `{label, hint?, overridden?, onRevert?, children}`. ~40 lines |

All three go in `components/index.ts` **and** `Showcase.tsx` per `AGENTS.md`.

**No form/validation layer.** Six fields, native `NumberField` clamping, no
submit step. The only real validation — non-empty printer name — is a 3-line
`createMemo` feeding `TextField`'s existing `error` prop.

## Risks and mitigations

Live inheritance's failure mode is a **silent wrong answer**, not a UI glitch: a
renamed or removed preset leaves the profile *absent*, and a phase-2 Slice
generated against the wrong build volume is a failed print. Three mitigations,
all cheap, and together they're what make the live-inheritance choice
defensible:

- **Pin identity four ways, resolve in a fixed cascade** — exact variant name →
  `(vendor, modelId, printerVariant)` → `(vendor, normalized(model),
  printerVariant)`. A non-exact match sets `catalogStatus: "rematched"` and
  shows a one-time banner with **Confirm** / **Pick manually**. **Never rematch
  across vendors or to a different `modelId`.**
- **`lastKnownGood`** — ~15 lines of JSON per printer, written on create and on
  every user confirmation. Not consulted while the ref resolves (live
  inheritance still wins), but a dangling ref degrades to a *correct, complete,
  flagged* profile instead of an absent one, and `printers.json` becomes
  self-describing when read by a human or restored from backup.
- **Diff on load; never drift silently** — resolve each printer at startup and
  diff against `lastKnownGood.profile` into `profileDrift[]`. A "Profile
  updated" chip on the card, a diff list in the Profile tab, and two buttons:
  **Accept** (rebaseline) or **Keep my value** (one-click convert the drifted
  fields to explicit overrides). This preserves the entire benefit of live
  inheritance — upstream fixes propagate — while removing the
  silent-wrong-answer mode.

Plus: **quarantine a corrupt `printers.json`** rather than defaulting to empty,
and **warn on unknown override keys** via `unknownOverrideKeys` so
hand-editability doesn't become a silent-typo trap.

Snapshot-only sourcing shrinks these risks rather than removing them. Dangling
refs and drift can now only appear when *farm3d itself* ships a snapshot bump —
bounded, release-correlated, and visible in a diff at review time.
`catalog_info` reports the snapshot's pinned tag and per-vendor versions, which
is what makes a drift banner explainable ("updated in farm3d 0.3.0") rather than
mysterious. The one risk that disappears outright is nondeterminism: the same
`printers.json` now resolves identically on every machine running the same
farm3d version.

**If phase 1 needs trimming,** the drift machinery (`profileDrift` +
Accept/Keep buttons) is the one piece that could ship in a follow-up — but
`lastKnownGood` should be written from day one regardless, since retrofitting it
can't recover baselines for printers already created.

## Implementation sequence

**Generator-only** (`src-tauri/src/catalog/ingest/`, reachable from
`bin/gen-catalog.rs`; never runs in the shipped app):

1. `inherits.rs` (the chain walk), `shape.rs` (`printable_area` → `BedShape`),
   `ingest.rs` (allowlisted field extraction). Pure Rust, no Tauri, no UI.
   Highest risk; do it first, against fixtures.
2. `bin/gen-catalog.rs` + `just gen-catalog [tag]` — sparse-clones the pinned
   OrcaSlicer tag into a temp dir, ingests, discards. Commit
   `src-tauri/resources/printer-catalog.json`; add `bundle.resources` to
   `tauri.conf.json`; write **ADR 0007** and amend ADR-0003's GPLv3 → AGPL-3.0.

**Runtime:**

3. `catalog/mod.rs` — the snapshot types + `load_snapshot(&Path)`.
   Deserialization only; no resolution logic.
4. `printers.rs` — structs, path-taking IO, quarantine, `apply_override` +
   tests.
5. `CatalogState` (`Arc<Catalog>`) in `setup()`, commands, `invoke_handler!`
   registration.
6. Design system: `NumberField`, `Combobox`, `Field` + `index.ts` +
   `Showcase.tsx` + component tests. *(Parallelizable with 1–5.)*
7. `src/printers/` — `types.ts`, `printer-store.ts`, `printer-catalog.ts`.
8. `PrinterDashboard` regroup + `groupPrintersByModel` + `App.tsx` rewiring
   (delete the `PRINTERS` const and the `Printer` interface).
9. `PrinterAddDialog`, `PrinterProfilePanel`, drift/rematch banners.

`summarizePrinters` keeps its signature; `status` moves to a phase-3
`runtimeStatus?: PrinterStatus` that is `undefined` in phase 1, so the status
bar reads "3 printers" with counts stubbed until polling lands.

## Files

**Create:** `src-tauri/src/printers.rs`, `src-tauri/src/catalog/mod.rs`,
`src-tauri/src/catalog/ingest/{mod,inherits,shape}.rs` *(generator-only)*,
`src-tauri/src/bin/gen-catalog.rs`, `src-tauri/resources/printer-catalog.json`,
`src-tauri/tests/fixtures/profiles/**`,
`src/printers/{types,printer-store,printer-catalog}.ts`,
`src/screens/{PrinterAddDialog,PrinterProfilePanel}.tsx` (+ `.module.css`,
`.test.tsx`), `src/design-system/components/{NumberField,Combobox,Field}.tsx`
(+ `.module.css`), `docs/adr/0007-printer-catalog-is-derived-data.md`

**Modify:** `src-tauri/src/lib.rs`, `src-tauri/tauri.conf.json`, `src/App.tsx`,
`src/screens/PrinterDashboard.{tsx,module.css}`,
`src/design-system/components/{index.ts,components.test.tsx}`,
`src/design-system/Showcase.tsx`, `justfile`, `README.md`,
`docs/adr/0003-wrap-orcaslicer.md` (GPLv3 → AGPL-3.0 correction), `CONTEXT.md`
(if `Printer Model` needs adding to the vocabulary)

## Verification

**Rust** (`just test-rust`; prefix `source "$HOME/.cargo/env" &&` in
non-interactive shells per `AGENTS.md`) — following `settings.rs`'s temp-dir
pattern against pure path-taking fns:

- `printers.rs`: round-trip; missing file → empty list; **corrupt file → `Err`
  + quarantine file exists + original preserved**; `skip_serializing_if`
  produces a file with only overridden keys (assert the exact JSON string, like
  the existing `json_uses_camel_case_keys` test); unknown `overrides` keys
  survive load→save; `apply_override` with a wrong-typed value returns `Err`
  and leaves the struct untouched.
- `catalog/ingest/` — **the highest-value tests here**, even though this code
  never runs in the shipped app: a bad ingestion silently ships a wrong
  catalog. A small synthetic fixture tree at
  `src-tauri/tests/fixtures/profiles/`, each case mirroring a real condition: a
  leaf with no `printable_area` needing a chain walk; a 3-deep chain ending at
  `fdm_machine_common`; a non-rectangular `printable_area` →
  `BedShape::Polygon`; a 5-toolhead variant (`nozzle_diameter: ["0.4"×5]`); an
  `inherits` **cycle** (must error, not hang); a **dangling** `inherits` (must
  not panic).
- `resolve_printer`: inherited-only; single override; override equal to the
  inherited value (still counts as overridden); each `CatalogStatus`.
- Two non-ignored tests over the *committed* `printer-catalog.json`, both cheap
  (278 KB) and both catching a bad regeneration at PR time:
  - `snapshot_is_complete()` — every variant resolves a `bedShape` and a
    `printableHeightMm`.
  - `snapshot_carries_no_disallowed_fields()` — the serialized snapshot
    contains no `gcode`, `bed_model`, `bed_texture`, or `hotend_model`
    substring. This is the executable form of the allowlist rule that ADR 0007
    rests on; without it the licensing argument decays the first time someone
    adds a field.

**TypeScript** (`just test`, `just build`) — colocated,
`@solidjs/testing-library`:

- `printer-store.test.ts` — near-copy of `settings-store.test.ts`: `vi.hoisted`
  tauri mock, `vi.resetModules()` in `beforeEach` (**mandatory**; module-level
  singleton state), both `isTauri()` branches; assert `revertField` sends
  `value: null` and that web-fallback mutations never invoke.
- `PrinterDashboard.test.tsx` — `groupPrintersByModel` as pure unit tests;
  mount a 3-printer / 2-model fixture and assert two headers with correct
  counts; a `variantMissing` printer lands in "Unlinked".
- `PrinterProfilePanel.test.tsx` — inherited row has no revert button,
  overridden does; `fireEvent.input` on the NumberField input calls
  `onOverrideField` after the debounce (`vi.useFakeTimers`).
- `PrinterAddDialog.test.tsx` — `vi.mock("../printers/printer-catalog")`; per
  `AGENTS.md`, Combobox opens on `fireEvent.pointerDown` and selects on
  `fireEvent.pointerUp`, typing is `fireEvent.input`. Confirm during
  implementation whether Combobox needs an `IntersectionObserver` polyfill in
  `vitest.setup.ts` alongside the existing `ResizeObserver` stub.

**End to end** — `just gen-catalog` regenerates the snapshot from the pinned
OrcaSlicer tag; at `v2.4.2` it must produce 358 models / 927 variants. Then
`just dev` and confirm visually per `AGENTS.md`: add three Centauri Carbons at
different names, see them grouped under one "Elegoo Centauri Carbon" header;
override air filtration on one and confirm the accent border, the revert
button, and that the other two are unaffected; revert and confirm the value
returns to inherited; inspect `~/.config/farm3d/printers.json` and confirm it
contains only the overridden key. Then hand-edit the file to a bogus
`catalogRef.variant`, relaunch, and confirm the printer appears under
"Unlinked" with its `lastKnownGood` profile rather than vanishing.

## Out of scope

- Connection config, "Test connection", live status polling — phases 2 and 3.
- A polygon bed-shape editor — read-only in phase 1.
- Scanning a local OrcaSlicer install for a fresher catalog — deliberately
  rejected; see "The bundled snapshot is the only runtime source" above.
- A user-defined grouping UI beyond the persisted-but-unedited `group` field.
