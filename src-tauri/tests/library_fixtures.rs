//! Library format fixtures (spec D20).
//!
//! `regenerate_library_fixtures` writes the generated fixtures under
//! `tests/fixtures/library/`. It is ignored by default and runs through
//! `just gen-library-fixtures`. The output is deterministic: no timestamps,
//! fixed ZIP entry dates, so a second run leaves no diff.
//!
//! The generator never writes or deletes `*.expected.json`. Those files are
//! hand-written oracles and must stay independent of any code under test.
//!
//! The slicer exports (`orca-*`, `prusa-*`) are committed as produced by
//! the slicers and are never written by the generator; the spike report
//! records how each was made.
//!
//! `every_fixture_matches_its_expected_inspection` runs detection and
//! inspection over every fixture that has an oracle, and fails if a
//! generated fixture or a slicer export lacks one.

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use farm3d_lib::library::content::CancelFlag;
use farm3d_lib::library::formats::{self, InspectError, InspectOutcome};
use serde_json::Value;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library")
}

/// A 2×2 RGB PNG (red, green / blue, white), written byte for byte so the
/// generator needs no image or checksum crate.
const THUMBNAIL_PNG: [u8; 77] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, //
    0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, //
    0x08, 0x02, 0x00, 0x00, 0x00, 0xfd, 0xd4, 0x9a, 0x73, 0x00, 0x00, 0x00, //
    0x14, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8, 0xcf, 0xc0, 0xc0, //
    0x00, 0xc2, 0x0c, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x00, 0x1f, 0xee, 0x05, //
    0xfb, 0x60, 0x6c, 0x70, 0xf2, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, //
    0x44, 0xae, 0x42, 0x60, 0x82,
];

type Vertex = [f32; 3];

struct Facet {
    normal: Vertex,
    vertices: [Vertex; 3],
}

/// The 10 mm cube from (0,0,0) to (10,10,10): 12 triangles, wound
/// counter-clockwise when seen from outside.
fn cube() -> Vec<Facet> {
    let s = 10.0;
    let quads: [(Vertex, [Vertex; 4]); 6] = [
        (
            [0.0, 0.0, -1.0],
            [[0.0, 0.0, 0.0], [0.0, s, 0.0], [s, s, 0.0], [s, 0.0, 0.0]],
        ),
        (
            [0.0, 0.0, 1.0],
            [[0.0, 0.0, s], [s, 0.0, s], [s, s, s], [0.0, s, s]],
        ),
        (
            [0.0, -1.0, 0.0],
            [[0.0, 0.0, 0.0], [s, 0.0, 0.0], [s, 0.0, s], [0.0, 0.0, s]],
        ),
        (
            [0.0, 1.0, 0.0],
            [[0.0, s, 0.0], [0.0, s, s], [s, s, s], [s, s, 0.0]],
        ),
        (
            [-1.0, 0.0, 0.0],
            [[0.0, 0.0, 0.0], [0.0, 0.0, s], [0.0, s, s], [0.0, s, 0.0]],
        ),
        (
            [1.0, 0.0, 0.0],
            [[s, 0.0, 0.0], [s, s, 0.0], [s, s, s], [s, 0.0, s]],
        ),
    ];
    quads
        .iter()
        .flat_map(|(normal, [a, b, c, d])| {
            [
                Facet {
                    normal: *normal,
                    vertices: [*a, *b, *c],
                },
                Facet {
                    normal: *normal,
                    vertices: [*a, *c, *d],
                },
            ]
        })
        .collect()
}

fn ascii_stl(first_line: &str, facets: &[Facet]) -> Vec<u8> {
    let mut out = format!("{first_line}\n");
    for facet in facets {
        let [nx, ny, nz] = facet.normal;
        out.push_str(&format!(
            "  facet normal {nx:.6} {ny:.6} {nz:.6}\n    outer loop\n"
        ));
        for [x, y, z] in facet.vertices {
            out.push_str(&format!("      vertex {x:.6} {y:.6} {z:.6}\n"));
        }
        out.push_str("    endloop\n  endfacet\n");
    }
    let name = first_line.trim_start_matches("solid").trim();
    if name.is_empty() {
        out.push_str("endsolid\n");
    } else {
        out.push_str(&format!("endsolid {name}\n"));
    }
    out.into_bytes()
}

