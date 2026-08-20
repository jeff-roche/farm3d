# Brand Identity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace farm3d's placeholder fonts, hand-rolled icons, and text-only wordmark with the settled brand identity — self-hosted Manrope/Fira Sans/Fira Code, `@tabler/icons-solidjs`, and the layer-stack-cube-with-sprout logo — so the print-farm screens implementation has real fonts and icons to build on.

**Architecture:** Font files are self-hosted static assets (`public/fonts/`) loaded via a new `@font-face` stylesheet; the typography tokens (`src/design-system/tokens/typography.ts`) just change which family/weight each role points at — no architectural change to the theming system. The icon mark becomes a `Logo` design-system component (hand-authored SVG, following the existing hand-rolled-inline-SVG pattern already used in `Chip.tsx`'s `RemoveIcon`). The Tauri app icon set and web favicon are generated from a standalone SVG source via the `tauri icon` CLI. The wordmark is a self-contained SVG asset (font embedded as base64) for use outside the running app (README, marketing).

**Tech Stack:** SolidJS, Vite, Vitest + `@solidjs/testing-library`, Tauri v2 CLI (`tauri icon`).

**Spec:** [docs/superpowers/specs/2026-08-19-brand-identity-plan.md](../specs/2026-08-19-brand-identity-plan.md)

## Global Constraints

- Font files are self-hosted (bundled, not a Google Fonts CDN link) — the app must render correctly offline.
- Never hardcode colors/font-sizes/radii in *component* CSS — reference `--f3d-color-*`/`--f3d-type-*`/`--f3d-radius-*` (per `AGENTS.md`). The `Logo` component's brand-mark fill colors are a deliberate, documented exception (see Task 3) — a logo is fixed brand artwork, not themed UI.
- New design-system components get added to `src/design-system/components/index.ts` and demoed in `Showcase.tsx` (per `AGENTS.md`).
- `cargo`/`rustc` are not on `PATH` in non-interactive shells — prefix any Tauri-CLI command that might invoke them with `source "$HOME/.cargo/env" &&`.
- Run `just build` and `just test` before considering any task done (per `AGENTS.md`).

---

## Task 1: Self-host the brand fonts

**Files:**
- Create: `public/fonts/Manrope-Variable.woff2`
- Create: `public/fonts/FiraSans-Regular.woff2`
- Create: `public/fonts/FiraSans-Medium.woff2`
- Create: `public/fonts/FiraCode-Variable.woff2`
- Create: `src/design-system/fonts.css`
- Create: `src/design-system/tokens/typography.test.ts`
- Modify: `src/design-system/tokens/typography.ts`
- Modify: `src/index.tsx`

**Interfaces:**
- Produces: `farm3dTypography.heading.fontFamily` containing `"Manrope"`, weight `600`; `.body`/`.bodySmall`/`.label.fontFamily` containing `"Fira Sans"`; `.mono.fontFamily` containing `"Fira Code"`. Sizes/line-heights/letter-spacing are unchanged from today. Task 5 depends on `public/fonts/FiraCode-Variable.woff2` existing.

- [ ] **Step 1: Download the four font files**

Manrope and Fira Code are variable fonts (Google serves one file covering their full weight range); Fira Sans is static per-weight. These are the exact latin-subset URLs Google Fonts currently serves:

```bash
mkdir -p public/fonts
curl -sL -o public/fonts/Manrope-Variable.woff2 \
  "https://fonts.gstatic.com/s/manrope/v20/xn7gYHE41ni1AdIRggexSg.woff2"
curl -sL -o public/fonts/FiraSans-Regular.woff2 \
  "https://fonts.gstatic.com/s/firasans/v18/va9E4kDNxMZdWfMOD5Vvl4jL.woff2"
curl -sL -o public/fonts/FiraSans-Medium.woff2 \
  "https://fonts.gstatic.com/s/firasans/v18/va9B4kDNxMZdWfMOD5VnZKveRhf6.woff2"
curl -sL -o public/fonts/FiraCode-Variable.woff2 \
  "https://fonts.gstatic.com/s/firacode/v27/uU9NCBsR6Z2vfE9aq3bh3dSD.woff2"
```

- [ ] **Step 2: Verify the downloads**

```bash
file public/fonts/*.woff2
```

Expected: all four report as `Web Open Font Format (Version 2)` data, each file larger than 0 bytes (Manrope/FiraCode variable files are ~20-30KB, Fira Sans statics are ~15-20KB).

- [ ] **Step 3: Write the `@font-face` stylesheet**

```css
/* src/design-system/fonts.css */
@font-face {
  font-family: "Manrope";
  src: url("/fonts/Manrope-Variable.woff2") format("woff2-variations");
  font-weight: 200 800;
  font-style: normal;
  font-display: swap;
}

@font-face {
  font-family: "Fira Sans";
  src: url("/fonts/FiraSans-Regular.woff2") format("woff2");
  font-weight: 400;
  font-style: normal;
  font-display: swap;
}

@font-face {
  font-family: "Fira Sans";
  src: url("/fonts/FiraSans-Medium.woff2") format("woff2");
  font-weight: 500;
  font-style: normal;
  font-display: swap;
}

@font-face {
  font-family: "Fira Code";
  src: url("/fonts/FiraCode-Variable.woff2") format("woff2-variations");
  font-weight: 300 700;
  font-style: normal;
  font-display: swap;
}
```

- [ ] **Step 4: Import the stylesheet once, at startup**

Modify `src/index.tsx` — add the import alongside the existing `styles.css` import:

```tsx
import { render } from "solid-js/web";
import App from "./App";
import { initTheme } from "./design-system";
import "./design-system/fonts.css";
import "./styles.css";
```

- [ ] **Step 5: Write the failing typography test**

```ts
// src/design-system/tokens/typography.test.ts
import { describe, expect, it } from "vitest";
import { farm3dTypography } from "./typography";

describe("farm3dTypography", () => {
  it("uses Manrope at weight 600 for heading", () => {
    expect(farm3dTypography.heading.fontFamily).toContain("Manrope");
    expect(farm3dTypography.heading.fontWeight).toBe(600);
  });

  it("uses Fira Sans for body, bodySmall, and label", () => {
    expect(farm3dTypography.body.fontFamily).toContain("Fira Sans");
    expect(farm3dTypography.body.fontWeight).toBe(400);
    expect(farm3dTypography.bodySmall.fontFamily).toContain("Fira Sans");
    expect(farm3dTypography.label.fontFamily).toContain("Fira Sans");
    expect(farm3dTypography.label.fontWeight).toBe(500);
  });

  it("uses Fira Code for mono", () => {
    expect(farm3dTypography.mono.fontFamily).toContain("Fira Code");
  });
});
```

- [ ] **Step 6: Run the test to verify it fails**

Run: `npx vitest run src/design-system/tokens/typography.test.ts`
Expected: FAIL — `farm3dTypography.heading.fontFamily` still contains `"Inter"`, not `"Manrope"`.

- [ ] **Step 7: Update the typography tokens**

```ts
// src/design-system/tokens/typography.ts
import type { TypographyScale, TypeStyle } from "./types";

const heading = "'Manrope', system-ui, sans-serif";
const body = "'Fira Sans', system-ui, sans-serif";
const mono = "'Fira Code', 'SF Mono', Menlo, Consolas, monospace";

function style(
  fontFamily: string,
  fontWeight: number,
  fontSize: string,
  lineHeight: string,
  letterSpacing: string,
): TypeStyle {
  return { fontFamily, fontWeight, fontSize, lineHeight, letterSpacing };
}

/** Dense, editor-style type scale — smaller sizes and tighter line-height than a consumer-app scale. */
export const farm3dTypography: TypographyScale = {
  heading: style(heading, 600, "0.8125rem", "1.25rem", "0.02em"),
  body: style(body, 400, "0.8125rem", "1.25rem", "0em"),
  bodySmall: style(body, 400, "0.75rem", "1.125rem", "0em"),
  label: style(body, 500, "0.6875rem", "1rem", "0.03em"),
  mono: style(mono, 400, "0.75rem", "1.125rem", "0em"),
};
```

- [ ] **Step 8: Run the test to verify it passes**

Run: `npx vitest run src/design-system/tokens/typography.test.ts`
Expected: PASS (3 tests).

- [ ] **Step 9: Run the full build and test suite**

Run: `just build && just test`
Expected: both succeed. `just build` type-checks and bundles; a bad font path in `fonts.css` won't fail the build (CSS `url()` isn't type-checked), so also run `just web`, open `http://localhost:1420`, and check the Network tab / DevTools computed styles to confirm the four fonts load with HTTP 200 (not 404) and body text visibly renders in Fira Sans rather than a fallback.

