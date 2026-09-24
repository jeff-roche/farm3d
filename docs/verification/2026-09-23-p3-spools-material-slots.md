# P3 Spools and Material Slots verification

**Date:** 2026-09-23
**Platform:** Linux
**Validated source:** `bf86002` (`docs: correct stale P3 doc comments`) on
`feature/p3-spools-material-slots`. The commit that adds this revision of
the document changes only this file. The automated evidence below was
re-run at `bf86002`, after the final whole-branch review fixes:

- `ff16953` fix: dock Spool detail inline on wide windows (the dock-mode
  bug from the browser pass, below)
- `5f1ca30` fix: send a zero tare for scale entries without one
- `47a49e1` fix: refresh Spool history after changes
- `99ee59f` fix: validate reservation amounts and tare ids
- `30da68f` fix: count the DataTable header row and tokenize the sort glyph
- `f71fd92` fix: guard printer records and tidy dialogs
- `bf86002` docs: correct stale P3 doc comments

The browser pass and its screenshots were taken earlier, before those
fixes. "Visual and keyboard verification" notes where the UI has changed
since.
Covers Tasks 1–12 (P3 in full).

## Automated evidence

| Command | Result |
| --- | --- |
| `just build` | Passed: TypeScript type check and Vite production build completed successfully (`dist/index.html`, `index-*.css`, `event-*.js`, `index-*.js`). |
| `just test` | Passed: 49 files, 511 tests. The runner emitted the same pre-existing jsdom `Window.scrollTo()` notices as P2; exited 0. |
| `source "$HOME/.cargo/env" && just test-rust` | Passed: 285 library tests (1 ignored); 28 export-contract tests (1 ignored); plus 5 `f0_tauri_path`, 3 `f1_contract_path`, 12 `f1_import_export`, 5 `f1_migration`, 7 `f1_repositories`, 12 `f1_residual_acceptance`, 15 `p2_batch`, 14 `p2_contract_path`, 10 `p2_lifecycle`, 6 `p2_migration`, 1 `p2_tracer`, 14 `p3_contract_path`, 11 `p3_ledger`, 17 `p3_lifecycle`, 8 `p3_migration`, 11 `p3_movement`, 10 `p3_reservations`, 12 `p3_setup`, **1 `p3_tracer`**, and 3 `snapshot` tests. That is 177 tests across the other integration files, and 490 passing tests in all. Two existing `ts-rs` transparent/`double_option`-serde-attribute warnings were emitted, as before. |
| `source "$HOME/.cargo/env" && cargo fmt --manifest-path src-tauri/Cargo.toml --check` | Passed (exit 0). |
| `source "$HOME/.cargo/env" && just gen-contracts` then `git diff --exit-code src/generated` | Passed: regeneration ran clean and the diff against the committed `src/generated` tree was empty (exit 0). The review fixes changed no wire types. `ReservationError::InvalidAmount` is Rust-only. |

### The tracer (spec acceptance criterion 14)

`src-tauri/tests/p3_tracer.rs`,
`the_tracer_loads_swaps_relocates_measures_and_survives_a_restart`, drives
the real `tauri::test` IPC path in one test:

1. `create_printer` for Printer X with no `slotLayout` → a Profile-only
   Printer with exactly one Material Slot, named `Main`.
