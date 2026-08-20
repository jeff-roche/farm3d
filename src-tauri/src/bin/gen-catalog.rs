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
