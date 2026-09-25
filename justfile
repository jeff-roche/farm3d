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
        --test p5_runtime_presets --test p5_geometry --test p5_process --test p5_publish --test p5_slicing \
        real_orca -- --ignored --test-threads=1

# Regenerate TypeScript contracts from the Rust wire types
gen-contracts:
    cargo test --locked --manifest-path src-tauri/Cargo.toml --test export_contracts regenerate_contracts -- --ignored --exact

# Regenerate the generated Library format fixtures (never the *.expected.json oracles)
gen-library-fixtures:
    cargo test --manifest-path src-tauri/Cargo.toml --test library_fixtures regenerate_library_fixtures -- --ignored --exact

# Regenerate the deterministic slicing fixtures (transform vectors, plate 3MF)
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
