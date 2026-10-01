# Backup fixtures

Each `vN/` directory holds a real backup written by the binary that shipped
that backup format and schema. `tests/p9_fixture_compat.rs` restores it
(preview, apply, install) and derives tampered variants from it in a temp
directory. Nothing tampered is committed.

- `v1/farm-v1-schema10.farm3d-backup`: format 1, schema 10, the seeded
  every-domain Farm with all media, `createdAt` 2026-09-29T12:00:00.000Z.

## The rule

**Never regenerate an existing version's fixture.** It stands for a backup
an operator already holds. If the format or schema changes, add a new
fixture (`v2/`, and a generator and tests for it) and keep the old one
restoring. A compatibility test that only ever sees fresh backups proves
nothing.

`just gen-backup-fixtures` exists to create the v1 file byte for byte (fixed
timestamps, ids, and content: two runs give the same SHA-256). It is not a
way to refresh it: it refuses to overwrite an existing fixture, so checking
determinism means moving the file aside, running it, and comparing the two.
