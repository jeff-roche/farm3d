# F1 Persistence and Cross-Tier Contract Implementation Plan

**Status:** Authoritative

**Design:** [F1 Persistence and Cross-Tier Contract Design](../../specs/2026-09-17-f1-foundation-design.md)

This is the F1 implementation plan. It predates the implementation and is the
source plan for the `feature/f1-foundation` worktree; reviews must not treat F1
as unplanned work merely because the plan was initially present only in the
repository's primary worktree.

F1 replaces JSON-backed authoritative state with SQLite, migrates canonical F0
data through crash-safe staging and archival, generates the cross-tier command
and event contracts from Rust, coordinates credentials without persisting
secret values, adds editable Settings and Printers import/export, introduces
stable navigation identity, and verifies the complete vertical path.

The ordered work packages are:

1. Approve the focused design and SQLite ADR.
2. Establish deterministic Rust-to-TypeScript contract generation.
3. Build the leased SQLite storage, migration, snapshot, and restore-staging core.
4. Stage, validate, import, and archive canonical F0 data crash-safely.
5. Move Settings and Printers behind revisioned repositories.
6. Harden serialized credential coordination and fallback-file replacement.
7. Convert all 23 Tauri handlers to versioned success/error contracts.
8. Implement the versioned status stream and race-safe frontend reconciliation.
9. Implement Settings import/export with native dialogs and safety snapshots.
10. Implement complete-set Printers import/export and supervisor reconciliation.
11. Wire fragment-serializable navigation identity and unavailable targets.
12. Run the F1 vertical tracer, secret scans, package assertions, and full gates.

The exact invariants, command matrix, crash boundaries, test evidence, and exit
criteria are normative in the linked design. This plan records the approved
scope and ordered implementation sequence used by the F1 branch.