2. `create_spool` for S1: 1 kg, estimated, in storage `"Shelf"`.
3. `move_spool` loads S1 into X's `Main` slot (`expectedOccupantSpoolId:
   null`).
4. `create_spool` for S2 (in storage, no label), then `move_spool` loads it
   into `Main` with `expectedOccupantSpoolId: <S1>` and
   `displacedStorageLabel: "Shelf"` — the swap. The result's `spools` array
   is `[S2 (now in Main), S1 (now in storage "Shelf")]`, matching D6, and its
   `movements` are `["displaced", "load"]` in write order.
5. `move_spool` relocates S2 (currently loaded) to storage `"Dry box"` — a
   `move_spool` reason of `"unload"` (D6: slot → storage is `unload`).
6. `record_spool_amount` on S1 with a scale entry (`grossMg: 750_000,
   tareMg: 50_000`) → `net = 700_000`, confidence flips from `estimated` to
   `measured`.
7. **Restart**: a brand-new `RuntimeServices`/IPC app is rebuilt over the
   *same* `Storage` Arc (mirrors `p2_tracer.rs`'s restart, which rebuilds
   `ConnectionManager`/`CredentialStore` rather than physically reopening the
   on-disk database file — Printer X here is Profile-only, so there is no
   Connection/credential state to carry across the rebuild at all).
8. After restart, `spool_history` for S1 and S2, and `list_spools`:
   - S1: `amountEvents` kinds `["initial", "measurement"]`, `confidenceAfter`
     `["estimated", "measured"]`, the `measurement` row's `afterMg = 700_000`
     with `note: "kitchen scale"`. `movements` are `["load", "displaced"]`
     (loaded into `Main`, then displaced back to storage by the swap).
     `list_spools` shows S1 in storage `"Shelf"`, confidence `measured`,
     `currentMg = 700_000`.
   - S2: `amountEvents` kinds `["initial"]`, confidence `["estimated"]`.
     `movements` are `["load", "unload"]` (loaded into `Main` by the swap,
     then unloaded to storage). `list_spools` shows S2 in storage
     `"Dry box"`.

All of the above assertions pass against the real Tauri command path (no
direct repository calls in the test body except through `common::runtime`'s
setup helpers).

### Acceptance criteria 1–15, mapped to automated evidence

| # | Criterion | Evidence |
| --- | --- | --- |
| 1 | Migration v3→v4 applies, is ledgered, survives crash-boundary injection, backfills one `Main` slot per existing Printer (including archived ones) | `p3_migration.rs`: `upgrading_v3_reaches_v4_and_backfills_one_main_slot_per_printer`, `a_crash_before_commit_leaves_the_database_byte_identical_at_v3` |
| 2 | The DB rejects a Spool in two slots and two Spools in one slot, even with the Rust check bypassed | `p3_migration.rs`: `two_spools_cannot_share_one_slot`, `an_archived_spool_cannot_keep_a_slot`, `a_spool_cannot_have_both_a_slot_and_a_storage_label` |
| 3 | Swap into an occupied slot writes displaced+loaded movements under one `operationId` in one transaction; an injected failure between them leaves both Spools where they were | `p3_movement.rs`: `swapping_into_an_occupied_slot_displaces_the_occupant_to_storage_first`, `a_failure_after_displacement_rolls_back_the_whole_swap`; `p3_tracer.rs` (the swap step) |
| 4 | Two concurrent `move_spool` calls into the same empty slot: exactly one succeeds, the other gets `CONFLICT` naming the winner | `p3_movement.rs`: `two_concurrent_loads_into_one_empty_slot_have_exactly_one_winner` |
| 5 | Replaying a `move_spool` `operationId` writes nothing new and returns current state | `p3_movement.rs`: `replaying_an_operation_id_returns_the_recorded_outcome_and_writes_nothing`; `p3_contract_path.rs`: `move_into_an_occupied_slot_emits_both_spools_the_printer_and_one_broadcast` (replay assertion at the end), `reusing_an_operation_id_for_a_different_spool_is_a_validation_error` |
| 6 | After restart, a Spool's amount ledger (initial, estimate, measurement correction) and movement history are intact and ordered; cached `current_mg`/`confidence` equal the last ledger row | `p3_ledger.rs`: `ledger_history_and_the_cache_survive_a_restart`; `p3_tracer.rs` (restart step) |
| 7 | Ledger rows cannot be updated or deleted; a tare edit after a measurement doesn't change that measurement's snapshot | `p3_migration.rs`: `spool_amount_events_reject_update_and_delete_as_append_only`; `p3_ledger.rs`: `scale_entry_snapshots_gross_and_tare_without_ever_changing_the_spools_default_tare` |
| 8 | Reservation primitives: over-`availableMg` reserve fails; `release` restores availability; `consume` writes one `consumption` row and clamps at 0; `unresolved` stays unavailable; a reserved Spool can't be archived/marked empty; the availability broadcast fires after commit only | `p3_reservations.rs` (all 10 tests): `reserving_tracks_available_mg_and_rejects_amounts_over_it`, `releasing_frees_the_amount_and_cannot_be_repeated`, `consuming_writes_one_estimated_consumption_event_carrying_the_reservation_id`, `consuming_more_than_the_current_amount_clamps_to_zero_and_notes_the_shortfall`, `marking_unresolved_keeps_the_amount_counted_against_availability`, `a_measurement_below_the_reserved_total_makes_availability_negative_and_blocks_further_reserves`, `reserving_on_an_empty_or_archived_spool_is_rejected`, `a_reservation_inside_a_failing_transaction_leaves_no_row`, `reserving_a_zero_or_negative_amount_is_rejected`, `consuming_a_negative_amount_is_rejected_but_zero_is_allowed`; `p3_lifecycle.rs`: `archiving_or_marking_empty_a_reserved_spool_is_blocked_by_its_reservation`; `p3_contract_path.rs`'s events tests confirm post-commit-only emission for ordinary mutations, `publish_ids_broadcasts_the_ids_even_when_the_record_read_fails` confirms the broadcast survives a failed post-commit read, and `debug_seed_reservation_reserves_and_reports_the_spool` demonstrates the demo-only fixture path |
| 9 | Single create with a 4-slot layout + 1 initial load, and batch create of 3 rows with a shared 4-slot layout, produce independent slot rows (differing ids); no batch row has an occupant; editing one Printer's layout leaves others unchanged | `p3_setup.rs`: `create_printer_with_a_layout_and_several_initial_loads_occupies_every_target_slot`, `batch_create_copies_the_shared_layout_into_each_row_with_disjoint_ids`, `set_material_slot_layout_reorders_renames_adds_and_removes_an_empty_slot` |
| 10 | Archive with dispositions (storage, another Printer's occupied slot with displacement, mark-empty) commits atomically; the Printer's slot/movement history is viewable after restart; a failing disposition rolls back everything | `p3_lifecycle.rs`: `archiving_applies_every_disposition_atomically_then_stops_supervision`, `a_failing_disposition_rolls_back_the_whole_archive`, `an_archived_printer_keeps_its_slots_and_every_spool_keeps_its_history_across_a_restart`, `a_mark_empty_disposition_on_a_reserved_spool_blocks_the_whole_archive` |
| 11 | Deleting that archived Printer removes its slots and every movement row touching them, leaves every Spool/ledger/reservation intact, no dangling references (`PRAGMA foreign_key_check` clean) | `p3_lifecycle.rs`: `deleting_an_archived_printer_cascades_its_slot_movements_and_leaves_every_spool_intact`; `p3_migration.rs`: `deleting_a_printer_cascades_its_slots_and_touching_movements` |
| 12 | Printers export v3 round-trips the slot layout without occupancy; v1/v2 imports get the default layout | `p3_setup.rs`: `export_v3_carries_material_slots_without_occupancy`, `importing_v1_and_v2_documents_gives_each_printer_the_default_main_layout`, `importing_v3_recreates_the_layout_with_new_ids` |
| 13 | Frontend tests cover inventory facets/filters/empty-states, add/record-amount/move/lifecycle, the slot editor in all three hosts, Status tab slots, wizard Equip, batch Shared+Results, and the archive disposition dialog | `src/screens/SpoolInventory.test.tsx`, `src/screens/SpoolDetailDock.test.tsx`, `src/screens/SpoolFormDialog.test.tsx`, `src/screens/RecordAmountDialog.test.tsx`, `src/screens/MoveSpoolDialog.test.tsx`, `src/screens/TareManagerDialog.test.tsx`, `src/screens/MaterialSlotsEditor.test.tsx`, `src/screens/PrinterStatusPanel.test.tsx`, `src/screens/PrinterSetupPanel.test.tsx`, `src/screens/PrinterSetupWizard.test.tsx`, `src/screens/PrinterDashboard.equip.test.tsx`, `src/screens/PrinterBatchDialog.test.tsx`, `src/screens/BatchRowsTable.test.tsx`, `src/screens/ArchivePrinterDialog.test.tsx`, `src/screens/PrinterDetailDock.test.tsx`, `src/spools/spool-store.test.ts`, `src/spools/facets.test.ts`, `src/spools/web-fixtures.test.ts`, `src/design-system/components/Timeline.test.tsx` (and the `DataTable`/`ColorSwatch` component tests) — a subset of the 511-test `just test` run above, all passing |
| 14 | The tracer completes through the Tauri command path: creates a Spool, loads it into one Material Slot, moves an existing loaded Spool to storage, reopens storage on the same paths (simulated restart), and reads movement and confidence history | `p3_tracer.rs` (new; see above) |
| 15 | Keyboard and viewport checks pass at 1440 × 900 and 1024 × 700, with screenshots in the P3 verification document | **Passed in a browser against `just web`** — see "Visual and keyboard verification" below. The native Tauri window's process launch was confirmed; its rendering is **unavailable** for visual confirmation (see below). |

## Visual and keyboard verification

Checks ran in Chrome (via Playwright) against `npm run dev`
(`http://localhost:1420`) at 1440 × 900 and 1024 × 700. `just web` has no
Rust backend, so every state below is the web-mode fixture (8 Spools across
lifecycle/facet states, 2 tares, and the 4-slot `prn-web-cc-1` Elegoo
Centauri Carbon — Bay 1 with Spool `#1` loaded in `Slot 1`), consistent with
the design's "Web mode: all flows work against fixtures, no path claims
desktop persistence."

