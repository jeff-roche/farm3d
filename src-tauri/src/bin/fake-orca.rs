//! `fake-orca`: a stand-in for the OrcaSlicer CLI that the slicing tests
//! drive (spec D23). Built only with `--features test-support`, and never
//! packaged (`scripts/assert-package-contents.sh` fails if it is).
//!
//! It reproduces the contract the P5 spike observed on v2.4.2:
//!
//! - `--help` prints `OrcaSlicer-<FAKE_ORCA_VERSION>:` on stdout and writes
//!   `result.json` into the working directory.
//! - A slice honours `--outputdir`, `--pipe`, `--slice`, `--load-settings`
//!   and `--load-filaments`, opens the progress FIFO with a ~1 s retry,
//!   writes `result.json` into the output directory, and exits with
//!   `return_code & 0xff`. A missing input is −3 and an unparseable preset
//!   −5, as in Gate F.
//! - SIGTERM keeps its default action, so it exits at once.
//!
//! `FAKE_ORCA_SCENARIO` picks the behaviour:
//!
//! | Scenario | Behaviour |
//! |---|---|
//! | `success` (default) | writes `plate_1.gcode` from `FAKE_ORCA_GCODE` (or a small built-in G-code), with `printer_settings_id` and `filament_settings_id` rewritten to the loaded preset names |
//! | `fail:<code>` | writes `result.json` with that return code and Gate F's error string; no output |
//! | `successNoOutput` | return code 0 and no G-code |
//! | `hang` | one progress line, then sleeps until killed |
//! | `hangWithGrandchild` | as `hang`, after starting a sleeping grandchild whose pid it writes to `grandchild.pid` in the working directory |
//! | `garbageProgress` | malformed, backwards, and out-of-range FIFO lines around a normal success |
//! | `oversizedLog` | `FAKE_ORCA_LOG_BYTES` (default 12 MiB) of stdout and stderr naming absolute paths, then success |
//! | `wrongPresetNames` | success, but the G-code names other presets |
//! | `malformedOutput` | return code 0 and a `plate_1.gcode` that is not G-code |
//! | `oversizedOutput` | return code 0 and a sparse `plate_1.gcode` of `FAKE_ORCA_OUTPUT_BYTES` (default 1 GiB + 1) |
//! | `crash` | aborts (SIGABRT), a signal farm3d did not send |
//!
//! `FAKE_ORCA_STEP_MS` (default 20) is the pause between progress lines,
//! and a non-empty `FAKE_ORCA_NO_RESULT` skips `result.json`, leaving only
//! the exit status.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const GRANDCHILD_ARG: &str = "--fake-orca-grandchild";

/// Gate F's error strings (v2.4.2).
fn error_string(code: i32) -> &'static str {
    match code {
        0 => "Success.",
        -3 => "The input files to the slicer are not found.",
        -5 => "The input preset file is invalid and can not be parsed.",
        -6 => "The input model file to the slicer can not be parsed.",
        -17 => "The selected printer is not compatible with the process preset in the 3mf.",
        -24 => "The input 3mf is from a newer version.",
        -50 => "One of the plate is empty or has no object fully inside it, please check.",
        _ => "Unknown error.",
    }
}

#[derive(Default)]
struct Args {
    help: bool,
    outputdir: Option<PathBuf>,
    load_settings: Vec<PathBuf>,
    load_filaments: Vec<PathBuf>,
    pipe: Option<PathBuf>,
    slice: Option<String>,
    inputs: Vec<PathBuf>,
}

