//! D5: Preparation documents — validation, seeding from a Model Source
//! Revision, and reload onto a newer revision. Staleness is derived on
//! read by the repository (the pinned revision isn't the Model's current
//! one) and never stored.
//!
//! **Seeding.** An STL becomes one plate holding its one object, centred on
//! the target bed. A 3MF with Orca/Bambu plates becomes one plate per plate
//! (in index order, with its name), and a 3MF without plates one plate
//! holding every printable build item. Each plate's instances keep their
//! relative XY layout from the build transforms, and the group is centred
//! on the target bed. Centring the group removes wherever the source put
//! its plate on OrcaSlicer's plate grid (the grid origin of plate N is
//! `N-1` strides of 1.2 × the *source* printer's bed), so seeding never
//! depends on the source printer. Build items marked unprintable are
//! skipped.
//!
//! **Reload** keeps every instance whose object still exists, with its
//! transform, removes the rest, and adds each object new in the current
//! revision to plate 1, in a row centred on the bed.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::catalog::BedShape;
use crate::contracts::command::CommandError;
use crate::library::formats::{BoundsMm, Inspection};
use crate::library::ModelFormat;
use crate::persistence::{RepositoryError, StorageError};
use crate::printers::repository::PrinterRepository;

use super::events;
use super::geometry::{
    compose_transform, transform_is_valid, transformed_bounds, LoadedRevision, Transform3mf,
    MAX_SCALE, MIN_SCALE,
};
use super::operations::has_active_operations;
use super::presets::{list_slice_options, resolve_target};
use super::repository;
use super::SliceTarget;
use super::{
    new_preparation_id, InstanceDoc, InstanceTransform, PlateDoc, PreparationDocument,
    PreparationRecord, SliceControls, SlicingServices,
};

/// D5: OrcaSlicer's `MAX_PLATE_COUNT`.
pub const MAX_PLATES: usize = 36;

/// A plate name is at most this many characters.
pub const MAX_PLATE_NAME_CHARS: usize = 128;

/// The gap between objects that reload places in a row.
const ARRANGE_GAP_MM: f64 = 5.0;

