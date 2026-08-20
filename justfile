# Run `just` with no argument to list recipes.
default:
    @just --list

# Install frontend dependencies
install:
    npm install

# Fetch Rust dependencies for the Tauri backend
install-rust:
    cargo fetch --manifest-path src-tauri/Cargo.toml

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
    npm run tauri build

# Run the frontend test suite
test:
    npm test