Checked, with screenshots in `docs/screenshots/p3-*.png`:

- **Inventory** (`p3-inventory-1440x900.png`, `p3-inventory-1024x700.png`):
  the `DataTable` with `#`, Material, Color, Manufacturer and product,
  Remaining (with an `est.` marker), Available (shown only where it
  differs), Location, and Facets columns; the facet chips (Loaded, Reserved,
  Low, Measured, Estimated), Lifecycle/Material/Printer filters, and the
  header roster counts. At 1024 wide the toolbar wraps ("More…" drops to its
  own row) and the table needs no horizontal scroll at this row/column
  count.
- **Filtered-empty** (`p3-filtered-empty-1440x900.png`): searching for a
  non-matching string shows "No Spools match" and a **Clear filters**
  button, distinct from the (unshown, but code-reviewed) true empty state's
  "No Spools yet — Add Spool".
- **Detail history** (`p3-detail-history-1440x900.png`,
  `p3-detail-history-1024x700.png`): selecting a row opens the detail dock
  (`#nav=v1/spools/spool/spl-web-1`), showing the color swatch, remaining
  amount, location, action row (Record amount / Move… / Unload / Edit /
  Lifecycle…), and the `Timeline`-based History list with its first
  "Initial 812 g" entry. At 1440 wide it docks inline on the right, same as
  `PrinterDashboard`'s 66rem threshold rule; at 1024 wide (below the
  threshold) it correctly still overlays as a centered dialog. See
  "Deviations and findings" below — the first pass of this screenshot was
  taken before a dock-mode bug was fixed, and showed the overlay at 1440
  wide instead.
