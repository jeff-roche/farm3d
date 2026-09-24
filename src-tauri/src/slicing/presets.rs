//! D3: the preset index over a preset source's `resources/profiles`, preset
//! flattening, the process and filament presets offered for a machine
//! preset, the default choices, and `list_slice_options`.
//!
//! The index reads every `<Vendor>.json` bundle once. It keeps a small
//! summary of each preset (the keys D3's filters read, plus `inherits`), the
//! union of keys per preset kind (D4's known-key check), and each preset's
//! file path. Full presets are read from disk only when one is flattened.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::catalog::ingest::inherits::{
    resolve_preset, InheritsError, InheritsErrorKind, PresetLookup,
};
use crate::catalog::resolve::{resolve_catalog_ref, resolve_printer, CatalogStatus};
use crate::catalog::{Catalog, CatalogVariant, PrinterProfile};
use crate::contracts::command::CommandError;
use crate::library::content::CancelFlag;
use crate::persistence::{RepositoryError, Storage};
use crate::printers::repository::PrinterRepository;
use crate::printers::CatalogRef;
use crate::spools::MaterialFamily;

use super::{
    FilamentPresetOption, ProcessPresetOption, ProfileSnapshot, SliceOptionDefaults, SliceOptions,
    SliceTarget,
};

/// D3: the three preset kinds a bundle lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PresetKind {
    Machine,
    Process,
    Filament,
}

impl PresetKind {
    pub const ALL: [PresetKind; 3] = [Self::Machine, Self::Process, Self::Filament];

    /// The kind's name in errors: `machine`, `process`, or `filament`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Machine => "machine",
            Self::Process => "process",
            Self::Filament => "filament",
        }
    }

    fn bundle_list(self) -> &'static str {
        match self {
            Self::Machine => "machine_list",
            Self::Process => "process_list",
            Self::Filament => "filament_list",
        }
    }
}

/// The keys a preset summary keeps: what D3's filters and defaults read.
const SUMMARY_KEYS: [&str; 6] = [
    "inherits",
    "instantiation",
    "compatible_printers",
    "compatible_printers_condition",
    "filament_type",
    "filament_diameter",
];

/// OrcaSlicer's shared filament bundle. A parent missing from a preset's
/// own bundle is looked up here next, as OrcaSlicer does.
pub const FILAMENT_LIBRARY_VENDOR: &str = "OrcaFilamentLibrary";

#[derive(Debug)]
struct IndexedPreset {
    /// The bundle's position in the index's `vendors`.
    vendor: usize,
    path: PathBuf,
    /// `None` when the file is missing or isn't a JSON object.
    summary: Option<Value>,
}

/// The presets of one kind.
#[derive(Debug, Default)]
struct KindIndex {
    entries: Vec<IndexedPreset>,
    /// By name: the first entry in sorted vendor order.
    by_name: HashMap<String, usize>,
    /// By bundle and name.
    by_vendor: HashMap<(usize, String), usize>,
    /// The union of every preset's own keys (D4).
    known_keys: BTreeSet<String>,
}

/// Why the index could not be built.
#[derive(Debug, PartialEq, Eq)]
pub enum PresetIndexError {
    /// The profiles directory can't be listed.
    Unreadable,
    Cancelled,
}

/// D3: every preset of one preset source, by `(kind, name)`. Built once per
/// preset-source identity; the caller caches it.
///
/// A name picks the first entry in sorted vendor order. A parent named by
/// `inherits` is looked up in the inheriting preset's own bundle first,
/// then in `OrcaFilamentLibrary`, and only then by name. Vendors reuse
/// base names (`fdm_process_common`, `fdm_filament_pla`, …) with different
/// values: resolving them by name alone would give an Elegoo preset
/// Afinia's base.
#[derive(Debug)]
pub struct PresetIndex {
    profiles_dir: PathBuf,
    version: String,
    vendors: Vec<String>,
    kinds: HashMap<PresetKind, KindIndex>,
}