/// A binary STL whose header is `header` padded with zeros, whose declared
/// count is `declared_count`, and which carries `facets` as written.
fn binary_stl(header: &str, declared_count: u32, facets: &[Facet]) -> Vec<u8> {
    assert!(header.len() <= 80, "STL header is at most 80 bytes");
    let mut out = header.as_bytes().to_vec();
    out.resize(80, 0);
    out.extend_from_slice(&declared_count.to_le_bytes());
    for facet in facets {
        for value in facet.normal.iter().chain(facet.vertices.iter().flatten()) {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    out
}

/// A 3MF `<mesh>` for `facets`, with vertices deduplicated in first-seen
/// order.
fn mesh_xml(facets: &[Facet], indent: &str) -> String {
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut triangles = Vec::new();
    for facet in facets {
        let mut indices = [0usize; 3];
        for (slot, vertex) in indices.iter_mut().zip(facet.vertices) {
            *slot = match vertices.iter().position(|v| *v == vertex) {
                Some(index) => index,
                None => {
                    vertices.push(vertex);
                    vertices.len() - 1
                }
            };
        }
        triangles.push(indices);
    }
    let mut out = format!("{indent}<mesh>\n{indent}  <vertices>\n");
    for [x, y, z] in vertices {
        out.push_str(&format!(
            "{indent}    <vertex x=\"{x}\" y=\"{y}\" z=\"{z}\"/>\n"
        ));
    }
    out.push_str(&format!("{indent}  </vertices>\n{indent}  <triangles>\n"));
    for [v1, v2, v3] in triangles {
        out.push_str(&format!(
            "{indent}    <triangle v1=\"{v1}\" v2=\"{v2}\" v3=\"{v3}\"/>\n"
        ));
    }
    out.push_str(&format!("{indent}  </triangles>\n{indent}</mesh>\n"));
    out
}

const CORE_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
const PRODUCTION_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/production/2015/06";
const BEAM_LATTICE_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/beamlattice/2017/02";
const MODEL_REL: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
const THUMBNAIL_REL: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail";

fn content_types(with_png: bool) -> String {
    let png = if with_png {
        "  <Default Extension=\"png\" ContentType=\"image/png\"/>\n"
    } else {
        ""
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n\
         \x20 <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\n\
         \x20 <Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/>\n\
         {png}</Types>\n"
    )
}

fn relationships(entries: &[(&str, &str, &str)]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n",
    );
    for (id, target, kind) in entries {
        out.push_str(&format!(
            "  <Relationship Id=\"{id}\" Target=\"{target}\" Type=\"{kind}\"/>\n"
        ));
    }
    out.push_str("</Relationships>\n");
    out
}

fn root_rels(with_thumbnail: bool) -> String {
    let mut entries = vec![("rel0", "/3D/3dmodel.model", MODEL_REL)];
    if with_thumbnail {
        entries.push(("rel1", "/Metadata/thumbnail.png", THUMBNAIL_REL));
    }
    relationships(&entries)
}

/// Builds a ZIP whose bytes depend only on `entries`: fixed dates, fixed
/// permissions, entries in the order given. PNGs are stored, XML is deflated.
fn zip_package(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        let method = if name.ends_with(".png") {
            CompressionMethod::Stored
        } else {
            CompressionMethod::Deflated
        };
        let options = SimpleFileOptions::default()
            .compression_method(method)
            .last_modified_time(DateTime::default())
            .unix_permissions(0o644);
        writer.start_file(*name, options).expect("start zip entry");
        writer.write_all(bytes).expect("write zip entry");
    }
    writer.finish().expect("finish zip").into_inner()
}