- **Move — swap** (`p3-move-swap-1440x900.png`): opening Move… on Spool `#2`
  (in storage), choosing Printer → "Elegoo Centauri Carbon — Bay 1" → "Slot
  1 (AMS 1) — occupied by #1 PLA Galaxy Black" surfaces "Swap: #1 PLA
  Galaxy Black goes to storage" with a displaced-Spool storage label field.
  The screenshot predates `f71fd92`. It shows the label as required and
  **Move** disabled until it's filled. Since `f71fd92` the label is optional
  (D6: blank means storage with no label) and **Move** is enabled once a
  slot is chosen (`MoveSpoolDialog.test.tsx`).
- **Printer Status tab slots** (`p3-status-slots-1440x900.png`,
  `p3-status-slots-1024x700.png`): the Material Slots list shows Slot 1's
  occupant (`#1`, material, color swatch/name, remaining) and Slots 2–4 as
  "Empty". At 1024 wide the detail dock overlays the Monitor card list
  (confirmed: the card grid is still visible, dimmed, behind the dock),
  matching the existing dock rule from P2.
- **Setup tab slot editor** (`p3-setup-slots-1440x900.png`): occupancy mode
  shows Swap…/Unload on the occupied Slot 1 (Remove disabled with "Unload
  first"), Load… on the three empty slots, and the D4 multi-material hint
  ("This model can feed more than one material...") above the list.
- **Wizard Equip step** (`p3-wizard-equip-1440x900.png`): the 5-step
  stepper (Identify, Connect, Equip, Operate, Review) with Equip as step 3;
  a Centauri Carbon selection shows the same multi-material hint and a
  default `Main` slot with a "Load into Main" select. Driven end-to-end by
  keyboard: Brand combobox (type + arrow + Enter), Model/Nozzle Selects
  (click-to-open, matching Kobalte's pointerdown-based trigger), Next.
- **Archive dispositions** (`p3-archive-dispositions-1440x900.png`):
  archiving "Elegoo Centauri Carbon — Bay 1" (which has Spool `#1` loaded)
  opens `ArchivePrinterDialog` with one row per loaded Spool and a
  disposition Select offering the three D10 kinds (**Storage**,
  **Another Printer's slot**, **Mark empty (used up)**) for Spool `#1`,
  which is active; choosing Storage reveals the optional storage-label field
  and enables **Archive**. Since `f71fd92`, **Mark empty (used up)** is
  offered only for an `active` Spool. An empty Spool can stay loaded (D5)
  and gets only the other two kinds (`ArchivePrinterDialog.test.tsx`).

**Keyboard-only interaction** confirmed: `Escape` closes the Spool detail
dock and reverts navigation to the inventory (checked from both an
open-dock state and mid-dialog); Kobalte Select/RadioGroup controls in the
Move dialog and the wizard were operated without a mouse click on the
underlying `<input>` (label/control activation, matching the documented
Kobalte pointerdown/pointerup pattern already used by this repo's component
tests). Full keyboard row-navigation of the `DataTable` (arrow keys,
Home/End, Enter-to-open) is exercised by `DataTable.test.tsx` and
`SpoolInventory.test.tsx`'s existing automated tests; the manual pass
additionally confirmed click-driven row selection updates the deep link
(`#nav=v1/spools/spool/<id>`) live in the browser.

**A timing-order finding surfaced during this pass** (see "Deviations and
findings" below): landing directly on a `#nav=v1/spools/...` deep link (a
hard reload) can race `SpoolInventory`'s own `loadInventory()` call ahead of
`printer-store`'s async catalog resolution in web mode, leaving Printer
occupancy metadata (`materialSlots[].occupantSpoolId`) unsynced until the
next remount or mutation — even though the Spool's own `location`/`loaded`
facet (driving the inventory table and detail dock) is unaffected and stays
correct throughout. It self-heals on the next navigation into the Spools
destination or Printer Status/Setup tab, and does not touch the Rust
backend (proven correct by the tracer and the full `p3_movement.rs`/
`p3_setup.rs` suites) or the P2-established shell architecture. Screenshots
above were taken after confirming (and, where needed, working around) this
race, so they reflect the intended, synced rendering.

Still **unavailable**:

- **The native desktop window's rendering** (`just dev`). `source
  "$HOME/.cargo/env" && npm run tauri dev` was run: `cargo` compiled the
  crate cleanly (`Finished `dev` profile`) and launched
  `target/debug/farm3d`, which stayed running (confirmed via `ps aux`) for
  the duration of the check with no panic in its log. This confirms the
  desktop build compiles and the process starts. Its actual rendered
  contents were **not** captured: the only available screenshot tool in
  this session captures the whole physical display, which is shared with
  the user's own unrelated windows (this is a live workstation, not an
  isolated test display) — capturing and inspecting that would expose
  content outside the scope of this task, so it was not done. The process
  was stopped afterward.
- Live connected-Printer probing and supervision against real hardware. No
  hardware was available; this is unrelated to P3, which uses no printer
  Connections in its own tests (every P3 test Printer is Profile-only or
  uses the injected connection factory, as the plan's "External
  dependencies" section anticipates).

## Clarification 4: "unresolved movements" → `SPOOLS_LOADED`

Per the design's D5 and the ledger's clarification 4: P3 has no "unresolved
movement" state — every `move_spool` commits atomically or not at all. The
phase plan's requirement for a delete/archive blocker on unresolved
movements is met instead by the new `LoadedSpoolBlockers` source
(`spools::lifecycle_blockers`), which contributes the `SPOOLS_LOADED`
`LifecycleBlockerCode` for `archive` while any slot on the Printer is
occupied, and for `delete` as a safety check (an archived Printer can never
hold a Spool, so this can only fire on a corrupt/racing state).

Evidence:

- `p3_lifecycle.rs::eligibility_of_a_printer_with_loaded_spools_blocks_archive_and_lists_them`
  — `printer_lifecycle_eligibility`'s `loadedSpools` is populated and
  `SPOOLS_LOADED` blocks `archive`.
- `p3_lifecycle.rs::archiving_with_loaded_spools_and_no_dispositions_is_lifecycle_blocked`
  — the safety check fires `LIFECYCLE_BLOCKED`/`SPOOLS_LOADED` if the UI's
  disposition prompt is somehow bypassed.
- `p3_lifecycle.rs::deleting_an_archived_printer_cascades_its_slot_movements_and_leaves_every_spool_intact`
  exercises the delete path on a Printer that (by construction, since only
  archived Printers can be deleted at all, per P2) has no loaded Spools —
  the delete-side `SPOOLS_LOADED` check is a documented, deliberately
  untested-by-name safety net (Task 6's ledger notes this as a known minor
  gap: "delete-side SPOOLS_LOADED safety blocker untested" — it cannot be
  reached through the public API without a corrupt database, since P2
  already prevents deleting a non-archived Printer, and P3 archive already
  clears all Spools from a Printer's slots before setting `archivedAt`).

This mapping is also recorded in
`docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`'s
sibling known-unknowns entries and in the P3 design's D5/D10.

## Deviations and findings

Summarized from the controller ledger
(`.superpowers/sdd/2026-09-23-p3-spools-material-slots/progress.md`)'s
rulings and deferred items, plus one new finding from this pass's browser
verification:

- **D2's OrcaSlicer family list was accepted on the Task 1 implementer's
  evidence, not independently reproduced.** The ledger records: "T1 ⚠️
  OrcaSlicer list check accepted on implementer's evidence
  (`MaterialType::all()` at v2.4.2 contains all 13 families) — not
  reproducible offline; cost if wrong: one family string mismatches Orca
  naming, fixable in a later migration." This verification pass did not
  have network access to re-check `PrintConfig.cpp` at the pinned tag
  either, so this remains an accepted, unverified-by-this-pass risk, per
  the plan's own "External dependencies" section.
- **`debug_seed_reservation` is a debug-only fixture command, deliberately
  excluded from `COMMAND_NAMES`/`COMMAND_CONTRACTS`.** P3 ships no
  production command that creates a reservation (spec D8: "P3 ships no
  Tauri command that creates reservations"); the fixture command exists
  only so the `reserved` facet can be demonstrated without a full P7 Job.
  It is tested directly in `p3_contract_path.rs::debug_seed_reservation_reserves_and_reports_the_spool`.
- **Web-mode fixture simplifications are dev-only and do not claim desktop
  persistence** (per spec's Errors/recovery table: "Web mode: all flows work
  against fixtures. No path claims desktop persistence."):
  - No `SPOOL_RESERVED` guard in the web fixture's lifecycle actions.
  - `initialLoads` passed to the web fixture's `createPrinter` mark the
    slot's `occupantSpoolId` locally but don't relocate the fixture Spool
    itself (there's no create-time seam into `spool-store` from
    `printer-store`).
  - Unarchiving a web-fixture Spool always restores it to `active`
    (the wire `SpoolRecord` has no `archivedFrom`, so the fixture can't
    model it).
  - `setSlotLayout` in web mode rejects removing an occupied slot with
    `SLOT_OCCUPIED` (`details: { slotId, spoolId }`), like the desktop
    command.
  - Archiving a web-mode Printer with loaded Spools applies the chosen
    dispositions. `spool-store.ts` registers
    `applyWebArchiveDispositions` through `registerWebDispositionApplier`.
    It validates the whole set first, then moves or marks empty each Spool
    through the store's own web paths. It is not atomic: a failure part-way
    through leaves the earlier moves applied. So "Nothing moves unless the
    whole archive succeeds" holds only on the real backend (proven by
    `p3_lifecycle.rs::a_failing_disposition_rolls_back_the_whole_archive`).
- **Newly observed in this pass: `SpoolInventory`'s own `loadInventory()`
  call can race `printer-store`'s async catalog resolution in web mode.**
  `SpoolInventory.tsx` intentionally calls `loadInventory()` on every mount
  (rather than the shared, load-once `ensureInventoryLoaded()` that
  `PrinterStatusPanel`/`MaterialSlotsEditor` use), per its own code comment.
  `App.tsx`'s startup sequence awaits `loadPrinters()` before its own
  `ensureInventoryLoaded()` call, but a user who deep-links or hard-reloads
  directly into `#nav=v1/spools/...` mounts `SpoolInventory` independently
  of that sequencing. If `printer-store`'s async
  `resolveWebCatalogVariant` calls haven't resolved yet at that moment,
  `syncWebPrinterOccupancy` (which reads `printerRecords()`) silently no-ops
  for that Printer, and its `materialSlots[].occupantSpoolId` stays unset
  until the next remount of `SpoolInventory` or the next Spool mutation
  re-triggers the sync. Observed effect: the Move dialog's slot picker and
  (transiently) the Printer Status/Setup tabs can show an occupied slot as
  "empty" right after a cold deep-link load. The Spool's own `location` and
  `loaded` facet — which drive the inventory table's Location column and
  the detail dock, and are what the Rust backend and the tracer actually
  assert — are unaffected throughout; only the *Printer-side* occupancy
  mirror used by the Move/Setup slot pickers can be transiently stale. This
  is a **web-fixture-only** issue (it does not exist on the real backend:
  `create_printer`/`set_material_slot_layout`/`move_spool` all read
  occupancy from the same SQLite `spools`/`material_slots` join in one
  transaction, so there is no analogous two-store race), is closely related
  to the already-deferred Task 11 ledger item ("web SpoolInventory
  `loadInventory` rebuilds fixture, losing Printer-screen web moves"), and
  is not gated by any spec acceptance criterion. It is recorded here rather
  than fixed, since a fix touches the two stores' independent mount-time
  loading strategy — a design-level ordering decision beyond this task's
  scope — and the existing screenshots in this document were taken from a
  correctly-synced state (confirmed via a genuine full reload, then
  cross-checked by remounting the Spools screen) to accurately depict the
  intended UI.
- **The Printers-import-blocked-while-loaded rule.** `import_printers`
  (`replace_all`) is rejected with `VALIDATION` ("Unload every Spool before
  importing Printers") while any Spool is loaded; with no Spools loaded, an
  import that replaces Printers deletes their slot/movement history, which
  is accepted and documented as consistent with user decision 2 (history is
  deleted with the Printer). Evidence:
  `p3_setup.rs::importing_printers_is_rejected_while_any_spool_is_loaded`,
  `p3_setup.rs::importing_with_no_spool_loaded_cascades_away_the_replaced_printers_old_slot_history`.
- **Slot constraints are deferred, as scoped.** Per spec D4/D12 and the
  design's non-goals, Material Slots carry only `name` and `feederLabel`;
  material restrictions, tool mapping, and other slot constraints are
  explicitly out of scope for P3 and would need hardware evidence plus a
  migration in a later phase.
- **Archive dialog behavior**: submitting a disposition set is disabled
  until every loaded Spool has a chosen disposition kind
  (`ArchivePrinterDialog.test.tsx`); a `CONFLICT` on one row's destination
  keeps the dialog open showing that row's error rather than closing (per
  the spec's errors table: "One archive disposition fails: the whole
  archive rolls back and the dialog shows the row error").
- **Numerous smaller, deliberately-deferred implementation notes** (rustfmt
  width, doc-comment accuracy, coarse `fieldPath`s on some slot validation
  errors, a few untested defensive branches, and similar) are listed in the
  controller ledger at each task's completion line and are not repeated
  here; none of them affect the acceptance criteria above or block P7's
  consumption of the reservation primitives.
- **Dock-mode bug found during this pass's browser verification, fixed in
  `ff16953`.** The first `p3-detail-history-1440x900.png` capture showed
  the Spool detail dock as a centered overlay dialog at 1440 wide, not the
  inline right dock the surrounding prose described. Root cause:
  `SpoolInventory.tsx`'s `workspace` ref (the element the `onMount`
  `ResizeObserver` measures to pick `"inline"` vs. `"overlay"`) lived inside
  `<Show when={spoolState.loaded}>`, which is false at mount — so the
  observer was never attached, and `dockMode` stayed at its `"overlay"`
  default forever, regardless of actual window width, once the inventory
  (and a deep-linked Spool) loaded in. Fixed by rendering the `workspace`
  container unconditionally (moving `<Show>` inside it), mirroring
  `PrinterDashboard.tsx`'s pattern, so the same 66rem threshold rule now
  actually takes effect. A regression test was added to
  `SpoolInventory.test.tsx` covering exactly this timing (inventory not yet
  loaded at mount, wide workspace, dock renders inline once loaded). Both
  `p3-detail-history-*.png` screenshots above were recaptured after the fix
  and now correctly show inline-at-1440/overlay-at-1024.

## Final review fixes

The final whole-branch review found the issues below. Each is fixed, with
a covering test.

- **Scale entry with "No tare" always failed on desktop** (`5f1ca30`). The
  Record amount and Add Spool dialogs sent neither `tareId` nor `tareMg`,
  which `ledger::resolve_entry` rejects. They now send `tareMg: 0`, and the
  web fixture rejects neither/both like Rust. Spec D3 now says so.
- **Spool history never refreshed** (`47a49e1`). The detail dock now
  reloads on `[id, revision]`, drops stale responses, and routes a load
  failure to the store banner.
- **Reservation amounts** (`99ee59f`). `reserve` rejects `amount_mg <= 0`
  and `consume` rejects `used_mg < 0` with `ReservationError::InvalidAmount`.
- **Unknown default `tareId`** (`99ee59f`). Spool insert and update return
  `VALIDATION` on `tareId` instead of a foreign-key failure
  (`PERSISTENCE_UNAVAILABLE`).
- **`publish_ids` broadcast** (`99ee59f`). The `InventoryChange` broadcast
  goes out from the ids even when the post-commit record read fails.
- **DataTable** (`30da68f`). The header row is `aria-rowindex` 1, data rows
  start at 2, and `aria-rowcount` includes the header. The sort glyph uses
  a type token.
- **Printer records and dialogs** (`f71fd92`). `spliceResolved` keeps a
  Printer whose revision is newer than the incoming record. The Move dialog
  forgets a `CONFLICT` occupant when the slot or Printer changes, and its
  displaced storage label is optional. The Archive dialog offers Mark empty
  only for an active Spool.

## Known follow-ups

Deferred from the final review. None blocks P3's acceptance criteria.

- The repository helpers rewrite whole Spool rows.
- The Printers-import error is mapped from a magic-string field path.
- The initial-load and loaded-count Spool logic lives in
  `printers/repository.rs`.
- Batch create doesn't validate the shared slot layout up front.
- `notes` has no length cap.
- The web-fixture cold-deep-link occupancy race (see "Deviations and
  findings").

## Files changed in this task

- `src-tauri/tests/p3_tracer.rs` (new): the tracer test (spec acceptance
  criterion 14).
- `CONTEXT.md`: added Tare, Storage label, and Spool number (spec §Product
  vocabulary), and a note under Material Slot that layouts are
  user-configured with no automatic AMS topology.
- `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`:
  marked the "Slot count/topology source" and "Weight precision/material
  taxonomy" known-unknowns resolved, pointing at spec D1, D2, and D4.
- `docs/screenshots/p3-*.png` (new, 11 files): the visual/keyboard
  verification screenshots listed above, including the fix-round-1
  recapture of `p3-detail-history-1440x900.png` and the new
  `p3-detail-history-1024x700.png`.
- `docs/verification/2026-09-23-p3-spools-material-slots.md` (this file).
- `src/screens/SpoolInventory.tsx`: fix-round-1 dock-mode bug fix (render
  `workspace` unconditionally so the `ResizeObserver` attaches even while
  the inventory is still loading at mount).
- `src/screens/SpoolInventory.test.tsx`: fix-round-1 regression test for the
  above.
- 20 pre-existing Rust source/test files, reformatted only, in the separate
  `style: apply cargo fmt` commit (deferred from Task 4).
