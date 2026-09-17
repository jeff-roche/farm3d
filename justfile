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

# Run the frontend test suite
test:
    npm test