fn model_xml(namespaces: &str, required: Option<&str>, body: &str) -> String {
    let required = required
        .map(|value| format!(" requiredextensions=\"{value}\""))
        .unwrap_or_default();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"{CORE_NS}\"{namespaces}{required}>\n\
         {body}</model>\n"
    )
}

fn core_two_objects() -> Vec<u8> {
    let body = format!(
        "  <metadata name=\"Title\">farm3d core two objects</metadata>\n\
         \x20 <resources>\n\
         \x20   <object id=\"1\" name=\"Cube A\" type=\"model\">\n\
         {mesh}\
         \x20   </object>\n\
         \x20   <object id=\"2\" name=\"Cube B\" type=\"model\">\n\
         \x20     <components>\n\
         \x20       <component objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 20 0 0\"/>\n\
         \x20     </components>\n\
         \x20   </object>\n\
         \x20 </resources>\n\
         \x20 <build>\n\
         \x20   <item objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 5 5 0\"/>\n\
         \x20   <item objectid=\"2\"/>\n\
         \x20 </build>\n",
        mesh = mesh_xml(&cube(), "      "),
    );
    let model = model_xml("", None, &body);
    zip_package(&[
        ("[Content_Types].xml", content_types(true).as_bytes()),
        ("_rels/.rels", root_rels(true).as_bytes()),
        ("3D/3dmodel.model", model.as_bytes()),
        ("Metadata/thumbnail.png", &THUMBNAIL_PNG),
    ])
}

fn required_beam_lattice() -> Vec<u8> {
    let body = format!(
        "  <resources>\n\
         \x20   <object id=\"1\" name=\"Cube\" type=\"model\">\n\
         {mesh}\
         \x20   </object>\n\
         \x20 </resources>\n\
         \x20 <build>\n\
         \x20   <item objectid=\"1\"/>\n\
         \x20 </build>\n",
        mesh = mesh_xml(&cube(), "      "),
    );
    let model = model_xml(&format!(" xmlns:b=\"{BEAM_LATTICE_NS}\""), Some("b"), &body);
    zip_package(&[
        ("[Content_Types].xml", content_types(false).as_bytes()),
        ("_rels/.rels", root_rels(false).as_bytes()),
        ("3D/3dmodel.model", model.as_bytes()),
    ])
}

fn zip_slip() -> Vec<u8> {
    let body = "  <resources>\n\
                \x20   <object id=\"2\" name=\"Escape\" type=\"model\">\n\
                \x20     <components>\n\
                \x20       <component p:path=\"/../../etc/passwd\" objectid=\"1\"/>\n\
                \x20     </components>\n\
                \x20   </object>\n\
                \x20 </resources>\n\
                \x20 <build>\n\
                \x20   <item objectid=\"2\"/>\n\
                \x20 </build>\n";
    let model = model_xml(&format!(" xmlns:p=\"{PRODUCTION_NS}\""), Some("p"), body);
    let part_rels = relationships(&[("rel0", "/../../etc/passwd", MODEL_REL)]);
    zip_package(&[
        ("[Content_Types].xml", content_types(false).as_bytes()),
        ("_rels/.rels", root_rels(false).as_bytes()),
        ("3D/3dmodel.model", model.as_bytes()),
        ("3D/_rels/3dmodel.model.rels", part_rels.as_bytes()),
    ])
}

fn no_objects() -> Vec<u8> {
    let model = model_xml("", None, "  <resources>\n  </resources>\n  <build/>\n");
    zip_package(&[
        ("[Content_Types].xml", content_types(false).as_bytes()),
        ("_rels/.rels", root_rels(false).as_bytes()),
        ("3D/3dmodel.model", model.as_bytes()),
    ])
}

fn cura_style_gcode() -> Vec<u8> {
    let mut out = String::from(
        ";FLAVOR:Marlin\n\
         ;TIME:1234\n\
         ;Filament used: 1.2m\n\
         ;Layer height: 0.2\n\
         ;Generated with Cura_SteamEngine 5.8.0\n",
    );
    for i in 1..=20 {
        out.push_str(&format!("G1 X{} Y{} F3000\n", i * 2, i));
    }
    out.into_bytes()
}

