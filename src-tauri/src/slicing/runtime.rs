//! D2: the slicer runtime — the OrcaSlicer engine farm3d runs, plus the
//! preset source it takes presets from.
//!
//! - **Engine discovery** tries the configured engine, then `orca-slicer`
//!   on `PATH`, then (Linux) the well-known AppImage folders, and stops at
//!   the first candidate that probes as a supported version.
//! - **The version probe** runs `<engine> --help` in a fresh temporary
//!   working directory (even `--help` writes `result.json` into it, Gate
//!   A), with the [`orca_environment`] allowlist and a 10 s limit, and reads
//!   only the first stdout line.
//! - **The preset source** is a directory holding `profiles/<Vendor>.json`,
//!   found next to an executable, inside a chosen directory, or extracted
//!   from an AppImage into `<cache_dir>/orca-profiles/<sha256>/`.
//! - **Paths never come from the frontend.** They arrive through the native
//!   picker behind [`SlicerRuntimeFileIo`], are validated, and are then
//!   saved in `slicer_runtime_config`.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Runtime};
use tauri_plugin_dialog::DialogExt;
use ts_rs::TS;

use crate::contracts::command::CommandError;
use crate::persistence::{RepositoryError, Storage};

use super::presets::vendor_bundles;
use super::process_group::{spawn_group, wait_or_stop, TERM_GRACE};
use super::repository::{load_runtime_config, save_runtime_config, SlicerRuntimeConfig};
use super::RuntimeChannel;

/// D2: the version probe's time limit.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// D2: how long a pre-slice probe result is reused ([`ProbeCache`]).
pub const PROBE_CACHE_TTL: Duration = Duration::from_secs(60);

/// The AppImage profile extraction's time limit. It took 0.35 s in the
/// spike; the limit only guards against a hung runtime.
pub const EXTRACT_TIMEOUT: Duration = Duration::from_secs(120);

/// The folder under the app cache directory that holds extracted AppImage
/// profiles, one `<appimage-sha256>/` per AppImage.
pub const PROFILE_CACHE_DIR: &str = "orca-profiles";

/// The executable name discovery looks for on `PATH`.
pub const PATH_EXECUTABLE: &str = "orca-slicer";

/// D8: the environment variables passed through to OrcaSlicer when set.
pub const ORCA_ENV_ALLOWLIST: [&str; 6] = [
    "HOME",
    "USER",
    "LANG",
    "LC_ALL",
    "TMPDIR",
    "XDG_RUNTIME_DIR",
];

/// D8: OrcaSlicer's `PATH` on Unix, whatever farm3d's own `PATH` is.
pub const ORCA_UNIX_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Stdout is read up to this many bytes for the version line.
const MAX_VERSION_LINE: u64 = 1024;

/// Stderr is kept up to this many bytes (only to spot the FUSE error).
const MAX_STDERR: u64 = 16 * 1024;

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// Why this engine was chosen.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/EngineSource.ts")]
pub enum EngineSource {
    /// The user chose it (**Choose engine…**).
    Configured,
    /// `orca-slicer` on `PATH`.
    Path,
    /// An OrcaSlicer AppImage in a well-known folder.
    WellKnown,
}

/// D2: the engine part of the runtime status. `executableName` is a
/// basename; `path` is the full path, shown only in the Settings section.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/EngineState.ts"
)]
pub enum EngineState {
    Available {
        version: String,
        channel: RuntimeChannel,
        source: EngineSource,
        executable_name: String,
        path: String,
        /// The AppImage runs through `--appimage-extract-and-run` because
        /// FUSE is unavailable; it leaves a `/tmp/appimage_extracted_*`
        /// directory.
        extract_and_run: bool,
    },
    NotFound,
    UnsupportedVersion {
        version: String,
        executable_name: String,
    },
    ProbeFailed {
        reason: String,
        executable_name: String,
    },
}

/// Where the presets come from.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PresetSourceOrigin.ts")]
pub enum PresetSourceOrigin {
    /// The engine's own presets.
    Engine,
    /// The user chose a preset source (**Choose preset source…**).
    Configured,
}

/// D2: the preset-source part of the runtime status. `path` is the engine
/// or the chosen preset source, shown only in the Settings section.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/PresetSourceState.ts"
)]
pub enum PresetSourceState {
    Available {
        version: String,
        channel: RuntimeChannel,
        origin: PresetSourceOrigin,
        vendor_count: u32,
        path: String,
    },
    /// No preset source is configured and no engine is available.
    NotConfigured,
    /// The presets are only in a format farm3d can't read (`.opc`).
    PresetsUnreadable,
    Unavailable {
        reason: String,
    },
}

/// D2: what `get_slicer_runtime` returns. `revision` is the
/// `slicer_runtime_config` revision the pickers and reset expect.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SlicerRuntimeStatus.ts")]
pub struct SlicerRuntimeStatus {
    pub engine: EngineState,
    pub preset_source: PresetSourceState,
    pub can_slice: bool,
    pub versions_differ: bool,
    #[ts(type = "number")]
    pub revision: i64,
}

// ---------------------------------------------------------------------------
// Versions
// ---------------------------------------------------------------------------

/// An OrcaSlicer version as the probe line states it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrcaVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    /// The suffix after `-` (for example `dev` or `rc2`).
    pub prerelease: Option<String>,
}

impl OrcaVersion {
    pub fn channel(&self) -> RuntimeChannel {
        if self.prerelease.is_some() {
            RuntimeChannel::Prerelease
        } else {
            RuntimeChannel::Release
        }
    }

    /// Only 2.x is accepted (user decision 2).
    pub fn is_supported(&self) -> bool {
        self.major == 2
    }

    /// Discovery's order: newest version first, and a release beats a
    /// prerelease of the same version.
    fn rank(&self) -> (u32, u32, u32, bool) {
        (
            self.major,
            self.minor,
            self.patch,
            self.prerelease.is_none(),
        )
    }
}

impl std::fmt::Display for OrcaVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(prerelease) = &self.prerelease {
            write!(f, "-{prerelease}")?;
        }
        Ok(())
    }
}