fn new_key() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// D5: checks a whole document before it is stored: 1–36 plates, unique
/// non-empty plate and instance keys, names within bounds, and finite
/// transforms with each scale within 0.01–100. Which objects exist, and
/// whether they fit the bed, is checked when slicing starts.
pub fn validate_document(document: &PreparationDocument) -> Result<(), CommandError> {
    let plates = &document.plates;
    if plates.is_empty() || plates.len() > MAX_PLATES {
        return Err(CommandError::validation_at(
            "document.plates",
            format!("A Preparation has 1 to {MAX_PLATES} plates."),
        ));
    }
    let mut plate_keys = HashSet::new();
    let mut instance_keys = HashSet::new();
    for (plate_position, plate) in plates.iter().enumerate() {
        let at = |field: &str| format!("document.plates[{plate_position}].{field}");
        if plate.plate_key.trim().is_empty() || !plate_keys.insert(plate.plate_key.as_str()) {
            return Err(CommandError::validation_at(
                at("plateKey"),
                "Each plate needs its own key.",
            ));
        }
        if plate
            .name
            .as_ref()
            .is_some_and(|name| name.chars().count() > MAX_PLATE_NAME_CHARS)
        {
            return Err(CommandError::validation_at(
                at("name"),
                format!("A plate name is at most {MAX_PLATE_NAME_CHARS} characters."),
            ));
        }
        for (instance_position, instance) in plate.instances.iter().enumerate() {
            let at = |field: &str| at(&format!("instances[{instance_position}].{field}"));
            if instance.instance_key.trim().is_empty()
                || !instance_keys.insert(instance.instance_key.as_str())
            {
                return Err(CommandError::validation_at(
                    at("instanceKey"),
                    "Each object on a plate needs its own key.",
                ));
            }
            if !transform_is_valid(&instance.transform) {
                return Err(CommandError::validation_at(
                    at("transform"),
                    format!(
                        "Positions and rotations must be numbers, and each scale from {MIN_SCALE} to {MAX_SCALE}."
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// The centre of the bed's printable area, in bed coordinates.
pub fn bed_center(bed: &BedShape) -> [f64; 2] {
    match bed {
        BedShape::Rectangular {
            width_mm,
            depth_mm,
            origin_x_mm,
            origin_y_mm,
        } => [origin_x_mm + width_mm / 2.0, origin_y_mm + depth_mm / 2.0],
        BedShape::Polygon { points } => {
            let xs = points.iter().map(|point| point.x_mm);
            let ys = points.iter().map(|point| point.y_mm);
            let span = |values: &mut dyn Iterator<Item = f64>| {
                values.fold(None, |span: Option<(f64, f64)>, value| match span {
                    Some((low, high)) => Some((low.min(value), high.max(value))),
                    None => Some((value, value)),
                })
            };
            match (span(&mut xs.into_iter()), span(&mut ys.into_iter())) {
                (Some((x0, x1)), Some((y0, y1))) => [(x0 + x1) / 2.0, (y0 + y1) / 2.0],
                _ => [0.0, 0.0],
            }
        }
    }
}

/// Rounds away float noise (1e-12, -0.0) so a seeded document reads
/// cleanly.
fn tidy(value: f64) -> f64 {
    let rounded = (value * 1e9).round() / 1e9;
    if rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}

/// D5's instance transform closest to a 3MF build transform: the scale is
/// each local axis's length and the rotation its X→Y→Z extrinsic angles.
/// The translation is the transform's own XY; Z is derived again when the
/// plate is written. A mirrored (or sheared) transform can't be expressed,
/// so it keeps its scale with no rotation.
pub fn instance_transform_from_3mf(m: &Transform3mf) -> InstanceTransform {
    let column = |local: usize| [m[local * 3], m[local * 3 + 1], m[local * 3 + 2]];
    let length = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let columns = [column(0), column(1), column(2)];
    let lengths = columns.map(length);
    let scale = lengths.map(|factor| {
        if factor.is_finite() && factor > 0.0 {
            tidy(factor.clamp(MIN_SCALE, MAX_SCALE))
        } else {
            1.0
        }
    });
    let determinant = columns[0][0]
        * (columns[1][1] * columns[2][2] - columns[2][1] * columns[1][2])
        - columns[1][0] * (columns[0][1] * columns[2][2] - columns[2][1] * columns[0][2])
        + columns[2][0] * (columns[0][1] * columns[1][2] - columns[1][1] * columns[0][2]);
    let rotate_deg = if determinant > 0.0 && lengths.iter().all(|factor| *factor > 0.0) {
        // r[world][local], with each local axis normalised.
        let r = |world: usize, local: usize| columns[local][world] / lengths[local];
        let sin_y = (-r(2, 0)).clamp(-1.0, 1.0);
        let y = sin_y.asin();
        let (x, z) = if y.cos().abs() > 1e-9 {
            (r(2, 1).atan2(r(2, 2)), r(1, 0).atan2(r(0, 0)))
        } else {
            // Gimbal lock: fold the X rotation into Z.
            (0.0, (-r(0, 1)).atan2(r(1, 1)))
        };
        [x, y, z].map(|radians| tidy(radians.to_degrees()))
    } else {
        [0.0, 0.0, 0.0]
    };
    InstanceTransform {
        translate_mm: [tidy(m[9]), tidy(m[10])],
        rotate_deg,
        scale,
    }
}

/// The XY bounds of `instances` as placed (over their meshes' vertices).
fn placed_bounds(instances: &[InstanceDoc], revision: &LoadedRevision) -> Option<BoundsMm> {
    let mut union: Option<BoundsMm> = None;
    for instance in instances {
        let Some(mesh) = revision.mesh(instance.object_key) else {
            continue;
        };
        let m = compose_transform(&instance.transform, &mesh.positions);
        let Some(bounds) = transformed_bounds(&m, &mesh.positions) else {
            continue;
        };
        union = Some(match union {
            Some(mut union) => {
                for axis in 0..3 {
                    union.min[axis] = union.min[axis].min(bounds.min[axis]);
                    union.max[axis] = union.max[axis].max(bounds.max[axis]);
                }
                union
            }
            None => bounds,
        });
    }
    union
}

/// Moves `instances` as a group so its XY bounds are centred on `center`.
fn center_group(instances: &mut [InstanceDoc], revision: &LoadedRevision, center: [f64; 2]) {
    let Some(bounds) = placed_bounds(instances, revision) else {
        return;
    };
    let shift = [
        center[0] - (bounds.min[0] + bounds.max[0]) / 2.0,
        center[1] - (bounds.min[1] + bounds.max[1]) / 2.0,
    ];
    for instance in instances {
        let [x, y] = instance.transform.translate_mm;
        instance.transform.translate_mm = [tidy(x + shift[0]), tidy(y + shift[1])];
    }
}

/// What seeding needs besides the geometry.
#[derive(Clone, Debug)]
pub struct SeedInput {
    pub target: SliceTarget,
    pub bed: BedShape,
    /// The source 3MF's Orca/Bambu plates: index and name (P4
    /// `ThreeMfInspection.plates`). Empty for an STL or a plain 3MF.
    pub plates: Vec<(u32, Option<String>)>,
    pub process_preset: Option<String>,
    pub filament_preset: Option<String>,
}

/// D5: the first document for a Preparation of `revision` (see the module
/// docs).
pub fn seed_document(revision: &LoadedRevision, input: SeedInput) -> PreparationDocument {
    let center = bed_center(&input.bed);
    let geometry = revision.geometry();
    let known: BTreeSet<u32> = geometry
        .objects
        .iter()
        .map(|object| object.object_key)
        .collect();
    let printable: Vec<_> = geometry
        .build_items
        .iter()
        .filter(|item| item.printable && known.contains(&item.object_key))
        .collect();
    let instance = |object_key: u32, transform: &Transform3mf| InstanceDoc {
        instance_key: new_key(),
        object_key,
        transform: instance_transform_from_3mf(transform),
    };

    // Plate index → (name, instances), in index order.
    let mut groups: BTreeMap<u32, (Option<String>, Vec<InstanceDoc>)> = BTreeMap::new();
    for (index, name) in &input.plates {
        groups
            .entry(*index)
            .or_insert_with(|| (name.clone(), Vec::new()));
    }
    let uses_plates = !groups.is_empty() || printable.iter().any(|item| item.plate_index.is_some());
    let first_plate = groups.keys().next().copied().unwrap_or(1);
    for item in &printable {
        let index = if uses_plates {
            item.plate_index.unwrap_or(first_plate)
        } else {
            1
        };
        groups
            .entry(index)
            .or_insert_with(|| (None, Vec::new()))
            .1
            .push(instance(item.object_key, &item.transform));
    }
    if groups.is_empty() {
        // Nothing printable is placed: one plate with one copy of each
        // object, so the user still has something to arrange.
        let instances = geometry
            .objects
            .iter()
            .map(|object| {
                instance(
                    object.object_key,
                    &crate::library::formats::IDENTITY_TRANSFORM,
                )
            })
            .collect();
        groups.insert(1, (None, instances));
    }

    let plates = groups
        .into_values()
        .take(MAX_PLATES)
        .map(|(name, mut instances)| {
            center_group(&mut instances, revision, center);
            PlateDoc {
                plate_key: new_key(),
                name: name.map(|name| name.chars().take(MAX_PLATE_NAME_CHARS).collect()),
                instances,
            }
        })
        .collect();
    PreparationDocument {
        plates,
        target: input.target,
        process_preset: input.process_preset,
        filament_preset: input.filament_preset,
        controls: SliceControls::default(),
    }
}

/// What [`rebase_document`] changed: the object keys whose instances were
/// removed and the object keys added to plate 1, each ascending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebaseChanges {
    pub removed_object_keys: Vec<u32>,
    pub added_object_keys: Vec<u32>,
}

/// D5 reload: `document`, prepared from `previous`, moved onto `current`.
pub fn rebase_document(
    document: &PreparationDocument,
    previous: &LoadedRevision,
    current: &LoadedRevision,
    bed: &BedShape,
) -> (PreparationDocument, RebaseChanges) {
    let keys = |revision: &LoadedRevision| -> BTreeSet<u32> {
        revision
            .geometry()
            .objects
            .iter()
            .map(|object| object.object_key)
            .collect()
    };
    let previous_keys = keys(previous);
    let current_keys = keys(current);
    let mut removed = BTreeSet::new();
    let mut next = document.clone();
    for plate in &mut next.plates {
        plate.instances.retain(|instance| {
            let keep = current_keys.contains(&instance.object_key);
            if !keep {
                removed.insert(instance.object_key);
            }
            keep
        });
    }
    let added: Vec<u32> = current_keys.difference(&previous_keys).copied().collect();
    if !added.is_empty() {
        let mut row: Vec<InstanceDoc> = added
            .iter()
            .map(|object_key| InstanceDoc {
                instance_key: new_key(),
                object_key: *object_key,
                transform: InstanceTransform {
                    translate_mm: [0.0, 0.0],
                    rotate_deg: [0.0, 0.0, 0.0],
                    scale: [1.0, 1.0, 1.0],
                },
            })
            .collect();
        // Side by side along X, then centred on the bed as a group.
        let mut cursor = 0.0;
        for instance in &mut row {
            let Some(bounds) = placed_bounds(std::slice::from_ref(instance), current) else {
                continue;
            };
            let width = bounds.max[0] - bounds.min[0];
            instance.transform.translate_mm = [tidy(cursor - bounds.min[0]), 0.0];
            cursor += width + ARRANGE_GAP_MM;
        }
        center_group(&mut row, current, bed_center(bed));
        next.plates[0].instances.extend(row);
    }
    (
        next,
        RebaseChanges {
            removed_object_keys: removed.into_iter().collect(),
            added_object_keys: added,
        },
    )
}

// ---------------------------------------------------------------------------
// The Preparation commands' work
// ---------------------------------------------------------------------------

/// `reload_preparation`'s result: the rebased Preparation, and the object
/// keys whose instances were removed or added (D5).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "command/ReloadPreparationData.ts"
)]
pub struct ReloadPreparationData {
    pub preparation: PreparationRecord,
    pub removed_object_keys: Vec<u32>,
    pub added_object_keys: Vec<u32>,
}

fn storage_error(error: StorageError) -> CommandError {
    CommandError::from_repository(RepositoryError::Storage(error))
}

/// A Model's current (highest-sequence) revision: its id, format, and
/// stored inspection.
struct CurrentRevision {
    id: String,
    format: ModelFormat,
    inspection_json: String,
}

fn current_revision(
    connection: &rusqlite::Connection,
    model_id: &str,
) -> rusqlite::Result<Option<CurrentRevision>> {
    connection
        .query_row(
            "SELECT id, format, inspection_json FROM model_source_revisions
             WHERE model_id = ?1 ORDER BY sequence DESC LIMIT 1",
            [model_id],
            |row| {
                let format: String = row.get(1)?;
                Ok(CurrentRevision {
                    id: row.get(0)?,
                    format: crate::spools::decode_enum(&format).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                    inspection_json: row.get(2)?,
                })
            },
        )
        .optional()
}

/// The source 3MF's Orca/Bambu plates, from its stored P4 inspection.
fn source_plates(inspection_json: &str) -> Vec<(u32, Option<String>)> {
    match serde_json::from_str::<Inspection>(inspection_json) {
        Ok(Inspection::ThreeMf(inspection)) => inspection
            .plates
            .into_iter()
            .map(|plate| (plate.index, plate.name))
            .collect(),
        _ => Vec::new(),
    }
}

/// With no target given, the first active Printer by name.
fn default_target<R: tauri::Runtime>(
    services: &SlicingServices<R>,
) -> Result<SliceTarget, CommandError> {
    let mut printers = PrinterRepository::new(Arc::clone(&services.storage))
        .list()
        .map_err(storage_error)?;
    printers.retain(|printer| printer.archived_at.is_none());
    printers.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    printers
        .into_iter()
        .next()
        .map(|printer| SliceTarget::Printer {
            printer_id: printer.id,
        })
        .ok_or_else(|| {
            CommandError::validation_at(
                "target",
                "Add a Printer, or choose a printer profile, to prepare for.",
            )
        })
}

/// D5 `create_preparation`: Model `model_id`'s Preparation, seeded from its
/// current revision for `target` (the first active Printer when absent),
/// with D3's default presets when a slicer runtime is available. A Model
/// that already has one gets it back unchanged. A G-code Model has no
/// Preparation (`VALIDATION`). Blocks while it loads geometry and presets.
pub fn create_preparation<R: tauri::Runtime>(
    services: &SlicingServices<R>,
    model_id: &str,
    target: Option<SliceTarget>,
) -> Result<PreparationRecord, CommandError> {
    let (exists, existing, current) = services
        .storage
        .read(|connection| {
            let exists: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM library_models WHERE id = ?1)",
                [model_id],
                |row| row.get(0),
            )?;
            let existing = repository::load_preparation_for_model(connection, model_id)
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok((exists, existing, current_revision(connection, model_id)?))
        })
        .map_err(storage_error)?;
    if !exists {
        return Err(CommandError::not_found(model_id));
    }
    if let Some(existing) = existing {
        return Ok(existing);
    }
    let current = current.ok_or_else(|| CommandError::not_found(model_id))?;
    if current.format == ModelFormat::Gcode {
        return Err(CommandError::validation_at(
            "modelId",
            "A G-code Model is already sliced, so it has no Preparation.",
        ));
    }
    let target = match target {
        Some(target) => target,
        None => default_target(services)?,
    };
    let resolved = resolve_target(&services.storage, &services.catalog, &target)?;
    let revision = services.revision_geometry(&current.id)?;
    // Default presets need a runtime; without one they stay unset.
    let defaults = services
        .usable_runtime()
        .ok()
        .and_then(|(_, source)| services.preset_index(&source).ok())
        .and_then(|index| {
            list_slice_options(&services.storage, &services.catalog, &index, &target).ok()
        })
        .map(|options| options.defaults);
    let document = seed_document(
        &revision,
        SeedInput {
            target,
            bed: resolved.profile.bed_shape.clone(),
            plates: source_plates(&current.inspection_json),
            process_preset: defaults.as_ref().and_then(|d| d.process_preset.clone()),
            filament_preset: defaults.and_then(|d| d.filament_preset),
        },
    );
    let created = services.storage.write_repo(|tx| {
        match repository::load_preparation_for_model(tx, model_id)? {
            Some(existing) => Ok((existing, false)),
            None => repository::insert_preparation(
                tx,
                &new_preparation_id(),
                model_id,
                &current.id,
                &document,
            )
            .map(|record| (record, true)),
        }
    });
    let (record, created) = created.map_err(CommandError::from_repository)?;
    services.remember_preparation(&record.model_id, &record.id);
    if created {
        services.publish(vec![events::preparation_changed(&record)]);
    }
    Ok(record)
}

/// D5 `update_preparation`: replaces the whole document after the
/// `expectedRevision` check (`CONFLICT` on a mismatch).
pub fn update_preparation<R: tauri::Runtime>(
    services: &SlicingServices<R>,
    preparation_id: &str,
    expected_revision: i64,
    document: &PreparationDocument,
) -> Result<PreparationRecord, CommandError> {
    validate_document(document)?;
    let record = services
        .storage
        .write_repo(|tx| {
            repository::update_preparation(tx, preparation_id, expected_revision, document)
        })
        .map_err(CommandError::from_repository)?;
    services.publish(vec![events::preparation_changed(&record)]);
    Ok(record)
}

/// D5 `reload_preparation`: re-bases the Preparation onto its Model's
/// current revision (see [`rebase_document`]). Blocks while it loads
/// geometry.
pub fn reload_preparation<R: tauri::Runtime>(
    services: &SlicingServices<R>,
    preparation_id: &str,
    expected_revision: i64,
) -> Result<ReloadPreparationData, CommandError> {
    let (preparation, current) = services
        .storage
        .read(|connection| {
            let preparation = repository::load_preparation(connection, preparation_id)
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            let current = match &preparation {
                Some(preparation) => current_revision(connection, &preparation.model_id)?,
                None => None,
            };
            Ok((preparation, current))
        })
        .map_err(storage_error)?;
    let preparation = preparation.ok_or_else(|| CommandError::not_found(preparation_id))?;
    if preparation.revision != expected_revision {
        return Err(CommandError::from_repository(RepositoryError::Conflict {
            entity_id: preparation.id,
            expected_revision,
            current_revision: preparation.revision,
        }));
    }
    let current = current.ok_or_else(|| CommandError::not_found(preparation.model_id.clone()))?;
    let resolved = resolve_target(
        &services.storage,
        &services.catalog,
        &preparation.document.target,
    )?;
    let previous = services.revision_geometry(&preparation.source_revision_id)?;
    let next = services.revision_geometry(&current.id)?;
    let (document, changes) = rebase_document(
        &preparation.document,
        &previous,
        &next,
        &resolved.profile.bed_shape,
    );
    let record = services
        .storage
        .write_repo(|tx| {
            repository::rebase_preparation(
                tx,
                preparation_id,
                expected_revision,
                &current.id,
                &document,
            )
        })
        .map_err(CommandError::from_repository)?;
    services.publish(vec![events::preparation_changed(&record)]);
    Ok(ReloadPreparationData {
        preparation: record,
        removed_object_keys: changes.removed_object_keys,
        added_object_keys: changes.added_object_keys,
    })
}

/// D5 `delete_preparation`: deletes it with its operations' history. A
/// Preparation with a queued or running slice is `CONFLICT`: cancel those
/// first.
pub fn delete_preparation<R: tauri::Runtime>(
    services: &SlicingServices<R>,
    preparation_id: &str,
    expected_revision: i64,
) -> Result<(), CommandError> {
    let model_id = services
        .storage
        .write_repo(|tx| {
            let model_id = repository::load_preparation(tx, preparation_id)?
                .map(|preparation| preparation.model_id);
            if has_active_operations(tx, preparation_id)? {
                return Ok(Err(CommandError::conflict(
                    "This Preparation is being sliced. Cancel its slices, then delete it.",
                )));
            }
            repository::delete_preparation(tx, preparation_id, expected_revision)?;
            Ok(Ok(model_id))
        })
        .map_err(CommandError::from_repository)??;
    // Committed; a blob that can't be unlinked now is retried at startup.
    let _ = services.content.release_unreferenced(&services.storage);
    if let Some(model_id) = model_id {
        services.forget_preparation_of(&model_id);
    }
    services.publish(vec![events::preparation_removed(preparation_id)]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::formats::ObjectMesh;
    use crate::printers::CatalogRef;
    use crate::slicing::geometry::{GeometryBuildItem, GeometryObject, RevisionGeometry};

    fn bed() -> BedShape {
        BedShape::Rectangular {
            width_mm: 200.0,
            depth_mm: 100.0,
            origin_x_mm: 0.0,
            origin_y_mm: 0.0,
        }
    }

    fn target() -> SliceTarget {
        SliceTarget::Profile {
            catalog_ref: CatalogRef {
                vendor: "V".to_string(),
                model: "M".to_string(),
                variant: "M 0.4 nozzle".to_string(),
                model_id: "V-M".to_string(),
                printer_variant: "0.4".to_string(),
            },
        }
    }

    /// A 10 mm cube from (0, 0, 0) to (10, 10, 10).
    fn cube() -> ObjectMesh {
        let mut positions = Vec::new();
        for x in [0.0, 10.0] {
            for y in [0.0, 10.0] {
                for z in [0.0, 10.0] {
                    positions.push([x, y, z]);
                }
            }
        }
        ObjectMesh {
            positions,
            triangles: vec![[0, 1, 2]],
        }
    }

    fn translation(x: f64, y: f64) -> Transform3mf {
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, x, y, 0.0]
    }

    fn revision(
        items: Vec<(u32, Transform3mf, Option<u32>, bool)>,
        keys: &[u32],
    ) -> LoadedRevision {
        let object = |key: u32| GeometryObject {
            object_key: key,
            name: None,
            triangle_count: 1,
            bounds_mm: BoundsMm {
                min: [0.0; 3],
                max: [10.0; 3],
            },
            lay_flat_faces: vec![],
        };
        LoadedRevision::from_parts(
            RevisionGeometry {
                objects: keys.iter().map(|key| object(*key)).collect(),
                build_items: items
                    .into_iter()
                    .map(
                        |(object_key, transform, plate_index, printable)| GeometryBuildItem {
                            object_key,
                            transform,
                            plate_index,
                            printable,
                        },
                    )
                    .collect(),
            },
            keys.iter().map(|key| (*key, cube())),
        )
    }

    fn input(plates: Vec<(u32, Option<String>)>) -> SeedInput {
        SeedInput {
            target: target(),
            bed: bed(),
            plates,
            process_preset: Some("P".to_string()),
            filament_preset: None,
        }
    }

    #[test]
    fn an_stl_is_one_plate_with_its_object_centred() {
        let stl = revision(vec![(1, translation(0.0, 0.0), None, true)], &[1]);
        let document = seed_document(&stl, input(vec![]));
        assert_eq!(document.plates.len(), 1);
        let instances = &document.plates[0].instances;
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].object_key, 1);
        // The cube spans 0..10, so centring on (100, 50) moves it by (95, 45).
        assert_eq!(instances[0].transform.translate_mm, [95.0, 45.0]);
        assert_eq!(document.process_preset.as_deref(), Some("P"));
        validate_document(&document).unwrap();
    }

    #[test]
    fn plates_keep_their_names_and_relative_layout_without_the_grid_origin() {
        // Plate 2 of a grid sits one stride (e.g. 1.2 × 256) to the right.
        let stride = 307.2;
        let three_mf = revision(
            vec![
                (1, translation(20.0, 20.0), Some(1), true),
                (2, translation(40.0, 20.0), Some(1), true),
                (3, translation(stride + 30.0, 60.0), Some(2), true),
                (3, translation(stride + 90.0, 60.0), Some(2), false),
            ],
            &[1, 2, 3],
        );
        let document = seed_document(
            &three_mf,
            input(vec![(1, Some("Left".to_string())), (2, None)]),
        );
        assert_eq!(document.plates.len(), 2);
        assert_eq!(document.plates[0].name.as_deref(), Some("Left"));
        let first = &document.plates[0].instances;
        assert_eq!(first.len(), 2);
        // Kept 20 mm apart, and centred: the pair spans 20..50 → centre 35.
        assert_eq!(first[0].transform.translate_mm, [85.0, 45.0]);
        assert_eq!(first[1].transform.translate_mm, [105.0, 45.0]);
        // Plate 2's grid offset is gone; its unprintable item is skipped.
        let second = &document.plates[1].instances;
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].transform.translate_mm, [95.0, 45.0]);
    }

    #[test]
    fn a_rotated_scaled_build_transform_becomes_d5_fields() {
        let transform = InstanceTransform {
            translate_mm: [12.0, -4.0],
            rotate_deg: [10.0, 20.0, 30.0],
            scale: [2.0, 1.0, 0.5],
        };
        let mut m = compose_transform(&transform, &cube().positions);
        m[11] = 0.0;
        let back = instance_transform_from_3mf(&m);
        for axis in 0..3 {
            assert!((back.rotate_deg[axis] - transform.rotate_deg[axis]).abs() < 1e-6);
            assert!((back.scale[axis] - transform.scale[axis]).abs() < 1e-6);
        }
        assert_eq!(back.translate_mm, [12.0, -4.0]);
    }

    #[test]
    fn documents_are_validated() {
        let stl = revision(vec![(1, translation(0.0, 0.0), None, true)], &[1]);
        let mut document = seed_document(&stl, input(vec![]));
        document.plates[0].instances[0].transform.scale[0] = 0.0;
        assert!(validate_document(&document).is_err());
        let mut document = seed_document(&stl, input(vec![]));
        document.plates.push(document.plates[0].clone());
        assert!(validate_document(&document).is_err(), "duplicate keys");
        let mut document = seed_document(&stl, input(vec![]));
        document.plates.clear();
        assert!(validate_document(&document).is_err());
    }

    #[test]
    fn reload_keeps_surviving_transforms_and_lists_changes() {
        let previous = revision(
            vec![
                (1, translation(0.0, 0.0), None, true),
                (2, translation(30.0, 0.0), None, true),
            ],
            &[1, 2],
        );
        let document = seed_document(&previous, input(vec![]));
        let kept = document.plates[0].instances[0].clone();
        let current = revision(
            vec![
                (1, translation(0.0, 0.0), None, true),
                (4, translation(0.0, 0.0), None, true),
            ],
            &[1, 4],
        );
        let (next, changes) = rebase_document(&document, &previous, &current, &bed());
        assert_eq!(changes.removed_object_keys, vec![2]);
        assert_eq!(changes.added_object_keys, vec![4]);
        let instances = &next.plates[0].instances;
        assert_eq!(instances.len(), 2);
        assert_eq!(
            instances[0], kept,
            "a surviving instance keeps its transform"
        );
        assert_eq!(instances[1].object_key, 4);
        assert_eq!(instances[1].transform.translate_mm, [95.0, 45.0]);
    }
}
