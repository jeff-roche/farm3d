//! D4/D12: Material Slot layouts. [`default_layout`] is the one-slot
//! `[Main]` layout `create_printer`/batch create use when no layout is
//! supplied (and what import falls back to for a v1/v2 document);
//! [`insert_layout`] creates a brand-new Printer's layout inside the
//! create transaction; [`set_layout`] is `set_material_slot_layout`'s
//! two-pass reorder/rename/add/remove (global constraints clarification
//! 2); [`live_slots`]/[`live_slots_by_printer`] are the read side every
//! `PrinterRepository` read path uses to fill `StoredPrinter::material_slots`
//! (global constraints clarification 1).
//!
//! Every write function here takes the caller's `&Transaction`, so
//! `printers::create::create_printer_with` can compose a layout insert with
//! `spools::movement::apply_move`'s initial loads inside one atomic commit
//! (D12: "load in the create transaction, with `load` movements sharing one
//! generated `operationId`").

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, Transaction};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::persistence::{RepositoryError, StorageError};
use crate::printers::now_rfc3339;

use super::MaterialSlot;

/// D4/D12: one entry of a layout a caller submits — `create_printer`'s
/// `slotLayout`, `BatchShared::slot_layout`, and
/// `set_material_slot_layout`'s `slots`. `id` is `None` for a new slot;
/// `Some` identifies an existing live slot to keep (rename/reorder/move),
/// meaningful only to [`set_layout`] (a fresh Printer has no existing slots,
/// so [`insert_layout`] always ignores it).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SlotSpec.ts")]
pub struct SlotSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub feeder_label: Option<String>,
}

/// D12: `create_printer`'s `initialLoads` entry — loads a storage Spool into
/// `slot_layout[slot_index]` inside the same create transaction as
/// [`insert_layout`]. Request-only (never returned), so it isn't a `TS`
/// export; `create_printer`'s hand-written contract decl describes its
/// shape inline.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialLoad {
    pub slot_index: usize,
    pub spool_id: String,
    pub expected_spool_revision: i64,
}

/// D4: the layout `create_printer`/batch create use when none is supplied,
/// and what a schemaVersion 1/2 Printers-import document falls back to
/// (its Printers never carried a layout at all).
pub fn default_layout() -> Vec<SlotSpec> {
    vec![SlotSpec {
        id: None,
        name: "Main".to_string(),
        feeder_label: None,
    }]
}

/// D4's normalized, validated form of one submitted [`SlotSpec`] — trimmed
/// name/feederLabel, feederLabel blanked to `None`. Shared by
/// [`insert_layout`]/[`set_layout`]'s validation pass.
struct NormalizedSlot {
    id: Option<String>,
    name: String,
    feeder_label: Option<String>,
}

/// D4: 1-16 slots; each name 1-32 chars trimmed, unique per Printer
/// case-insensitively; `feederLabel` optional, 1-32 chars trimmed. Every
/// failure is a plain `VALIDATION` on the `"slots"` field — D4 doesn't call
/// for a more specific path, and (unlike `initialLoads[i].spoolId`, whose
/// index the caller already knows) there's no established per-entry field
/// path convention to match here.
fn normalize_and_validate(layout: &[SlotSpec]) -> Result<Vec<NormalizedSlot>, RepositoryError> {
    if layout.is_empty() || layout.len() > 16 {
        return Err(RepositoryError::Validation { field_path: "slots" });
    }
    let mut normalized = Vec::with_capacity(layout.len());
    let mut seen_names: Vec<String> = Vec::with_capacity(layout.len());
    for spec in layout {
        let name = spec.name.trim().to_string();
        let len = name.chars().count();
        if len < 1 || len > 32 {
            return Err(RepositoryError::Validation { field_path: "slots" });
        }
        let folded = name.to_lowercase();
        if seen_names.contains(&folded) {
            return Err(RepositoryError::Validation { field_path: "slots" });
        }
        seen_names.push(folded);

        let feeder_label = match spec.feeder_label.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(trimmed) => {
                if trimmed.chars().count() > 32 {
                    return Err(RepositoryError::Validation { field_path: "slots" });
                }
                Some(trimmed.to_string())
            }
        };

        normalized.push(NormalizedSlot {
            id: spec.id.clone(),
            name,
            feeder_label,
        });
    }
    Ok(normalized)
}

