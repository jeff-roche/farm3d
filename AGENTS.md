# AGENTS.md

Guidance for AI coding agents working in this repo. See also
[README.md](./README.md) (setup/scripts) and [DESIGN.md](./DESIGN.md)
(design system architecture) — read DESIGN.md before touching anything
under `src/design-system/`.

## What this is

A Tauri (Rust) + SolidJS (TypeScript) desktop app for 3D farm simulation.
Frontend in `src/`, Rust backend in `src-tauri/`.

## Commands

Prefer the `justfile` recipes (`just --list` for the full set); they're
thin wrappers over the npm scripts, shown here for reference:

```sh
just build   # (npm run build) tsc typecheck + vite build — run before considering frontend work done
just test    # (npm test) vitest run — run before considering frontend work done
just dev     # (npm run tauri dev) full app with hot reload (needs a display)
just web     # (npm run dev) frontend only, browser at localhost:1420 (no Tauri/Rust)
```

`cargo`/`rustc` are installed via rustup but **not on `PATH` in
non-interactive shells** (`~/.bashrc` only sources `~/.cargo/env` when
`$-` contains `i`) — this affects `just dev`/`just package` too, since they
invoke `cargo` transitively via the Tauri CLI. Prefix Rust-related commands
with `source "$HOME/.cargo/env" &&` when running them via a non-interactive
shell/tool.

## Conventions

- **Never hardcode colors, font sizes, or radii in component CSS.**
  Reference the `--f3d-color-*` / `--f3d-type-*` / `--f3d-radius-*` custom
  properties (see DESIGN.md). Hardcoding breaks theme switching.
- **Prefer a Kobalte primitive over hand-rolled behavior** for anything
  interactive (dialogs, menus, form controls) — check
  `node_modules/@kobalte/core/src/<name>/` for the actual prop names/data
  attributes before wiring one up; don't guess.
- **CSS Modules**, not global CSS, for component styles
  (`Component.module.css` next to `Component.tsx`). `styles.css` is only
  for the html/body-level base reset.
- New design-system components: add to `components/index.ts` and to
  `Showcase.tsx` (visible at `/#showcase` in `just web`).
- New npm script that a human/agent would run directly: add a matching
  `just` recipe too.
- Component tests use `@solidjs/testing-library`. Kobalte's `Select` and
  `DropdownMenu` triggers open on **`pointerdown`** (not `click`), and menu
  item selection fires on **`pointerup`** — use `fireEvent.pointerDown`/
  `fireEvent.pointerUp` in tests for those, not `fireEvent.click`. jsdom
  also lacks `ResizeObserver`; it's polyfilled in `vitest.setup.ts`.
- This is an editor/tool aesthetic (Blender/Godot/Unity-inspired), not
  Material Design — dense, flat, small radii, no elevation/ripple. Don't
  reintroduce M3 patterns (shadows-as-elevation, large touch targets,
  ripple animation) without an explicit request to do so.

## Before claiming frontend work is done

Run `just build` and `just test`; both must pass. If a display is
available, also launch `just dev` and visually confirm — a passing
build/test suite does not guarantee the UI actually renders correctly.