fn parse_args() -> Args {
    let mut args = Args::default();
    let mut raw = std::env::args_os().skip(1);
    let split = |value: std::ffi::OsString| -> Vec<PathBuf> {
        value
            .to_string_lossy()
            .split(';')
            .filter(|part| !part.is_empty())
            .map(PathBuf::from)
            .collect()
    };
    while let Some(arg) = raw.next() {
        match arg.to_str() {
            Some("--help" | "-h") => args.help = true,
            Some("--outputdir") => args.outputdir = raw.next().map(PathBuf::from),
            Some("--load-settings") => {
                args.load_settings = raw.next().map(split).unwrap_or_default()
            }
            Some("--load-filaments") => {
                args.load_filaments = raw.next().map(split).unwrap_or_default()
            }
            Some("--pipe") => args.pipe = raw.next().map(PathBuf::from),
            Some("--slice") => args.slice = raw.next().map(|value| value.to_string_lossy().into()),
            Some("--datadir" | "--arrange" | "--orient") => {
                raw.next();
            }
            Some(flag) if flag.starts_with("--") => {}
            _ => args.inputs.push(PathBuf::from(arg)),
        }
    }
    args
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn env_number(name: &str, default: u64) -> u64 {
    env(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn write_result(dir: &Path, code: i32) {
    let result = serde_json::json!({
        "error_string": error_string(code),
        "export_time": 0,
        "layer_height": 0.0,
        "plate_index": if code == 0 { 1 } else { 0 },
        "prepare_time": 0,
        "return_code": code,
        "sparse_infill_density": 0.0,
        "wall_loops": 0,
    });
    let _ = fs::write(
        dir.join("result.json"),
        serde_json::to_string_pretty(&result).unwrap(),
    );
}

/// Writes `result.json` and exits with the code as OrcaSlicer does.
fn finish(out: &Path, code: i32) -> ! {
    if env("FAKE_ORCA_NO_RESULT").is_none() {
        write_result(out, code);
    }
    if code != 0 {
        eprintln!("run found error, return {code}, exit...");
    }
    std::process::exit(code & 0xff)
}

fn sleep_forever() -> ! {
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

/// The FIFO writer. Like OrcaSlicer's, it retries its non-blocking open
/// for about 1 s and then carries on without progress.
struct Progress(Option<File>);

impl Progress {
    #[cfg(unix)]
    fn open(path: Option<&Path>) -> Self {
        use rustix::fs::{fcntl_setfl, open, Mode, OFlags};
        let Some(path) = path else {
            return Self(None);
        };
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(1) {
            match open(
                path,
                OFlags::WRONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(fd) => {
                    let _ = fcntl_setfl(&fd, OFlags::empty());
                    return Self(Some(File::from(fd)));
                }
                Err(_) => thread::sleep(Duration::from_millis(50)),
            }
        }
        eprintln!("fake-orca: no progress reader; slicing without progress");
        Self(None)
    }

    #[cfg(not(unix))]
    fn open(_path: Option<&Path>) -> Self {
        Self(None)
    }

    fn raw(&mut self, line: &str) {
        if let Some(file) = &mut self.0 {
            let _ = file.write_all(format!("{line}\n").as_bytes());
        }
    }

    fn update(&mut self, message: &str, percent: i64) {
        self.raw(
            &serde_json::json!({
                "message": message,
                "plate_index": 1,
                "plate_count": 1,
                "plate_percent": percent,
                "total_percent": percent,
            })
            .to_string(),
        );
    }
}

fn step() {
    thread::sleep(Duration::from_millis(env_number("FAKE_ORCA_STEP_MS", 20)));
}

fn preset_name(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("name")?.as_str().map(str::to_string)
}

/// Gate F: a missing input is −3, and a preset that isn't JSON −5.
fn check_inputs(args: &Args, out: &Path) -> (Option<String>, Option<String>) {
    let presets: Vec<&PathBuf> = args
        .load_settings
        .iter()
        .chain(&args.load_filaments)
        .collect();
    if args.inputs.is_empty()
        || args
            .inputs
            .iter()
            .chain(presets.iter().copied())
            .any(|path| !path.is_file())
    {
        for path in args.inputs.iter().chain(presets.iter().copied()) {
            if !path.is_file() {
                println!("No such file: {}", path.display());
            }
        }
        finish(out, -3);
    }
    for path in &presets {
        let parses = fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .is_some_and(|value| value.is_object());
        if !parses {
            println!("load_from_json: failed to parse {}", path.display());
            finish(out, -5);
        }
    }
    let machine = args
        .load_settings
        .first()
        .and_then(|path| preset_name(path));
    let filament = args
        .load_filaments
        .first()
        .and_then(|path| preset_name(path));
    (machine, filament)
}

const BUILT_IN_GCODE: &str = "\
; HEADER_BLOCK_START
; generated by OrcaSlicer 2.4.2 on 2026-09-24 at 12:00:00
; total layer number: 1
; HEADER_BLOCK_END
G28
G1 Z0.2 F600
G1 X10 Y10 E1 F1200
G1 X20 Y10 E2
; CONFIG_BLOCK_START
; filament_settings_id = \"Fake Filament\"
; printer_settings_id = Fake Printer
; CONFIG_BLOCK_END
";

/// The G-code to write, with the preset claims rewritten (R6).
fn gcode(machine: Option<&str>, filament: Option<&str>) -> String {
    let source = env("FAKE_ORCA_GCODE")
        .map(|path| fs::read_to_string(path).expect("FAKE_ORCA_GCODE is readable"))
        .unwrap_or_else(|| BUILT_IN_GCODE.to_string());
    source
        .lines()
        .map(|line| match (line.trim_start(), machine, filament) {
            (text, Some(name), _) if text.starts_with("; printer_settings_id =") => {
                format!("; printer_settings_id = {name}")
            }
            (text, _, Some(name)) if text.starts_with("; filament_settings_id =") => {
                format!("; filament_settings_id = \"{name}\"")
            }
            _ => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn noisy_log(args: &Args) {
    let bytes = env_number("FAKE_ORCA_LOG_BYTES", 12 * 1024 * 1024);
    let input = args
        .inputs
        .first()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let home = env("HOME").unwrap_or_default();
    let exe = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut out = stdout.lock();
    let mut err = stderr.lock();
    let _ = writeln!(out, "FAKE-ORCA FIRST LINE");
    let mut written = 0u64;
    let mut index = 0u64;
    while written < bytes {
        let line = format!("log line {index}: reading {input} from {home}/.config via {exe}\n");
        let target: &mut dyn Write = if index.is_multiple_of(2) {
            &mut out
        } else {
            &mut err
        };
        let _ = target.write_all(line.as_bytes());
        written += line.len() as u64;
        index += 1;
    }
    let _ = writeln!(out, "FAKE-ORCA LAST LINE");
    let _ = out.flush();
}

fn slice_success(
    args: &Args,
    out: &Path,
    progress: &mut Progress,
    names: (Option<String>, Option<String>),
) -> ! {
    for (message, percent) in [
        ("Slicing mesh", 30),
        ("Generating perimeters", 60),
        ("Exporting G-code", 90),
    ] {
        progress.update(message, percent);
        step();
    }
    if args.slice.as_deref() == Some("1") || args.slice.is_none() {
        let _ = fs::write(
            out.join("plate_1.gcode"),
            gcode(names.0.as_deref(), names.1.as_deref()),
        );
    }
    progress.update("Slicing finished", 100);
    finish(out, 0)
}

fn main() {
    let mut raw = std::env::args().skip(1);
    if raw.next().as_deref() == Some(GRANDCHILD_ARG) {
        sleep_forever();
    }

    let args = parse_args();
    let version = env("FAKE_ORCA_VERSION").unwrap_or_else(|| "2.4.2".to_string());
    if args.help {
        println!("OrcaSlicer-{version}:");
        println!("Usage: orca-slicer [ OPTIONS ] [ file.3mf/file.stl ... ]");
        write_result(Path::new("."), 0);
        std::process::exit(0);
    }
    let out = args.outputdir.clone().unwrap_or_else(|| PathBuf::from("."));
    let scenario = env("FAKE_ORCA_SCENARIO").unwrap_or_else(|| "success".to_string());
    println!("OrcaSlicer-{version}: fake-orca scenario {scenario}");
    eprintln!("Error: unable to open display");
    if let Some(code) = scenario.strip_prefix("fail:") {
        let code: i32 = code.parse().expect("fail:<code> takes an integer");
        finish(&out, code);
    }

    let names = check_inputs(&args, &out);
    if let Some(input) = args.inputs.first() {
        println!("loading {}", input.display());
    }
    let mut progress = Progress::open(args.pipe.as_deref());
    progress.update("Loading file", 1);
    step();

    match scenario.as_str() {
        "success" => slice_success(&args, &out, &mut progress, names),
        "wrongPresetNames" => slice_success(
            &args,
            &out,
            &mut progress,
            (
                Some("Some Other Printer".into()),
                Some("Some Other Filament".into()),
            ),
        ),
        "successNoOutput" => {
            progress.update("Slicing finished", 100);
            finish(&out, 0)
        }
        "hang" => sleep_forever(),
        "crash" => std::process::abort(),
        "hangWithGrandchild" => {
            let exe = std::env::current_exe().expect("own path");
            // Never waited on: it must outlive this process unless the
            // supervisor stops the whole group.
            #[allow(clippy::zombie_processes)]
            let grandchild = Command::new(exe)
                .arg(GRANDCHILD_ARG)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("start the grandchild");
            fs::write("grandchild.pid", grandchild.id().to_string()).expect("write grandchild.pid");
            println!("started grandchild {}", grandchild.id());
            sleep_forever()
        }
        "garbageProgress" => {
            for line in [
                "not json at all",
                "{\"message\": \"half a line\"",
                "[1, 2, 3]",
                "{}",
                "{\"message\": 42}",
                "{\"message\":\"Too far\",\"plate_index\":1,\"plate_count\":1,\"plate_percent\":250,\"total_percent\":250}",
                "{\"message\":\"Backwards\",\"plate_index\":1,\"plate_count\":1,\"plate_percent\":5,\"total_percent\":5}",
                "\u{1}\u{2}binary\u{7f}",
            ] {
                progress.raw(line);
            }
            step();
            slice_success(&args, &out, &mut progress, names)
        }
        "oversizedLog" => {
            noisy_log(&args);
            slice_success(&args, &out, &mut progress, names)
        }
        "malformedOutput" => {
            let _ = fs::write(out.join("plate_1.gcode"), "this is not G-code\n\0\0\0");
            finish(&out, 0)
        }
        "oversizedOutput" => {
            let file = File::create(out.join("plate_1.gcode")).expect("create output");
            file.set_len(env_number("FAKE_ORCA_OUTPUT_BYTES", (1 << 30) + 1))
                .expect("size output");
            finish(&out, 0)
        }
        other => {
            eprintln!("fake-orca: unknown FAKE_ORCA_SCENARIO {other}");
            std::process::exit(2)
        }
    }
}
