//! D7: the per-plate input OrcaSlicer slices, a core-only 3MF (2015/02).
//!
//! - One `<object>` per distinct `objectKey` on the plate, numbered from 1
//!   in ascending key order, holding the source revision's mesh in its own
//!   frame (components already flattened).
//! - One `<item>` per instance, in plate order, with the D5 transform
//!   ([`compose_transform`]): Z is already dropped to the bed.
//! - No Orca metadata, no Production extension, and no settings: with
//!   `--arrange 0 --orient 0`, OrcaSlicer places items exactly by their
//!   transforms (spike Gate C).
//!
//! The bytes depend only on the plate and the mesh: the ZIP entries are
//! `[Content_Types].xml`, `_rels/.rels`, and `3D/3dmodel.model`, in that
//! order, deflated, dated 1980-01-01 00:00, with mode 0644. Numbers are
//! written with 6 decimal places.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

use super::geometry::{compose_transform, transform_is_valid, LoadedRevision, Transform3mf};
use super::PlateDoc;
use crate::contracts::command::CommandError;
use crate::library::formats::ObjectMesh;

pub const CONTENT_TYPES_PART: &str = "[Content_Types].xml";
pub const ROOT_RELS_PART: &str = "_rels/.rels";
pub const MODEL_PART: &str = "3D/3dmodel.model";

const CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n\
 <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\n\
 <Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/>\n\
</Types>\n";

const ROOT_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n\
 <Relationship Target=\"/3D/3dmodel.model\" Id=\"rel0\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/>\n\
</Relationships>\n";

const MODEL_HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\">\n";

/// The model part is streamed into the ZIP in chunks of about this size.
const CHUNK_BYTES: usize = 64 * 1024;

/// Why a plate can't be written. Each maps to `PREPARATION_INVALID` with
/// the plate's key and [`PlateInvalidReason::as_str`] as `reason`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlateInvalidReason {
    /// No instances. OrcaSlicer would fail with code −6 (Gate F).
    Empty,
    /// An instance's `objectKey` isn't an object of the source revision.
    UnknownObject,
    /// An instance's object has no vertices, so there is nothing to slice.
    EmptyObject,
    /// An instance's transform is non-finite or its scale is out of range.
    InvalidTransform,
}