impl PresetIndex {
    /// Indexes every `<Vendor>.json` bundle in `profiles_dir`, in sorted
    /// vendor order. `version` is the preset source's OrcaSlicer version,
    /// for errors. A bundle that isn't valid JSON is skipped.
    pub fn build(
        profiles_dir: &Path,
        version: &str,
        cancel: &CancelFlag,
    ) -> Result<Self, PresetIndexError> {
        if !profiles_dir.is_dir() {
            return Err(PresetIndexError::Unreadable);
        }
        let bundles = vendor_bundles(profiles_dir);

        let mut index = Self {
            profiles_dir: profiles_dir.to_path_buf(),
            version: version.to_string(),
            vendors: Vec::new(),
            kinds: PresetKind::ALL
                .map(|kind| (kind, KindIndex::default()))
                .into(),
        };
        for bundle_path in bundles {
            let Some(vendor) = bundle_path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let Some(bundle) = read_json_object(&bundle_path) else {
                continue;
            };
            let vendor_id = index.vendors.len();
            index.vendors.push(vendor.to_string());
            for kind in PresetKind::ALL {
                let presets = index.kinds.get_mut(&kind).expect("every kind is indexed");
                let entries = bundle
                    .get(kind.bundle_list())
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for entry in entries {
                    if cancel.is_cancelled() {
                        return Err(PresetIndexError::Cancelled);
                    }
                    let (Some(name), Some(sub_path)) = (
                        entry.get("name").and_then(Value::as_str),
                        entry.get("sub_path").and_then(Value::as_str),
                    ) else {
                        continue;
                    };
                    let key = (vendor_id, name.to_string());
                    if presets.by_vendor.contains_key(&key) {
                        continue;
                    }
                    let path = profiles_dir.join(vendor).join(sub_path);
                    let preset = read_json_object(&path);
                    if let Some(Value::Object(fields)) = &preset {
                        presets.known_keys.extend(fields.keys().cloned());
                    }
                    let position = presets.entries.len();
                    presets.entries.push(IndexedPreset {
                        vendor: vendor_id,
                        path,
                        summary: preset.map(summarize),
                    });
                    presets.by_vendor.insert(key, position);
                    presets.by_name.entry(name.to_string()).or_insert(position);
                }
            }
        }
        Ok(index)
    }

    pub fn profiles_dir(&self) -> &Path {
        &self.profiles_dir
    }

    /// The preset source's OrcaSlicer version.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// How many `<Vendor>.json` bundles were read.
    pub fn vendor_count(&self) -> usize {
        self.vendors.len()
    }

    pub fn contains(&self, kind: PresetKind, name: &str) -> bool {
        self.kinds[&kind].by_name.contains_key(name)
    }

    /// Where `name` is found when inherited from a preset of bundle `from`
    /// (see [`PresetIndex`]), or by name alone when `from` is `None`.
    fn find(&self, kind: PresetKind, name: &str, from: Option<usize>) -> Option<&IndexedPreset> {
        let presets = &self.kinds[&kind];
        let in_vendor = |vendor: usize| presets.by_vendor.get(&(vendor, name.to_string()));
        let library = self
            .vendors
            .iter()
            .position(|vendor| vendor == FILAMENT_LIBRARY_VENDOR);
        let position = match from {
            None => presets.by_name.get(name),
            Some(vendor) => in_vendor(vendor)
                .or_else(|| library.and_then(in_vendor))
                .or_else(|| presets.by_name.get(name)),
        }?;
        presets.entries.get(*position)
    }

    /// D4: a key is known when at least one preset of `kind` sets it. One
    /// flat preset is not enough, because presets leave out keys that keep
    /// their default (Gate B).
    pub fn is_known_key(&self, kind: PresetKind, key: &str) -> bool {
        self.kinds[&kind].known_keys.contains(key)
    }

    /// D3: the preset `name` with its `inherits` chain merged (parent keys
    /// first, the child's winning), `inherits` removed, and `name` and
    /// `from: "system"` kept. v2.4.2 refuses a `from: "User"` process preset
    /// (Gate B). A missing preset is `PRESET_NOT_FOUND`; a cycle, a chain
    /// deeper than 20, a missing parent, or an unreadable file is
    /// `PRESET_INVALID`.
    pub fn flatten(
        &self,
        kind: PresetKind,
        name: &str,
    ) -> Result<Map<String, Value>, CommandError> {
        if !self.contains(kind, name) {
            return Err(CommandError::preset_not_found(
                kind.label(),
                name,
                &self.version,
            ));
        }
        let lookup = FileLookup { index: self, kind };
        let merged = resolve_preset(&lookup, name)
            .map_err(|error| CommandError::preset_invalid(kind.label(), name, &error.message))?;
        let Value::Object(mut flat) = merged else {
            unreachable!("the resolver always merges into an object");
        };
        flat.remove("inherits");
        flat.insert("name".to_string(), Value::String(name.to_string()));
        flat.insert("from".to_string(), Value::String("system".to_string()));
        Ok(flat)
    }

    /// The preset's summary keys with its chain merged, or `None` when the
    /// chain can't be resolved.
    fn resolved_summary(&self, kind: PresetKind, name: &str) -> Option<Map<String, Value>> {
        let lookup = SummaryLookup { index: self, kind };
        match resolve_preset(&lookup, name).ok()? {
            Value::Object(fields) => Some(fields),
            _ => None,
        }
    }

    /// D3: the instantiable presets of `kind` whose flattened
    /// `compatible_printers` names `machine_preset`, in name order. A preset
    /// with an empty `compatible_printers` is never offered, even when it
    /// has a `compatible_printers_condition`: P5 does not evaluate the
    /// condition language.
    pub fn offered(&self, kind: PresetKind, machine_preset: &str) -> Vec<OfferedPreset> {
        let mut offered: Vec<OfferedPreset> = self.kinds[&kind]
            .by_name
            .keys()
            .filter_map(|name| {
                let summary = self.resolved_summary(kind, name)?;
                is_offered(&summary, machine_preset).then(|| OfferedPreset {
                    name: name.clone(),
                    filament_type: first_string(summary.get("filament_type")),
                })
            })
            .collect();
        offered.sort_by(|a, b| a.name.cmp(&b.name));
        offered
    }