fn plain_gcode() -> Vec<u8> {
    let mut out = String::from("G28\nG90\n");
    for i in 1..=10 {
        out.push_str(&format!("G1 X{} Y{} F3000\n", i * 5, i * 3));
    }
    out.into_bytes()
}

fn binary_gcode() -> Vec<u8> {
    let mut out = b"GCDE".to_vec();
    out.extend_from_slice(&[0u8; 12]);
    out
}

/// Every generated fixture, by file name. Slicer exports are not listed:
/// they are committed as the slicer produced them.
fn generated_fixtures() -> Vec<(&'static str, Vec<u8>)> {
    let facets = cube();
    let mut with_nan = cube();
    with_nan[4].vertices[1][0] = f32::NAN;
    vec![
        ("cube-ascii.stl", ascii_stl("solid farm3d-cube", &facets)),
        ("cube-ascii-bare-solid.stl", ascii_stl("solid", &facets)),
        (
            "cube-binary.stl",
            binary_stl("farm3d binary cube", 12, &facets),
        ),
        (
            "cube-binary-solid-header.stl",
            binary_stl(
                "solid farm3d-binary cube with an ASCII-looking header",
                12,
                &facets,
            ),
        ),
        (
            "cube-for-slicers.stl",
            binary_stl("farm3d cube for slicer exports", 12, &facets),
        ),
        (
            "truncated-binary.stl",
            binary_stl("farm3d truncated cube", 12, &facets[..3]),
        ),
        (
            "nan.stl",
            binary_stl("farm3d cube with a NaN coordinate", 12, &with_nan),
        ),
        ("empty.stl", binary_stl("farm3d empty", 0, &[])),
        ("core-two-objects.3mf", core_two_objects()),
        ("required-beam-lattice.3mf", required_beam_lattice()),
        ("zip-slip.3mf", zip_slip()),
        ("no-objects.3mf", no_objects()),
        ("cura-style.gcode", cura_style_gcode()),
        ("plain.gcode", plain_gcode()),
        ("binary.bgcode", binary_gcode()),
    ]
}

#[test]
#[ignore = "writes fixtures; run through `just gen-library-fixtures`"]
fn regenerate_library_fixtures() {
    let dir = fixture_dir();
    fs::create_dir_all(&dir).expect("create fixture dir");
    for (name, bytes) in generated_fixtures() {
        assert!(
            !name.ends_with(".expected.json"),
            "the generator must never write an expected-inspection file"
        );
        fs::write(dir.join(name), bytes).expect("write fixture");
    }
}

const EXPECTED_SUFFIX: &str = ".expected.json";
/// Slicer exports committed as produced (spike report, Step 2).
const SLICER_FIXTURES: [&str; 4] = [
    "orca-two-plates.3mf",
    "orca-cube.gcode",
    "prusa-project.3mf",
    "prusa-cube.gcode",
];
const FLOAT_TOLERANCE: f64 = 1e-4;

fn inspect_fixture(path: &Path) -> Result<InspectOutcome, InspectError> {
    let detected = formats::detect(path)?;
    formats::inspect(path, &detected, &CancelFlag::never())
}

/// Structural equality, with numbers equal within [`FLOAT_TOLERANCE`].
fn assert_matches(actual: &Value, expected: &Value, at: &str, fixture: &str) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(e)) => {
            let (a, e) = (a.as_f64().unwrap(), e.as_f64().unwrap());
            assert!(
                (a - e).abs() <= FLOAT_TOLERANCE,
                "{fixture} {at}: {a} != {e}"
            );
        }
        (Value::Array(a), Value::Array(e)) => {
            assert_eq!(a.len(), e.len(), "{fixture} {at}: array length\n{actual:#}");
            for (index, (a, e)) in a.iter().zip(e).enumerate() {
                assert_matches(a, e, &format!("{at}[{index}]"), fixture);
            }
        }
        (Value::Object(a), Value::Object(e)) => {
            let mut a_keys: Vec<_> = a.keys().collect();
            let mut e_keys: Vec<_> = e.keys().collect();
            a_keys.sort();
            e_keys.sort();
            assert_eq!(a_keys, e_keys, "{fixture} {at}: keys\n{actual:#}");
            for (key, e) in e {
                assert_matches(&a[key], e, &format!("{at}.{key}"), fixture);
            }
        }
        _ => assert_eq!(actual, expected, "{fixture} {at}"),
    }
}

