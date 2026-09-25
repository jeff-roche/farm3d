# Run `just` with no argument to list recipes.
default:
    @just --list

# Install frontend dependencies
install:
    npm install

# Fetch Rust dependencies for the Tauri backend
install-rust:
    cargo fetch --manifest-path src-tauri/Cargo.toml

# Run the Tauri backend's Rust test suite (with the fake-orca test double)
test-rust:
    cargo test --manifest-path src-tauri/Cargo.toml --features test-support

# Run the ignored real-OrcaSlicer tests; FARM3D_ORCA names the engine (e.g. the v2.4.2 AppImage), FARM3D_ORCA_PRESETS optionally a preset source
test-orca:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -z "${FARM3D_ORCA:-}" ]; then
        echo "error: set FARM3D_ORCA to an OrcaSlicer engine, e.g. FARM3D_ORCA=~/Downloads/OrcaSlicer_Linux_AppImage_Ubuntu2404_V2.4.2.AppImage just test-orca" >&2
        exit 1
    fi
    # One at a time: the cancel twin counts AppImage mounts.
    cargo test --manifest-path src-tauri/Cargo.toml --features test-support \
        --test p5_runtime_presets --test p5_geometry --test p5_process --test p5_publish --test p5_slicing --test p5_tracer \
        real_orca -- --ignored --test-threads=1

# Run the ignored A0.1 Moonraker live checks: probe (read-only), watch (read-only), or drive (sends M112/FIRMWARE_RESTART). FARM3D_MOONRAKER_HOST names the instance
moonraker-live mode="probe":
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -z "${FARM3D_MOONRAKER_HOST:-}" ]; then
        echo "error: set FARM3D_MOONRAKER_HOST, e.g. FARM3D_MOONRAKER_HOST=192.0.2.10 just moonraker-live probe" >&2
        exit 1
    fi
    case "{{ mode }}" in
        probe) test=live_probe ;;
        watch) test=live_watch ;;
        drive) test=live_lifecycle_drive ;;
        *) echo "error: mode must be probe, watch, or drive" >&2; exit 2 ;;
    esac
    cargo test --manifest-path src-tauri/Cargo.toml --test a0_moonraker_live "$test" \
        -- --ignored --exact --nocapture

# Manage the local Klipper + Moonraker simulator (build, up [trusted|apikey], down, status, restart klipper, ...)
moonraker-sim *args:
    scripts/moonraker-sim/sim.sh {{ args }}

# Run the ignored live OctoPrint checks; FARM3D_OCTOPRINT_HOST (required), FARM3D_OCTOPRINT_PORT, FARM3D_OCTOPRINT_API_KEY, FARM3D_OCTOPRINT_POLL_SECONDS
test-octoprint-live:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -z "${FARM3D_OCTOPRINT_HOST:-}" ]; then
        echo "error: set FARM3D_OCTOPRINT_HOST (and FARM3D_OCTOPRINT_API_KEY unless access control is off), e.g. FARM3D_OCTOPRINT_HOST=octopi.local just test-octoprint-live" >&2
        exit 1
    fi
    cargo test --manifest-path src-tauri/Cargo.toml --test a0_octoprint_live -- --ignored --nocapture --test-threads=1

# Type-check the backend for Windows from Linux (no mingw needed; nothing is linked). `just check-windows clippy` lints instead
check-windows mode="check":
    scripts/check-windows.sh {{mode}}

# Regenerate TypeScript contracts from the Rust wire types
gen-contracts:
    cargo test --locked --manifest-path src-tauri/Cargo.toml --test export_contracts regenerate_contracts -- --ignored --exact

# Regenerate the generated Library format fixtures (never the *.expected.json oracles)
gen-library-fixtures:
    cargo test --manifest-path src-tauri/Cargo.toml --test library_fixtures regenerate_library_fixtures -- --ignored --exact

# Regenerate the deterministic slicing fixtures (transform vectors, plate 3MF, argument vectors, flat presets)
gen-slicing-fixtures:
    cargo test --manifest-path src-tauri/Cargo.toml --test p5_geometry regenerate_slicing_fixtures -- --ignored --exact

# Regenerate the bundled printer catalog from a pinned OrcaSlicer git tag
gen-catalog tag="v2.4.2":
    cargo run --manifest-path src-tauri/Cargo.toml --features catalog-generator --bin gen-catalog -- {{ tag }}

# Run the full desktop app (Rust + frontend) with hot reload
dev:
    npm run tauri dev

# Run just the frontend in a browser at localhost:1420 (no Tauri/Rust)
web:
    npm run dev

# Type-check and build the frontend for production
build:
    npm run build

# Build the distributable desktop app
package:
    # linuxdeploy's bundled strip cannot parse modern ELF sections such as .relr.dyn.
    rm -rf src-tauri/target/release/bundle
    NO_STRIP=1 npm run tauri build
    scripts/assert-package-contents.sh

# Build an Arch Linux package (.pkg.tar.zst) by repackaging the .deb from
# `just package` — it does not run `just package` itself, since that's a
# slow full app rebuild and this recipe would otherwise trigger it as a
# surprising side effect every time; it fails with a clear message instead.
package-arch:
    #!/usr/bin/env bash
    set -euo pipefail
    deb=$(ls src-tauri/target/release/bundle/deb/*.deb 2>/dev/null | head -n1 || true)
    if [ -z "${deb:-}" ]; then
        echo "error: no .deb in src-tauri/target/release/bundle/deb/ — run 'just package' first (it's slow, so this recipe won't run it for you)" >&2
        exit 1
    fi
    cp "$deb" packaging/arch/
    cd packaging/arch
    updpkgsums
    makepkg -f

# Run the frontend test suite
test:
    npm test

# Start the printer simulators (Klipper+Moonraker, OctoPrint, fault proxy) and wait until ready; see sim/README.md
sim-up:
    sim/simctl up

# Stop the printer simulators and delete their state
sim-down:
    sim/simctl down

# Show the printer simulators' containers and readiness
sim-status:
    sim/simctl status

# Pass-through to sim/simctl, e.g. `just sim fault klipper-shutdown` or `just sim manifest`
sim *args:
    sim/simctl {{ args }}

# Run the simulator-backed adapter tests; skips with a message if `just sim-up` has not run. Records a manifest and log under src-tauri/target/sim-runs/
test-sim:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! sim/simctl status >/dev/null 2>&1; then
        if [ "${FARM3D_SIM_REQUIRED:-}" = 1 ]; then
            echo "error: FARM3D_SIM_REQUIRED=1 but the simulators are not ready; run 'just sim-up'" >&2
            exit 1
        fi
        echo "test-sim: SKIPPED: the simulators are not running. Start them with 'just sim-up', then rerun 'just test-sim'." >&2
        exit 0
    fi
    eval "$(sim/simctl env)"
    out="src-tauri/target/sim-runs/$(date -u +%Y%m%dT%H%M%SZ)"
    mkdir -p "$out"
    # ADR-0012: every simulator run records exactly what it ran against.
    sim/simctl manifest >"$out/manifest.json"
    echo "test-sim: recording to $out"
    cargo test --manifest-path src-tauri/Cargo.toml \
        --test sim_moonraker --test sim_octoprint --test sim_elegoolink \
        -- --include-ignored --test-threads=1 --nocapture 2>&1 | tee "$out/test.log"

# Fail if a tracked or staged file names one of the owner's private hosts (listed in FARM3D_PRIVATE_HOSTS or an untracked .private-hosts file)
check-hosts:
    scripts/check-private-hosts.sh