fn parse_number(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// D2: parses the probe's first stdout line, which must match
/// `^OrcaSlicer-(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?:$`.
pub fn parse_version_line(line: &str) -> Option<OrcaVersion> {
    let rest = line.strip_prefix("OrcaSlicer-")?.strip_suffix(':')?;
    let (numbers, prerelease) = match rest.split_once('-') {
        Some((numbers, suffix)) => {
            let valid = !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-');
            if !valid {
                return None;
            }
            (numbers, Some(suffix.to_string()))
        }
        None => (rest, None),
    };
    let mut parts = numbers.split('.');
    let version = OrcaVersion {
        major: parse_number(parts.next()?)?,
        minor: parse_number(parts.next()?)?,
        patch: parse_number(parts.next()?)?,
        prerelease,
    };
    parts.next().is_none().then_some(version)
}

// ---------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------

/// D8: the explicit environment every OrcaSlicer process gets (the probe,
/// the AppImage extraction, and slicing). Nothing else is inherited, so
/// farm3d's own AppImage variables never leak (Gate J).
pub fn orca_environment() -> Vec<(OsString, OsString)> {
    orca_environment_from(|name| std::env::var_os(name))
}

/// [`orca_environment`] over any variable lookup.
pub fn orca_environment_from(
    lookup: impl Fn(&str) -> Option<OsString>,
) -> Vec<(OsString, OsString)> {
    let mut environment: Vec<(OsString, OsString)> = ORCA_ENV_ALLOWLIST
        .iter()
        .filter_map(|name| lookup(name).map(|value| (OsString::from(name), value)))
        .collect();
    if cfg!(unix) {
        environment.push((OsString::from("PATH"), OsString::from(ORCA_UNIX_PATH)));
    } else {
        // No support claim off Linux (D24); Windows needs these to start a
        // process at all.
        for name in ["PATH", "SystemRoot"] {
            if let Some(value) = lookup(name) {
                environment.push((OsString::from(name), value));
            }
        }
    }
    environment
}

// ---------------------------------------------------------------------------
// Running OrcaSlicer briefly
// ---------------------------------------------------------------------------

/// A fresh, empty directory under the system temp dir, removed on drop.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(parent: &Path, prefix: &str) -> io::Result<Self> {
        let path = parent.join(format!("{prefix}{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn basename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn orca_command(executable: &Path, args: &[&str], working_dir: &Path) -> Command {
    let mut command = Command::new(executable);
    command
        .args(args)
        .current_dir(working_dir)
        .env_clear()
        .envs(orca_environment())
        .stdin(Stdio::null());
    command
}

/// What one `--help` run produced.
struct RawProbe {
    first_line: Option<String>,
    stderr: String,
    failure: Option<String>,
}

fn run_probe(executable: &Path, args: &[&str], timeout: Duration) -> RawProbe {
    let failed = |reason: String| RawProbe {
        first_line: None,
        stderr: String::new(),
        failure: Some(reason),
    };
    let scratch = match ScratchDir::new(&std::env::temp_dir(), "farm3d-orca-probe-") {
        Ok(scratch) => scratch,
        Err(_) => return failed("farm3d could not create a temporary folder.".to_string()),
    };
    let mut command = orca_command(executable, args, &scratch.0);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match spawn_group(&mut command) {
        Ok(child) => child,
        Err(error) => {
            return failed(format!(
                "{} could not be started ({error}).",
                basename(executable)
            ))
        }
    };
    let deadline = Instant::now() + timeout;

    let stdout = child.stdout.take().expect("stdout is piped");
    let (line_sender, line_receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();
        let _ = (&mut reader)
            .take(MAX_VERSION_LINE)
            .read_until(b'\n', &mut line);
        let _ = line_sender.send(line);
        // Keep draining so a long `--help` never blocks on a full pipe.
        let _ = io::copy(&mut reader, &mut io::sink());
    });
    let stderr = child.stderr.take().expect("stderr is piped");
    let (stderr_sender, stderr_receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut kept = Vec::new();
        let _ = (&mut reader).take(MAX_STDERR).read_to_end(&mut kept);
        let _ = io::copy(&mut reader, &mut io::sink());
        let _ = stderr_sender.send(kept);
    });

    let line = line_receiver.recv_timeout(timeout).ok();
    // Stopping the whole group closes the pipes, so both reader threads
    // finish.
    wait_or_stop(&mut child, deadline, TERM_GRACE);
    let stderr = stderr_receiver
        .recv_timeout(Duration::from_millis(200))
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    let first_line = line.map(|bytes| {
        String::from_utf8_lossy(&bytes)
            .trim_end_matches(['\n', '\r'])
            .to_string()
    });
    // No line at all means the reader never finished: the process hung.
    let failure = first_line.is_none().then(|| {
        format!(
            "{} did not answer within {} s.",
            basename(executable),
            timeout.as_secs()
        )
    });
    RawProbe {
        first_line,
        stderr,
        failure,
    }
}

/// The result of one version probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    Supported(OrcaVersion),
    /// A parseable version whose major version isn't 2.
    UnsupportedVersion(OrcaVersion),
    /// The probe failed: it didn't start, timed out, or printed no version.
    Failed(String),
}

/// A probe's outcome, and whether it needed the AppImage FUSE fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineProbe {
    pub outcome: ProbeOutcome,
    pub extract_and_run: bool,
}

fn interpret(executable: &Path, raw: &RawProbe) -> ProbeOutcome {
    if let Some(failure) = &raw.failure {
        return ProbeOutcome::Failed(failure.clone());
    }
    match raw.first_line.as_deref().and_then(parse_version_line) {
        Some(version) if version.is_supported() => ProbeOutcome::Supported(version),
        Some(version) => ProbeOutcome::UnsupportedVersion(version),
        None => ProbeOutcome::Failed(format!(
            "{} did not report an OrcaSlicer version.",
            basename(executable)
        )),
    }
}

/// D2: the version probe. Runs `<engine> --help`; when an AppImage fails
/// with the AppImage runtime's FUSE error, retries once through
/// `--appimage-extract-and-run`.
pub fn probe_engine(executable: &Path, timeout: Duration) -> EngineProbe {
    let raw = run_probe(executable, &["--help"], timeout);
    let outcome = interpret(executable, &raw);
    let fuse_error = raw.stderr.to_ascii_lowercase().contains("fuse");
    if matches!(outcome, ProbeOutcome::Failed(_)) && fuse_error && is_appimage(executable) {
        let retry = run_probe(
            executable,
            &["--appimage-extract-and-run", "--help"],
            timeout,
        );
        return EngineProbe {
            outcome: interpret(executable, &retry),
            extract_and_run: true,
        };
    }
    EngineProbe {
        outcome,
        extract_and_run: false,
    }
}

/// An AppImage (type 2) is an ELF file with `AI\x02` at offset 8.
pub fn is_appimage(path: &Path) -> bool {
    let mut header = [0_u8; 11];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok()
        && header.starts_with(b"\x7fELF")
        && &header[8..11] == b"AI\x02"
}

/// D2: re-probes before each slice, reusing a result for 60 s while the
/// engine file's size and modification time are unchanged.
#[derive(Default)]
pub struct ProbeCache {
    entries: Mutex<HashMap<PathBuf, CachedProbe>>,
}

struct CachedProbe {
    size: u64,
    modified: Option<SystemTime>,
    at: Instant,
    probe: EngineProbe,
}

impl ProbeCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn probe(&self, executable: &Path, timeout: Duration) -> EngineProbe {
        let metadata = fs::metadata(executable).ok();
        let size = metadata.as_ref().map_or(0, fs::Metadata::len);
        let modified = metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok());
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = entries.get(executable) {
            if metadata.is_some()
                && cached.size == size
                && cached.modified == modified
                && cached.at.elapsed() < PROBE_CACHE_TTL
            {
                return cached.probe.clone();
            }
        }
        let probe = probe_engine(executable, timeout);
        entries.insert(
            executable.to_path_buf(),
            CachedProbe {
                size,
                modified,
                at: Instant::now(),
                probe: probe.clone(),
            },
        );
        probe
    }
}

// ---------------------------------------------------------------------------
// Engine discovery
// ---------------------------------------------------------------------------

/// Where discovery looks: the home directory (for the well-known AppImage
/// folders), the `PATH` to search, and the probe's time limit.
#[derive(Clone, Debug)]
pub struct DiscoveryEnv {
    pub home: Option<PathBuf>,
    pub path_var: Option<OsString>,
    pub probe_timeout: Duration,
}

impl DiscoveryEnv {
    /// farm3d's own environment.
    pub fn from_process() -> Self {
        Self {
            home: std::env::var_os("HOME").map(PathBuf::from),
            path_var: std::env::var_os("PATH"),
            probe_timeout: PROBE_TIMEOUT,
        }
    }
}

/// An engine that probed as supported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedEngine {
    pub path: PathBuf,
    pub version: OrcaVersion,
    pub source: EngineSource,
    pub extract_and_run: bool,
}

impl ResolvedEngine {
    pub fn state(&self) -> EngineState {
        EngineState::Available {
            version: self.version.to_string(),
            channel: self.version.channel(),
            source: self.source,
            executable_name: basename(&self.path),
            path: self.path.to_string_lossy().into_owned(),
            extract_and_run: self.extract_and_run,
        }
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

/// The first `orca-slicer` on `path_var`, as a shell would find it.
fn find_on_path(path_var: Option<&OsString>) -> Option<PathBuf> {
    std::env::split_paths(path_var?)
        .map(|dir| dir.join(PATH_EXECUTABLE))
        .find(|candidate| is_executable_file(candidate))
}

/// D2 step 3 (Linux): every `*OrcaSlicer*.AppImage` in the well-known
/// folders, in folder order and then name order.
fn well_known_appimages(home: Option<&Path>) -> Vec<PathBuf> {
    let Some(home) = home else {
        return Vec::new();
    };
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    let mut found = Vec::new();
    for folder in ["Applications", ".local/bin", "Downloads"] {
        let Ok(entries) = fs::read_dir(home.join(folder)) else {
            continue;
        };
        let mut matches: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                let name = basename(path);
                name.contains("OrcaSlicer") && name.ends_with(".AppImage") && path.is_file()
            })
            .collect();
        matches.sort();
        found.extend(matches);
    }
    found
}