/// D4/D12: creates every slot of a brand-new layout for `printer_id`, in
/// array order starting at position 0. Only for a Printer that has no live
/// slots yet (`create_printer`/batch create, and import's per-row insert) —
/// every entry's `id` is ignored, since there's nothing existing to
/// reference.
pub fn insert_layout(
    tx: &Transaction<'_>,
    printer_id: &str,
    layout: &[SlotSpec],
) -> Result<Vec<MaterialSlot>, RepositoryError> {
    let normalized = normalize_and_validate(layout)?;
    let now = now_rfc3339();
    for (position, slot) in normalized.iter().enumerate() {
        tx.execute(
            "INSERT INTO material_slots(id, printer_id, position, name, feeder_label, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                generate_slot_id(),
                printer_id,
                position as i64,
                slot.name,
                slot.feeder_label,
                now
            ],
        )?;
    }
    Ok(live_slots(tx, printer_id)?)
}

/// D12/global-constraints-clarification-2: replaces `printer_id`'s layout
/// with `layout`, in two passes so the reorder never collides with the
/// live-position unique index:
///
/// 1. Every currently-live slot moves to `position + 100` (the migration's
///    CHECK allows 0-115 for exactly this).
/// 2. Every entry of `layout` gets its final position (its array index): an
///    entry with an existing `id` is renamed/repositioned in place; an
///    entry without one is a new slot.
///
/// A live slot missing from `layout` is soft-removed (`removed_at` set, row
/// kept — its history stays readable). Soft-removing an occupied slot fails
/// the whole call with [`RepositoryError::SlotOccupied`] before either pass
/// runs.
pub fn set_layout(
    tx: &Transaction<'_>,
    printer_id: &str,
    layout: &[SlotSpec],
) -> Result<Vec<MaterialSlot>, RepositoryError> {
    let normalized = normalize_and_validate(layout)?;
    let existing = live_slots(tx, printer_id)?;
    let existing_ids: HashSet<&str> = existing.iter().map(|slot| slot.id.as_str()).collect();

    for slot in &normalized {
        if let Some(id) = &slot.id {
            if !existing_ids.contains(id.as_str()) {
                return Err(RepositoryError::Validation { field_path: "slots" });
            }
        }
    }

    let kept_ids: HashSet<&str> = normalized
        .iter()
        .filter_map(|slot| slot.id.as_deref())
        .collect();
    let removed: Vec<&MaterialSlot> = existing
        .iter()
        .filter(|slot| !kept_ids.contains(slot.id.as_str()))
        .collect();
    for slot in &removed {
        if let Some(spool_id) = &slot.occupant_spool_id {
            return Err(RepositoryError::SlotOccupied {
                slot_id: slot.id.clone(),
                spool_id: spool_id.clone(),
            });
        }
    }

    let now = now_rfc3339();
    // Pass 1: every live slot out of the live 0-15 range so pass 2 never
    // collides with `material_slots_live_position`.
    tx.execute(
        "UPDATE material_slots SET position = position + 100
         WHERE printer_id = ?1 AND removed_at IS NULL",
        params![printer_id],
    )?;
    for slot in &removed {
        tx.execute(
            "UPDATE material_slots SET removed_at = ?2 WHERE id = ?1",
            params![slot.id, now],
        )?;
    }
    // Pass 2: final positions (array order), renaming kept slots and
    // inserting new ones.
    for (position, slot) in normalized.iter().enumerate() {
        match &slot.id {
            Some(id) => {
                tx.execute(
                    "UPDATE material_slots SET position = ?2, name = ?3, feeder_label = ?4
                     WHERE id = ?1",
                    params![id, position as i64, slot.name, slot.feeder_label],
                )?;
            }
            None => {
                tx.execute(
                    "INSERT INTO material_slots(id, printer_id, position, name, feeder_label, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        generate_slot_id(),
                        printer_id,
                        position as i64,
                        slot.name,
                        slot.feeder_label,
                        now
                    ],
                )?;
            }
        }
    }

    Ok(live_slots(tx, printer_id)?)
}

