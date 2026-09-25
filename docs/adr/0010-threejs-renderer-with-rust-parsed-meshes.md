# three.js renderer with Rust-parsed meshes

**Status:** Accepted. The user chose three.js on 2026-09-24, when approving
the P5 design.

## Context

P5 adds a real 3D viewport for preparing build plates (ADR-0006: the
viewport inspects Models, it is not a farm floor plan). The viewport must
show meshes up to about a million triangles, the build volume, and exclude
areas; pick objects; and stay usable without a pointer and without WebGL.
Model files are already parsed in Rust by P4's STL and 3MF readers, with
their safety limits, and the sliced plate must match the viewport exactly.

The P5 spike (Gate H) measured three.js r186 in WebKitGTK 2.52.6, the
webview Tauri uses on Linux. A 1 M-triangle indexed mesh drew its first
frame in 64 ms, then held a 17 ms median frame time while orbiting
(vsync-bound). Over a custom URI scheme, as Tauri IPC works, an 18 MB mesh
transferred in 42 ms and 50 MiB in 160 ms.

## Decision

**Rust owns geometry; the frontend only draws it.**

- `get_revision_geometry` returns JSON per Model Source Revision: objects
  with bounds, triangle counts, and up to 8 lay-flat faces from a
  farm3d-owned convex hull, plus the build items with their transforms and
  plates.
- `get_revision_mesh` returns one object's mesh as a binary
  `tauri::ipc::Response`: the magic `F3DM`, a version, the vertex and index
  counts, then `f32` positions and `u32` indices, little-endian. STL
  vertices are welded by bit pattern. The mesh is read through the P4
  readers, so the same parser and limits apply to import, the viewport, and
  the sliced plate.
- Geometry is computed off the async runtime and cached in memory (an LRU
  keyed by content hash).
- The D5 transform composition is implemented in Rust and reproduced by
  `src/slicing/transforms.ts`. Both check the same committed golden vectors
  (`src-tauri/tests/fixtures/slicing/transform-vectors.json`), so the
  viewport and the plate 3MF agree.

**three.js, plain, behind an interface.** `three` is pinned in
`package.json` (0.186.1). Only `src/slicing/viewport/three-renderer.ts`
imports it, behind the `ViewportRenderer` interface (mount, build volume,
meshes, instances, camera, overlays, pick, theme, dispose). It is loaded with
a dynamic `import()` on first use, so three.js stays out of the main bundle.
Instances share one indexed `BufferGeometry` per object. Colors come from
the `--f3d-color-*` tokens. Disposing the renderer forces a WebGL context
loss, because browsers cap how many contexts may be live.

**The viewport is never the only way in.** Every tool has a numeric field
and a keyboard command, and the canvas has a live text description. If
WebGL is unavailable, a text panel replaces the canvas and every numeric
tool still works.

**Tests use a fake renderer.** jsdom tests run against
`FakeViewportRenderer`, which records calls. The real renderer is checked by
screenshots and by hand.

## Options considered

### Parse meshes in the frontend

three.js ships STL and 3MF loaders. Using them would mean a second parser
with its own limits and component handling, and the viewport could
disagree with the plate farm3d writes for OrcaSlicer. Raw files can also be
up to 1 GiB, which the webview would have to load whole.

## Consequences

- The frontend never reads model files. Mesh transfer is binary and
  proportional to the mesh: about 18 MB for 1 M triangles.
- The performance budget is 1 M triangles at 60 fps on the development
  host. Above 2 M triangles per plate, the object list warns that the
  viewport may be slow.
- three.js is a lazily loaded chunk of about 560 kB, above Vite's default
  chunk warning, so `vite.config.ts` raises `chunkSizeWarningLimit` to
  600 kB.
- Changing the transform composition means regenerating the golden vectors
  with `just gen-slicing-fixtures` and matching them in both Rust and
  TypeScript.
- The WebGL renderer is verified only in WebKitGTK on Linux. WebView2 and
  WKWebView are untested, like the rest of Windows and macOS.
