//! D8: one slice operation's work directory and OrcaSlicer's argument
//! vector.
//!
//! ```text
//! <content_root>/slicing-work/<sop-id>/
//!   input/plate.3mf
//!   input/machine.json
//!   input/process.json
//!   input/filament.json
//!   datadir/          empty, per operation; never the user's data dir
//!   out/
//!   progress.fifo     Linux only
//! ```
//!
//! Every argument is an absolute path inside the work directory, and the
//! flags D8 forbids (`--allow-newer-file`, `--mstpp`, `--no-check`,
//! `--debug`) are never produced.
//!
//! [`InvocationManifest`] is D8's record of one invocation, stored as the
//! Slice Revision's `manifest` blob (D13). It names no absolute path: the
//! work directory is `<work>` and the engine is `<engine>`.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::printed_bounds::BoundsCheck;
use super::runtime::{FileHashCache, OrcaVersion, PresetSourceOrigin};
use super::{RuntimeChannel, SliceControls, SliceRevisionTarget, SliceRuntimeInfo};

/// The folder under the content root that holds the work directories.
pub const WORK_ROOT_DIR: &str = "slicing-work";

/// The G-code OrcaSlicer writes for the one plate of a farm3d plate 3MF.
pub const PLATE_GCODE: &str = "plate_1.gcode";

/// The file OrcaSlicer writes its return code and error string to (spike
/// Gate F). With `--outputdir` it lands in the output directory.
pub const RESULT_JSON: &str = "result.json";

/// One operation's work directory (D8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkDir {
    root: PathBuf,
}

impl WorkDir {
    /// `<content_root>/slicing-work/<operation_id>/`. Nothing is created.
    pub fn for_operation(content_root: &Path, operation_id: &str) -> Self {
        Self::at(content_root.join(WORK_ROOT_DIR).join(operation_id))
    }

    /// A work directory at `root`, which must be absolute.
    pub fn at(root: PathBuf) -> Self {
        debug_assert!(root.is_absolute(), "a work directory is absolute");
        Self { root }
    }

    /// Creates `input/`, `datadir/`, and `out/`.
    pub fn create(&self) -> io::Result<()> {
        for dir in [self.input_dir(), self.datadir(), self.out_dir()] {
            fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn input_dir(&self) -> PathBuf {
        self.root.join("input")
    }

    pub fn plate_3mf(&self) -> PathBuf {
        self.input_dir().join("plate.3mf")
    }

    pub fn machine_json(&self) -> PathBuf {
        self.input_dir().join("machine.json")
    }

    pub fn process_json(&self) -> PathBuf {
        self.input_dir().join("process.json")
    }

    pub fn filament_json(&self) -> PathBuf {
        self.input_dir().join("filament.json")
    }

    pub fn datadir(&self) -> PathBuf {
        self.root.join("datadir")
    }

    pub fn out_dir(&self) -> PathBuf {
        self.root.join("out")
    }

    pub fn gcode(&self) -> PathBuf {
        self.out_dir().join(PLATE_GCODE)
    }

    pub fn result_json(&self) -> PathBuf {
        self.out_dir().join(RESULT_JSON)
    }

    /// The progress FIFO. Only Linux creates it (D9, D24).
    pub fn progress_fifo(&self) -> PathBuf {
        self.root.join("progress.fifo")
    }

    /// D8's arguments, after the engine itself. `--pipe` is included when
    /// `progress` is true, which only Linux asks for.
    pub fn arguments(&self, progress: bool) -> Vec<OsString> {
        let mut settings = self.machine_json().into_os_string();
        settings.push(";");
        settings.push(self.process_json());
        let mut args: Vec<OsString> = vec![
            "--datadir".into(),
            self.datadir().into(),
            "--outputdir".into(),
            self.out_dir().into(),
            "--load-settings".into(),
            settings,
            "--load-filaments".into(),
            self.filament_json().into(),
            "--arrange".into(),
            "0".into(),
            "--orient".into(),
            "0".into(),
            "--slice".into(),
            "1".into(),
        ];
        if progress {
            args.push("--pipe".into());
            args.push(self.progress_fifo().into());
        }
        args.push(self.plate_3mf().into());
        args
    }
}

/// The work directory's placeholder in the manifest's argument vector.
pub const WORK_PLACEHOLDER: &str = "<work>";

/// The engine's placeholder as the manifest's first argument.
pub const ENGINE_PLACEHOLDER: &str = "<engine>";

/// The manifest format written by this build.
pub const MANIFEST_VERSION: u32 = 1;

/// The engine an operation ran: its version, channel, and the SHA-256 of
/// the engine file (D8).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EngineIdentity {
    pub version: String,
    pub channel: RuntimeChannel,
    pub sha256: String,
}

impl EngineIdentity {
    /// The engine file at `path` (an AppImage is hashed whole), hashed
    /// through `hashes`, so an unchanged engine is hashed once per size and
    /// modification time rather than once per operation.
    pub fn of(path: &Path, version: &OrcaVersion, hashes: &FileHashCache) -> io::Result<Self> {
        Ok(Self {
            version: version.to_string(),
            channel: version.channel(),
            sha256: hashes.sha256(path)?,
        })
    }
}

/// The preset source an operation's presets came from (D8).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PresetSourceIdentity {
    pub version: String,
    pub channel: RuntimeChannel,
    pub origin: PresetSourceOrigin,
}

