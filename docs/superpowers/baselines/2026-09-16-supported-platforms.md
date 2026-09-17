# Supported Platforms

## Decision

No operating system is promoted to `Supported` by configuration intent alone.
The F0 run was performed only on a Linux x86_64 CachyOS development host. Its
automated suites and release-executable smoke launch passed, but the required
`just package` command failed while producing the AppImage. Linux x86_64 is
therefore `Candidate, unverified`, and F0 remains open.

`src-tauri/tauri.conf.json:25-35` sets bundle `"targets": "all"`. In Tauri this
means all bundle types available on the current build host; it does not mean all
operating systems or CPU architectures.

## Evidence Levels

| Level | Meaning | F0 use |
|---|---|---|
| Source/configuration | Code and configuration inspection establishes intended paths and target settings. | Available for the shared codebase; not runtime platform proof. |
| Automated tests | Type checking, frontend unit/component tests, Rust unit/integration tests, and mock-runtime IPC execute repeatable behavior. | Passed on the Linux x86_64 host; mock IPC is below real-WebView E2E. |
| Package build | The native packaging command completes and emits the expected host bundles. | Failed overall on Linux at AppImage `linuxdeploy`; `.deb` and `.rpm` completed first. |
| Packaged/installed launch | A packaged or release executable starts on the native platform; installed launch is stronger than a build-tree executable. | Build-tree release executable survived 10 seconds on Linux; no bundle was installed. |
| Live external-system exercise | A bounded run uses a representative authorized external service or device. | Not run; no authorized Moonraker endpoint was present. |

Evidence accumulates. A higher row does not erase a failed lower prerequisite,
and source/configuration or tests alone cannot establish a platform support
claim.

## Matrix

| Platform | v1 release status | F0 evidence | Required before platform claim |
|---|---|---|---|
| Linux x86_64 | Candidate, unverified | On CachyOS x86_64: clean frontend build; 130 frontend and 113 top-level Cargo tests passed. `just package` exited 1 after completing 6,383,488-byte `.deb`, 6,379,636-byte `.rpm`, and 18,825,208-byte release executable, because AppImage `linuxdeploy` could not strip host ELF `.relr.dyn` sections. With both display variables present, the release executable stayed alive for 10 seconds and was terminated normally by SIGTERM. No install or visual acceptance. | Clean build/test/package; packaged or installed executable launch; later feature-specific installed-bundle checks |
| Windows x86_64 | Candidate, unverified | Not run in F0 | Native clean build/test/package; installed launch; credential-store and later permission/notification/process checks |
| Windows arm64 | Candidate, unverified | Not run in F0 | Native clean build/test/package; installed launch; credential-store and later permission/notification/process checks |
| macOS x86_64 | Candidate, unverified | Not run in F0 | Native clean build/test/package; signed/notarized installation decision; launch; Keychain and later permission/notification/process checks |
| macOS arm64 | Candidate, unverified | Not run in F0 | Native clean build/test/package; signed/notarized installation decision; launch; Keychain and later permission/notification/process checks |

## Promotion Requirements

Linux x86_64 can become `Supported` only after a clean native rerun completes
`just build`, `just test`, `just test-rust`, and the full `just package`, followed
by a packaged or installed executable launch. The current build-tree smoke
launch is useful but cannot compensate for the failed package command. Later
phases must add feature-specific installed-bundle checks and representative
external-system evidence where those features depend on OS integration or
hardware.

Each Windows and macOS row requires native execution on that exact architecture.
Cross-platform Rust compilation, broad Tauri bundle configuration, or another
platform's test results are insufficient. Credential-store behavior and later
permission, notification, and process-management paths must be exercised on the
native OS. macOS additionally requires an explicit signing/notarization and
installation decision before release support is claimed.

## F0 Evidence

| Evidence | UTC start | Result | Artifact/log |
|---|---|---|---|
| Frontend build | 2026-09-17T12:17:37Z | Exit 0; TypeScript/Vite passed | `/tmp/farm3d-f0/just-build.log` |
| Frontend tests | 2026-09-17T12:17:47Z | Exit 0; 14 files, 130 tests passed | `/tmp/farm3d-f0/just-test.log` |
| Rust tests | 2026-09-17T12:18:06Z | Exit 0; 109 library + 1 IPC integration + 3 snapshot tests passed | `/tmp/farm3d-f0/just-test-rust.log` |
| Frontend-only server | 2026-09-17T12:18:54Z | Root and catalog resource returned HTTP 200; not Tauri evidence | `/tmp/farm3d-f0/just-web-result.log` |
| Full package command | 2026-09-17T12:19:00Z | Exit 1 at AppImage `linuxdeploy`; `.deb`, `.rpm`, and release executable had completed | `/tmp/farm3d-f0/just-package.log` |
| Verbose package diagnosis | 2026-09-17T12:20:42Z | Exit 1; downloaded strip tool rejected host `.relr.dyn` sections | `/tmp/farm3d-f0/package-verbose.log` |
| Release executable launch | 2026-09-17T12:21:52Z | Alive for 10 seconds with display available; then SIGTERM, wait status 143 | `/tmp/farm3d-f0/packaged-launch-result.log` |
| Live Moonraker | 2026-09-17T12:21:42Z | Not run; approved endpoint variable absent, no probe attempted | `/tmp/farm3d-f0/moonraker-presence.log` |

The baseline details, limitations, source citations, command inventory and
artifact sizes are recorded in
`docs/superpowers/baselines/2026-09-16-f0-baseline.md`.