fn failure_state(path: &Path, outcome: &ProbeOutcome) -> Option<EngineState> {
    match outcome {
        ProbeOutcome::Supported(_) => None,
        ProbeOutcome::UnsupportedVersion(version) => Some(EngineState::UnsupportedVersion {
            version: version.to_string(),
            executable_name: basename(path),
        }),
        ProbeOutcome::Failed(reason) => Some(EngineState::ProbeFailed {
            reason: reason.clone(),
            executable_name: basename(path),
        }),
    }
}

/// D2: engine discovery. Stops at the first candidate that probes as
/// supported. Among the well-known AppImages, every match is probed and
/// the newest version wins. When nothing is supported, the state is the
/// first candidate's failure, or `notFound` when there was no candidate.
pub fn discover_engine(
    configured: Option<&Path>,
    env: &DiscoveryEnv,
) -> (EngineState, Option<ResolvedEngine>) {
    let mut first_failure: Option<EngineState> = None;
    let try_one = |path: &Path, source: EngineSource, first_failure: &mut Option<EngineState>| {
        let probe = probe_engine(path, env.probe_timeout);
        match probe.outcome {
            ProbeOutcome::Supported(version) => Some(ResolvedEngine {
                path: path.to_path_buf(),
                version,
                source,
                extract_and_run: probe.extract_and_run,
            }),
            outcome => {
                if first_failure.is_none() {
                    *first_failure = failure_state(path, &outcome);
                }
                None
            }
        }
    };

    let mut ordered: Vec<(PathBuf, EngineSource)> = Vec::new();
    if let Some(path) = configured {
        ordered.push((path.to_path_buf(), EngineSource::Configured));
    }
    if let Some(path) = find_on_path(env.path_var.as_ref()) {
        ordered.push((path, EngineSource::Path));
    }
    for (path, source) in ordered {
        if let Some(engine) = try_one(&path, source, &mut first_failure) {
            return (engine.state(), Some(engine));
        }
    }

    let best = well_known_appimages(env.home.as_deref())
        .into_iter()
        .filter_map(|path| try_one(&path, EngineSource::WellKnown, &mut first_failure))
        .fold(None::<ResolvedEngine>, |best, engine| match best {
            Some(best) if best.version.rank() >= engine.version.rank() => Some(best),
            _ => Some(engine),
        });
    match best {
        Some(engine) => (engine.state(), Some(engine)),
        None => (first_failure.unwrap_or(EngineState::NotFound), None),
    }
}

// ---------------------------------------------------------------------------
// Preset source
// ---------------------------------------------------------------------------

/// A preset source whose `profiles/` holds readable JSON bundles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPresetSource {
    /// The `profiles` directory the preset index reads.
    pub profiles_dir: PathBuf,
    pub version: OrcaVersion,
    pub origin: PresetSourceOrigin,
    pub vendor_count: usize,
    /// The engine or the chosen preset source.
    pub path: PathBuf,
    /// The extraction cache entry's name, when the source is an AppImage.
    pub cache_hash: Option<String>,
}

impl ResolvedPresetSource {
    pub fn state(&self) -> PresetSourceState {
        PresetSourceState::Available {
            version: self.version.to_string(),
            channel: self.version.channel(),
            origin: self.origin,
            vendor_count: u32::try_from(self.vendor_count).unwrap_or(u32::MAX),
            path: self.path.to_string_lossy().into_owned(),
        }
    }
}

enum ProfilesLookup {
    Found {
        profiles_dir: PathBuf,
        vendor_count: usize,
    },
    /// Only `.opc` preset caches (a cache-only build).
    Unreadable,
    Missing,
}

fn count_by_extension(dir: &Path, extension: &str) -> usize {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    let path = entry.path();
                    path.is_file() && path.extension().is_some_and(|ext| ext == extension)
                })
                .count()
        })
        .unwrap_or(0)
}

/// The first `resources` candidate whose `profiles/` has `<Vendor>.json`
/// bundles.
fn find_profiles(resources_candidates: &[PathBuf]) -> ProfilesLookup {
    let mut unreadable = false;
    for resources in resources_candidates {
        let profiles_dir = resources.join("profiles");
        if !profiles_dir.is_dir() {
            continue;
        }
        let vendor_count = vendor_bundles(&profiles_dir).len();
        if vendor_count > 0 {
            return ProfilesLookup::Found {
                profiles_dir,
                vendor_count,
            };
        }
        unreadable |= count_by_extension(&profiles_dir, "opc") > 0;
    }
    if unreadable {
        ProfilesLookup::Unreadable
    } else {
        ProfilesLookup::Missing
    }
}

/// D2: an installed executable's resources, after resolving symlinks.
fn executable_resources(executable: &Path) -> Vec<PathBuf> {
    let resolved = fs::canonicalize(executable).unwrap_or_else(|_| executable.to_path_buf());
    let Some(dir) = resolved.parent() else {
        return Vec::new();
    };
    vec![
        dir.join("../resources"),
        dir.join("../share/OrcaSlicer/resources"),
        dir.join("../../resources"),
    ]
}

/// D2: a chosen directory's resources.
fn directory_resources(dir: &Path) -> Vec<PathBuf> {
    vec![
        dir.to_path_buf(),
        dir.join("resources"),
        dir.join("share/OrcaSlicer/resources"),
    ]
}

/// The OrcaSlicer executable that states a chosen directory's version: an
/// extracted AppImage's `AppRun`, or an install's `bin/orca-slicer`.
fn directory_executable(dir: &Path) -> Option<PathBuf> {
    ["AppRun", "bin/orca-slicer", "orca-slicer"]
        .iter()
        .map(|relative| dir.join(relative))
        .find(|candidate| is_executable_file(candidate))
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// The result of extracting an AppImage's profiles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtractOutcome {
    /// The cache entry: it holds `resources/profiles/`, JSON files only.
    Extracted {
        root: PathBuf,
        hash: String,
    },
    /// The AppImage holds only `.opc` preset caches.
    Unreadable,
    /// The AppImage holds no presets at all.
    NoProfiles,
    Failed(String),
}

