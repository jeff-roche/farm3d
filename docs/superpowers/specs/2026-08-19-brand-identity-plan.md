# Brand identity plan: type, icons, and logo

## Context

Per the [print farm screens design](./2026-08-19-print-farm-screens-design.md),
farm3d needed real font choices, an icon library, and a logo — none of
which existed:

- Typography tokens (`src/design-system/tokens/typography.ts`) were
  placeholder fallback stacks (`Inter, Avenir, Helvetica, Arial,
  sans-serif`) never deliberately chosen.
- Icons were hand-rolled inline SVG per component, no library dependency.
- The "logo" was a plain text wordmark with no mark and no real asset.

This plan settles all three ahead of implementing the screens spec, per
the agreed sequencing: brand identity lands first, since the screens spec
depends on real icons existing.

This is a **planning/asset-specification document**, not implementation.
Font files, the icon dependency, and rasterized logo assets (Tauri app
icons, favicon) still need to be produced/installed as a follow-up
implementation pass.

## Typography

- **heading**: Manrope, weight 600 (matches the existing dense editor
  type scale in `typography.ts` — size/line-height/letter-spacing
  unchanged, only the font family and this weight change).
- **body / bodySmall**: Fira Sans, weight 400.
- **label**: Fira Sans, weight 500.
- **mono**: Fira Code, weight 400 — **replaces JetBrains Mono**.

All four fonts are **self-hosted** (bundled font files via `@font-face`,
not a Google Fonts CDN link) — the app needs to render correctly offline,
which a live CDN dependency can't guarantee for a LAN-connected desktop
tool. Weights actually needed: Manrope 600 (UI) + 700 (logo/wordmark
display use, see below); Fira Sans 400/500; Fira Code 400 (UI) + 700
(wordmark display use).

Rationale: Manrope's rounded geometric character distinguishes headings;
Fira Sans (built for extended technical reading) carries the dense
body/label text; Fira Code was already the natural mono pairing once Fira
Sans was chosen, and reads as more "considered" than the arbitrary
JetBrains Mono default.

## Icon library

**`@tabler/icons-solidjs`** — official first-party Tabler Icons SolidJS
package, MIT, actively maintained, 4000+ icons at a consistent 2px
stroke on a 24px grid. Chosen over Feather (dormant since 2024), Lucide
(no dedicated Solid package, would need vendoring), and Iconoir (same
vendoring gap) specifically because it's the only candidate with a real,
maintained Solid package — less to maintain ourselves.

Icons render via tree-shakeable named imports:
`import { IconPrinter } from "@tabler/icons-solidjs"`.

## Logo

Two assets: an icon mark (used standalone as the app icon, favicon,
activity-bar branding) and a wordmark (used for headers/about
screens/marketing — not the compact in-app top bar, which stays a plain
Manrope 600 text label as the screens spec already defines).

### Icon mark — layer-stack cube with a sprout

A flat isometric cube built from print-layer bands (visual nod to
additive manufacturing), with a two-leaf sprout planted on the top face
(visual nod to "farm" — the app cultivates prints, layer by layer). Flat
color only, no gradients or shadows, consistent with DESIGN.md's
no-elevation rule. Outer silhouette corners are rounded via an SVG
`clipPath`, not hand-rounded per vertex — this keeps the three faces
meeting seamlessly while still softening the six outer points.

```
viewBox: 0 0 100 100

Rounded silhouette clip path:
M44.3,11.15 Q50,8 55.7,11.15 L82.3,25.85 Q88,29 88,35.3
L88,64.7 Q88,71 82.3,74.15 L55.7,88.85 Q50,92 44.3,88.85
L17.7,74.15 Q12,71 12,64.7 L12,35.3 Q12,29 17.7,25.85 Z

Top face:    50,8 88,29 50,50 12,29           fill #8fc46b
Left bands:  12,29 50,50 50,64 12,43          fill #7cb867
             12,43 50,64 50,78 12,57          fill #6fa855
             12,57 50,78 50,92 12,71          fill #5f9349
Right bands: 88,29 50,50 50,64 88,43          fill #578f43
             88,43 50,64 50,78 88,57          fill #4c7a3a
             88,57 50,78 50,92 88,71          fill #426b32

Sprout stem: line 50,31 to 50,13, stroke #4c7a3a, round cap
Left leaf:   M50,19 C36,18 22,9 27,1 C40,4 50,13 50,19 Z    fill #4c7a3a
Right leaf:  M50,19 C64,18 78,9 73,1 C60,4 50,13 50,19 Z    fill #4c7a3a
```

Stem/gap stroke widths need to scale *up* in raw units as physical
render size shrinks (a fixed unit width gets thinner in physical pixels
as the icon shrinks) — use width 4 at ~92px render, 6–7 at ~26–16px
render, so the mark stays legible from the full-size icon down to a
16px favicon.

### Wordmark — "farm3d" in Fira Code

`farm3d` set in Fira Code weight 700, lowercase, the digit `3` in accent
green (`#8fc46b`), rest in `text` (`#e8e6e0`). Distinct from the compact
Manrope top-bar label — this is for contexts where the wordmark stands
alone or pairs with the icon mark (README header, about screen, external
marketing), not embedded in dense UI chrome.

## Follow-up implementation work (not part of this plan)

- Update `typography.ts` with the new font families/weights; add
  self-hosted font files and `@font-face` declarations.
- Add `@tabler/icons-solidjs` to `package.json`.
- Rasterize the icon mark into the actual Tauri icon set
  (`src-tauri/icons/`: 32x32, 128x128, `.icns`, `.ico`, etc. — currently
  the default Tauri scaffold icons) and a favicon.
- Export the wordmark/lockup as reusable SVG assets for docs/README use.

Per the agreed sequencing, this work is implemented as its **own pass,
before** the screens spec implementation starts — the screens spec
depends on real fonts and the icon library existing, not the other way
around.