    /// D3: farm3d checks filament compatibility itself, at validation and
    /// again at start, because OrcaSlicer silently slices with an
    /// incompatible filament (Gate F). A mismatch is
    /// `FILAMENT_INCOMPATIBLE`.
    pub fn check_filament_compatible(
        &self,
        filament_preset: &str,
        machine_preset: &str,
    ) -> Result<(), CommandError> {
        self.check_offered(PresetKind::Filament, filament_preset, machine_preset)
            .map_err(|error| {
                error.unwrap_or_else(|| {
                    CommandError::filament_incompatible(filament_preset, machine_preset)
                })
            })
    }

    /// The process preset must be offered for the machine too: v2.4.2
    /// refuses an incompatible process preset (Gate B, code -17).
    pub fn check_process_compatible(
        &self,
        process_preset: &str,
        machine_preset: &str,
    ) -> Result<(), CommandError> {
        self.check_offered(PresetKind::Process, process_preset, machine_preset)
            .map_err(|error| {
                error.unwrap_or_else(|| {
                    CommandError::validation_at(
                        "processPreset",
                        format!(
                            "The process preset \"{process_preset}\" is not made for \"{machine_preset}\"."
                        ),
                    )
                })
            })
    }

    /// `Err(Some(_))` when the preset is missing or broken, `Err(None)` when
    /// it resolves but is not offered.
    fn check_offered(
        &self,
        kind: PresetKind,
        name: &str,
        machine_preset: &str,
    ) -> Result<(), Option<CommandError>> {
        if !self.contains(kind, name) {
            return Err(Some(CommandError::preset_not_found(
                kind.label(),
                name,
                &self.version,
            )));
        }
        let summary = self.resolved_summary(kind, name).ok_or_else(|| {
            Some(
                self.flatten(kind, name)
                    .err()
                    .unwrap_or_else(CommandError::internal),
            )
        })?;
        if is_offered(&summary, machine_preset) {
            Ok(())
        } else {
            Err(None)
        }
    }
}

/// The `<Vendor>.json` bundles in `profiles_dir`, in sorted order. A bundle
/// is a JSON file with a `<Vendor>/` directory beside it, so other JSON
/// files there (v2.4.2's `blacklist.json`) are not vendors.
pub fn vendor_bundles(profiles_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(profiles_dir) else {
        return Vec::new();
    };
    let mut bundles: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.is_file()
                && path.extension().is_some_and(|ext| ext == "json")
                && path.with_extension("").is_dir()
        })
        .collect();
    bundles.sort();
    bundles
}

fn read_json_object(path: &Path) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text).ok()? {
        value @ Value::Object(_) => Some(value),
        _ => None,
    }
}

fn summarize(preset: Value) -> Value {
    let Value::Object(fields) = preset else {
        return Value::Object(Map::new());
    };
    Value::Object(
        fields
            .into_iter()
            .filter(|(key, _)| SUMMARY_KEYS.contains(&key.as_str()))
            .collect(),
    )
}

fn is_offered(summary: &Map<String, Value>, machine_preset: &str) -> bool {
    let instantiable = summary.get("instantiation").and_then(Value::as_str) != Some("false");
    let compatible = summary
        .get("compatible_printers")
        .and_then(Value::as_array)
        .is_some_and(|printers| {
            printers
                .iter()
                .any(|printer| printer.as_str() == Some(machine_preset))
        });
    instantiable && compatible
}

/// A scalar string, or the first string of an array (OrcaSlicer stores
/// per-extruder values as arrays).
fn first_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => items.first()?.as_str().map(str::to_string),
        _ => None,
    }
}

/// Reads full presets from disk.
struct FileLookup<'a> {
    index: &'a PresetIndex,
    kind: PresetKind,
}

impl PresetLookup for FileLookup<'_> {
    type Scope = usize;

    fn lookup(
        &self,
        name: &str,
        from: Option<&usize>,
    ) -> Result<Option<(Value, usize)>, InheritsError> {
        let Some(preset) = self.index.find(self.kind, name, from.copied()) else {
            return Ok(None);
        };
        let value = read_json_object(&preset.path).ok_or_else(|| unreadable(name))?;
        Ok(Some((value, preset.vendor)))
    }
}

/// Reads the in-memory summaries.
struct SummaryLookup<'a> {
    index: &'a PresetIndex,
    kind: PresetKind,
}