/// Deletes every entry under `dir` that isn't a `.json` regular file or a
/// directory. Symlinks are deleted too.
fn keep_only_json(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            keep_only_json(&path)?;
        } else if !(file_type.is_file() && path.extension().is_some_and(|ext| ext == "json")) {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// D2: extracts an AppImage's presets with
/// `<appimage> --appimage-extract 'resources/profiles/*'` into a temporary
/// directory under `<cache_dir>/orca-profiles/`, deletes every non-`.json`
/// file, and renames the result to `<cache_dir>/orca-profiles/<sha256>/`.
/// An existing entry for the same hash is reused without running anything.
/// A cache-only build (`.opc` presets) is not cached.
pub fn extract_appimage_profiles(
    appimage: &Path,
    cache_dir: &Path,
    timeout: Duration,
) -> ExtractOutcome {
    let name = basename(appimage);
    let hash = match sha256_file(appimage) {
        Ok(hash) => hash,
        Err(_) => return ExtractOutcome::Failed(format!("{name} could not be read.")),
    };
    let cache_root = cache_dir.join(PROFILE_CACHE_DIR);
    let entry = cache_root.join(&hash);
    if entry.join("resources/profiles").is_dir() {
        return ExtractOutcome::Extracted { root: entry, hash };
    }
    let scratch = match ScratchDir::new(&cache_root, ".extract-") {
        Ok(scratch) => scratch,
        Err(_) => {
            return ExtractOutcome::Failed("farm3d could not create its preset cache.".to_string())
        }
    };
    let mut command = orca_command(
        appimage,
        &["--appimage-extract", "resources/profiles/*"],
        &scratch.0,
    );
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = match spawn_group(&mut command) {
        Ok(child) => child,
        Err(error) => {
            return ExtractOutcome::Failed(format!("{name} could not be started ({error})."))
        }
    };
    if !wait_or_stop(&mut child, Instant::now() + timeout, TERM_GRACE) {
        return ExtractOutcome::Failed(format!(
            "Extracting presets from {name} took longer than {} s.",
            timeout.as_secs()
        ));
    }
    let extracted = scratch.0.join("squashfs-root");
    let profiles = extracted.join("resources/profiles");
    if vendor_bundles(&profiles).is_empty() {
        return if count_by_extension(&profiles, "opc") > 0 {
            ExtractOutcome::Unreadable
        } else {
            ExtractOutcome::NoProfiles
        };
    }
    if keep_only_json(&extracted).is_err() {
        return ExtractOutcome::Failed(
            "farm3d could not prepare the extracted presets.".to_string(),
        );
    }
    if fs::rename(&extracted, &entry).is_err() && !entry.join("resources/profiles").is_dir() {
        return ExtractOutcome::Failed("farm3d could not store the extracted presets.".to_string());
    }
    ExtractOutcome::Extracted { root: entry, hash }
}

/// D2: deletes every `<cache_dir>/orca-profiles/` entry whose name is not in
/// `keep_hashes`, including interrupted extractions. Returns how many were
/// deleted. Run at startup.
pub fn sweep_stale_profile_caches(cache_dir: &Path, keep_hashes: &[&str]) -> io::Result<usize> {
    let cache_root = cache_dir.join(PROFILE_CACHE_DIR);
    let entries = match fs::read_dir(&cache_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        if keep_hashes.iter().any(|keep| name == *keep) {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
        removed += 1;
    }
    Ok(removed)
}

/// The version a configured preset source states: the probe of `executable`
/// must be a supported version.
fn source_version(executable: &Path, env: &DiscoveryEnv) -> Result<OrcaVersion, PresetSourceState> {
    match probe_engine(executable, env.probe_timeout).outcome {
        ProbeOutcome::Supported(version) => Ok(version),
        ProbeOutcome::UnsupportedVersion(version) => Err(PresetSourceState::Unavailable {
            reason: format!("OrcaSlicer {version} presets are not supported."),
        }),
        ProbeOutcome::Failed(reason) => Err(PresetSourceState::Unavailable { reason }),
    }
}

/// D2: resolves a preset source. `path` is the engine (origin `engine`,
/// whose `engine_version` is already known) or the configured preset
/// source: a directory, an AppImage, or an installed executable.
pub fn resolve_preset_source(
    path: &Path,
    origin: PresetSourceOrigin,
    engine_version: Option<&OrcaVersion>,
    env: &DiscoveryEnv,
    cache_dir: &Path,
) -> (PresetSourceState, Option<ResolvedPresetSource>) {
    let unavailable = |reason: String| (PresetSourceState::Unavailable { reason }, None);
    let name = basename(path);
    let (lookup, cache_hash, version_executable) = if path.is_dir() {
        (
            find_profiles(&directory_resources(path)),
            None,
            directory_executable(path),
        )
    } else if path.is_file() && is_appimage(path) {
        match extract_appimage_profiles(path, cache_dir, EXTRACT_TIMEOUT) {
            ExtractOutcome::Extracted { root, hash } => (
                find_profiles(&[root.join("resources")]),
                Some(hash),
                Some(path.to_path_buf()),
            ),
            ExtractOutcome::Unreadable => (ProfilesLookup::Unreadable, None, None),
            ExtractOutcome::NoProfiles => (ProfilesLookup::Missing, None, None),
            ExtractOutcome::Failed(reason) => return unavailable(reason),
        }
    } else if path.is_file() {
        (
            find_profiles(&executable_resources(path)),
            None,
            Some(path.to_path_buf()),
        )
    } else {
        return unavailable(format!("{name} no longer exists."));
    };

    let (profiles_dir, vendor_count) = match lookup {
        ProfilesLookup::Found {
            profiles_dir,
            vendor_count,
        } => (profiles_dir, vendor_count),
        ProfilesLookup::Unreadable => return (PresetSourceState::PresetsUnreadable, None),
        ProfilesLookup::Missing => {
            return unavailable(format!("No OrcaSlicer presets were found in {name}."))
        }
    };
    let version = match (engine_version, version_executable) {
        (Some(version), _) => version.clone(),
        (None, Some(executable)) => match source_version(&executable, env) {
            Ok(version) => version,
            Err(state) => return (state, None),
        },
        (None, None) => {
            return unavailable(format!(
                "{name} has no OrcaSlicer program to tell which version its presets are."
            ))
        }
    };
    let source = ResolvedPresetSource {
        profiles_dir,
        version,
        origin,
        vendor_count,
        path: path.to_path_buf(),
        cache_hash,
    };
    (source.state(), Some(source))
}

// ---------------------------------------------------------------------------
// The whole runtime
// ---------------------------------------------------------------------------

/// The runtime as resolved from the configuration: the wire status, plus
/// the resolved parts later work needs (paths never cross to the UI from
/// here except through the Settings section's status).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlicerRuntime {
    pub status: SlicerRuntimeStatus,
    pub engine: Option<ResolvedEngine>,
    pub preset_source: Option<ResolvedPresetSource>,
}

/// D2: discovers the engine and resolves the preset source. The preset
/// source is the configured one, otherwise the engine's own presets, and
/// `notConfigured` when there is neither. Always probes afresh.
pub fn resolve_runtime(
    config: &SlicerRuntimeConfig,
    env: &DiscoveryEnv,
    cache_dir: &Path,
) -> SlicerRuntime {
    let (engine_state, engine) = discover_engine(config.engine_path.as_deref().map(Path::new), env);
    let (preset_state, preset_source) = match (&config.preset_source_path, &engine) {
        (Some(path), _) => resolve_preset_source(
            Path::new(path),
            PresetSourceOrigin::Configured,
            None,
            env,
            cache_dir,
        ),
        (None, Some(engine)) => resolve_preset_source(
            &engine.path,
            PresetSourceOrigin::Engine,
            Some(&engine.version),
            env,
            cache_dir,
        ),
        (None, None) => (PresetSourceState::NotConfigured, None),
    };
    let versions_differ = match (&engine, &preset_source) {
        (Some(engine), Some(source)) => engine.version != source.version,
        _ => false,
    };
    SlicerRuntime {
        status: SlicerRuntimeStatus {
            can_slice: engine.is_some() && preset_source.is_some(),
            versions_differ,
            engine: engine_state,
            preset_source: preset_state,
            revision: config.revision,
        },
        engine,
        preset_source,
    }
}

// ---------------------------------------------------------------------------
// Pickers and saving the configuration
// ---------------------------------------------------------------------------

/// What the preset-source picker chooses: a file (an executable or an
/// AppImage) or a folder. A native dialog picks one or the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresetSourcePickKind {
    File,
    Folder,
}

/// The native dialog boundary for D2's pickers. Production is
/// [`NativeSlicerRuntimeFileIo`]; tests use [`FixedSlicerRuntimeFileIo`].
pub trait SlicerRuntimeFileIo: Send + Sync {
    /// `Ok(None)` when the user cancels.
    fn pick_engine(&self) -> Result<Option<PathBuf>, CommandError>;
    /// `Ok(None)` when the user cancels.
    fn pick_preset_source(
        &self,
        kind: PresetSourcePickKind,
    ) -> Result<Option<PathBuf>, CommandError>;
}

/// `tauri-plugin-dialog`'s native dialogs.
pub struct NativeSlicerRuntimeFileIo<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> NativeSlicerRuntimeFileIo<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

fn local_path(
    picked: Option<tauri_plugin_dialog::FilePath>,
) -> Result<Option<PathBuf>, CommandError> {
    picked
        .map(|file| file.into_path())
        .transpose()
        .map_err(|_| CommandError::validation("Choose a file on this computer."))
}

impl<R: Runtime> SlicerRuntimeFileIo for NativeSlicerRuntimeFileIo<R> {
    fn pick_engine(&self) -> Result<Option<PathBuf>, CommandError> {
        local_path(
            self.app
                .dialog()
                .file()
                .set_title("Choose OrcaSlicer")
                .blocking_pick_file(),
        )
    }

    fn pick_preset_source(
        &self,
        kind: PresetSourcePickKind,
    ) -> Result<Option<PathBuf>, CommandError> {
        let dialog = self
            .app
            .dialog()
            .file()
            .set_title("Choose an OrcaSlicer preset source");
        local_path(match kind {
            PresetSourcePickKind::File => dialog.blocking_pick_file(),
            PresetSourcePickKind::Folder => dialog.blocking_pick_folder(),
        })
    }
}