/// A Printer's live (non-removed) slots, in position order, with each
/// slot's current occupant (if any) joined in. Takes `&Connection` (not
/// `&Transaction`) so it works from both a read (`PrinterRepository::get`)
/// and inside any write transaction — `Transaction` derefs to `Connection`,
/// so callers pass either.
pub fn live_slots(
    connection: &Connection,
    printer_id: &str,
) -> Result<Vec<MaterialSlot>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT ms.id, ms.position, ms.name, ms.feeder_label, s.id
         FROM material_slots ms
         LEFT JOIN spools s ON s.slot_id = ms.id
         WHERE ms.printer_id = ?1 AND ms.removed_at IS NULL
         ORDER BY ms.position",
    )?;
    let rows = statement
        .query_map([printer_id], decode_slot)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// [`live_slots`]'s batched form for `PrinterRepository::list` — one query
/// for every Printer's live slots at once, grouped by `printer_id`, rather
/// than one extra query per row (global constraints clarification 1: "one
/// extra query per read").
pub fn live_slots_by_printer(
    connection: &Connection,
) -> Result<HashMap<String, Vec<MaterialSlot>>, StorageError> {
    // `decode_slot` reads columns 0-4, so `printer_id` is selected LAST
    // (not first) — it's the one extra column this query has over
    // `live_slots`'s, and keeping the shared columns' offsets identical
    // between the two queries is what lets them share `decode_slot` safely.
    let mut statement = connection.prepare(
        "SELECT ms.id, ms.position, ms.name, ms.feeder_label, s.id, ms.printer_id
         FROM material_slots ms
         LEFT JOIN spools s ON s.slot_id = ms.id
         WHERE ms.removed_at IS NULL
         ORDER BY ms.printer_id, ms.position",
    )?;
    let mut by_printer: HashMap<String, Vec<MaterialSlot>> = HashMap::new();
    let rows = statement.query_map([], |row| {
        let printer_id: String = row.get(5)?;
        Ok((printer_id, decode_slot(row)?))
    })?;
    for row in rows {
        let (printer_id, slot) = row?;
        by_printer.entry(printer_id).or_default().push(slot);
    }
    Ok(by_printer)
}

fn decode_slot(row: &rusqlite::Row<'_>) -> rusqlite::Result<MaterialSlot> {
    Ok(MaterialSlot {
        id: row.get(0)?,
        position: row.get(1)?,
        name: row.get(2)?,
        feeder_label: row.get(3)?,
        occupant_spool_id: row.get(4)?,
    })
}

fn generate_slot_id() -> String {
    format!("slt-{}", uuid::Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_is_one_slot_named_main() {
        let layout = default_layout();
        assert_eq!(layout.len(), 1);
        assert_eq!(layout[0].name, "Main");
        assert_eq!(layout[0].id, None);
        assert_eq!(layout[0].feeder_label, None);
    }

    fn spec(name: &str) -> SlotSpec {
        SlotSpec {
            id: None,
            name: name.to_string(),
            feeder_label: None,
        }
    }

    #[test]
    fn normalize_and_validate_rejects_empty_and_over_sixteen() {
        assert!(matches!(
            normalize_and_validate(&[]),
            Err(RepositoryError::Validation { field_path: "slots" })
        ));
        let too_many: Vec<SlotSpec> = (0..17).map(|i| spec(&format!("Slot {i}"))).collect();
        assert!(matches!(
            normalize_and_validate(&too_many),
            Err(RepositoryError::Validation { field_path: "slots" })
        ));
        let sixteen: Vec<SlotSpec> = (0..16).map(|i| spec(&format!("Slot {i}"))).collect();
        assert!(normalize_and_validate(&sixteen).is_ok());
    }

    #[test]
    fn normalize_and_validate_rejects_case_insensitive_duplicate_names() {
        let layout = vec![spec("Main"), spec("main")];
        assert!(matches!(
            normalize_and_validate(&layout),
            Err(RepositoryError::Validation { field_path: "slots" })
        ));
    }

    #[test]
    fn normalize_and_validate_trims_names_and_blanks_feeder_label() {
        let layout = vec![SlotSpec {
            id: None,
            name: "  Main  ".to_string(),
            feeder_label: Some("   ".to_string()),
        }];
        let normalized = normalize_and_validate(&layout).unwrap();
        assert_eq!(normalized[0].name, "Main");
        assert_eq!(normalized[0].feeder_label, None);
    }
}