impl PresetLookup for SummaryLookup<'_> {
    type Scope = usize;

    fn lookup(
        &self,
        name: &str,
        from: Option<&usize>,
    ) -> Result<Option<(Value, usize)>, InheritsError> {
        let Some(preset) = self.index.find(self.kind, name, from.copied()) else {
            return Ok(None);
        };
        let value = preset.summary.clone().ok_or_else(|| unreadable(name))?;
        Ok(Some((value, preset.vendor)))
    }
}

fn unreadable(name: &str) -> InheritsError {
    InheritsError::new(
        InheritsErrorKind::Unreadable,
        name,
        format!("the preset {name:?} can't be read"),
    )
}

/// One offered preset, before it becomes a wire option.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfferedPreset {
    pub name: String,
    /// The flattened `filament_type` (filament presets only).
    pub filament_type: Option<String>,
}

/// D15: an OrcaSlicer `filament_type` as a `MaterialFamily`. A type farm3d
/// doesn't list is `OTHER`, with the raw string kept.
pub fn material_family_for(filament_type: &str) -> (MaterialFamily, Option<String>) {
    match serde_json::from_value::<MaterialFamily>(Value::String(filament_type.to_string())) {
        Ok(MaterialFamily::Other) | Err(_) => {
            (MaterialFamily::Other, Some(filament_type.to_string()))
        }
        Ok(family) => (family, None),
    }
}

/// D3: the offered process whose name contains "Standard", otherwise the
/// first offered process in name order. `offered` is in name order.
pub fn default_process(offered: &[OfferedPreset]) -> Option<String> {
    offered
        .iter()
        .find(|preset| preset.name.contains("Standard"))
        .or_else(|| offered.first())
        .map(|preset| preset.name.clone())
}

/// D3: the first offered filament whose `filament_type` matches a loaded
/// Spool's family (in the order given), otherwise the first "Generic PLA"
/// match, otherwise the first in name order. `offered` is in name order.
pub fn default_filament(
    offered: &[OfferedPreset],
    loaded_families: &[MaterialFamily],
) -> Option<String> {
    let family_match = loaded_families
        .iter()
        .filter(|family| **family != MaterialFamily::Other)
        .find_map(|family| {
            offered.iter().find(|preset| {
                preset
                    .filament_type
                    .as_deref()
                    .is_some_and(|filament_type| material_family_for(filament_type).0 == *family)
            })
        });
    family_match
        .or_else(|| {
            offered
                .iter()
                .find(|preset| preset.name.contains("Generic PLA"))
        })
        .or_else(|| offered.first())
        .map(|preset| preset.name.clone())
}

// ---------------------------------------------------------------------------
// The slice target and `list_slice_options`
// ---------------------------------------------------------------------------

/// A [`SliceTarget`] resolved against the catalog and the Printer store.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedTarget {
    /// Set when the target is a Printer.
    pub printer_id: Option<String>,
    pub catalog_ref: CatalogRef,
    /// D3: the catalog `variant`, which is the OrcaSlicer machine preset name.
    pub machine_preset: String,
    /// The effective profile: the catalog's, with a Printer's overrides.
    pub profile: PrinterProfile,
    /// A Printer's overridden `PrinterProfile` fields (D4 applies them).
    pub overridden_fields: Vec<String>,
    /// Override keys farm3d doesn't know; each one blocks slicing (D4).
    pub unknown_override_keys: Vec<String>,
}

impl ResolvedTarget {
    pub fn profile_snapshot(&self) -> ProfileSnapshot {
        ProfileSnapshot::new(self.catalog_ref.clone(), &self.profile)
    }
}

fn storage_error(error: crate::persistence::StorageError) -> CommandError {
    CommandError::from_repository(RepositoryError::Storage(error))
}