- [ ] **Step 10: Commit**

```bash
git add public/fonts src/design-system/fonts.css src/design-system/tokens/typography.ts src/design-system/tokens/typography.test.ts src/index.tsx
git commit -m "Self-host Manrope, Fira Sans, and Fira Code as the brand type scale"
```

---

## Task 2: Add the Tabler Icons dependency

**Files:**
- Modify: `package.json`
- Create: `src/design-system/tabler-icons.test.tsx`

**Interfaces:**
- Produces: `@tabler/icons-solidjs` importable as `import { IconName } from "@tabler/icons-solidjs"` — each icon a SolidJS component accepting standard SVG props (`size`, `color`, `stroke`, etc.). Nothing else in this plan consumes a specific icon yet (the screens implementation does); this task's job is proving the dependency actually renders in this project's Vite/Vitest/Solid setup.

- [ ] **Step 1: Install the dependency**

```bash
npm install @tabler/icons-solidjs
```

- [ ] **Step 2: Verify it landed in `package.json`**

```bash
grep tabler package.json
```

Expected: a line like `"@tabler/icons-solidjs": "^3.x.x"` under `dependencies`.

- [ ] **Step 3: Write the failing smoke test**

This is a permanent regression test, not throwaway — it guards the specific reason Tabler was chosen over the alternatives (Feather/Lucide/Iconoir): a real, tree-shakeable SolidJS package that actually renders in this stack.

