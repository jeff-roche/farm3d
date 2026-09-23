//! D1-D5, D9: the Spool repository — [`StoredSpool`] (the full persisted
//! row) and the read/write primitives later P3 tasks compose inside their
//! own transaction (Task 3's `move_spool`, Task 4's reservations, Task 6's
//! archive dispositions). Every function here takes the caller's
//! `&Transaction` rather than opening its own, so a caller can combine
//! several of these — plus `ledger`/`tares` calls — into one atomic write
//! via `Storage::write`.

use rusqlite::{params, OptionalExtension, Transaction};

use crate::persistence::{RepositoryError, StorageError};
use crate::printers::now_rfc3339;

use super::ledger::{self, AmountEntry, AmountEventKind};
use super::{
    decode_enum, encode_enum, validate_fields, AmountConfidence, Availability, FilamentDiameter,
    MaterialFamily, SpoolFacets, SpoolFields, SpoolLifecycle, SpoolLocation, SpoolRecord,
};

/// Every user-editable Spool field a patch may change — exactly
/// [`SpoolFields`]'s shape, since it already excludes `lifecycle`,
/// `location`, `availability`, and `facets` (see that type's doc comment).
pub type SpoolPatch = SpoolFields;

/// The full persisted Spool row (D1-D5, D9): everything [`SpoolRecord`]
/// derives from ([`list_spools`]'s job), plus the raw fields a `SpoolRecord`
/// doesn't carry directly (`archivedFrom`, the bare `slotId`, `storageLabel`
/// instead of a combined `location`).
#[derive(Clone, Debug, PartialEq)]
pub struct StoredSpool {
    pub id: String,
    pub revision: i64,
    pub spool_number: i64,
    pub manufacturer: String,
    pub product: Option<String>,
    pub material_family: MaterialFamily,
    pub material_other: Option<String>,
    pub color_name: String,
    pub color_hex: Option<String>,
    pub diameter: FilamentDiameter,
    pub nominal_mg: i64,
    pub current_mg: i64,
    pub confidence: AmountConfidence,
    pub low_threshold_mg: i64,
    pub tare_id: Option<String>,
    pub lifecycle: SpoolLifecycle,
    pub archived_from: Option<SpoolLifecycle>,
    pub slot_id: Option<String>,
    pub storage_label: Option<String>,
    pub last_measured_at: Option<String>,
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Column order shared by every `SELECT`/`INSERT`/`UPDATE` below, and by
/// [`decode_stored`]'s row indices.
const SPOOL_COLUMNS: [&str; 23] = [
    "id",
    "revision",
    "spool_number",
    "manufacturer",
    "product",
    "material_family",
    "material_other",
    "color_name",
    "color_hex",
    "diameter",
    "nominal_mg",
    "current_mg",
    "confidence",
    "low_threshold_mg",
    "tare_id",
    "lifecycle",
    "archived_from",
    "slot_id",
    "storage_label",
    "last_measured_at",
    "notes",
    "created_at",
    "updated_at",
];

fn bare_columns() -> String {
    SPOOL_COLUMNS.join(", ")
}

fn prefixed_columns(prefix: &str) -> String {
    SPOOL_COLUMNS
        .iter()
        .map(|column| format!("{prefix}.{column}"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn generate_id() -> String {
    format!("spl-{}", uuid::Uuid::new_v4())
}

pub fn load_spool(tx: &Transaction<'_>, id: &str) -> Result<Option<StoredSpool>, StorageError> {
    tx.query_row(
        &format!("SELECT {} FROM spools WHERE id = ?1", bare_columns()),
        [id],
        decode_stored,
    )
    .optional()
    .map_err(StorageError::from)
}

/// D9's derived, wire-shaped Spool list, ordered by `spoolNumber`. Joins
/// `material_slots` for the occupied slot's `printerId` (a Spool's
/// `location`) and sums `spool_reservations` in `{active, unresolved}` for
/// `reservedMg`, all in one query — Rust is the only thing that derives
/// `location`/`availability`/`facets` (global constraint: the frontend
/// never re-derives them).
pub fn list_spools(tx: &Transaction<'_>) -> Result<Vec<SpoolRecord>, StorageError> {
    let query = format!(
        "SELECT {columns}, ms.printer_id, COALESCE(r.reserved_mg, 0)
         FROM spools s
         LEFT JOIN material_slots ms ON ms.id = s.slot_id
         LEFT JOIN (
             SELECT spool_id, SUM(amount_mg) AS reserved_mg
             FROM spool_reservations
             WHERE state IN ('active', 'unresolved')
             GROUP BY spool_id
         ) r ON r.spool_id = s.id
         ORDER BY s.spool_number",
        columns = prefixed_columns("s")
    );
    let mut statement = tx.prepare(&query)?;
    let rows = statement
        .query_map([], decode_record)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// D1-D9: normalizes and validates `fields`, resolves `initial` (D3/D7's
/// scale math via `ledger::resolve_entry`), allocates the next
/// `spoolNumber` (D9: `MAX(spoolNumber) + 1`, inside this same
/// transaction), and writes the Spool row plus its `initial` ledger event
/// (via `ledger::append`) atomically. The new Spool starts `active`, with
/// no slot (a fresh Spool always starts in storage; loading it is a
/// separate `move_spool` call, Task 3).
pub fn insert_spool(
    tx: &Transaction<'_>,
    fields: &SpoolFields,
    initial: &AmountEntry,
    storage_label: Option<&str>,
) -> Result<StoredSpool, RepositoryError> {
    let mut fields = fields.clone();
    fields.normalize();
    validate_fields(&fields)?;
    let storage_label = normalize_storage_label(storage_label)?;

    let (after_mg, confidence, snapshot) = ledger::resolve_entry(tx, initial)?;
    let spool_number: i64 = tx.query_row(
        "SELECT COALESCE(MAX(spool_number), 0) + 1 FROM spools",
        [],
        |row| row.get(0),
    )?;
    let id = generate_id();
    let now = now_rfc3339();
    // D3: the Spool's default tare comes only from `fields.tareId` — a
    // Scale entry's `tareId` (already resolved into `snapshot` above) never
    // sets or changes it.
    let tare_id = fields.tare_id.clone();
    let last_measured_at =
        matches!(confidence, AmountConfidence::Measured).then(|| now.clone());

    let stored = StoredSpool {
        id: id.clone(),
        revision: 1,
        spool_number,
        manufacturer: fields.manufacturer,
        product: fields.product,
        material_family: fields.material_family,
        material_other: fields.material_other,
        color_name: fields.color_name,
        color_hex: fields.color_hex,
        diameter: fields.diameter,
        nominal_mg: fields.nominal_mg,
        current_mg: after_mg,
        confidence,
        low_threshold_mg: fields.low_threshold_mg,
        tare_id,
        lifecycle: SpoolLifecycle::Active,
        archived_from: None,
        slot_id: None,
        storage_label,
        last_measured_at,
        notes: fields.notes,
        created_at: now.clone(),
        updated_at: now,
    };
    insert(tx, &stored)?;
    // Writes the `initial` ledger row and reconfirms the cache this INSERT
    // already set (append's UPDATE is idempotent here — see its doc
    // comment on why it derives `before_mg`/`sequence` from the ledger
    // table rather than the row we just inserted).
    ledger::append(
        tx,
        &id,
        AmountEventKind::Initial,
        after_mg,
        confidence,
        snapshot,
    )?;
    load_spool(tx, &id)?.ok_or(RepositoryError::Storage(StorageError::OperationFailed))
}

/// Replaces every field in `patch` (normalized and validated first) after
/// an optimistic-concurrency check against `expected_revision`. Never
/// touches `lifecycle`/`location`/`availability`/`facets` — those change
/// only through `set_spool_lifecycle`/`move_spool`/derivation (later
/// tasks).
pub fn update_spool_fields(
    tx: &Transaction<'_>,
    id: &str,
    expected_revision: i64,
    patch: &SpoolPatch,
) -> Result<StoredSpool, RepositoryError> {
    if expected_revision <= 0 {
        return Err(RepositoryError::Validation {
            field_path: "expectedRevision",
        });
    }
    let mut fields = patch.clone();
    fields.normalize();
    validate_fields(&fields)?;

    let mut spool = load_spool(tx, id)?.ok_or_else(|| RepositoryError::NotFound {
        entity_id: id.to_string(),
    })?;
    if spool.revision != expected_revision {
        return Err(RepositoryError::Conflict {
            entity_id: id.to_string(),
            expected_revision,
            current_revision: spool.revision,
        });
    }

    spool.manufacturer = fields.manufacturer;
    spool.product = fields.product;
    spool.material_family = fields.material_family;
    spool.material_other = fields.material_other;
    spool.color_name = fields.color_name;
    spool.color_hex = fields.color_hex;
    spool.diameter = fields.diameter;
    spool.nominal_mg = fields.nominal_mg;
    spool.low_threshold_mg = fields.low_threshold_mg;
    spool.tare_id = fields.tare_id;
    spool.notes = fields.notes;
    spool.revision += 1;
    spool.updated_at = now_rfc3339();
    replace(tx, &spool)?;
    Ok(spool)
}

/// The optimistic-concurrency load-check-bump every later mutation that
/// doesn't change a field value (Task 3's `move_spool`, primarily) still
/// needs: verifies `expected_revision`, then bumps `revision`/`updated_at`
/// with no other field changed.
pub fn check_and_bump_revision(
    tx: &Transaction<'_>,
    id: &str,
    expected_revision: i64,
) -> Result<StoredSpool, RepositoryError> {
    if expected_revision <= 0 {
        return Err(RepositoryError::Validation {
            field_path: "expectedRevision",
        });
    }
    let mut spool = load_spool(tx, id)?.ok_or_else(|| RepositoryError::NotFound {
        entity_id: id.to_string(),
    })?;
    if spool.revision != expected_revision {
        return Err(RepositoryError::Conflict {
            entity_id: id.to_string(),
            expected_revision,
            current_revision: spool.revision,
        });
    }
    spool.revision += 1;
    spool.updated_at = now_rfc3339();
    replace(tx, &spool)?;
    Ok(spool)
}

/// D5's blank-to-`None` storage label rule (mirrors
/// `SpoolFields::normalize`'s optional-text handling), plus the migration's
/// 1-64 char length CHECK.
pub(crate) fn normalize_storage_label(label: Option<&str>) -> Result<Option<String>, RepositoryError> {
    let Some(label) = label else {
        return Ok(None);
    };
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > 64 {
        return Err(RepositoryError::Validation {
            field_path: "storageLabel",
        });
    }
    Ok(Some(trimmed.to_string()))
}

/// D9's derivation from a [`StoredSpool`] row plus its join results
/// (`printer_id` for an occupied slot, the summed `reserved_mg`) to the
/// wire-shaped [`SpoolRecord`]: `location`, `availability`, and `facets`.
fn to_record(stored: StoredSpool, printer_id: Option<String>, reserved_mg: i64) -> SpoolRecord {
    let location = match (stored.slot_id.clone(), printer_id) {
        (Some(slot_id), Some(printer_id)) => SpoolLocation::Slot {
            slot_id,
            printer_id,
        },
        _ => SpoolLocation::Storage {
            storage_label: stored.storage_label.clone(),
        },
    };
    let facets = SpoolFacets {
        loaded: stored.slot_id.is_some(),
        reserved: reserved_mg > 0,
        low: matches!(stored.lifecycle, SpoolLifecycle::Active)
            && stored.current_mg <= stored.low_threshold_mg,
        confidence: stored.confidence,
    };
    SpoolRecord {
        id: stored.id,
        revision: stored.revision,
        spool_number: stored.spool_number,
        manufacturer: stored.manufacturer,
        product: stored.product,
        material_family: stored.material_family,
        material_other: stored.material_other,
        color_name: stored.color_name,
        color_hex: stored.color_hex,
        diameter: stored.diameter,
        nominal_mg: stored.nominal_mg,
        low_threshold_mg: stored.low_threshold_mg,
        tare_id: stored.tare_id,
        lifecycle: stored.lifecycle,
        location,
        availability: Availability {
            current_mg: stored.current_mg,
            reserved_mg,
            available_mg: stored.current_mg - reserved_mg,
        },
        facets,
        last_measured_at: stored.last_measured_at,
        notes: stored.notes,
        created_at: stored.created_at,
        updated_at: stored.updated_at,
    }
}

fn decode_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<SpoolRecord> {
    let stored = decode_stored(row)?;
    let printer_id: Option<String> = row.get(23)?;
    let reserved_mg: i64 = row.get(24)?;
    Ok(to_record(stored, printer_id, reserved_mg))
}

fn decode_stored(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredSpool> {
    let material_family_text: String = row.get(5)?;
    let diameter_text: String = row.get(9)?;
    let confidence_text: String = row.get(12)?;
    let lifecycle_text: String = row.get(15)?;
    let archived_from_text: Option<String> = row.get(16)?;
    Ok(StoredSpool {
        id: row.get(0)?,
        revision: row.get(1)?,
        spool_number: row.get(2)?,
        manufacturer: row.get(3)?,
        product: row.get(4)?,
        material_family: decode_enum(&material_family_text).map_err(|error| from_sql_error(5, error))?,
        material_other: row.get(6)?,
        color_name: row.get(7)?,
        color_hex: row.get(8)?,
        diameter: decode_enum(&diameter_text).map_err(|error| from_sql_error(9, error))?,
        nominal_mg: row.get(10)?,
        current_mg: row.get(11)?,
        confidence: decode_enum(&confidence_text).map_err(|error| from_sql_error(12, error))?,
        low_threshold_mg: row.get(13)?,
        tare_id: row.get(14)?,
        lifecycle: decode_enum(&lifecycle_text).map_err(|error| from_sql_error(15, error))?,
        archived_from: archived_from_text
            .map(|text| decode_enum(&text))
            .transpose()
            .map_err(|error| from_sql_error(16, error))?,
        slot_id: row.get(17)?,
        storage_label: row.get(18)?,
        last_measured_at: row.get(19)?,
        notes: row.get(20)?,
        created_at: row.get(21)?,
        updated_at: row.get(22)?,
    })
}

fn from_sql_error(column: usize, error: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(error))
}

/// The enum-encoded column values shared by [`insert`]/[`replace`], in
/// [`SPOOL_COLUMNS`] order (after `id`).
struct EncodedColumns {
    material_family: String,
    diameter: String,
    confidence: String,
    lifecycle: String,
    archived_from: Option<String>,
}

fn encode_columns(spool: &StoredSpool) -> EncodedColumns {
    EncodedColumns {
        material_family: encode_enum(spool.material_family),
        diameter: encode_enum(spool.diameter),
        confidence: encode_enum(spool.confidence),
        lifecycle: encode_enum(spool.lifecycle),
        archived_from: spool.archived_from.map(encode_enum),
    }
}

fn insert(tx: &Transaction<'_>, spool: &StoredSpool) -> Result<(), StorageError> {
    let encoded = encode_columns(spool);
    tx.execute(
        &format!(
            "INSERT INTO spools({columns}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
            columns = bare_columns()
        ),
        params![
            spool.id,
            spool.revision,
            spool.spool_number,
            spool.manufacturer,
            spool.product,
            encoded.material_family,
            spool.material_other,
            spool.color_name,
            spool.color_hex,
            encoded.diameter,
            spool.nominal_mg,
            spool.current_mg,
            encoded.confidence,
            spool.low_threshold_mg,
            spool.tare_id,
            encoded.lifecycle,
            encoded.archived_from,
            spool.slot_id,
            spool.storage_label,
            spool.last_measured_at,
            spool.notes,
            spool.created_at,
            spool.updated_at,
        ],
    )?;
    Ok(())
}

fn replace(tx: &Transaction<'_>, spool: &StoredSpool) -> Result<(), StorageError> {
    let encoded = encode_columns(spool);
    tx.execute(
        "UPDATE spools SET
            revision = ?2, spool_number = ?3, manufacturer = ?4, product = ?5,
            material_family = ?6, material_other = ?7, color_name = ?8, color_hex = ?9,
            diameter = ?10, nominal_mg = ?11, current_mg = ?12, confidence = ?13,
            low_threshold_mg = ?14, tare_id = ?15, lifecycle = ?16, archived_from = ?17,
            slot_id = ?18, storage_label = ?19, last_measured_at = ?20, notes = ?21,
            created_at = ?22, updated_at = ?23
         WHERE id = ?1",
        params![
            spool.id,
            spool.revision,
            spool.spool_number,
            spool.manufacturer,
            spool.product,
            encoded.material_family,
            spool.material_other,
            spool.color_name,
            spool.color_hex,
            encoded.diameter,
            spool.nominal_mg,
            spool.current_mg,
            encoded.confidence,
            spool.low_threshold_mg,
            spool.tare_id,
            encoded.lifecycle,
            encoded.archived_from,
            spool.slot_id,
            spool.storage_label,
            spool.last_measured_at,
            spool.notes,
            spool.created_at,
            spool.updated_at,
        ],
    )?;
    Ok(())
}
