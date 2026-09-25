//! D5 and D6: revision geometry for the viewport, the binary mesh buffer,
//! the geometry cache, and D5's instance transform composition.
//!
//! [`load_revision`] reads an STL or 3MF through the P4 format readers
//! ([`formats::read_mesh`]), so the same parser, safety limits, component
//! flattening, and rejections apply. Everything here is synchronous and
//! `Send + Sync`, so a command can run it on `spawn_blocking` and share one
//! [`GeometryCache`].
//!
//! Tauri commands are registered elsewhere (`get_revision_geometry` and
//! `get_revision_mesh`).

use std::collections::{BTreeMap, VecDeque};
use std::f64::consts::PI;
use std::io::{Read, Seek};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::hull::{hull_faces, HullFace};
use super::InstanceTransform;
use crate::library::content::CancelFlag;
use crate::library::formats::{self, BoundsMm, InspectError, ObjectMesh};
use crate::library::ModelFormat;

/// D6: at most this many lay-flat faces per object, largest first.
pub const MAX_LAY_FLAT_FACES: usize = 8;
/// Lay-flat faces smaller than this fraction of the object's largest
/// hull face are left out.
pub const MIN_LAY_FLAT_FRACTION: f64 = 0.01;
/// D6: revisions kept in memory, least recently used evicted first.
pub const GEOMETRY_CACHE_CAPACITY: usize = 4;
/// D6: the mesh buffer's magic bytes and format version.
pub const MESH_MAGIC: [u8; 4] = *b"F3DM";
pub const MESH_FORMAT_VERSION: u32 = 1;
/// D5: each scale factor is within this range.
pub const MIN_SCALE: f64 = 0.01;
pub const MAX_SCALE: f64 = 100.0;

/// A 3MF `transform` attribute: `m00 m01 m02 m10 m11 m12 m20 m21 m22 m30
/// m31 m32`. A point maps as `x' = x·m[0] + y·m[3] + z·m[6] + m[9]`,
/// `y' = x·m[1] + y·m[4] + z·m[7] + m[10]`, `z' = x·m[2] + y·m[5] +
/// z·m[8] + m[11]`.
pub type Transform3mf = [f64; 12];

/// D6: `get_revision_geometry`'s result.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/RevisionGeometry.ts")]
pub struct RevisionGeometry {
    pub objects: Vec<GeometryObject>,
    pub build_items: Vec<GeometryBuildItem>,
}

/// D6: one object, in its own frame. An STL is object 1; a 3MF object is
/// keyed by its id, with components flattened into it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/GeometryObject.ts")]
pub struct GeometryObject {
    pub object_key: u32,
    /// The 3MF object's `name` attribute, else its name in Orca's
    /// `model_settings.config`. Settings names aren't held to P4's listing
    /// cap (64 entries); they are kept for up to the 3MF object safety
    /// limit (1,000,000), so every object in an accepted file keeps its
    /// name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    #[ts(type = "number")]
    pub triangle_count: u64,
    /// Over every vertex. An object with no vertices has a zero box.
    pub bounds_mm: BoundsMm,
    /// Up to 8 convex-hull faces, largest first.
    pub lay_flat_faces: Vec<LayFlatFace>,
}

/// D6: a convex-hull face the object can rest on.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/LayFlatFace.ts")]
pub struct LayFlatFace {
    /// The outward unit normal, in the object's frame.
    pub normal: [f64; 3],
    pub area_mm2: f64,
}

/// D6: a build item as the source file places it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/GeometryBuildItem.ts")]
pub struct GeometryBuildItem {
    pub object_key: u32,
    /// The 3MF `transform` attribute's 12 numbers (`m00 m01 m02 m10 m11
    /// m12 m20 m21 m22 m30 m31 m32`), in millimetres: a point maps to
    /// `x' = x·m[0] + y·m[3] + z·m[6] + m[9]`, and likewise for y and z.
    #[ts(type = "number[]")]
    pub transform: Transform3mf,
    /// The Orca/Bambu plate that lists this item, when the 3MF has plates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub plate_index: Option<u32>,
    pub printable: bool,
}