impl PresetSourceIdentity {
    pub fn new(version: &OrcaVersion, origin: PresetSourceOrigin) -> Self {
        Self {
            version: version.to_string(),
            channel: version.channel(),
            origin,
        }
    }
}

/// `slice_revisions.runtime_json` for a revision sliced with `engine` and
/// `preset_source`.
pub fn runtime_info(
    engine: &EngineIdentity,
    preset_source: &PresetSourceIdentity,
) -> SliceRuntimeInfo {
    SliceRuntimeInfo {
        engine_version: engine.version.clone(),
        engine_channel: engine.channel,
        preset_source_version: preset_source.version.clone(),
        preset_source_channel: preset_source.channel,
    }
}

/// One flat preset the engine loaded: its name and the SHA-256 of the
/// file farm3d wrote.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ManifestPreset {
    pub name: String,
    pub sha256: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ManifestPresets {
    pub machine: ManifestPreset,
    pub process: ManifestPreset,
    pub filament: ManifestPreset,
}

/// The SHA-256 of each input file, as staged for publishing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InputHashes {
    pub plate_3mf: String,
    pub machine: String,
    pub process: String,
    pub filament: String,
}

/// D8's invocation manifest.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InvocationManifest {
    pub manifest_version: u32,
    pub engine: EngineIdentity,
    pub preset_source: PresetSourceIdentity,
    /// The whole argument vector: `<engine>`, then D8's arguments with the
    /// work directory written as `<work>`.
    pub arguments: Vec<String>,
    pub presets: ManifestPresets,
    #[serde(rename = "plate3mfSha256")]
    pub plate_3mf_sha256: String,
    pub controls: SliceControls,
    /// The machine-preset keys farm3d wrote from the target's Printer
    /// Profile overrides (D4). Only the keys: the values are in the
    /// `machinePreset` blob, whose hash is `presets.machine.sha256`.
    pub profile_overrides: Vec<String>,
    /// Whether D11 check 5 (the printed bounds) ran, or why it was
    /// skipped.
    pub bounds_check: BoundsCheck,
}

