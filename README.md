# farm3d

A 3D farm simulation/tool built on [Tauri](https://tauri.app) (Rust) with a [SolidJS](https://www.solidjs.com/) + TypeScript frontend.

## Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (stable, via `rustup`)
- Node.js + npm
- Linux system deps for Tauri (Arch/CachyOS): `webkit2gtk-4.1 base-devel curl wget file openssl appmenu-gtk-module gtk3 libappindicator-gtk3 librsvg patchelf`. See [Tauri's prerequisites guide](https://tauri.app/start/prerequisites/) for other platforms.

## Getting started

```sh
npm install
npm run tauri dev   # launches the app with hot reload
```

## Scripts

| Command | Description |
| --- | --- |
| `npm run tauri dev` | Run the full desktop app (Rust + frontend) with hot reload |
| `npm run dev` | Run just the frontend in a browser at `localhost:1420` (no Tauri/Rust) |
| `npm run build` | Type-check and build the frontend for production |
| `npm run tauri build` | Build the distributable desktop app |
| `npm test` | Run the frontend test suite (Vitest) |

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
mode (`npm run dev`, then open `http://localhost:1420/#showcase`) to see
every component and its states.

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md).

## Recommended IDE setup

[VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