/// A revision's geometry plus the meshes behind it, as the cache holds it.
#[derive(Debug, PartialEq)]
pub struct LoadedRevision {
    geometry: RevisionGeometry,
    meshes: BTreeMap<u32, ObjectMesh>,
}

impl LoadedRevision {
    pub fn geometry(&self) -> &RevisionGeometry {
        &self.geometry
    }

    pub fn mesh(&self, object_key: u32) -> Option<&ObjectMesh> {
        self.meshes.get(&object_key)
    }

    /// D6: the object's mesh as a binary buffer ([`encode_mesh`]).
    pub fn mesh_buffer(&self, object_key: u32) -> Option<Vec<u8>> {
        self.mesh(object_key).map(encode_mesh)
    }

    #[cfg(test)]
    pub(crate) fn from_parts(
        geometry: RevisionGeometry,
        meshes: impl IntoIterator<Item = (u32, ObjectMesh)>,
    ) -> Self {
        Self {
            geometry,
            meshes: meshes.into_iter().collect(),
        }
    }
}

/// D6: reads a revision's content (`format` as stored on the revision) and
/// computes its geometry: bounds, lay-flat faces, and build items.
pub fn load_revision<R: Read + Seek>(
    reader: R,
    format: ModelFormat,
    cancel: &CancelFlag,
) -> Result<LoadedRevision, InspectError> {
    let model = formats::read_mesh(reader, format, cancel)?;
    let mut objects = Vec::with_capacity(model.objects.len());
    let mut meshes = BTreeMap::new();
    for object in model.objects {
        if cancel.is_cancelled() {
            return Err(InspectError::Cancelled);
        }
        let points = || {
            object
                .mesh
                .positions
                .iter()
                .map(|point| point.map(f64::from))
        };
        let bounds_mm = points()
            .fold(None, |bounds: Option<BoundsMm>, point| match bounds {
                Some(mut bounds) => {
                    bounds.include(point);
                    Some(bounds)
                }
                None => Some(BoundsMm {
                    min: point,
                    max: point,
                }),
            })
            .unwrap_or(BoundsMm {
                min: [0.0; 3],
                max: [0.0; 3],
            });
        let lay_flat_faces = lay_flat_faces(hull_faces(points()));
        objects.push(GeometryObject {
            object_key: object.id,
            name: object.name,
            triangle_count: object.mesh.triangles.len() as u64,
            bounds_mm,
            lay_flat_faces,
        });
        meshes.insert(object.id, object.mesh);
    }
    let build_items = model
        .build_items
        .into_iter()
        .map(|item| GeometryBuildItem {
            object_key: item.object_id,
            transform: item.transform,
            plate_index: item.plate_index,
            printable: item.printable,
        })
        .collect();
    Ok(LoadedRevision {
        geometry: RevisionGeometry {
            objects,
            build_items,
        },
        meshes,
    })
}

/// Up to [`MAX_LAY_FLAT_FACES`] of `faces` (largest first), leaving out
/// any below [`MIN_LAY_FLAT_FRACTION`] of the largest, such as `f32`
/// slivers.
fn lay_flat_faces(faces: Vec<HullFace>) -> Vec<LayFlatFace> {
    let largest = faces.first().map_or(0.0, |face| face.area);
    faces
        .into_iter()
        .filter(|face| face.area >= largest * MIN_LAY_FLAT_FRACTION)
        .take(MAX_LAY_FLAT_FACES)
        .map(|face| LayFlatFace {
            normal: face.normal,
            area_mm2: face.area,
        })
        .collect()
}