impl InvocationManifest {
    /// The manifest for a run in `work`, which passed `--pipe` when
    /// `progress_piped`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        work: &WorkDir,
        progress_piped: bool,
        engine: &EngineIdentity,
        preset_source: &PresetSourceIdentity,
        target: &SliceRevisionTarget,
        profile_overrides: &[String],
        hashes: &InputHashes,
        bounds_check: &BoundsCheck,
    ) -> Self {
        let root = work.root().to_string_lossy().into_owned();
        let arguments = std::iter::once(ENGINE_PLACEHOLDER.to_string())
            .chain(
                work.arguments(progress_piped)
                    .iter()
                    .map(|arg| arg.to_string_lossy().replace(&root, WORK_PLACEHOLDER)),
            )
            .collect();
        let preset = |name: &str, sha256: &str| ManifestPreset {
            name: name.to_string(),
            sha256: sha256.to_string(),
        };
        Self {
            manifest_version: MANIFEST_VERSION,
            engine: engine.clone(),
            preset_source: preset_source.clone(),
            arguments,
            presets: ManifestPresets {
                machine: preset(&target.machine_preset, &hashes.machine),
                process: preset(&target.process_preset, &hashes.process),
                filament: preset(&target.filament_preset, &hashes.filament),
            },
            plate_3mf_sha256: hashes.plate_3mf.clone(),
            controls: target.controls.clone(),
            profile_overrides: profile_overrides.to_vec(),
            bounds_check: bounds_check.clone(),
        }
    }

    /// The bytes stored as the `manifest` blob.
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("the manifest always serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arguments_match_d8() {
        let work = WorkDir::for_operation(Path::new("/c"), "sop-1");
        let args: Vec<String> = work
            .arguments(true)
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect();
        assert_eq!(
            args,
            [
                "--datadir",
                "/c/slicing-work/sop-1/datadir",
                "--outputdir",
                "/c/slicing-work/sop-1/out",
                "--load-settings",
                "/c/slicing-work/sop-1/input/machine.json;/c/slicing-work/sop-1/input/process.json",
                "--load-filaments",
                "/c/slicing-work/sop-1/input/filament.json",
                "--arrange",
                "0",
                "--orient",
                "0",
                "--slice",
                "1",
                "--pipe",
                "/c/slicing-work/sop-1/progress.fifo",
                "/c/slicing-work/sop-1/input/plate.3mf",
            ]
        );
        let without_pipe = work.arguments(false);
        assert!(!without_pipe.iter().any(|arg| arg == "--pipe"));
        assert_eq!(without_pipe.len(), args.len() - 2);
        for forbidden in ["--allow-newer-file", "--mstpp", "--no-check", "--debug"] {
            assert!(!args.iter().any(|arg| arg == forbidden));
        }
    }

    #[test]
    fn the_manifest_records_d8_without_absolute_paths() {
        use crate::slicing::facts::tests::a_profile_snapshot;
        use crate::slicing::SliceTarget;

        let work = WorkDir::for_operation(Path::new("/c"), "sop-1");
        let version = OrcaVersion {
            major: 2,
            minor: 4,
            patch: 2,
            prerelease: None,
        };
        let engine = EngineIdentity {
            version: version.to_string(),
            channel: version.channel(),
            sha256: "e".repeat(64),
        };
        let presets = PresetSourceIdentity::new(&version, PresetSourceOrigin::Engine);
        let target = SliceRevisionTarget {
            target: SliceTarget::Printer {
                printer_id: "prn-a".to_string(),
            },
            profile: a_profile_snapshot(),
            machine_preset: "M".to_string(),
            process_preset: "P".to_string(),
            filament_preset: "F".to_string(),
            controls: SliceControls {
                wall_loops: Some(3),
                ..SliceControls::default()
            },
        };
        let hashes = InputHashes {
            plate_3mf: "a".repeat(64),
            machine: "b".repeat(64),
            process: "c".repeat(64),
            filament: "d".repeat(64),
        };
        let manifest = InvocationManifest::new(
            &work,
            true,
            &engine,
            &presets,
            &target,
            &["bed_exclude_area".to_string()],
            &hashes,
            &BoundsCheck::Skipped {
                reason: "The G-code uses inch units.".to_string(),
            },
        );
        let json: serde_json::Value = serde_json::from_slice(&manifest.to_bytes()).unwrap();

        assert_eq!(json["manifestVersion"], 1);
        assert_eq!(json["engine"]["version"], "2.4.2");
        assert_eq!(json["engine"]["channel"], "release");
        assert_eq!(json["engine"]["sha256"], "e".repeat(64));
        assert_eq!(json["presetSource"]["origin"], "engine");
        assert_eq!(json["presets"]["machine"]["name"], "M");
        assert_eq!(json["presets"]["filament"]["sha256"], "d".repeat(64));
        assert_eq!(json["plate3mfSha256"], "a".repeat(64));
        assert_eq!(json["controls"]["wallLoops"], 3);
        assert_eq!(json["profileOverrides"][0], "bed_exclude_area");
        assert_eq!(
            json["boundsCheck"],
            serde_json::json!({ "status": "skipped", "reason": "The G-code uses inch units." })
        );
        let arguments: Vec<&str> = json["arguments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap())
            .collect();
        assert_eq!(arguments[0], "<engine>");
        assert_eq!(arguments[2], "<work>/datadir");
        assert_eq!(
            arguments[6],
            "<work>/input/machine.json;<work>/input/process.json"
        );
        assert_eq!(arguments.last(), Some(&"<work>/input/plate.3mf"));
        assert!(arguments.contains(&"--pipe"));
        assert!(arguments.iter().all(|arg| !arg.contains("/c/")));
        assert_eq!(
            InvocationManifest::new(
                &work,
                false,
                &engine,
                &presets,
                &target,
                &[],
                &hashes,
                &BoundsCheck::Checked {
                    scope: crate::slicing::printed_bounds::BoundsScope::PrintBody,
                },
            )
            .arguments
            .len(),
            arguments.len() - 2
        );
    }
}