/// A picker that returns fixed choices, for tests and for services built
/// without a window.
#[derive(Clone, Debug, Default)]
pub struct FixedSlicerRuntimeFileIo {
    pub engine: Option<PathBuf>,
    pub preset_source: Option<PathBuf>,
}

impl SlicerRuntimeFileIo for FixedSlicerRuntimeFileIo {
    fn pick_engine(&self) -> Result<Option<PathBuf>, CommandError> {
        Ok(self.engine.clone())
    }

    fn pick_preset_source(
        &self,
        _kind: PresetSourcePickKind,
    ) -> Result<Option<PathBuf>, CommandError> {
        Ok(self.preset_source.clone())
    }
}

/// A picker's result: cancelled, or the configuration it saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimePick {
    Cancelled,
    Saved(SlicerRuntimeConfig),
}

fn storage_error(error: crate::persistence::StorageError) -> CommandError {
    CommandError::from_repository(RepositoryError::Storage(error))
}

fn current_config(storage: &Storage) -> Result<SlicerRuntimeConfig, CommandError> {
    storage
        .read(|connection| Ok(load_runtime_config(connection)))
        .map_err(storage_error)?
        .map_err(storage_error)
}

/// Refuses a stale `expected_revision` before a dialog opens.
fn check_revision(storage: &Storage, expected_revision: i64) -> Result<(), CommandError> {
    let current = current_config(storage)?;
    if current.revision != expected_revision {
        return Err(CommandError::from_repository(RepositoryError::Conflict {
            entity_id: super::repository::RUNTIME_CONFIG_ENTITY_ID.to_string(),
            expected_revision,
            current_revision: current.revision,
        }));
    }
    Ok(())
}

fn path_text(path: &Path) -> Result<String, CommandError> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| CommandError::validation("farm3d can't store this file's path."))
}

/// Saves new paths; `None` in `engine`/`preset_source` keeps the current
/// value, `Some(None)` clears it.
fn save(
    storage: &Storage,
    expected_revision: i64,
    engine: Option<Option<&str>>,
    preset_source: Option<Option<&str>>,
) -> Result<SlicerRuntimeConfig, CommandError> {
    storage
        .write_repo(|tx| {
            let current = load_runtime_config(tx)?;
            save_runtime_config(
                tx,
                expected_revision,
                engine.unwrap_or(current.engine_path.as_deref()),
                preset_source.unwrap_or(current.preset_source_path.as_deref()),
            )
        })
        .map_err(CommandError::from_repository)
}

/// `pick_slicer_engine`: opens the picker, probes the choice, and saves it
/// only when it is a supported OrcaSlicer. A failed probe is
/// `SLICER_UNAVAILABLE`, and nothing is saved.
pub fn pick_slicer_engine(
    io: &dyn SlicerRuntimeFileIo,
    storage: &Storage,
    expected_revision: i64,
    env: &DiscoveryEnv,
) -> Result<RuntimePick, CommandError> {
    check_revision(storage, expected_revision)?;
    let Some(path) = io.pick_engine()? else {
        return Ok(RuntimePick::Cancelled);
    };
    let text = path_text(&path)?;
    match probe_engine(&path, env.probe_timeout).outcome {
        ProbeOutcome::Supported(_) => {}
        ProbeOutcome::UnsupportedVersion(version) => {
            return Err(CommandError::slicer_unavailable(&format!(
                "OrcaSlicer {version} is not supported. Choose an OrcaSlicer 2.x release or nightly."
            )))
        }
        ProbeOutcome::Failed(reason) => return Err(CommandError::slicer_unavailable(&reason)),
    }
    save(storage, expected_revision, Some(Some(&text)), None).map(RuntimePick::Saved)
}

/// `pick_preset_source`: opens the picker, resolves the choice, and saves
/// it only when its presets are readable. Anything else is
/// `PRESET_SOURCE_UNAVAILABLE`, and nothing is saved.
pub fn pick_preset_source(
    io: &dyn SlicerRuntimeFileIo,
    storage: &Storage,
    expected_revision: i64,
    kind: PresetSourcePickKind,
    env: &DiscoveryEnv,
    cache_dir: &Path,
) -> Result<RuntimePick, CommandError> {
    check_revision(storage, expected_revision)?;
    let Some(path) = io.pick_preset_source(kind)? else {
        return Ok(RuntimePick::Cancelled);
    };
    let text = path_text(&path)?;
    match resolve_preset_source(&path, PresetSourceOrigin::Configured, None, env, cache_dir).0 {
        PresetSourceState::Available { .. } => {}
        PresetSourceState::PresetsUnreadable => {
            return Err(CommandError::preset_source_unavailable(
                "This OrcaSlicer build stores its presets in a format farm3d can't read. Choose an OrcaSlicer 2.4 install or AppImage.",
            ))
        }
        PresetSourceState::Unavailable { reason } => {
            return Err(CommandError::preset_source_unavailable(&reason))
        }
        // Never produced for a chosen source.
        PresetSourceState::NotConfigured => return Err(CommandError::internal()),
    }
    save(storage, expected_revision, None, Some(Some(&text))).map(RuntimePick::Saved)
}