/// D6: `F3DM`, `u32` version 1, `u32` vertex count, `u32` index count, then
/// `f32` x/y/z positions and `u32` indices, all little-endian.
pub fn encode_mesh(mesh: &ObjectMesh) -> Vec<u8> {
    let index_count = mesh.triangles.len() * 3;
    let mut out = Vec::with_capacity(16 + mesh.positions.len() * 12 + index_count * 4);
    out.extend_from_slice(&MESH_MAGIC);
    out.extend_from_slice(&MESH_FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&(mesh.positions.len() as u32).to_le_bytes());
    out.extend_from_slice(&(index_count as u32).to_le_bytes());
    for value in mesh.positions.iter().flatten() {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for index in mesh.triangles.iter().flatten() {
        out.extend_from_slice(&index.to_le_bytes());
    }
    out
}

// --- D5 transforms ----------------------------------------------------------

/// D5: whether every value is finite and each scale is within
/// [`MIN_SCALE`]..=[`MAX_SCALE`].
pub fn transform_is_valid(transform: &InstanceTransform) -> bool {
    let finite = transform
        .translate_mm
        .iter()
        .chain(&transform.rotate_deg)
        .chain(&transform.scale)
        .all(|value| value.is_finite());
    finite
        && transform
            .scale
            .iter()
            .all(|factor| (MIN_SCALE..=MAX_SCALE).contains(factor))
}

/// D5: the 3MF transform for an instance of an object whose local vertices
/// are `positions`.
///
/// The local mesh is scaled, rotated about X, then Y, then Z (extrinsic,
/// degrees, `rad = deg · (π / 180)`), and translated by `translateMm` on
/// XY. The Z translation is derived so the lowest *vertex* (not a bounding
/// box corner) lands exactly on Z = 0: `m[11] = −min((x·m[2] + y·m[5]) +
/// z·m[8])`. With no vertices it is 0.
pub fn compose_transform(transform: &InstanceTransform, positions: &[[f32; 3]]) -> Transform3mf {
    let [sx, sy, sz] = transform.scale;
    let [ax, ay, az] = transform.rotate_deg.map(|degrees| degrees * (PI / 180.0));
    let (sin_x, cos_x) = ax.sin_cos();
    let (sin_y, cos_y) = ay.sin_cos();
    let (sin_z, cos_z) = az.sin_cos();
    let rotate_x = [[1.0, 0.0, 0.0], [0.0, cos_x, -sin_x], [0.0, sin_x, cos_x]];
    let rotate_y = [[cos_y, 0.0, sin_y], [0.0, 1.0, 0.0], [-sin_y, 0.0, cos_y]];
    let rotate_z = [[cos_z, -sin_z, 0.0], [sin_z, cos_z, 0.0], [0.0, 0.0, 1.0]];
    // Column-vector form: world = Rz · Ry · Rx · S · local.
    let r = multiply(&rotate_z, &multiply(&rotate_y, &rotate_x));
    let scales = [sx, sy, sz];
    // The 3MF attribute holds the transpose (row-vector form).
    let mut m = [0.0; 12];
    for local in 0..3 {
        for world in 0..3 {
            m[local * 3 + world] = r[world][local] * scales[local];
        }
    }
    m[9] = transform.translate_mm[0];
    m[10] = transform.translate_mm[1];
    let lowest = positions
        .iter()
        .map(|&[x, y, z]| {
            let (x, y, z) = (f64::from(x), f64::from(y), f64::from(z));
            x * m[2] + y * m[5] + z * m[8]
        })
        .reduce(f64::min);
    m[11] = lowest.map_or(0.0, |lowest| -lowest);
    m
}

fn multiply(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for row in 0..3 {
        for column in 0..3 {
            out[row][column] = (0..3).map(|k| a[row][k] * b[k][column]).sum();
        }
    }
    out
}

/// Applies a [`Transform3mf`] to a point.
pub fn transform_point(m: &Transform3mf, [x, y, z]: [f64; 3]) -> [f64; 3] {
    [
        x * m[0] + y * m[3] + z * m[6] + m[9],
        x * m[1] + y * m[4] + z * m[7] + m[10],
        x * m[2] + y * m[5] + z * m[8] + m[11],
    ]
}

/// The world bounds of `positions` placed by `m`, over the vertices
/// themselves. `None` with no vertices.
pub fn transformed_bounds(m: &Transform3mf, positions: &[[f32; 3]]) -> Option<BoundsMm> {
    let mut points = positions
        .iter()
        .map(|point| transform_point(m, point.map(f64::from)));
    let first = points.next()?;
    let mut bounds = BoundsMm {
        min: first,
        max: first,
    };
    for point in points {
        bounds.include(point);
    }
    Some(bounds)
}

// --- D6 cache ---------------------------------------------------------------

/// D6: loaded revisions keyed by `content_sha256`, least recently used
/// evicted past the capacity. Loads run outside the lock, so two callers
/// may load the same content at once; the later insert wins.
pub struct GeometryCache {
    capacity: usize,
    /// Most recently used first.
    entries: Mutex<VecDeque<(String, Arc<LoadedRevision>)>>,
}

impl Default for GeometryCache {
    fn default() -> Self {
        Self::with_capacity(GEOMETRY_CACHE_CAPACITY)
    }
}

impl GeometryCache {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: Mutex::new(VecDeque::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<(String, Arc<LoadedRevision>)>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The cached revision, which becomes the most recently used.
    pub fn get(&self, content_sha256: &str) -> Option<Arc<LoadedRevision>> {
        let mut entries = self.lock();
        let position = entries.iter().position(|(key, _)| key == content_sha256)?;
        let entry = entries.remove(position).expect("the position is in range");
        let revision = Arc::clone(&entry.1);
        entries.push_front(entry);
        Some(revision)
    }

    /// Caches `revision` as the most recently used, evicting the least
    /// recently used past the capacity.
    pub fn insert(&self, content_sha256: &str, revision: LoadedRevision) -> Arc<LoadedRevision> {
        let revision = Arc::new(revision);
        let mut entries = self.lock();
        entries.retain(|(key, _)| key != content_sha256);
        entries.push_front((content_sha256.to_string(), Arc::clone(&revision)));
        entries.truncate(self.capacity);
        revision
    }

    /// The cached revision, or `load`'s result, cached.
    pub fn get_or_load<E>(
        &self,
        content_sha256: &str,
        load: impl FnOnce() -> Result<LoadedRevision, E>,
    ) -> Result<Arc<LoadedRevision>, E> {
        if let Some(revision) = self.get(content_sha256) {
            return Ok(revision);
        }
        Ok(self.insert(content_sha256, load()?))
    }

    /// The cached keys, most recently used first.
    pub fn keys(&self) -> Vec<String> {
        self.lock().iter().map(|(key, _)| key.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn instance(translate: [f64; 2], rotate: [f64; 3], scale: [f64; 3]) -> InstanceTransform {
        InstanceTransform {
            translate_mm: translate,
            rotate_deg: rotate,
            scale,
        }
    }

    /// A box from (-5, -3, 2) to (15, 7, 12): off-origin, so the Z drop
    /// and the translation are visible.
    fn box_positions() -> Vec<[f32; 3]> {
        (0..8)
            .map(|corner| {
                [
                    if corner & 1 == 0 { -5.0 } else { 15.0 },
                    if corner & 2 == 0 { -3.0 } else { 7.0 },
                    if corner & 4 == 0 { 2.0 } else { 12.0 },
                ]
            })
            .collect()
    }

    fn assert_near(actual: &[f64], expected: &[f64]) {
        assert_eq!(actual.len(), expected.len());
        for (index, (a, e)) in actual.iter().zip(expected).enumerate() {
            assert!((a - e).abs() <= 1e-9, "[{index}] {a} != {e}\n{actual:?}");
        }
    }

    fn place(m: &Transform3mf, point: [f64; 3]) -> [f64; 3] {
        transform_point(m, point)
    }

    #[test]
    fn identity_only_drops_the_lowest_vertex_to_the_bed() {
        let m = compose_transform(&instance([0.0; 2], [0.0; 3], [1.0; 3]), &box_positions());
        assert_near(
            &m,
            &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -2.0],
        );
    }

    #[test]
    fn each_axis_rotation_is_right_handed_in_degrees() {
        let positions = box_positions();
        let x = compose_transform(&instance([0.0; 2], [90.0, 0.0, 0.0], [1.0; 3]), &positions);
        // About X: (x, y, z) → (x, −z, y).
        assert_near(&place(&x, [1.0, 2.0, 3.0]), &[1.0, -3.0, 2.0 + x[11]]);
        let y = compose_transform(&instance([0.0; 2], [0.0, 90.0, 0.0], [1.0; 3]), &positions);
        // About Y: (x, y, z) → (z, y, −x).
        assert_near(&place(&y, [1.0, 2.0, 3.0]), &[3.0, 2.0, -1.0 + y[11]]);
        let z = compose_transform(&instance([0.0; 2], [0.0, 0.0, 90.0], [1.0; 3]), &positions);
        // About Z: (x, y, z) → (−y, x, z).
        assert_near(&place(&z, [1.0, 2.0, 3.0]), &[-2.0, 1.0, 3.0 + z[11]]);
    }

    #[test]
    fn rotations_apply_x_then_y_then_z_after_scale() {
        let m = compose_transform(
            &instance([0.0; 2], [90.0, 0.0, 90.0], [1.0; 3]),
            &box_positions(),
        );
        // X first: +Y → +Z, which Z leaves alone. Z first would give −X.
        assert_near(&place(&m, [0.0, 1.0, 0.0]), &[0.0, 0.0, 1.0 + m[11]]);
        // X leaves +X alone, then Z turns it to +Y.
        assert_near(&place(&m, [1.0, 0.0, 0.0]), &[0.0, 1.0, m[11]]);

        let scaled = compose_transform(
            &instance([0.0; 2], [0.0, 0.0, 90.0], [2.0, 1.0, 1.0]),
            &box_positions(),
        );
        // Scale first: +X is doubled, then turned to +Y.
        assert_near(&place(&scaled, [1.0, 0.0, 0.0]), &[0.0, 2.0, scaled[11]]);
    }

    #[test]
    fn translation_is_on_xy_and_the_lowest_vertex_lands_exactly_on_zero() {
        let positions = box_positions();
        let m = compose_transform(
            &instance([150.0, 120.0], [30.0, 45.0, 60.0], [1.5, 1.0, 0.75]),
            &positions,
        );
        let origin = place(&m, [0.0; 3]);
        assert_near(&origin[..2], &[150.0, 120.0]);
        let bounds = transformed_bounds(&m, &positions).unwrap();
        assert_eq!(bounds.min[2], 0.0);
    }

    #[test]
    fn no_vertices_means_no_drop() {
        let m = compose_transform(&instance([1.0, 2.0], [0.0; 3], [1.0; 3]), &[]);
        assert_eq!(m[9..], [1.0, 2.0, 0.0]);
        assert_eq!(transformed_bounds(&m, &[]), None);
    }

    #[test]
    fn transform_validity_checks_finiteness_and_the_scale_range() {
        assert!(transform_is_valid(&instance([0.0; 2], [0.0; 3], [1.0; 3])));
        assert!(transform_is_valid(&instance(
            [0.0; 2],
            [720.0, 0.0, 0.0],
            [0.01, 100.0, 1.0]
        )));
        assert!(!transform_is_valid(&instance(
            [0.0; 2],
            [0.0; 3],
            [0.009, 1.0, 1.0]
        )));
        assert!(!transform_is_valid(&instance(
            [0.0; 2],
            [0.0; 3],
            [1.0, 100.5, 1.0]
        )));
        assert!(!transform_is_valid(&instance(
            [f64::NAN, 0.0],
            [0.0; 3],
            [1.0; 3]
        )));
        assert!(!transform_is_valid(&instance(
            [0.0; 2],
            [0.0, f64::INFINITY, 0.0],
            [1.0; 3]
        )));
    }

    /// A binary STL of the given triangles.
    pub(crate) fn binary_stl(triangles: &[[[f32; 3]; 3]]) -> Vec<u8> {
        let mut out = vec![0u8; 80];
        out.extend_from_slice(&(triangles.len() as u32).to_le_bytes());
        for triangle in triangles {
            out.extend_from_slice(&[0u8; 12]);
            for value in triangle.iter().flatten() {
                out.extend_from_slice(&value.to_le_bytes());
            }
            out.extend_from_slice(&[0, 0]);
        }
        out
    }

    fn one_triangle(offset: f32) -> LoadedRevision {
        let stl = binary_stl(&[[[offset, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]]);
        load_revision(Cursor::new(stl), ModelFormat::Stl, &CancelFlag::never()).unwrap()
    }

    #[test]
    fn the_mesh_buffer_has_the_d6_layout() {
        let revision = one_triangle(0.5);
        let buffer = revision.mesh_buffer(1).unwrap();
        assert_eq!(&buffer[..4], b"F3DM");
        let word = |at: usize| u32::from_le_bytes(buffer[at..at + 4].try_into().unwrap());
        assert_eq!((word(4), word(8), word(12)), (1, 3, 3));
        assert_eq!(buffer.len(), 16 + 3 * 12 + 3 * 4);
        let float = |at: usize| f32::from_le_bytes(buffer[at..at + 4].try_into().unwrap());
        assert_eq!([float(16), float(20), float(24)], [0.5, 0.0, 0.0]);
        assert_eq!([float(28), float(32), float(36)], [1.0, 0.0, 0.0]);
        assert_eq!([word(52), word(56), word(60)], [0, 1, 2]);
        assert_eq!(revision.mesh_buffer(2), None);
    }

    #[test]
    fn a_flat_stl_has_one_object_one_build_item_and_two_lay_flat_faces() {
        let revision = one_triangle(0.0);
        let geometry = revision.geometry();
        assert_eq!(geometry.objects.len(), 1);
        let object = &geometry.objects[0];
        assert_eq!((object.object_key, object.triangle_count), (1, 1));
        assert_eq!(object.bounds_mm.max, [1.0, 1.0, 0.0]);
        assert_eq!(object.lay_flat_faces.len(), 2);
        assert!((object.lay_flat_faces[0].area_mm2 - 0.5).abs() < 1e-12);
        assert_eq!(
            geometry.build_items,
            vec![GeometryBuildItem {
                object_key: 1,
                transform: formats::IDENTITY_TRANSFORM,
                plate_index: None,
                printable: true,
            }]
        );
    }

    #[test]
    fn the_cache_evicts_the_least_recently_used_past_four() {
        let cache = GeometryCache::default();
        let mut loads = 0;
        for key in ["a", "b", "c", "d"] {
            cache
                .get_or_load(key, || {
                    loads += 1;
                    Ok::<_, InspectError>(one_triangle(0.0))
                })
                .unwrap();
        }
        assert_eq!(cache.keys(), ["d", "c", "b", "a"]);
        // Touching "a" makes "b" the least recently used.
        assert!(cache.get("a").is_some());
        cache.insert("e", one_triangle(0.0));
        assert_eq!(cache.keys(), ["e", "a", "d", "c"]);
        assert!(cache.get("b").is_none());

        // A hit doesn't load; a miss does, and a failed load caches nothing.
        let hit = cache
            .get_or_load("c", || -> Result<_, InspectError> { panic!("c is cached") })
            .unwrap();
        assert_eq!(hit.geometry().objects.len(), 1);
        let failed = cache.get_or_load("f", || Err(InspectError::Cancelled));
        assert_eq!(failed.unwrap_err(), InspectError::Cancelled);
        assert_eq!(cache.keys(), ["c", "e", "a", "d"]);
        assert_eq!(loads, 4);
    }

    #[test]
    fn lay_flat_faces_leave_out_slivers_and_keep_the_largest_eight() {
        let face = |area: f64| HullFace {
            normal: [0.0, 0.0, -1.0],
            area,
        };
        let mut faces: Vec<HullFace> = (0..10).map(|i| face(100.0 - f64::from(i))).collect();
        faces.insert(3, face(1.0));
        faces.sort_by(|a, b| b.area.total_cmp(&a.area));
        faces.push(face(0.999));
        let kept = lay_flat_faces(faces);
        assert_eq!(kept.len(), 8);
        assert_eq!(kept[0].area_mm2, 100.0);
        assert_eq!(kept[7].area_mm2, 93.0);
        let kept = lay_flat_faces(vec![face(100.0), face(1.0), face(0.5)]);
        let areas: Vec<f64> = kept.iter().map(|face| face.area_mm2).collect();
        assert_eq!(areas, [100.0, 1.0]);
        assert!(lay_flat_faces(Vec::new()).is_empty());
    }

    #[test]
    fn gcode_has_no_geometry() {
        let error = load_revision(
            Cursor::new(b"G28\nG1 X1\n".to_vec()),
            ModelFormat::Gcode,
            &CancelFlag::never(),
        )
        .unwrap_err();
        assert_eq!(error.code(), "UNSUPPORTED_FORMAT");
    }
}
