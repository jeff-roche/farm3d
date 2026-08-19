# Contributing

## Setup

See [README.md](./README.md#prerequisites) for prerequisites, then:

```sh
npm install
npm run tauri dev
```

## Before opening a PR

```sh
npm run build   # type-checks and builds the frontend
npm test        # runs the Vitest suite
```

Both must pass. If you're on Linux and have a display available, also run
`npm run tauri dev` and manually confirm the app still launches and behaves
as expected — `npm run build`/`npm test` verify correctness, not that the
app actually renders right.

## Working on the design system

If you touch `src/design-system/`, read [DESIGN.md](./DESIGN.md) first — it
explains the token architecture, the theme engine, and why components are
built the way they are (Kobalte for behavior, CSS Modules against `--f3d-*`
tokens for styling).

- **New component**: add `ComponentName.tsx` + `ComponentName.module.css`
  under `src/design-system/components/`, export it from
  `components/index.ts`, and add it to `Showcase.tsx` so it's visible for
  visual QA at `/#showcase`. Prefer wrapping a Kobalte primitive over
  hand-rolling behavior/ARIA — check `node_modules/@kobalte/core/src/` for
  what's available before building something from scratch.
- **New/changed color, typography, or shape role**: edit
  `src/design-system/tokens/types.ts` and both
  `src/design-system/themes/{light,dark}.ts` together — the `Theme`
  interface and the two built-in themes must stay in sync, or `tsc` will
  fail on the theme files.
- **CSS**: always reference the `--f3d-*` custom properties (never
  hardcoded colors/sizes) so theme switching actually repaints the
  component. Reuse `components/shared.module.css` (`composes: x from
  "./shared.module.css"`) for cross-component basics like the focus ring.

## Code style

- No comments explaining *what* code does — names should already make that
  clear. A comment is only worth adding for a non-obvious *why* (a hidden
  constraint, a workaround, a surprising invariant).
- Don't add abstractions, config options, or error handling for scenarios
  that can't currently happen. Keep changes scoped to what the task needs.
- Match existing patterns in the file/directory you're editing rather than
  introducing a new convention.

## Commits

Commit messages should explain *why* a change was made, not just *what*
changed — the diff already shows the what.
