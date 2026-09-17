# farm3d

A Tauri (Rust) + SolidJS (TypeScript) desktop app for managing a 3D-printer print farm — connecting to printers, slicing models, and organizing a model library.

## Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (stable, via `rustup`)
- Node.js + npm
- [`just`](https://github.com/casey/just) (a command runner — recipes below)
- Linux system deps for Tauri (Arch/CachyOS): `webkit2gtk-4.1 base-devel curl wget file openssl appmenu-gtk-module gtk3 libappindicator-gtk3 librsvg patchelf`. See [Tauri's prerequisites guide](https://tauri.app/start/prerequisites/) for other platforms.

## Getting started

```sh
just install
just dev   # launches the app with hot reload
```

## Commands

Run `just` with no argument to list recipes. Each wraps the equivalent npm script (shown for reference):

| Command | npm equivalent | Description |
| --- | --- | --- |
| `just install` | `npm install` | Install frontend dependencies |
| `just install-rust` | — | Fetch Rust dependencies for the Tauri backend |
| `just dev` | `npm run tauri dev` | Run the full desktop app (Rust + frontend) with hot reload |
| `just web` | `npm run dev` | Run just the frontend in a browser at `localhost:1420` (no Tauri/Rust) |
| `just build` | `npm run build` | Type-check and build the frontend for production |
| `just package` | `NO_STRIP=1 npm run tauri build` (Linux) | Build the distributable desktop app; Linux disables linuxdeploy's legacy strip step because it cannot parse modern `.relr.dyn` sections |
| `just test` | `npm test` | Run the frontend test suite (Vitest) |
| `just test-rust` | — | Run the Tauri backend's Rust test suite |
| `just gen-catalog` | — | Regenerate the bundled printer catalog from a pinned OrcaSlicer git tag |

## Project structure

```
src/                    Frontend (SolidJS + TypeScript)
  App.tsx               Root component
  design-system/        Theming + component library — see DESIGN.md
src-tauri/               Rust backend (Tauri)
  src/lib.rs             Tauri commands exposed to the frontend
```

## Design system

The UI is built on a custom pluggable theming system and a small component
library backed by [Kobalte](https://kobalte.dev/). See
[DESIGN.md](./DESIGN.md) for the architecture, and visit `/#showcase` in dev
mode (`just web`, then open `http://localhost:1420/#showcase`) to see every
component and its states.

Fonts in `public/fonts/` are licensed under the SIL Open Font License 1.1;
see the accompanying `OFL-*.txt` notices in that directory.

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md).

## Recommended IDE setup

[VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
