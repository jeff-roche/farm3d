# OrcaSlicer runtime contract

**Status:** Approved. The approval comes from the P5 design and runtime
spike, which the user approved on 2026-09-24.

## Context

ADR-0003 decided that farm3d slices by running OrcaSlicer as a separate
process. Until P5, OrcaSlicer ran only at build time, to generate the printer
catalog. P5 runs it on the user's machine to slice Models, so farm3d needs a
fixed contract for which OrcaSlicer it runs, where the presets come from, how
it is invoked, and how progress, cancellation, and failure are observed.

The P5 runtime spike
(`docs/superpowers/baselines/2026-09-24-p5-orca-runtime-spike.md`) measured
OrcaSlicer v2.4.2 and a 2.5.0-dev nightly on Linux x86_64. Two findings shape
this decision:

- Nightlies built after 2026-08-21 ship their system presets only as binary
  `<Vendor>.opc` caches. Their CLI cannot resolve `inherits` from that cache,
  but it slices correctly with flat presets built from v2.4.2's JSON.
- The CLI's behavior has sharp edges: `--help` writes `result.json` into the
  working directory, `--mstpp` is not enforced, a missing plate reports
  "Success." with no G-code, an incompatible filament slices silently, and
  SIGKILL of an AppImage can leave a stale FUSE mount.

The full contract is spec `2026-09-24-p5-runtime-slicing-design.md`, D2–D11
and D24. This ADR records the parts that are hard to reverse.

## Decision

**The user installs OrcaSlicer; farm3d never bundles it.** farm3d looks
for an engine in this order and stops at the first step that finds a
supported one:

1. the configured `engine_path`;
2. `orca-slicer` on `PATH`;
3. on Linux, `*OrcaSlicer*.AppImage` in `~/Applications`, `~/.local/bin`,
   and `~/Downloads`. Every match is probed, and the newest probed version
   wins, a release before a prerelease of the same version.

Discovery never scans the disk. Paths reach the backend only through
Rust-owned native pickers, never from the frontend.

**Version policy: every 2.x release, prerelease, and nightly.** The probe
runs `<engine> --help` in a fresh temporary directory with a 10 s timeout
and reads the first stdout line, which must match
`^OrcaSlicer-(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?:$`. Major version 2 is
accepted; a suffix marks the build `prerelease`. Anything else is refused as
`unsupportedVersion` or `probeFailed`. A 2.x runtime is trusted only as far
as farm3d checks it: every mapped key a slice writes must be known to the
preset source, and every output passes D11's checks.

**The engine and the preset source are separate.** A slicer runtime is an
engine plus a preset source: a JSON-bearing OrcaSlicer install, AppImage, or
`resources` directory. The preset source defaults to the engine. An engine
with only `.opc` caches and no configured source is `presetsUnreadable`, and
the user is asked to choose a v2.4-era source. farm3d resolves `inherits`
itself in OrcaSlicer's order (own bundle, then `OrcaFilamentLibrary`, then
the rest by name) and passes flat presets that keep `from: "system"` and the
system names. An AppImage source's profiles are extracted once per AppImage
SHA-256 into the app cache. farm3d never reads `.opc` files and never
writes to the user's OrcaSlicer data directory. Each Slice Revision records
both versions.

**Invocation: one process per plate, fixed arguments.** Each operation gets
a work directory under the content root and runs:

```text
<engine> --datadir <work>/datadir --outputdir <work>/out
  --load-settings "<work>/input/machine.json;<work>/input/process.json"
  --load-filaments <work>/input/filament.json
  --arrange 0 --orient 0 --slice 1 [--pipe <work>/progress.fifo]
  <work>/input/plate.3mf
```

The input is a farm3d-written core-only 3MF with the instances already
placed. `--allow-newer-file`, `--mstpp`, `--no-check`, and `--debug` are never
passed. The working directory is the work directory, and the environment is
an explicit allowlist, so variables leaked by an AppImage-packaged farm3d
never reach OrcaSlicer. The argument vector is a committed fixture
(`src-tauri/tests/fixtures/slicing/argument-vectors.json`) and is recorded
in every revision's invocation manifest.

**Supervision.** The process runs in its own process group, with
`PR_SET_PDEATHSIG(SIGTERM)` on Linux so it dies with farm3d. Cancel and the
30-minute timeout send SIGTERM to the group, then SIGKILL after 5 s. Only
after a SIGKILL does farm3d look for a stale FUSE mount of the engine's
AppImage, and it unmounts only a mount that appeared during the run, so a
user's own open OrcaSlicer is never touched.
At most one OrcaSlicer runs at a time. On restart, queued and running
operations become `interrupted` and their work directories are removed;
published revisions are never touched.

**Success is checked, not trusted.** A slice succeeds only when the return
code is 0, `out/plate_1.gcode` exists and passes the G-code inspector, its
printed bounds fit the target, and its preset-name claims equal the presets
farm3d passed. farm3d checks filament compatibility itself before starting.

**Progress is Linux-only.** On Linux, farm3d creates the FIFO and opens its
read end before spawning, because OrcaSlicer gives up on the pipe after about
1 s without a reader. Progress lines are JSON, and the stage message is
always shown, because the percentage stalls while a model loads. On other
platforms progress is indeterminate ("Slicing…").

**Platform scope.** Linux x86_64 is the only supported platform. Windows (a
Job Object) and macOS (a process group, no PDEATHSIG) compile the process
layer and make no runtime claim. The Flatpak build is unsupported until it
is tested.

## Options considered

### Bundle a pinned OrcaSlicer

Bundling would fix the version and remove discovery, at the cost of
shipping a 138 MB AGPL AppImage in every farm3d package. The user installs
OrcaSlicer themselves and chose not to bundle it (spec open question 1).

### Accept only JSON-bearing runtimes

This is option A in the spike: presets come only from the engine's own
`resources/profiles`. It is the simplest contract, but it blocks every
current nightly.

### Read OrcaSlicer's `.opc` preset cache

This is option C in the spike: it is exact for any runtime, but it depends
on an internal binary format about a month old that changes whenever
OrcaSlicer's cache version or value encoding changes. It stays possible
later, because options A and B need the same detection and resolver.

### Invoke OrcaSlicer on the source 3MF with its project settings

Slicing the user's own project file would keep Orca's plate metadata, but
it depends on the source printer's bed and on Orca's plate grid, and v2.4.2
refuses 3MFs from newer Orca builds without `--allow-newer-file`. A
farm3d-written plate 3MF with explicit build transforms slices at exactly
the placed positions (spike Gates C and D).

## Consequences

- farm3d's slicing works only when the user has installed a 2.x OrcaSlicer.
  The Slicer settings dialog shows which engine was chosen and why, and the
  preparation panel replaces Slice with the runtime state until one is
  found.
- A new OrcaSlicer major version is refused until farm3d decides to accept
  it. A 2.x release that renames a mapped key fails with
  `UNSUPPORTED_SETTING_FOR_RUNTIME` rather than slicing with a dropped
  setting.
- A cache-only nightly needs a second, JSON-bearing install as its preset
  source, and its revisions say that the presets came from another version.
- Every Slice Revision records the engine version, channel, and file hash,
  the preset-source version, the argument vector, and the flat preset
  hashes, so a result can be traced to the runtime that produced it.
- Restart recovery can stop a survivor only when its executable is
  `orca-slicer` or the configured engine's basename. A v2.4.2 AppImage runs
  briefly as a `/bin/sh` wrapper before it execs `orca-slicer`, and a
  survivor caught in that window is left running. PDEATHSIG is the primary
  guard.
- Progress, cancellation, and orphan protection are verified only on Linux.