```tsx
// src/design-system/tabler-icons.test.tsx
import { render } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import { IconPrinter } from "@tabler/icons-solidjs";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("@tabler/icons-solidjs", () => {
  it("renders an icon as an inline svg", () => {
    const { container } = render(() => <IconPrinter size={16} />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("width")).toBe("16");
  });
});
```

- [ ] **Step 4: Run the test to verify it fails**

Run: `npx vitest run src/design-system/tabler-icons.test.tsx`
Expected: FAIL before Step 1/2 are done (module not found). If Steps 1-2 are already done, this step instead confirms the test is meaningful by temporarily renaming the import to a nonexistent icon (e.g. `IconDoesNotExist`) and observing a build/type error, then reverting to `IconPrinter`.

- [ ] **Step 5: Run the test to verify it passes**

Run: `npx vitest run src/design-system/tabler-icons.test.tsx`
Expected: PASS.

- [ ] **Step 6: Run the full build and test suite**

Run: `just build && just test`
Expected: both succeed.

- [ ] **Step 7: Commit**

```bash
git add package.json package-lock.json src/design-system/tabler-icons.test.tsx
git commit -m "Add @tabler/icons-solidjs as the icon library"
```

---

## Task 3: Build the `Logo` icon-mark component

**Files:**
- Create: `src/design-system/components/Logo.tsx`
- Create: `src/design-system/assets/icon-mark.svg`
- Modify: `src/design-system/components/index.ts`
- Modify: `src/design-system/components/components.test.tsx`
- Modify: `src/design-system/Showcase.tsx`

**Interfaces:**
- Produces: `Logo(props: LogoProps)` where `LogoProps = { size?: number; class?: string; title?: string }`, default `size` 24. Renders the layer-stack-cube-with-sprout mark as an inline `<svg viewBox="0 0 100 100">`. Task 4 reuses this exact path/color data (via `icon-mark.svg`) as the mark layer inside the app-icon source.

- [ ] **Step 1: Write the standalone SVG asset**

This is the portable, tool-agnostic copy of the mark (for design handoff, docs, etc.) — the `Logo` component in Step 3 inlines the same paths directly rather than fetching this file at runtime.