/// Resolves `target`: a Printer through the Printer store and
/// `catalog::resolve`, a catalog profile through `resolve_catalog_ref`. A
/// missing Printer or catalog entry is `NOT_FOUND`. P5 slices with exactly
/// one nozzle (D4), so a multi-nozzle profile is `VALIDATION`.
pub fn resolve_target(
    storage: &Arc<Storage>,
    catalog: &Catalog,
    target: &SliceTarget,
) -> Result<ResolvedTarget, CommandError> {
    let resolved = match target {
        SliceTarget::Printer { printer_id } => {
            let stored = PrinterRepository::new(Arc::clone(storage))
                .get(printer_id)
                .map_err(storage_error)?
                .ok_or_else(|| CommandError::not_found(printer_id.clone()))?;
            let printer = resolve_printer(catalog, &stored);
            let resolution = printer.profile_resolution;
            let catalog_found = matches!(
                resolution.catalog_status,
                CatalogStatus::Ok | CatalogStatus::Rematched
            );
            if !catalog_found && stored.last_known_good.is_none() {
                return Err(CommandError::validation_at(
                    "target",
                    "This Printer's profile is not in the printer catalog.",
                ));
            }
            // A rematched Printer slices the rematched variant, so the
            // snapshot names that variant, not the stored (stale) ref.
            let catalog_ref = resolve_catalog_ref(catalog, &stored.catalog_ref)
                .0
                .and_then(|variant| catalog_ref_for(catalog, variant))
                .unwrap_or(stored.catalog_ref);
            ResolvedTarget {
                printer_id: Some(stored.id),
                catalog_ref,
                machine_preset: resolution.variant_label,
                profile: resolution.profile,
                overridden_fields: resolution.overridden_fields,
                unknown_override_keys: resolution.unknown_override_keys,
            }
        }
        SliceTarget::Profile { catalog_ref } => {
            let (variant, _) = resolve_catalog_ref(catalog, catalog_ref);
            let variant =
                variant.ok_or_else(|| CommandError::not_found(catalog_ref.variant.clone()))?;
            ResolvedTarget {
                printer_id: None,
                catalog_ref: catalog_ref_for(catalog, variant)
                    .unwrap_or_else(|| catalog_ref.clone()),
                machine_preset: variant.variant.clone(),
                profile: PrinterProfile::from(variant),
                overridden_fields: Vec::new(),
                unknown_override_keys: Vec::new(),
            }
        }
    };
    if resolved.profile.nozzle_diameter_mm.len() != 1 {
        return Err(CommandError::validation_at(
            "target",
            "farm3d slices for printers with exactly one nozzle.",
        ));
    }
    Ok(resolved)
}

/// The catalog reference that names `variant` itself: its model's vendor,
/// name, and id, and its own variant names.
fn catalog_ref_for(catalog: &Catalog, variant: &CatalogVariant) -> Option<CatalogRef> {
    let model = catalog
        .models
        .iter()
        .find(|model| model.variants.iter().any(|v| std::ptr::eq(v, variant)))?;
    Some(CatalogRef {
        vendor: model.vendor.clone(),
        model: model.model.clone(),
        variant: variant.variant.clone(),
        model_id: model.model_id.clone(),
        printer_variant: variant.printer_variant.clone(),
    })
}

/// D15: whether two profiles match on the fields the matching-Printer
/// count compares.
fn profiles_match(a: &PrinterProfile, b: &PrinterProfile) -> bool {
    a.bed_shape == b.bed_shape
        && a.printable_height_mm == b.printable_height_mm
        && a.nozzle_diameter_mm == b.nozzle_diameter_mm
        && a.nozzle_type == b.nozzle_type
        && a.gcode_flavor == b.gcode_flavor
}

/// D15: the active (unarchived) Printers whose resolved profile equals
/// `profile` on `bedShape`, `printableHeightMm`, `nozzleDiameterMm`,
/// `nozzleType`, and `gcodeFlavor`, by id. A display count, not an
/// eligibility rule.
pub fn matching_printer_ids(
    storage: &Arc<Storage>,
    catalog: &Catalog,
    profile: &PrinterProfile,
) -> Result<Vec<String>, CommandError> {
    let printers = PrinterRepository::new(Arc::clone(storage))
        .list()
        .map_err(storage_error)?;
    Ok(printers
        .iter()
        .filter(|printer| printer.archived_at.is_none())
        .filter(|printer| {
            profiles_match(
                &resolve_printer(catalog, printer).profile_resolution.profile,
                profile,
            )
        })
        .map(|printer| printer.id.clone())
        .collect())
}

/// The material families of the Spools loaded on `printer_id`, in
/// `spoolNumber` order.
fn loaded_families(
    storage: &Storage,
    printer_id: &str,
) -> Result<Vec<MaterialFamily>, CommandError> {
    storage
        .read_transaction(|tx| Ok(crate::spools::repository::loaded_on_printer(tx, printer_id)))
        .map_err(storage_error)?
        .map(|spools| spools.iter().map(|spool| spool.material_family).collect())
        .map_err(storage_error)
}