/// `reset_slicer_runtime`: clears the engine (back to automatic discovery),
/// the preset source (back to the engine's presets), or both.
pub fn reset_slicer_runtime(
    storage: &Storage,
    expected_revision: i64,
    engine: bool,
    preset_source: bool,
) -> Result<SlicerRuntimeConfig, CommandError> {
    save(
        storage,
        expected_revision,
        engine.then_some(None),
        preset_source.then_some(None),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_lines_parse_as_d2_specifies() {
        let release = parse_version_line("OrcaSlicer-2.4.2:").unwrap();
        assert_eq!(release.to_string(), "2.4.2");
        assert_eq!(release.channel(), RuntimeChannel::Release);
        assert!(release.is_supported());

        let dev = parse_version_line("OrcaSlicer-2.5.0-dev:").unwrap();
        assert_eq!(dev.to_string(), "2.5.0-dev");
        assert_eq!(dev.channel(), RuntimeChannel::Prerelease);
        assert!(dev.is_supported());

        let rc = parse_version_line("OrcaSlicer-2.3.0-rc2:").unwrap();
        assert_eq!(rc.prerelease.as_deref(), Some("rc2"));

        let three = parse_version_line("OrcaSlicer-3.0.0:").unwrap();
        assert!(!three.is_supported());

        for garbage in [
            "OrcaSlicer-2.4:",
            "OrcaSlicer-2.4.2",
            "OrcaSlicer-2.4.2: ",
            "orcaslicer-2.4.2:",
            "OrcaSlicer-2.4.2.1:",
            "OrcaSlicer-2.4.x:",
            "OrcaSlicer-2.4.2-:",
            "OrcaSlicer-2.4.2-dev build:",
            "Usage: orca-slicer",
            "",
        ] {
            assert_eq!(parse_version_line(garbage), None, "{garbage:?}");
        }
    }

    #[test]
    fn a_release_ranks_above_a_prerelease_of_the_same_version() {
        let release = parse_version_line("OrcaSlicer-2.5.0:").unwrap();
        let dev = parse_version_line("OrcaSlicer-2.5.0-dev:").unwrap();
        let older = parse_version_line("OrcaSlicer-2.4.2:").unwrap();
        assert!(release.rank() > dev.rank());
        assert!(dev.rank() > older.rank());
    }

    #[test]
    fn the_orca_environment_is_an_allowlist() {
        let inherited: HashMap<&str, &str> = HashMap::from([
            ("HOME", "/home/u"),
            ("LANG", "en_US.UTF-8"),
            ("DISPLAY", ":0"),
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("LD_LIBRARY_PATH", "/tmp/.mount_farm3d/usr/lib"),
            ("APPIMAGE", "/x/farm3d.AppImage"),
            ("GTK_THEME", "Adwaita"),
            ("PATH", "/tmp/.mount_farm3d/usr/bin:/usr/bin"),
        ]);
        let environment = orca_environment_from(|name| inherited.get(name).map(OsString::from));
        let names: Vec<&str> = environment
            .iter()
            .map(|(name, _)| name.to_str().unwrap())
            .collect();
        assert_eq!(names, ["HOME", "LANG", "PATH"]);
        assert_eq!(environment[2].1, OsString::from(ORCA_UNIX_PATH));
    }

    #[test]
    fn appimages_are_detected_by_elf_magic_and_the_type_2_marker() {
        let temp = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let path = temp.path().join(name);
            fs::write(&path, bytes).unwrap();
            path
        };
        assert!(is_appimage(&write(
            "a",
            b"\x7fELF\x02\x01\x01\x00AI\x02rest"
        )));
        assert!(!is_appimage(&write(
            "b",
            b"\x7fELF\x02\x01\x01\x00\x00\x00\x00rest"
        )));
        assert!(!is_appimage(&write("c", b"#!/bin/sh\nAI\x02")));
        assert!(!is_appimage(&write("d", b"\x7fELF")));
        assert!(!is_appimage(&temp.path().join("missing")));
    }

    #[test]
    fn stale_profile_caches_are_swept() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(PROFILE_CACHE_DIR);
        for name in ["keep", "old", ".extract-1234"] {
            fs::create_dir_all(root.join(name).join("resources")).unwrap();
        }
        fs::write(root.join("stray"), b"x").unwrap();
        let removed = sweep_stale_profile_caches(temp.path(), &["keep"]).unwrap();
        assert_eq!(removed, 3);
        let left: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["keep"]);
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(sweep_stale_profile_caches(empty.path(), &[]).unwrap(), 0);
    }

    #[test]
    fn a_directory_source_needs_json_bundles() {
        let temp = tempfile::tempdir().unwrap();
        let env = DiscoveryEnv {
            home: None,
            path_var: None,
            probe_timeout: Duration::from_secs(5),
        };
        let opc = temp.path().join("opc/resources/profiles");
        fs::create_dir_all(&opc).unwrap();
        fs::write(opc.join("Elegoo.opc"), b"ZCRO").unwrap();
        let (state, _) = resolve_preset_source(
            &temp.path().join("opc"),
            PresetSourceOrigin::Configured,
            None,
            &env,
            temp.path(),
        );
        assert_eq!(state, PresetSourceState::PresetsUnreadable);

        fs::create_dir_all(temp.path().join("empty")).unwrap();
        let (state, _) = resolve_preset_source(
            &temp.path().join("empty"),
            PresetSourceOrigin::Configured,
            None,
            &env,
            temp.path(),
        );
        assert!(matches!(state, PresetSourceState::Unavailable { .. }));

        let (state, _) = resolve_preset_source(
            &temp.path().join("gone"),
            PresetSourceOrigin::Configured,
            None,
            &env,
            temp.path(),
        );
        assert_eq!(
            state,
            PresetSourceState::Unavailable {
                reason: "gone no longer exists.".to_string()
            }
        );
    }

    #[test]
    fn status_serializes_with_its_state_tags() {
        let status = SlicerRuntimeStatus {
            engine: EngineState::NotFound,
            preset_source: PresetSourceState::NotConfigured,
            can_slice: false,
            versions_differ: false,
            revision: 1,
        };
        assert_eq!(
            serde_json::to_value(&status).unwrap(),
            serde_json::json!({
                "engine": { "state": "notFound" },
                "presetSource": { "state": "notConfigured" },
                "canSlice": false,
                "versionsDiffer": false,
                "revision": 1,
            })
        );
    }
}