impl PlateInvalidReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::UnknownObject => "unknownObject",
            Self::EmptyObject => "emptyObject",
            Self::InvalidTransform => "invalidTransform",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::Empty => "The plate has no objects to slice.",
            Self::UnknownObject => "The plate has an object that isn't in the source revision.",
            Self::EmptyObject => "An object on the plate has no geometry.",
            Self::InvalidTransform => {
                "An object on the plate has an invalid position, rotation, or scale."
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlateWriteError {
    Invalid {
        plate_key: String,
        reason: PlateInvalidReason,
    },
    /// Writing the ZIP failed (not expected for an in-memory buffer).
    Package(String),
}

impl From<PlateWriteError> for CommandError {
    fn from(error: PlateWriteError) -> Self {
        match error {
            PlateWriteError::Invalid { plate_key, reason } => {
                CommandError::preparation_invalid(&plate_key, reason.as_str(), reason.message())
            }
            PlateWriteError::Package(_) => CommandError::internal(),
        }
    }
}

impl From<zip::result::ZipError> for PlateWriteError {
    fn from(error: zip::result::ZipError) -> Self {
        Self::Package(error.to_string())
    }
}

impl From<std::io::Error> for PlateWriteError {
    fn from(error: std::io::Error) -> Self {
        Self::Package(error.to_string())
    }
}

/// D7: writes `plate` as a core-only 3MF over the source revision's
/// meshes, which instances name by `objectKey`.
pub fn write_plate_3mf(
    plate: &PlateDoc,
    revision: &LoadedRevision,
) -> Result<Vec<u8>, PlateWriteError> {
    let invalid = |reason| PlateWriteError::Invalid {
        plate_key: plate.plate_key.clone(),
        reason,
    };
    if plate.instances.is_empty() {
        return Err(invalid(PlateInvalidReason::Empty));
    }

    // Validate every instance before writing anything.
    let mut meshes: BTreeMap<u32, &ObjectMesh> = BTreeMap::new();
    let mut placed: Vec<(u32, Transform3mf)> = Vec::with_capacity(plate.instances.len());
    for instance in &plate.instances {
        let key = instance.object_key;
        let mesh = revision
            .mesh(key)
            .ok_or_else(|| invalid(PlateInvalidReason::UnknownObject))?;
        if mesh.positions.is_empty() {
            return Err(invalid(PlateInvalidReason::EmptyObject));
        }
        if !transform_is_valid(&instance.transform) {
            return Err(invalid(PlateInvalidReason::InvalidTransform));
        }
        meshes.insert(key, mesh);
        placed.push((key, compose_transform(&instance.transform, &mesh.positions)));
    }
    let object_ids: BTreeMap<u32, usize> = meshes
        .keys()
        .enumerate()
        .map(|(index, key)| (*key, index + 1))
        .collect();

    let items: Vec<String> = placed
        .iter()
        .map(|(key, transform)| {
            let values: Vec<String> = transform.iter().map(|value| decimal(*value)).collect();
            format!(
                "  <item objectid=\"{}\" transform=\"{}\"/>\n",
                object_ids[key],
                values.join(" ")
            )
        })
        .collect();

    // An upper bound on the model part's size decides ZIP64, which only a
    // model over 4 GiB needs. Each coordinate is written with at most
    // `number_width` characters for the largest magnitude in its mesh.
    let estimated: u64 = meshes
        .values()
        .map(|mesh| {
            let largest = mesh
                .positions
                .iter()
                .flatten()
                .fold(0.0f64, |largest, value| largest.max(f64::from(value.abs())));
            let vertex_line = 30 + 3 * number_width(largest);
            let triangle_line = 35 + 3 * 10;
            mesh.positions.len() as u64 * vertex_line
                + mesh.triangles.len() as u64 * triangle_line
                + 256
        })
        .sum::<u64>()
        + items.iter().map(|item| item.len() as u64).sum::<u64>()
        + 1024;
    let options = |large| {
        SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(DateTime::default())
            .unix_permissions(0o644)
            .large_file(large)
    };

    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(CONTENT_TYPES_PART, options(false))?;
    zip.write_all(CONTENT_TYPES.as_bytes())?;
    zip.start_file(ROOT_RELS_PART, options(false))?;
    zip.write_all(ROOT_RELS.as_bytes())?;
    zip.start_file(MODEL_PART, options(estimated >= u64::from(u32::MAX)))?;

    let mut chunk = String::with_capacity(CHUNK_BYTES * 2);
    let mut flush = |chunk: &mut String, force: bool| -> std::io::Result<()> {
        if force || chunk.len() >= CHUNK_BYTES {
            zip.write_all(chunk.as_bytes())?;
            chunk.clear();
        }
        Ok(())
    };
    chunk.push_str(MODEL_HEADER);
    chunk.push_str(" <resources>\n");
    for (key, mesh) in &meshes {
        let _ = writeln!(
            chunk,
            "  <object id=\"{}\" type=\"model\">\n   <mesh>\n    <vertices>",
            object_ids[key]
        );
        for [x, y, z] in &mesh.positions {
            let _ = writeln!(
                chunk,
                "     <vertex x=\"{}\" y=\"{}\" z=\"{}\"/>",
                decimal(f64::from(*x)),
                decimal(f64::from(*y)),
                decimal(f64::from(*z))
            );
            flush(&mut chunk, false)?;
        }
        chunk.push_str("    </vertices>\n    <triangles>\n");
        for [a, b, c] in &mesh.triangles {
            let _ = writeln!(chunk, "     <triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\"/>");
            flush(&mut chunk, false)?;
        }
        chunk.push_str("    </triangles>\n   </mesh>\n  </object>\n");
    }
    chunk.push_str(" </resources>\n <build>\n");
    for item in &items {
        chunk.push_str(item);
        flush(&mut chunk, false)?;
    }
    chunk.push_str(" </build>\n</model>\n");
    flush(&mut chunk, true)?;

    Ok(zip.finish()?.into_inner())
}

/// At least the characters [`decimal`] writes for a value of at most
/// `largest` in magnitude: a sign, the integer digits (plus one for a
/// rounding carry), a point, and six decimals.
fn number_width(largest: f64) -> u64 {
    largest.max(1.0).log10().floor() as u64 + 2 + 8
}

/// Six decimal places, with a rounded negative zero written as zero.
fn decimal(value: f64) -> String {
    let text = format!("{value:.6}");
    if text == "-0.000000" {
        "0.000000".to_string()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::library::content::CancelFlag;
    use crate::library::formats::{self, Inspection};
    use crate::library::ModelFormat;
    use crate::slicing::geometry::load_revision;
    use crate::slicing::{InstanceDoc, InstanceTransform};

    /// The 10 mm cube, (0, 0, 0) to (10, 10, 10), as a binary STL.
    fn cube_revision() -> LoadedRevision {
        let v = |x: f32, y: f32, z: f32| [x * 10.0, y * 10.0, z * 10.0];
        let quads = [
            [v(0., 0., 0.), v(0., 1., 0.), v(1., 1., 0.), v(1., 0., 0.)],
            [v(0., 0., 1.), v(1., 0., 1.), v(1., 1., 1.), v(0., 1., 1.)],
            [v(0., 0., 0.), v(1., 0., 0.), v(1., 0., 1.), v(0., 0., 1.)],
            [v(0., 1., 0.), v(0., 1., 1.), v(1., 1., 1.), v(1., 1., 0.)],
            [v(0., 0., 0.), v(0., 0., 1.), v(0., 1., 1.), v(0., 1., 0.)],
            [v(1., 0., 0.), v(1., 1., 0.), v(1., 1., 1.), v(1., 0., 1.)],
        ];
        let mut stl = vec![0u8; 80];
        stl.extend_from_slice(&12u32.to_le_bytes());
        for [a, b, c, d] in quads {
            for triangle in [[a, b, c], [a, c, d]] {
                stl.extend_from_slice(&[0u8; 12]);
                for value in triangle.iter().flatten() {
                    stl.extend_from_slice(&value.to_le_bytes());
                }
                stl.extend_from_slice(&[0, 0]);
            }
        }
        load_revision(Cursor::new(stl), ModelFormat::Stl, &CancelFlag::never()).unwrap()
    }

    fn instance(key: &str, object_key: u32, translate: [f64; 2], rotate_z: f64) -> InstanceDoc {
        InstanceDoc {
            instance_key: key.to_string(),
            object_key,
            transform: InstanceTransform {
                translate_mm: translate,
                rotate_deg: [0.0, 0.0, rotate_z],
                scale: [1.0; 3],
            },
        }
    }

    fn plate(instances: Vec<InstanceDoc>) -> PlateDoc {
        PlateDoc {
            plate_key: "plate-a".to_string(),
            name: None,
            instances,
        }
    }

    fn two_cubes() -> PlateDoc {
        plate(vec![
            instance("i-a", 1, [40.0, 40.0], 0.0),
            instance("i-b", 1, [150.0, 120.0], 45.0),
        ])
    }

    /// Each entry's name, date, permission bits, and compression.
    type Entry = (
        String,
        (u16, u8, u8, u8, u8),
        Option<u32>,
        CompressionMethod,
    );

    fn entries(bytes: &[u8]) -> Vec<Entry> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        (0..archive.len())
            .map(|index| {
                let entry = archive.by_index(index).unwrap();
                let date = entry.last_modified().unwrap();
                (
                    entry.name().to_string(),
                    (
                        date.year(),
                        date.month(),
                        date.day(),
                        date.hour(),
                        date.minute(),
                    ),
                    entry.unix_mode().map(|mode| mode & 0o777),
                    entry.compression(),
                )
            })
            .collect()
    }

    fn model_xml(bytes: &[u8]) -> String {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut text = String::new();
        std::io::Read::read_to_string(&mut archive.by_name(MODEL_PART).unwrap(), &mut text)
            .unwrap();
        text
    }

    #[test]
    fn entries_are_fixed_in_order_date_and_mode() {
        let bytes = write_plate_3mf(&two_cubes(), &cube_revision()).unwrap();
        let expected = |name: &str| {
            (
                name.to_string(),
                (1980, 1, 1, 0, 0),
                Some(0o644),
                CompressionMethod::Deflated,
            )
        };
        assert_eq!(
            entries(&bytes),
            vec![
                expected(CONTENT_TYPES_PART),
                expected(ROOT_RELS_PART),
                expected(MODEL_PART)
            ]
        );
        // No clock and no randomness: the same input writes the same bytes.
        assert_eq!(
            bytes,
            write_plate_3mf(&two_cubes(), &cube_revision()).unwrap()
        );
    }

    #[test]
    fn one_object_per_key_and_one_item_per_instance_with_six_decimals() {
        let xml = model_xml(&write_plate_3mf(&two_cubes(), &cube_revision()).unwrap());
        assert_eq!(xml.matches("<object ").count(), 1);
        assert_eq!(xml.matches("<vertex ").count(), 8);
        assert_eq!(xml.matches("<triangle ").count(), 12);
        assert!(xml.contains(
            "<item objectid=\"1\" transform=\"1.000000 0.000000 0.000000 0.000000 1.000000 \
             0.000000 0.000000 0.000000 1.000000 40.000000 40.000000 0.000000\"/>"
        ));
        assert!(xml.contains(
            "<item objectid=\"1\" transform=\"0.707107 0.707107 0.000000 -0.707107 0.707107 \
             0.000000 0.000000 0.000000 1.000000 150.000000 120.000000 0.000000\"/>"
        ));
        assert!(xml.contains("<vertex x=\"10.000000\" y=\"10.000000\" z=\"10.000000\"/>"));
        for forbidden in ["Orca", "Bambu", "xmlns:", "metadata", "-0.000000"] {
            assert!(!xml.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn the_written_plate_reads_back_through_the_p4_inspector() {
        let bytes = write_plate_3mf(&two_cubes(), &cube_revision()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plate.3mf");
        std::fs::write(&path, &bytes).unwrap();
        let detected = formats::detect(&path).unwrap();
        let outcome = formats::inspect(&path, &detected, &CancelFlag::never()).unwrap();
        let Inspection::ThreeMf(model) = outcome.inspection else {
            panic!("not a 3MF: {:?}", outcome.inspection);
        };
        assert_eq!((model.object_count, model.build_item_count), (1, 2));
        assert_eq!(model.triangle_count, 24);
        assert!(model.unsupported.is_empty() && model.required_extensions.is_empty());
        let half_diagonal = 10.0 * std::f64::consts::FRAC_1_SQRT_2;
        let expected_min = [40.0, 40.0, 0.0];
        let expected_max = [150.0 + half_diagonal, 120.0 + 2.0 * half_diagonal, 10.0];
        for axis in 0..3 {
            assert!((model.bounds_mm.min[axis] - expected_min[axis]).abs() < 1e-5);
            assert!((model.bounds_mm.max[axis] - expected_max[axis]).abs() < 1e-5);
        }
    }

    #[test]
    fn number_width_bounds_the_written_text() {
        for value in [
            0.0f64,
            9.999_999_9,
            -9.999_999_9,
            10.0,
            99.5,
            123_456.789,
            -1e7,
            3.4e38,
            -3.4e38,
        ] {
            let width = number_width(value.abs()) as usize;
            assert!(decimal(value).len() <= width, "{value}: {}", decimal(value));
        }
    }

    #[test]
    fn distinct_keys_become_objects_numbered_in_key_order() {
        let mut source = cube_revision();
        let geometry = source.geometry().clone();
        source = LoadedRevision::from_parts(
            geometry,
            [
                (7, source.mesh(1).unwrap().clone()),
                (3, source.mesh(1).unwrap().clone()),
            ],
        );
        let plate = plate(vec![
            instance("i-a", 7, [0.0; 2], 0.0),
            instance("i-b", 3, [50.0, 0.0], 0.0),
            instance("i-c", 7, [100.0, 0.0], 0.0),
        ]);
        let xml = model_xml(&write_plate_3mf(&plate, &source).unwrap());
        assert_eq!(xml.matches("<object ").count(), 2);
        let items: Vec<&str> = xml
            .match_indices("<item objectid=\"")
            .map(|(at, _)| &xml[at + 16..at + 17])
            .collect();
        // Key 3 is object 1 and key 7 is object 2; items keep plate order.
        assert_eq!(items, ["2", "1", "2"]);
    }

    #[test]
    fn invalid_plates_are_refused_with_their_reason() {
        let revision = cube_revision();
        let reason = |plate: PlateDoc| match write_plate_3mf(&plate, &revision) {
            Err(PlateWriteError::Invalid { plate_key, reason }) => {
                assert_eq!(plate_key, "plate-a");
                reason
            }
            other => panic!("expected an invalid plate, got {other:?}"),
        };
        assert_eq!(reason(plate(vec![])), PlateInvalidReason::Empty);
        assert_eq!(
            reason(plate(vec![instance("i", 2, [0.0; 2], 0.0)])),
            PlateInvalidReason::UnknownObject
        );
        let mut scaled = instance("i", 1, [0.0; 2], 0.0);
        scaled.transform.scale = [0.001, 1.0, 1.0];
        assert_eq!(
            reason(plate(vec![scaled])),
            PlateInvalidReason::InvalidTransform
        );

        // An object with no vertices is refused rather than written empty.
        let hollow = LoadedRevision::from_parts(
            revision.geometry().clone(),
            [
                (1, revision.mesh(1).unwrap().clone()),
                (5, ObjectMesh::default()),
            ],
        );
        let result = write_plate_3mf(
            &plate(vec![
                instance("i-a", 1, [0.0; 2], 0.0),
                instance("i-b", 5, [50.0, 0.0], 0.0),
            ]),
            &hollow,
        );
        assert_eq!(
            result,
            Err(PlateWriteError::Invalid {
                plate_key: "plate-a".to_string(),
                reason: PlateInvalidReason::EmptyObject,
            })
        );
        let error = CommandError::from(result.unwrap_err());
        assert_eq!(
            serde_json::to_value(&error).unwrap()["details"]["reason"],
            "emptyObject"
        );

        let error = CommandError::from(PlateWriteError::Invalid {
            plate_key: "plate-a".to_string(),
            reason: PlateInvalidReason::Empty,
        });
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["code"], "PREPARATION_INVALID");
        assert_eq!(json["details"]["plateKey"], "plate-a");
        assert_eq!(json["details"]["reason"], "empty");
    }
}
