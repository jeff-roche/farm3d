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
//! `--debug`) are never produced. The invocation manifest is written by the
//! operation layer, not here.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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
}