```svg
<!-- src/design-system/assets/icon-mark.svg -->
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" role="img" aria-label="farm3d">
  <defs>
    <clipPath id="icon-mark-clip">
      <path d="M44.3,11.15 Q50,8 55.7,11.15 L82.3,25.85 Q88,29 88,35.3 L88,64.7 Q88,71 82.3,74.15 L55.7,88.85 Q50,92 44.3,88.85 L17.7,74.15 Q12,71 12,64.7 L12,35.3 Q12,29 17.7,25.85 Z"/>
    </clipPath>
  </defs>
  <g clip-path="url(#icon-mark-clip)">
    <polygon points="50,8 88,29 50,50 12,29" fill="#8fc46b"/>
    <polygon points="12,29 50,50 50,64 12,43" fill="#7cb867"/>
    <polygon points="12,43 50,64 50,78 12,57" fill="#6fa855"/>
    <polygon points="12,57 50,78 50,92 12,71" fill="#5f9349"/>
    <polygon points="88,29 50,50 50,64 88,43" fill="#578f43"/>
    <polygon points="88,43 50,64 50,78 88,57" fill="#4c7a3a"/>
    <polygon points="88,57 50,78 50,92 88,71" fill="#426b32"/>
  </g>
  <line x1="50" y1="31" x2="50" y2="13" stroke="#4c7a3a" stroke-width="4" stroke-linecap="round"/>
  <path d="M50,19 C36,18 22,9 27,1 C40,4 50,13 50,19 Z" fill="#4c7a3a"/>
  <path d="M50,19 C64,18 78,9 73,1 C60,4 50,13 50,19 Z" fill="#4c7a3a"/>
</svg>
```

- [ ] **Step 2: Write the failing component test**

Add to `src/design-system/components/components.test.tsx` (alongside the existing `describe` blocks, with a new import added to the top of the file next to the others):

```tsx
import { Logo } from "./Logo";
```

```tsx
describe("Logo", () => {
  it("defaults to a 24px svg", () => {
    const { container } = render(() => <Logo />);
    const svg = container.querySelector("svg");
    expect(svg?.getAttribute("width")).toBe("24");
    expect(svg?.getAttribute("height")).toBe("24");
  });

  it("respects the size prop", () => {
    const { container } = render(() => <Logo size={48} />);
    const svg = container.querySelector("svg");
    expect(svg?.getAttribute("width")).toBe("48");
  });

  it("gives each instance a unique clip-path id", () => {
    const { container } = render(() => (
      <>
        <Logo />
        <Logo />
      </>
    ));
    const ids = [...container.querySelectorAll("clipPath")].map((el) => el.id);
    expect(new Set(ids).size).toBe(2);
  });
});
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `npx vitest run src/design-system/components/components.test.tsx -t Logo`
Expected: FAIL — `./Logo` doesn't exist yet.

- [ ] **Step 4: Implement the `Logo` component**

Note on the hardcoded hex colors below: this is the one deliberate exception to "never hardcode colors" in `AGENTS.md` — the mark is fixed brand artwork (like a favicon), not a themed UI element, so it does not repaint with `--f3d-color-*` when the theme switches, the same way most apps' logos don't reskin with light/dark mode.

```tsx
// src/design-system/components/Logo.tsx
import { createUniqueId, type JSX } from "solid-js";

export interface LogoProps {
  /** Rendered width/height in px. Defaults to 24. */
  size?: number;
  class?: string;
  /** Accessible name. Omit for decorative use (e.g. next to visible text). */
  title?: string;
}

