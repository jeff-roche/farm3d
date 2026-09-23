# The printer catalog is derived factual data from OrcaSlicer profiles

farm3d ships a printer catalog (`public/catalog/printer-catalog.json`)
generated from OrcaSlicer's `resources/profiles` at a pinned git tag
(`OrcaSlicer/OrcaSlicer`, AGPL-3.0-or-later — see the correction to ADR-0003).
The generator (`src-tauri/src/bin/gen-catalog.rs`) allowlists exactly the
factual capability fields farm3d reads — build volume, nozzle diameters,
capability flags, suggested host type — and never copies g-code templates,
bed/hotend model filenames, or vendor descriptive text. Facts aren't
copyrightable and this extraction is mechanical and exhaustive, not creative,
so the catalog's own license note ships as data alongside the snapshot rather
than farm3d inheriting AGPL obligations for a value it derives, not copies.

This means: adding a new field to the catalog must add it to the generator's
explicit allowlist (`extract_variant` in `catalog/ingest/mod.rs`), never via a
blanket copy of the resolved preset object. `src-tauri/tests/snapshot.rs`
enforces this mechanically by scanning the committed snapshot for known
disallowed field markers.