/// Discovery, the probe, extraction, and the pickers against fake
/// executables: small `#!/bin/sh` scripts.
#[cfg(all(test, unix))]
mod fake_executable_tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::contracts::command::ErrorCode;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// A fake OrcaSlicer that prints `version` like the real one: the
    /// version line on stdout, then usage, with the display error on
    /// stderr, and a `result.json` in its working directory (Gate A).
    fn fake_orca(dir: &Path, name: &str, version: &str) -> PathBuf {
        script(
            dir,
            name,
            &format!(
                "echo 'OrcaSlicer-{version}:'\necho 'Usage: orca-slicer [ OPTIONS ] [ file.3mf ]'\n\
                 echo 'Error: unable to open display' >&2\necho '{{}}' > result.json"
            ),
        )
    }

    fn env(home: Option<&Path>, path_var: Option<&Path>) -> DiscoveryEnv {
        DiscoveryEnv {
            home: home.map(Path::to_path_buf),
            path_var: path_var.map(|path| path.as_os_str().to_os_string()),
            probe_timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn the_probe_reads_the_first_stdout_line() {
        let temp = tempfile::tempdir().unwrap();
        for (version, expected) in [
            (
                "2.4.2",
                ProbeOutcome::Supported(parse_version_line("OrcaSlicer-2.4.2:").unwrap()),
            ),
            (
                "2.5.0-dev",
                ProbeOutcome::Supported(parse_version_line("OrcaSlicer-2.5.0-dev:").unwrap()),
            ),
            (
                "3.0.0",
                ProbeOutcome::UnsupportedVersion(parse_version_line("OrcaSlicer-3.0.0:").unwrap()),
            ),
        ] {
            let engine = fake_orca(temp.path(), &format!("orca-{version}"), version);
            let probe = probe_engine(&engine, Duration::from_secs(5));
            assert_eq!(probe.outcome, expected, "{version}");
            assert!(!probe.extract_and_run);
        }
    }

    #[test]
    fn garbage_output_is_a_probe_failure() {
        let temp = tempfile::tempdir().unwrap();
        let engine = script(temp.path(), "garbage", "echo 'Usage: something else'");
        let ProbeOutcome::Failed(reason) = probe_engine(&engine, Duration::from_secs(5)).outcome
        else {
            panic!("garbage must fail");
        };
        assert_eq!(reason, "garbage did not report an OrcaSlicer version.");

        let silent = script(temp.path(), "silent", "exit 1");
        assert!(matches!(
            probe_engine(&silent, Duration::from_secs(5)).outcome,
            ProbeOutcome::Failed(_)
        ));
        assert!(matches!(
            probe_engine(&temp.path().join("missing"), Duration::from_secs(5)).outcome,
            ProbeOutcome::Failed(_)
        ));
    }

    #[test]
    fn a_hung_probe_times_out() {
        let temp = tempfile::tempdir().unwrap();
        let engine = script(temp.path(), "hang", "exec sleep 30");
        let started = Instant::now();
        let ProbeOutcome::Failed(reason) =
            probe_engine(&engine, Duration::from_millis(300)).outcome
        else {
            panic!("a hang must fail");
        };
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(reason.contains("did not answer"), "{reason}");
    }

    /// A grandchild holding stdout open: the probe times out, stops the
    /// whole process group (the grandchild too), and returns.
    #[test]
    fn a_hung_probe_with_a_grandchild_stops_the_whole_group() {
        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("grandchild");
        let engine = script(
            temp.path(),
            "hang-with-child",
            &format!("sleep 30 &\necho $! > '{}'\nwait", pid_file.display()),
        );
        let started = Instant::now();
        let ProbeOutcome::Failed(reason) =
            probe_engine(&engine, Duration::from_millis(300)).outcome
        else {
            panic!("a hang must fail");
        };
        assert!(reason.contains("did not answer"), "{reason}");
        assert!(started.elapsed() < Duration::from_secs(5));
        let grandchild: i32 = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let pid = rustix::process::Pid::from_raw(grandchild).unwrap();
        assert!(
            rustix::process::test_kill_process(pid).is_err(),
            "the grandchild survived the probe"
        );
    }

    #[test]
    fn the_probe_runs_in_a_scratch_directory_with_the_allowlisted_environment() {
        let temp = tempfile::tempdir().unwrap();
        let dump = temp.path().join("dump");
        let engine = script(
            temp.path(),
            "env-orca",
            &format!(
                "echo 'OrcaSlicer-2.4.2:'\npwd > '{0}.cwd'\nenv > '{0}.env'\necho '{{}}' > result.json",
                dump.display()
            ),
        );
        assert!(matches!(
            probe_engine(&engine, Duration::from_secs(5)).outcome,
            ProbeOutcome::Supported(_)
        ));
        let cwd = PathBuf::from(
            fs::read_to_string(temp.path().join("dump.cwd"))
                .unwrap()
                .trim(),
        );
        assert!(
            cwd.starts_with(std::env::temp_dir())
                || cwd.to_string_lossy().contains("farm3d-orca-probe-")
        );
        assert!(
            !cwd.exists(),
            "the scratch directory (and its result.json) is removed"
        );
        assert!(!temp.path().join("result.json").exists());

        let names: Vec<String> = fs::read_to_string(temp.path().join("dump.env"))
            .unwrap()
            .lines()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name.to_string()))
            .collect();
        // The shell itself adds `PWD`, `OLDPWD`, `SHLVL`, and `_`.
        let allowed = [
            "HOME",
            "USER",
            "LANG",
            "LC_ALL",
            "TMPDIR",
            "XDG_RUNTIME_DIR",
            "PATH",
            "PWD",
            "OLDPWD",
            "SHLVL",
            "_",
        ];
        for name in &names {
            assert!(
                allowed.contains(&name.as_str()),
                "{name} leaked into OrcaSlicer's environment"
            );
        }
        assert!(fs::read_to_string(temp.path().join("dump.env"))
            .unwrap()
            .contains(&format!("PATH={ORCA_UNIX_PATH}")));
    }

    #[test]
    fn discovery_finds_orca_slicer_on_path() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        fake_orca(&bin, PATH_EXECUTABLE, "2.4.2");
        let path_var = std::env::join_paths([temp.path().join("empty"), bin.clone()]).unwrap();
        let env = DiscoveryEnv {
            home: None,
            path_var: Some(path_var),
            probe_timeout: Duration::from_secs(5),
        };
        let (state, engine) = discover_engine(None, &env);
        let engine = engine.unwrap();
        assert_eq!(engine.source, EngineSource::Path);
        assert_eq!(engine.path, bin.join(PATH_EXECUTABLE));
        let EngineState::Available {
            version,
            executable_name,
            ..
        } = state
        else {
            panic!("expected available");
        };
        assert_eq!(version, "2.4.2");
        assert_eq!(executable_name, "orca-slicer");
    }

    #[test]
    fn a_configured_engine_wins_and_a_broken_one_falls_through() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        fake_orca(&bin, PATH_EXECUTABLE, "2.4.2");
        let configured = fake_orca(&temp.path().join("chosen"), "my-orca", "2.5.0-dev");
        let env = env(None, Some(&bin));

        let (_, engine) = discover_engine(Some(&configured), &env);
        let engine = engine.unwrap();
        assert_eq!(engine.source, EngineSource::Configured);
        assert_eq!(engine.version.to_string(), "2.5.0-dev");

        let unsupported = fake_orca(&temp.path().join("chosen"), "orca-three", "3.0.0");
        let (_, engine) = discover_engine(Some(&unsupported), &env);
        assert_eq!(engine.unwrap().source, EngineSource::Path);

        // With nothing else to try, the configured engine's failure shows.
        let (state, engine) = discover_engine(Some(&unsupported), &self::env(None, None));
        assert!(engine.is_none());
        assert_eq!(
            state,
            EngineState::UnsupportedVersion {
                version: "3.0.0".to_string(),
                executable_name: "orca-three".to_string(),
            }
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn well_known_appimages_pick_the_newest_release() {
        let home = tempfile::tempdir().unwrap();
        fake_orca(
            &home.path().join("Applications"),
            "OrcaSlicer_old.AppImage",
            "2.4.2",
        );
        fake_orca(
            &home.path().join(".local/bin"),
            "OrcaSlicer_nightly.AppImage",
            "2.5.0-dev",
        );
        fake_orca(
            &home.path().join("Downloads"),
            "OrcaSlicer_V2.5.0.AppImage",
            "2.5.0",
        );
        fake_orca(
            &home.path().join("Downloads"),
            "OrcaSlicer_V3.AppImage",
            "3.0.0",
        );
        fake_orca(&home.path().join("Downloads"), "Other.AppImage", "2.9.9");
        let (state, engine) = discover_engine(None, &env(Some(home.path()), None));
        let engine = engine.unwrap();
        assert_eq!(engine.source, EngineSource::WellKnown);
        assert_eq!(engine.version.to_string(), "2.5.0");
        assert!(matches!(state, EngineState::Available { .. }));
    }

    #[test]
    fn nothing_found_is_not_found() {
        let home = tempfile::tempdir().unwrap();
        let (state, engine) = discover_engine(None, &env(Some(home.path()), Some(home.path())));
        assert_eq!(state, EngineState::NotFound);
        assert!(engine.is_none());
    }

    #[test]
    fn the_probe_cache_reuses_a_result_until_the_file_changes() {
        let temp = tempfile::tempdir().unwrap();
        let counter = temp.path().join("count");
        let body = |version: &str| {
            format!(
                "echo x >> '{}'\necho 'OrcaSlicer-{version}:'",
                counter.display()
            )
        };
        let engine = script(temp.path(), "orca", &body("2.4.2"));
        let cache = ProbeCache::new();
        let runs = || fs::read_to_string(&counter).unwrap().lines().count();
        cache.probe(&engine, Duration::from_secs(5));
        cache.probe(&engine, Duration::from_secs(5));
        assert_eq!(runs(), 1);
        // A different size invalidates the entry.
        script(
            temp.path(),
            "orca",
            &format!("{}\n# changed", body("2.4.3")),
        );
        let probe = cache.probe(&engine, Duration::from_secs(5));
        assert_eq!(runs(), 2);
        assert_eq!(
            probe.outcome,
            ProbeOutcome::Supported(parse_version_line("OrcaSlicer-2.4.3:").unwrap())
        );
    }

    /// A fake AppImage extractor: `--appimage-extract` writes
    /// `squashfs-root/resources/profiles` into its working directory.
    fn fake_extractor(dir: &Path, name: &str, contents: &str) -> PathBuf {
        script(
            dir,
            name,
            &format!(
                "if [ \"$1\" = --appimage-extract ] && [ \"$2\" = 'resources/profiles/*' ]; then\n\
                 p=squashfs-root/resources/profiles\nmkdir -p \"$p/V/process\"\n{contents}\nfi"
            ),
        )
    }

    #[test]
    fn appimage_profiles_are_extracted_once_and_keep_only_json() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let appimage = fake_extractor(
            temp.path(),
            "Orca.AppImage",
            "echo '{}' > \"$p/V.json\"\necho '{}' > \"$p/V/process/a.json\"\n\
             echo x > \"$p/V/cover.png\"\nln -s V.json \"$p/link.json\"\n\
             echo '{}' > \"$p/blacklist.json\"",
        );
        let ExtractOutcome::Extracted { root, hash } =
            extract_appimage_profiles(&appimage, &cache, Duration::from_secs(10))
        else {
            panic!("expected an extraction");
        };
        assert_eq!(root, cache.join(PROFILE_CACHE_DIR).join(&hash));
        assert_eq!(hash, sha256_file(&appimage).unwrap());
        let profiles = root.join("resources/profiles");
        assert!(profiles.join("V.json").is_file());
        assert!(profiles.join("V/process/a.json").is_file());
        assert!(!profiles.join("V/cover.png").exists());
        assert!(fs::symlink_metadata(profiles.join("link.json")).is_err());
        let leftovers: Vec<_> = fs::read_dir(cache.join(PROFILE_CACHE_DIR))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, [std::ffi::OsString::from(&hash)]);

        // The cache is reused: a second run doesn't re-extract.
        fs::write(profiles.join("marker.json"), b"{}").unwrap();
        extract_appimage_profiles(&appimage, &cache, Duration::from_secs(10));
        assert!(profiles.join("marker.json").exists());
    }

    #[test]
    fn a_cache_only_appimage_is_unreadable() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        // The nightly's layout: `.opc` bundles, vendor folders, and a
        // stray `blacklist.json` that is not a vendor.
        let opc = fake_extractor(
            temp.path(),
            "Nightly.AppImage",
            "echo ZCRO > \"$p/V.opc\"\necho '{}' > \"$p/blacklist.json\"",
        );
        assert_eq!(
            extract_appimage_profiles(&opc, &cache, Duration::from_secs(10)),
            ExtractOutcome::Unreadable
        );
        let none = fake_extractor(temp.path(), "Empty.AppImage", "true");
        assert_eq!(
            extract_appimage_profiles(&none, &cache, Duration::from_secs(10)),
            ExtractOutcome::NoProfiles
        );
        let leftovers = fs::read_dir(cache.join(PROFILE_CACHE_DIR)).unwrap().count();
        assert_eq!(leftovers, 0);
    }

    /// An install layout: `<prefix>/bin/orca-slicer` beside
    /// `<prefix>/share/OrcaSlicer/resources/profiles`.
    fn install(prefix: &Path, version: &str) -> PathBuf {
        let profiles = prefix.join("share/OrcaSlicer/resources/profiles");
        fs::create_dir_all(&profiles).unwrap();
        for vendor in ["A", "B"] {
            fs::write(profiles.join(format!("{vendor}.json")), b"{}").unwrap();
            fs::create_dir_all(profiles.join(vendor)).unwrap();
        }
        fake_orca(&prefix.join("bin"), PATH_EXECUTABLE, version)
    }

    #[test]
    fn an_installed_engine_supplies_its_own_presets() {
        let temp = tempfile::tempdir().unwrap();
        let engine = install(&temp.path().join("usr"), "2.4.2");
        let config = SlicerRuntimeConfig {
            revision: 3,
            engine_path: Some(engine.to_str().unwrap().to_string()),
            preset_source_path: None,
            updated_at: None,
        };
        let runtime = resolve_runtime(&config, &env(None, None), temp.path());
        assert!(runtime.status.can_slice);
        assert!(!runtime.status.versions_differ);
        assert_eq!(runtime.status.revision, 3);
        let PresetSourceState::Available {
            origin,
            vendor_count,
            version,
            ..
        } = runtime.status.preset_source
        else {
            panic!("expected available presets");
        };
        assert_eq!(origin, PresetSourceOrigin::Engine);
        assert_eq!(vendor_count, 2);
        assert_eq!(version, "2.4.2");
    }

    #[test]
    fn a_separate_preset_source_can_differ_in_version() {
        let temp = tempfile::tempdir().unwrap();
        let engine = fake_orca(&temp.path().join("nightly"), "orca", "2.5.0-dev");
        let presets = install(&temp.path().join("usr"), "2.4.2");
        let config = SlicerRuntimeConfig {
            revision: 1,
            engine_path: Some(engine.to_str().unwrap().to_string()),
            preset_source_path: Some(temp.path().join("usr").to_str().unwrap().to_string()),
            updated_at: None,
        };
        let runtime = resolve_runtime(&config, &env(None, None), temp.path());
        assert!(runtime.status.can_slice);
        assert!(runtime.status.versions_differ);
        let source = runtime.preset_source.unwrap();
        assert_eq!(source.origin, PresetSourceOrigin::Configured);
        assert_eq!(source.version.to_string(), "2.4.2");
        assert_eq!(
            source.profiles_dir,
            temp.path().join("usr/share/OrcaSlicer/resources/profiles")
        );
        // The installed executable works as a preset source too.
        let (state, _) = resolve_preset_source(
            &presets,
            PresetSourceOrigin::Configured,
            None,
            &env(None, None),
            temp.path(),
        );
        assert!(matches!(state, PresetSourceState::Available { .. }));
    }

    #[test]
    fn an_engine_without_readable_presets_is_presets_unreadable() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("opt");
        let profiles = prefix.join("resources/profiles");
        fs::create_dir_all(&profiles).unwrap();
        fs::write(profiles.join("Elegoo.opc"), b"ZCRO").unwrap();
        let engine = fake_orca(&prefix.join("bin"), "orca-slicer", "2.5.0-dev");
        let config = SlicerRuntimeConfig {
            revision: 1,
            engine_path: Some(engine.to_str().unwrap().to_string()),
            preset_source_path: None,
            updated_at: None,
        };
        let runtime = resolve_runtime(&config, &env(None, None), temp.path());
        assert!(matches!(
            runtime.status.engine,
            EngineState::Available { .. }
        ));
        assert_eq!(
            runtime.status.preset_source,
            PresetSourceState::PresetsUnreadable
        );
        assert!(!runtime.status.can_slice);

        let nothing = resolve_runtime(
            &SlicerRuntimeConfig {
                revision: 1,
                engine_path: None,
                preset_source_path: None,
                updated_at: None,
            },
            &env(Some(temp.path()), Some(temp.path())),
            temp.path(),
        );
        assert_eq!(
            nothing.status.preset_source,
            PresetSourceState::NotConfigured
        );
    }

    fn config(storage: &Storage) -> SlicerRuntimeConfig {
        current_config(storage).unwrap()
    }

    #[test]
    fn picking_an_engine_validates_then_saves() {
        let (temp, _lease, storage) = crate::test_storage();
        let good = fake_orca(&temp.path().join("bin"), "orca-good", "2.4.2");
        let bad = fake_orca(&temp.path().join("bin"), "orca-three", "3.0.0");
        let env = env(None, None);

        let cancelled =
            pick_slicer_engine(&FixedSlicerRuntimeFileIo::default(), &storage, 1, &env).unwrap();
        assert_eq!(cancelled, RuntimePick::Cancelled);

        let io = FixedSlicerRuntimeFileIo {
            engine: Some(bad),
            preset_source: None,
        };
        let error = pick_slicer_engine(&io, &storage, 1, &env).unwrap_err();
        assert_eq!(error.code, ErrorCode::SlicerUnavailable);
        assert!(!error.message.contains(temp.path().to_str().unwrap()));
        assert_eq!(config(&storage).revision, 1);

        let io = FixedSlicerRuntimeFileIo {
            engine: Some(good.clone()),
            preset_source: None,
        };
        let RuntimePick::Saved(saved) = pick_slicer_engine(&io, &storage, 1, &env).unwrap() else {
            panic!("expected a save");
        };
        assert_eq!(saved.revision, 2);
        assert_eq!(saved.engine_path.as_deref(), good.to_str());

        let stale = pick_slicer_engine(&io, &storage, 1, &env).unwrap_err();
        assert_eq!(stale.code, ErrorCode::Conflict);
    }

    #[test]
    fn picking_a_preset_source_validates_then_saves_and_reset_clears() {
        let (temp, _lease, storage) = crate::test_storage();
        let env = env(None, None);
        let engine = fake_orca(&temp.path().join("bin"), "orca", "2.4.2");
        storage
            .write_repo(|tx| save_runtime_config(tx, 1, engine.to_str(), None))
            .unwrap();

        let unreadable = temp.path().join("nightly");
        fs::create_dir_all(unreadable.join("resources/profiles")).unwrap();
        fs::write(unreadable.join("resources/profiles/V.opc"), b"ZCRO").unwrap();
        let io = FixedSlicerRuntimeFileIo {
            engine: None,
            preset_source: Some(unreadable),
        };
        let error = pick_preset_source(
            &io,
            &storage,
            2,
            PresetSourcePickKind::Folder,
            &env,
            temp.path(),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::PresetSourceUnavailable);

        install(&temp.path().join("usr"), "2.4.2");
        let io = FixedSlicerRuntimeFileIo {
            engine: None,
            preset_source: Some(temp.path().join("usr")),
        };
        let RuntimePick::Saved(saved) = pick_preset_source(
            &io,
            &storage,
            2,
            PresetSourcePickKind::Folder,
            &env,
            temp.path(),
        )
        .unwrap() else {
            panic!("expected a save");
        };
        assert_eq!(saved.revision, 3);
        assert_eq!(
            saved.engine_path.as_deref(),
            engine.to_str(),
            "the engine is kept"
        );
        assert!(saved.preset_source_path.is_some());

        let reset = reset_slicer_runtime(&storage, 3, false, true).unwrap();
        assert_eq!(reset.engine_path.as_deref(), engine.to_str());
        assert_eq!(reset.preset_source_path, None);
        let reset = reset_slicer_runtime(&storage, 4, true, false).unwrap();
        assert_eq!(reset.engine_path, None);
        assert_eq!(
            reset_slicer_runtime(&storage, 4, true, true)
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
    }
}