/// `list_slice_options`: the target's machine preset, the process and
/// filament presets offered for it, D3's defaults (the material default
/// reads a Printer target's loaded Spools), the target's profile snapshot,
/// and the matching Printers (D15). A machine preset the preset source
/// lacks is `PRESET_NOT_FOUND`.
pub fn list_slice_options(
    storage: &Arc<Storage>,
    catalog: &Catalog,
    index: &PresetIndex,
    target: &SliceTarget,
) -> Result<SliceOptions, CommandError> {
    let resolved = resolve_target(storage, catalog, target)?;
    if !index.contains(PresetKind::Machine, &resolved.machine_preset) {
        return Err(CommandError::preset_not_found(
            PresetKind::Machine.label(),
            &resolved.machine_preset,
            index.version(),
        ));
    }
    let processes = index.offered(PresetKind::Process, &resolved.machine_preset);
    let filaments = index.offered(PresetKind::Filament, &resolved.machine_preset);
    let families = match &resolved.printer_id {
        Some(printer_id) => loaded_families(storage, printer_id)?,
        None => Vec::new(),
    };
    let defaults = SliceOptionDefaults {
        process_preset: default_process(&processes),
        filament_preset: default_filament(&filaments, &families),
    };
    Ok(SliceOptions {
        machine_preset: resolved.machine_preset.clone(),
        process_presets: processes
            .into_iter()
            .map(|preset| ProcessPresetOption { name: preset.name })
            .collect(),
        filament_presets: filaments
            .into_iter()
            .map(|preset| FilamentPresetOption {
                material_family: preset
                    .filament_type
                    .as_deref()
                    .map(|filament_type| material_family_for(filament_type).0),
                filament_type: preset.filament_type,
                name: preset.name,
            })
            .collect(),
        defaults,
        profile_snapshot: resolved.profile_snapshot(),
        matching_printer_ids: matching_printer_ids(storage, catalog, &resolved.profile)?,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::contracts::command::ErrorCode;
    use serde_json::json;

    pub(crate) fn fixtures_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/profiles")
    }

    pub(crate) fn fixture_index() -> PresetIndex {
        PresetIndex::build(&fixtures_dir(), "2.4.2", &CancelFlag::never()).expect("index")
    }

    fn names(offered: &[OfferedPreset]) -> Vec<&str> {
        offered.iter().map(|preset| preset.name.as_str()).collect()
    }

    fn write(dir: &Path, relative: &str, value: Value) {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string(&value).unwrap()).unwrap();
    }

    #[test]
    fn indexes_every_bundle_in_the_preset_source() {
        let index = fixture_index();
        assert_eq!(index.vendor_count(), 2);
        assert!(index.contains(PresetKind::Machine, "Test Printer 0.4 nozzle"));
        assert!(index.contains(PresetKind::Process, "0.20mm Standard @Test"));
        assert!(index.contains(PresetKind::Filament, "Generic PLA @Test Library"));
        assert!(!index.contains(PresetKind::Process, "Test Printer 0.4 nozzle"));
    }

    #[test]
    fn only_json_files_with_a_vendor_folder_are_bundles() {
        let temp = tempfile::tempdir().unwrap();
        write(
            temp.path(),
            "blacklist.json",
            json!({ "process": ["GP008"] }),
        );
        write(temp.path(), "V.json", json!({ "process_list": [] }));
        fs::create_dir_all(temp.path().join("V")).unwrap();
        assert_eq!(vendor_bundles(temp.path()), [temp.path().join("V.json")]);
        let index = PresetIndex::build(temp.path(), "2.4.2", &CancelFlag::never()).unwrap();
        assert_eq!(index.vendor_count(), 1);
        assert_eq!(
            PresetIndex::build(&temp.path().join("missing"), "2.4.2", &CancelFlag::never())
                .unwrap_err(),
            PresetIndexError::Unreadable
        );
    }

    #[test]
    fn flattening_merges_the_chain_and_keeps_system_identity() {
        let index = fixture_index();
        let fine = index
            .flatten(PresetKind::Process, "0.12mm Fine @Test")
            .unwrap();
        assert_eq!(fine["name"], "0.12mm Fine @Test");
        assert_eq!(fine["from"], "system");
        assert!(!fine.contains_key("inherits"));
        // Own key, the parent's override, and the grandparent's default.
        assert_eq!(fine["layer_height"], "0.12");
        assert_eq!(fine["wall_loops"], "3");
        assert_eq!(fine["sparse_infill_density"], "15%");
        assert_eq!(
            fine["compatible_printers"],
            json!(["Test Printer 0.4 nozzle"])
        );

        let machine = index
            .flatten(PresetKind::Machine, "Test Printer 0.4 nozzle")
            .unwrap();
        assert_eq!(machine["from"], "system");
        assert_eq!(machine["printable_height"], "250");
        assert_eq!(machine["auxiliary_fan"], "1");
    }

    #[test]
    fn filament_parents_resolve_across_bundles() {
        let index = fixture_index();
        let pla = index
            .flatten(PresetKind::Filament, "Generic PLA @Test Printer")
            .unwrap();
        assert_eq!(pla["filament_type"], json!(["PLA"]));
        assert_eq!(pla["filament_diameter"], json!(["1.75"]));
        assert_eq!(pla["nozzle_temperature"], json!(["215"]));
        assert_eq!(
            pla["compatible_printers"],
            json!(["Test Printer 0.4 nozzle"])
        );
    }

    #[test]
    fn a_missing_preset_is_preset_not_found() {
        let index = fixture_index();
        let error = index
            .flatten(PresetKind::Machine, "Nope 0.4 nozzle")
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::PresetNotFound);
        let details = error.details.unwrap();
        assert_eq!(
            details["preset"],
            crate::contracts::command::JsonValue::String("Nope 0.4 nozzle".to_string())
        );
        assert_eq!(
            details["presetSourceVersion"],
            crate::contracts::command::JsonValue::String("2.4.2".to_string())
        );
    }

    #[test]
    fn cycles_and_dangling_parents_are_preset_invalid() {
        let temp = tempfile::tempdir().unwrap();
        write(
            temp.path(),
            "V.json",
            json!({ "process_list": [
                { "name": "a", "sub_path": "process/a.json" },
                { "name": "b", "sub_path": "process/b.json" },
                { "name": "orphan", "sub_path": "process/orphan.json" },
                { "name": "broken", "sub_path": "process/missing.json" },
            ]}),
        );
        write(
            temp.path(),
            "V/process/a.json",
            json!({ "name": "a", "inherits": "b" }),
        );
        write(
            temp.path(),
            "V/process/b.json",
            json!({ "name": "b", "inherits": "a" }),
        );
        write(
            temp.path(),
            "V/process/orphan.json",
            json!({ "name": "orphan", "inherits": "gone" }),
        );
        let index = PresetIndex::build(temp.path(), "2.4.2", &CancelFlag::never()).unwrap();
        for name in ["a", "orphan", "broken"] {
            let error = index.flatten(PresetKind::Process, name).unwrap_err();
            assert_eq!(error.code, ErrorCode::PresetInvalid, "{name}");
        }
        assert!(index.offered(PresetKind::Process, "M").is_empty());
    }

    #[test]
    fn the_first_bundle_in_sorted_order_wins_a_name() {
        let temp = tempfile::tempdir().unwrap();
        for vendor in ["B", "A"] {
            write(
                temp.path(),
                &format!("{vendor}.json"),
                json!({ "filament_list": [{ "name": "shared", "sub_path": "filament/shared.json" }] }),
            );
            write(
                temp.path(),
                &format!("{vendor}/filament/shared.json"),
                json!({ "name": "shared", "vendor": vendor }),
            );
        }
        let index = PresetIndex::build(temp.path(), "2.4.2", &CancelFlag::never()).unwrap();
        let shared = index.flatten(PresetKind::Filament, "shared").unwrap();
        assert_eq!(shared["vendor"], "A");
    }

    /// Vendors reuse base names with different values. A parent resolves in
    /// the inheriting preset's own bundle, then in `OrcaFilamentLibrary`,
    /// and only then by name.
    #[test]
    fn parents_resolve_in_their_own_bundle_first() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = |vendor: &str, presets: &[(&str, Value)]| {
            let list: Vec<Value> = presets
                .iter()
                .map(|(name, _)| json!({ "name": name, "sub_path": format!("filament/{name}.json") }))
                .collect();
            write(
                temp.path(),
                &format!("{vendor}.json"),
                json!({ "filament_list": list }),
            );
            for (name, preset) in presets {
                write(
                    temp.path(),
                    &format!("{vendor}/filament/{name}.json"),
                    preset.clone(),
                );
            }
        };
        bundle(
            "Afinia",
            &[("fdm_filament_pla", json!({ "source": "Afinia" }))],
        );
        bundle(
            "Elegoo",
            &[
                ("fdm_filament_pla", json!({ "source": "Elegoo" })),
                ("Elegoo PLA", json!({ "inherits": "fdm_filament_pla" })),
            ],
        );
        bundle(
            FILAMENT_LIBRARY_VENDOR,
            &[
                ("fdm_filament_pla", json!({ "source": "Library" })),
                (
                    "Generic PLA @System",
                    json!({ "inherits": "fdm_filament_pla" }),
                ),
            ],
        );
        bundle(
            "Prusa",
            &[
                ("Prusa PLA", json!({ "inherits": "fdm_filament_pla" })),
                (
                    "Prusa Generic",
                    json!({ "inherits": "Generic PLA @System" }),
                ),
            ],
        );
        let index = PresetIndex::build(temp.path(), "2.4.2", &CancelFlag::never()).unwrap();
        let source =
            |name: &str| index.flatten(PresetKind::Filament, name).unwrap()["source"].clone();
        assert_eq!(source("Elegoo PLA"), "Elegoo");
        assert_eq!(source("Prusa PLA"), "Library");
        assert_eq!(source("Prusa Generic"), "Library");
        assert_eq!(source("Generic PLA @System"), "Library");
        // A name alone picks the first bundle in sorted order.
        assert_eq!(source("fdm_filament_pla"), "Afinia");
    }

    #[test]
    fn a_cancelled_build_stops() {
        let (sender, receiver) = tokio::sync::watch::channel(true);
        let error =
            PresetIndex::build(&fixtures_dir(), "2.4.2", &CancelFlag::new(receiver)).unwrap_err();
        assert_eq!(error, PresetIndexError::Cancelled);
        drop(sender);
    }

    #[test]
    fn offered_processes_are_instantiable_and_list_the_machine() {
        let index = fixture_index();
        // Not offered: the template (instantiation false), the
        // condition-only preset, the Delta's preset, and the base.
        assert_eq!(
            names(&index.offered(PresetKind::Process, "Test Printer 0.4 nozzle")),
            ["0.12mm Fine @Test", "0.20mm Standard @Test"]
        );
        assert_eq!(
            names(&index.offered(PresetKind::Process, "Test Delta 0.4 nozzle")),
            ["0.24mm Draft @Test Delta"]
        );
    }

    #[test]
    fn offered_filaments_skip_condition_only_and_library_presets() {
        let index = fixture_index();
        let offered = index.offered(PresetKind::Filament, "Test Printer 0.4 nozzle");
        assert_eq!(
            names(&offered),
            [
                "Generic PLA @Test Printer",
                "Test ABS @Test Printer",
                "Test PETG @Test Printer"
            ]
        );
        assert_eq!(offered[2].filament_type.as_deref(), Some("PETG"));
    }

    #[test]
    fn filament_compatibility_is_checked_by_farm3d() {
        let index = fixture_index();
        let machine = "Test Printer 0.4 nozzle";
        index
            .check_filament_compatible("Test PETG @Test Printer", machine)
            .unwrap();
        for incompatible in [
            "Test PLA @Test Delta",
            "Test PLA Conditional",
            "Generic PLA @Test Library",
        ] {
            let error = index
                .check_filament_compatible(incompatible, machine)
                .unwrap_err();
            assert_eq!(
                error.code,
                ErrorCode::FilamentIncompatible,
                "{incompatible}"
            );
        }
        let missing = index
            .check_filament_compatible("Nope", machine)
            .unwrap_err();
        assert_eq!(missing.code, ErrorCode::PresetNotFound);

        index
            .check_process_compatible("0.12mm Fine @Test", machine)
            .unwrap();
        let process = index
            .check_process_compatible("0.24mm Draft @Test Delta", machine)
            .unwrap_err();
        assert_eq!(process.code, ErrorCode::Validation);
    }

    #[test]
    fn known_keys_are_the_union_across_presets_of_a_kind() {
        let index = fixture_index();
        // `wall_loops` is only in the base process, not in "0.12mm Fine".
        assert!(index.is_known_key(PresetKind::Process, "wall_loops"));
        assert!(index.is_known_key(PresetKind::Machine, "printable_area"));
        assert!(!index.is_known_key(PresetKind::Machine, "wall_loops"));
        assert!(!index.is_known_key(PresetKind::Process, "not_a_key"));
    }

    fn offered(names: &[(&str, Option<&str>)]) -> Vec<OfferedPreset> {
        names
            .iter()
            .map(|(name, filament_type)| OfferedPreset {
                name: name.to_string(),
                filament_type: filament_type.map(str::to_string),
            })
            .collect()
    }

    #[test]
    fn the_default_quality_is_standard_then_the_first_by_name() {
        let with_standard = offered(&[("0.12mm Fine", None), ("0.20mm Standard", None)]);
        assert_eq!(
            default_process(&with_standard).as_deref(),
            Some("0.20mm Standard")
        );
        let without = offered(&[("0.12mm Fine", None), ("0.24mm Draft", None)]);
        assert_eq!(default_process(&without).as_deref(), Some("0.12mm Fine"));
        assert_eq!(default_process(&[]), None);
    }

    #[test]
    fn the_default_material_follows_the_loaded_spool_then_generic_pla() {
        let filaments = offered(&[
            ("Brand PLA", Some("PLA")),
            ("Generic PETG", Some("PETG")),
            ("Generic PLA @X", Some("PLA")),
            ("Weird", Some("PEEK")),
        ]);
        assert_eq!(
            default_filament(&filaments, &[MaterialFamily::Petg]).as_deref(),
            Some("Generic PETG")
        );
        // No match for the first family: the next loaded family is tried.
        assert_eq!(
            default_filament(&filaments, &[MaterialFamily::Tpu, MaterialFamily::Pla]).as_deref(),
            Some("Brand PLA")
        );
        // OTHER never matches, even an unknown type.
        assert_eq!(
            default_filament(&filaments, &[MaterialFamily::Other]).as_deref(),
            Some("Generic PLA @X")
        );
        assert_eq!(
            default_filament(&filaments, &[]).as_deref(),
            Some("Generic PLA @X")
        );
        let no_generic = offered(&[("B", Some("ABS")), ("A", Some("ASA"))]);
        assert_eq!(default_filament(&no_generic, &[]).as_deref(), Some("B"));
    }

    #[test]
    fn filament_types_map_to_material_families() {
        assert_eq!(material_family_for("PLA"), (MaterialFamily::Pla, None));
        assert_eq!(material_family_for("PETG"), (MaterialFamily::Petg, None));
        assert_eq!(
            material_family_for("PEEK"),
            (MaterialFamily::Other, Some("PEEK".to_string()))
        );
        assert_eq!(
            material_family_for("OTHER"),
            (MaterialFamily::Other, Some("OTHER".to_string()))
        );
    }
}