/** farm3d's icon mark: a layer-stack cube with a sprout, fixed brand colors (not theme tokens). */
export function Logo(props: LogoProps): JSX.Element {
  const size = () => props.size ?? 24;
  const clipId = `f3d-logo-clip-${createUniqueId()}`;

  return (
    <svg
      width={size()}
      height={size()}
      viewBox="0 0 100 100"
      class={props.class}
      role={props.title ? "img" : undefined}
      aria-label={props.title}
      aria-hidden={props.title ? undefined : "true"}
    >
      <defs>
        <clipPath id={clipId}>
          <path d="M44.3,11.15 Q50,8 55.7,11.15 L82.3,25.85 Q88,29 88,35.3 L88,64.7 Q88,71 82.3,74.15 L55.7,88.85 Q50,92 44.3,88.85 L17.7,74.15 Q12,71 12,64.7 L12,35.3 Q12,29 17.7,25.85 Z" />
        </clipPath>
      </defs>
      <g clip-path={`url(#${clipId})`}>
        <polygon points="50,8 88,29 50,50 12,29" fill="#8fc46b" />
        <polygon points="12,29 50,50 50,64 12,43" fill="#7cb867" />
        <polygon points="12,43 50,64 50,78 12,57" fill="#6fa855" />
        <polygon points="12,57 50,78 50,92 12,71" fill="#5f9349" />
        <polygon points="88,29 50,50 50,64 88,43" fill="#578f43" />
        <polygon points="88,43 50,64 50,78 88,57" fill="#4c7a3a" />
        <polygon points="88,57 50,78 50,92 88,71" fill="#426b32" />
      </g>
      <line x1="50" y1="31" x2="50" y2="13" stroke="#4c7a3a" stroke-width="4" stroke-linecap="round" />
      <path d="M50,19 C36,18 22,9 27,1 C40,4 50,13 50,19 Z" fill="#4c7a3a" />
      <path d="M50,19 C64,18 78,9 73,1 C60,4 50,13 50,19 Z" fill="#4c7a3a" />
    </svg>
  );
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `npx vitest run src/design-system/components/components.test.tsx -t Logo`
Expected: PASS (3 tests).

- [ ] **Step 6: Export it from the design system**

Modify `src/design-system/components/index.ts` — add, alphabetically-ish with the existing list:

```ts
export { Logo, type LogoProps } from "./Logo";
```

- [ ] **Step 7: Add it to the Showcase**

Modify `src/design-system/Showcase.tsx` — add `Logo` to the import list from `"."`, and add a panel (after the `Theme` panel is a reasonable spot):

```tsx
<Panel title="Logo">
  <div class={styles.row}>
    <Logo size={16} />
    <Logo size={24} />
    <Logo size={48} title="farm3d" />
  </div>
</Panel>
```

- [ ] **Step 8: Run the full build and test suite**

Run: `just build && just test`
Expected: both succeed.

- [ ] **Step 9: Visually confirm**

Run: `just web`, open `http://localhost:1420/#showcase`, confirm the Logo panel renders three cube-with-sprout marks at visibly increasing sizes, in green tones, with no console errors about duplicate SVG ids.

- [ ] **Step 10: Commit**

```bash
git add src/design-system/components/Logo.tsx src/design-system/assets/icon-mark.svg \
  src/design-system/components/index.ts src/design-system/components/components.test.tsx \
  src/design-system/Showcase.tsx
git commit -m "Add the Logo component (layer-stack cube + sprout icon mark)"
```

---

## Task 4: Generate the Tauri app icon set and web favicon

**Files:**
- Create: `public/app-icon.svg`
- Modify: `src-tauri/icons/*` (regenerated, not hand-edited)
- Modify: `index.html`

**Interfaces:**
- Consumes: the mark path/color data from Task 3 (`icon-mark.svg`), with a background rect added.
- Produces: `src-tauri/icons/` matching what `src-tauri/tauri.conf.json`'s `bundle.icon` list already expects (`32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.icns`, `icon.ico`) plus the full default set (`icon.png` and the Windows Store tile PNGs) that `tauri icon` always writes; `public/app-icon.svg` served at `/app-icon.svg` as the web favicon.

- [ ] **Step 1: Write the app-icon source SVG**

Same mark as `icon-mark.svg`, with an opaque background so it reads as a standalone app icon rather than floating on transparency — `tauri icon` and browsers handle platform-specific corner rounding/masking themselves from a full-bleed square source.

```svg
<!-- public/app-icon.svg -->
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" role="img" aria-label="farm3d">
  <rect x="0" y="0" width="100" height="100" fill="#17181a"/>
  <defs>
    <clipPath id="app-icon-clip">
      <path d="M44.3,11.15 Q50,8 55.7,11.15 L82.3,25.85 Q88,29 88,35.3 L88,64.7 Q88,71 82.3,74.15 L55.7,88.85 Q50,92 44.3,88.85 L17.7,74.15 Q12,71 12,64.7 L12,35.3 Q12,29 17.7,25.85 Z"/>
    </clipPath>
  </defs>
  <g clip-path="url(#app-icon-clip)">
    <polygon points="50,8 88,29 50,50 12,29" fill="#8fc46b"/>
    <polygon points="12,29 50,50 50,64 12,43" fill="#7cb867"/>
    <polygon points="12,43 50,64 50,78 12,57" fill="#6fa855"/>
    <polygon points="12,57 50,78 50,92 12,71" fill="#5f9349"/>
    <polygon points="88,29 50,50 50,64 88,43" fill="#578f43"/>
    <polygon points="88,43 50,64 50,78 88,57" fill="#4c7a3a"/>
    <polygon points="88,57 50,78 50,92 88,71" fill="#426b32"/>
  </g>
  <line x1="50" y1="31" x2="50" y2="13" stroke="#4c7a3a" stroke-width="4" stroke-linecap="round"/>
  <path d="M50,19 C36,18 22,9 27,1 C40,4 50,13 50,19 Z" fill="#4c7a3a"/>
  <path d="M50,19 C64,18 78,9 73,1 C60,4 50,13 50,19 Z" fill="#4c7a3a"/>
</svg>
```

- [ ] **Step 2: Generate the Tauri icon set**

`tauri icon` is a pure image-processing subcommand (no Rust compilation), but it still goes through the Tauri CLI, so keep the `cargo`-env prefix per `AGENTS.md` for safety:

```bash
source "$HOME/.cargo/env" && npx tauri icon public/app-icon.svg
```

Expected output: a summary listing the generated files, written to `src-tauri/icons/` (the directory next to `src-tauri/tauri.conf.json`, which is the tool's default).

- [ ] **Step 3: Verify the icons actually changed**

```bash
git status --short src-tauri/icons
```

Expected: every file under `src-tauri/icons/` shows as modified (`M`) — confirms the default Tauri scaffold icons were overwritten, not left in place.

```bash
file src-tauri/icons/32x32.png src-tauri/icons/icon.icns src-tauri/icons/icon.ico
```

Expected: `32x32.png` reports `PNG image data, 32 x 32`; the `.icns`/`.ico` report as their respective icon-container formats (not zero-byte).

- [ ] **Step 4: Wire up the web favicon and page title**

Modify `index.html`:

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <link rel="icon" type="image/svg+xml" href="/app-icon.svg" />
    <title>farm3d</title>
    <script type="module" src="/src/index.tsx" defer></script>
  </head>

  <body>
    <div id="root"></div>
  </body>
</html>
```

- [ ] **Step 5: Run the full build and test suite**

Run: `just build && just test`
Expected: both succeed.

- [ ] **Step 6: Visually confirm**

Run: `just web`, open `http://localhost:1420` in a browser, confirm the browser tab shows "farm3d" as the title and the cube-with-sprout mark as the favicon (not the default Vite/Tauri icon).

- [ ] **Step 7: Commit**

```bash
git add public/app-icon.svg src-tauri/icons index.html
git commit -m "Generate the Tauri app icon set and web favicon from the icon mark"
```

---

## Task 5: Produce the wordmark and lockup assets

**Files:**
- Create: `src/design-system/assets/wordmark.svg`
- Create: `src/design-system/assets/lockup.svg`

**Interfaces:**
- Consumes: `public/fonts/FiraCode-Variable.woff2` from Task 1 (embedded as base64, not linked — these assets need to be self-contained/portable for use outside the running app, e.g. a GitHub README, where the app's own font-loading setup isn't present) and the mark path/color data from Task 3's `icon-mark.svg`.

- [ ] **Step 1: Generate the self-contained wordmark SVG**

```bash
FONT_B64=$(base64 -w0 public/fonts/FiraCode-Variable.woff2)
cat > src/design-system/assets/wordmark.svg << SVGEOF
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 220 56" role="img" aria-label="farm3d">
  <defs>
    <style>
      @font-face {
        font-family: 'Fira Code';
        font-weight: 300 700;
        src: url(data:font/woff2;base64,${FONT_B64}) format('woff2-variations');
      }
      text {
        font-family: 'Fira Code', monospace;
        font-weight: 700;
        font-size: 40px;
      }
    </style>
  </defs>
  <text x="8" y="40" fill="#e8e6e0">farm<tspan fill="#8fc46b">3</tspan>d</text>
</svg>
SVGEOF
```

- [ ] **Step 2: Verify it's well-formed and self-contained**

```bash
xmllint --noout src/design-system/assets/wordmark.svg && echo "valid XML"
grep -c "url(data:font/woff2;base64," src/design-system/assets/wordmark.svg
```

Expected: `valid XML` printed, and the grep count is `1` (the font is embedded inline, not linked to `/fonts/...`).

If `xmllint` isn't available, substitute: `python3 -c "import xml.dom.minidom as m; m.parse('src/design-system/assets/wordmark.svg'); print('valid XML')"`.

- [ ] **Step 3: Visually confirm the rendered wordmark**

```bash
rsvg-convert -w 440 -h 112 src/design-system/assets/wordmark.svg -o /tmp/wordmark-check.png
```

Open `/tmp/wordmark-check.png` (e.g. via an image viewer, or `Read` it as an image) and confirm it reads "farm3d" in a bold monospace face, with the "3" in green and the rest in off-white, on a transparent background.

- [ ] **Step 4: Generate the combined icon + wordmark lockup**

Same self-contained approach, but the mark sits in its own nested `<svg>` (reusing its native `0 0 100 100` coordinate system at a fixed 56x56 slot) next to the wordmark text:

```bash
FONT_B64=$(base64 -w0 public/fonts/FiraCode-Variable.woff2)
cat > src/design-system/assets/lockup.svg << SVGEOF
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 260 56" role="img" aria-label="farm3d">
  <defs>
    <style>
      @font-face {
        font-family: 'Fira Code';
        font-weight: 300 700;
        src: url(data:font/woff2;base64,${FONT_B64}) format('woff2-variations');
      }
      text {
        font-family: 'Fira Code', monospace;
        font-weight: 700;
        font-size: 40px;
      }
    </style>
    <clipPath id="lockup-mark-clip">
      <path d="M44.3,11.15 Q50,8 55.7,11.15 L82.3,25.85 Q88,29 88,35.3 L88,64.7 Q88,71 82.3,74.15 L55.7,88.85 Q50,92 44.3,88.85 L17.7,74.15 Q12,71 12,64.7 L12,35.3 Q12,29 17.7,25.85 Z"/>
    </clipPath>
  </defs>
  <svg x="0" y="0" width="56" height="56" viewBox="0 0 100 100">
    <g clip-path="url(#lockup-mark-clip)">
      <polygon points="50,8 88,29 50,50 12,29" fill="#8fc46b"/>
      <polygon points="12,29 50,50 50,64 12,43" fill="#7cb867"/>
      <polygon points="12,43 50,64 50,78 12,57" fill="#6fa855"/>
      <polygon points="12,57 50,78 50,92 12,71" fill="#5f9349"/>
      <polygon points="88,29 50,50 50,64 88,43" fill="#578f43"/>
      <polygon points="88,43 50,64 50,78 88,57" fill="#4c7a3a"/>
      <polygon points="88,57 50,78 50,92 88,71" fill="#426b32"/>
    </g>
    <line x1="50" y1="31" x2="50" y2="13" stroke="#4c7a3a" stroke-width="4" stroke-linecap="round"/>
    <path d="M50,19 C36,18 22,9 27,1 C40,4 50,13 50,19 Z" fill="#4c7a3a"/>
    <path d="M50,19 C64,18 78,9 73,1 C60,4 50,13 50,19 Z" fill="#4c7a3a"/>
  </svg>
  <text x="72" y="40" fill="#e8e6e0">farm<tspan fill="#8fc46b">3</tspan>d</text>
</svg>
SVGEOF
```

- [ ] **Step 5: Verify and visually confirm the lockup**

```bash
xmllint --noout src/design-system/assets/lockup.svg && echo "valid XML"
rsvg-convert -w 520 -h 112 src/design-system/assets/lockup.svg -o /tmp/lockup-check.png
```

Open `/tmp/lockup-check.png` and confirm the cube-with-sprout mark sits to the left of the "farm3d" wordmark, vertically centered, with a sensible gap between them (no overlap, no excess whitespace).

- [ ] **Step 6: Commit**

```bash
git add src/design-system/assets/wordmark.svg src/design-system/assets/lockup.svg
git commit -m "Add the farm3d wordmark and icon+wordmark lockup assets"
```
