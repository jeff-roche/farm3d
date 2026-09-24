# Run `just` with no argument to list recipes.
default:
    @just --list

# Install frontend dependencies
install:
    npm install

# Fetch Rust dependencies for the Tauri backend
install-rust:
    cargo fetch --manifest-path src-tauri/Cargo.toml

# Run the Tauri backend's Rust test suite
test-rust:
    cargo test --manifest-path src-tauri/Cargo.toml

# Regenerate TypeScript contracts from the Rust wire types
gen-contracts:
    cargo test --locked --manifest-path src-tauri/Cargo.toml --test export_contracts regenerate_contracts -- --ignored --exact

# Regenerate the generated Library format fixtures (never the *.expected.json oracles)
gen-library-fixtures:
    cargo test --manifest-path src-tauri/Cargo.toml --test library_fixtures regenerate_library_fixtures -- --ignored --exact

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
