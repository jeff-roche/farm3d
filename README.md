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

## Slicing

farm3d slices with an OrcaSlicer you install yourself; it doesn't bundle
one. Any OrcaSlicer 2.x works, releases and nightly or dev builds alike.
farm3d looks for `orca-slicer` on your `PATH`, and on Linux for an
`*OrcaSlicer*.AppImage` in `~/Applications`, `~/.local/bin` or
`~/Downloads`. To point it at another install, open the **Settings** menu,
choose **Slicer...**, then **Choose engine…**.

Current nightly builds store their printer presets in a format farm3d can't
read. With a nightly, also choose an OrcaSlicer 2.4 AppImage (**Choose
preset source file…**) or install folder (**Choose preset source folder…**)
as the preset source, in the same dialog.

## Commands

Run `just` with no argument to list recipes. Each wraps the equivalent npm script (shown for reference):

| Command | npm equivalent | Description |
| --- | --- | --- |
| `just install` | `npm install` | Install frontend dependencies |
| `just install-rust` | — | Fetch Rust dependencies for the Tauri backend |
| `just dev` | `npm run tauri dev` | Run the full desktop app (Rust + frontend) with hot reload |
| `just web` | `npm run dev` | Run just the frontend in a browser at `localhost:1420` (no Tauri/Rust) |
| `just build` | `npm run build` | Type-check and build the frontend for production |
| `just package` | `NO_STRIP=1 npm run tauri build` (Linux) | Build clean distributable bundles, then reject any bundle that contains the developer-only catalog generator or the `fake-orca` test double; Linux disables linuxdeploy's legacy strip step because it cannot parse modern `.relr.dyn` sections |
| `just package-arch` | — | Repackage the `.deb` from `just package` into an Arch Linux `.pkg.tar.zst` (Tauri's bundler has no pacman target); requires `just package` to have run first — see [`packaging/arch/PKGBUILD`](./packaging/arch/PKGBUILD) |
| `just test` | `npm test` | Run the frontend test suite (Vitest) |
| `just test-rust` | — | Run the Tauri backend's Rust test suite; builds with `--features test-support`, which adds the `fake-orca` OrcaSlicer test double |
| `just test-orca` | — | Run the ignored `real_orca*` tests against a real OrcaSlicer; set `FARM3D_ORCA` to the engine (for example the v2.4.2 AppImage), and optionally `FARM3D_ORCA_PRESETS` to a preset source |
| `just gen-catalog` | — | Build the disabled-by-default developer generator and regenerate the bundled printer catalog from a pinned OrcaSlicer git tag |
| `just gen-contracts` | — | Regenerate committed TypeScript contracts from Rust wire types |
| `just gen-slicing-fixtures` | — | Regenerate the deterministic slicing fixtures (transform vectors, plate 3MF, argument vectors, flat presets) |

## Packaging

`just package` builds `.deb`, `.rpm` and AppImage bundles. On Arch Linux,
`just package-arch` repackages that `.deb` into a pacman package instead of
building one from scratch (see [`packaging/arch/PKGBUILD`](./packaging/arch/PKGBUILD)
for how). Install the result with:

```sh
sudo pacman -U packaging/arch/farm3d-bin-*.pkg.tar.zst
```

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