#[test]
fn every_fixture_matches_its_expected_inspection() {
    let dir = fixture_dir();
    let mut names: Vec<String> = fs::read_dir(&dir)
        .expect("read fixture dir")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| !name.ends_with(EXPECTED_SUFFIX))
        .collect();
    names.sort();

    let mut checked = Vec::new();
    for name in names {
        let expected_path = dir.join(format!("{name}{EXPECTED_SUFFIX}"));
        if !expected_path.exists() {
            continue;
        }
        let expected: Value =
            serde_json::from_slice(&fs::read(&expected_path).unwrap()).expect("oracle is JSON");
        let result = inspect_fixture(&dir.join(&name));
        if let Some(code) = expected.get("error") {
            let error = result.expect_err(&format!("{name} should be rejected"));
            assert_eq!(error.code(), code.as_str().unwrap(), "{name}: {error:?}");
            let reason = expected["reasonContains"].as_str().unwrap();
            assert!(
                error.message().contains(reason),
                "{name}: {:?} lacks {reason:?}",
                error.message()
            );
            if let Some(extensions) = expected.get("extensions") {
                let InspectError::UnsupportedFormat {
                    extensions: actual, ..
                } = &error
                else {
                    panic!("{name}: extensions expected on {error:?}");
                };
                assert_eq!(&serde_json::to_value(actual).unwrap(), extensions, "{name}");
            }
        } else {
            let outcome = result.unwrap_or_else(|error| panic!("{name}: {error:?}"));
            assert!(
                outcome.warnings.is_empty(),
                "{name}: {:?}",
                outcome.warnings
            );
            let actual = serde_json::to_value(&outcome.inspection).unwrap();
            assert_matches(&actual, &expected, "$", &name);
        }
        checked.push(name);
    }

    for (name, _) in generated_fixtures() {
        assert!(
            checked.iter().any(|checked| checked == name),
            "generated fixture {name} has no {EXPECTED_SUFFIX} oracle"
        );
    }
    for name in SLICER_FIXTURES {
        assert!(
            checked.iter().any(|checked| checked == name),
            "slicer fixture {name} is missing or has no {EXPECTED_SUFFIX} oracle"
        );
    }
}

#[test]
fn core_two_objects_yields_its_embedded_thumbnail() {
    let outcome = inspect_fixture(&fixture_dir().join("core-two-objects.3mf")).unwrap();
    let thumbnail = outcome
        .thumbnail
        .expect("Metadata/thumbnail.png is present");
    assert_eq!(thumbnail.origin_part, "Metadata/thumbnail.png");
    assert_eq!((thumbnail.width, thumbnail.height), (2, 2));
    assert_eq!(thumbnail.bytes, THUMBNAIL_PNG);
}

#[test]
fn prusa_project_dangling_thumbnail_relationship_is_no_thumbnail() {
    let outcome = inspect_fixture(&fixture_dir().join("prusa-project.3mf")).unwrap();
    assert_eq!(outcome.thumbnail, None);
}

/// Headless OrcaSlicer renders no plate images, so `Thumbnail_Middle` and
/// the package thumbnail relationship both name a missing
/// `Metadata/plate_1.png`.
#[test]
fn orca_two_plates_dangling_thumbnail_middle_is_no_thumbnail() {
    let outcome = inspect_fixture(&fixture_dir().join("orca-two-plates.3mf")).unwrap();
    assert_eq!(outcome.thumbnail, None);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
}
