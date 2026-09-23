//! D7's append-only amount ledger, plus D3's scale-entry math. Every
//! `spool_amount_events` row this module writes is immutable once
//! committed — the migration's triggers reject `UPDATE`/`DELETE` on that
//! table, so a correction is always a new row, never a rewrite of history
//! (see [`history`]'s `is_correction`).
//!
//! Both public functions here take the caller's `&Transaction`, so a
//! higher-level operation (`repository::insert_spool` in this task; Task
//! 4's `consume`, Task 6's archive dispositions later) can call `resolve_entry`
//! and `append` as part of one larger atomic write.

use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::persistence::{RepositoryError, StorageError};

use super::{decode_enum, encode_enum, tares, weight, AmountConfidence};

/// A caller-supplied amount entry (D1/D3/D7): either a direct net weight
/// (carrying its own confidence, since a Net entry may be "I weighed it" or
/// "I'm estimating"), or a scale reading resolved against a tare. Exactly
/// one of `tareId`/`tareMg` must be given for `Scale` — see [`resolve_entry`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    export_to = "domain/AmountEntry.ts"
)]
pub enum AmountEntry {
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Net {
        #[ts(type = "number")]
        net_mg: i64,
        confidence: AmountConfidence,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Scale {
        #[ts(type = "number")]
        gross_mg: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        tare_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional, type = "number")]
        tare_mg: Option<i64>,
    },
}

/// D7's ledger row kinds. The table's `kind` CHECK lists these five values
/// verbatim (camelCase, matching this enum's serde rename).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AmountEventKind.ts")]
pub enum AmountEventKind {
    Initial,
    Measurement,
    Estimate,
    Consumption,
    MarkedEmpty,
}

/// One immutable row of the amount ledger (D7). `is_correction` is never
/// stored — [`history`] computes it per row on read.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AmountEvent.ts")]
pub struct AmountEvent {
    pub id: String,
    pub spool_id: String,
    #[ts(type = "number")]
    pub sequence: i64,
    pub kind: AmountEventKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub before_mg: Option<i64>,
    #[ts(type = "number")]
    pub after_mg: i64,
    pub confidence_after: AmountConfidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub gross_mg: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub tare_mg: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reservation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub note: Option<String>,
    pub occurred_at: String,
    pub is_correction: bool,
}

/// The D3/D7 snapshot values a ledger row carries alongside `kind`/`afterMg`
/// (append's other parameters): [`resolve_entry`] fills `gross_mg`/
/// `tare_mg`/`tare_id` for a `Scale` entry and leaves them `None` for a
/// `Net` one; a future D8 `consume` call (Task 4) is expected to fill
/// `reservation_id`/`note` instead and leave the D3 fields `None`.
///
/// `tare_id` is never itself written to `spool_amount_events` — a ledger
/// row only ever stores the *value* it measured against (`tare_mg`, a
/// snapshot untouched by later tare edits, per D3). [`append`] uses
/// `tare_id` solely to refresh `spools.tare_id`, the Spool's own default
/// tare (D3: "Scale entry uses it unless the user picks another" — an
/// explicit `tareId` becomes the new default going forward).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LedgerSnapshot {
    pub gross_mg: Option<i64>,
    pub tare_mg: Option<i64>,
    pub tare_id: Option<String>,
    pub reservation_id: Option<String>,
    pub note: Option<String>,
}

/// D3's scale math plus D1's range checks: turns a caller's [`AmountEntry`]
/// into `(afterMg, confidence, snapshot)`, ready for [`append`]. Reads
/// `spool_tares` (for a `Scale` entry's `tareId`) but never writes
/// `spools`/`spool_amount_events` itself.
pub fn resolve_entry(
    tx: &Transaction<'_>,
    entry: &AmountEntry,
) -> Result<(i64, AmountConfidence, LedgerSnapshot), RepositoryError> {
    match entry {
        AmountEntry::Net { net_mg, confidence } => {
            validate_current_mg(*net_mg)?;
            Ok((*net_mg, *confidence, LedgerSnapshot::default()))
        }
        AmountEntry::Scale {
            gross_mg,
            tare_id,
            tare_mg,
        } => {
            let (resolved_tare_mg, resolved_tare_id) = match (tare_id, tare_mg) {
                (Some(id), None) => {
                    let tare = tares::get(tx, id)?.ok_or(RepositoryError::Validation {
                        field_path: "entry.tareId",
                    })?;
                    (tare.weight_mg, Some(tare.id))
                }
                (None, Some(mg)) => {
                    if !weight::TARE_MG_RANGE.contains(mg) {
                        return Err(RepositoryError::Validation {
                            field_path: "entry.tareMg",
                        });
                    }
                    (*mg, None)
                }
                // Exactly one of `tareId`/`tareMg` must be given (D3): both
                // or neither is ambiguous about which tare weight to use.
                (Some(_), Some(_)) | (None, None) => {
                    return Err(RepositoryError::Validation {
                        field_path: "entry.tareId",
                    });
                }
            };
            if *gross_mg < resolved_tare_mg {
                return Err(RepositoryError::Validation {
                    field_path: "entry.grossMg",
                });
            }
            let net_mg = gross_mg - resolved_tare_mg;
            validate_current_mg(net_mg)?;
            Ok((
                net_mg,
                AmountConfidence::Measured,
                LedgerSnapshot {
                    gross_mg: Some(*gross_mg),
                    tare_mg: Some(resolved_tare_mg),
                    tare_id: resolved_tare_id,
                    ..Default::default()
                },
            ))
        }
    }
}

fn validate_current_mg(net_mg: i64) -> Result<(), RepositoryError> {
    if weight::CURRENT_MG_RANGE.contains(&net_mg) {
        Ok(())
    } else {
        Err(RepositoryError::Validation {
            field_path: "entry.netMg",
        })
    }
}

/// D7: appends one immutable row to `spool_amount_events` for `spool_id`
/// and, in the same transaction, updates the `spools.current_mg`/
/// `confidence` cache (plus `updated_at`, `last_measured_at` when
/// `confidence` is `measured`, and `tare_id` when `snapshot.tare_id` is
/// `Some`) so the cache always equals this row (D7's invariant).
///
/// `before_mg` and `sequence` are derived from `spool_amount_events`
/// itself (the prior row's `after_mg`, or `None`/`1` when there is none)
/// rather than read from the `spools` cache — this is what lets
/// `repository::insert_spool` call `append` for a brand-new Spool's
/// `initial` row immediately after inserting it, without that insert's own
/// placeholder `current_mg` leaking into `before_mg`.
pub fn append(
    tx: &Transaction<'_>,
    spool_id: &str,
    kind: AmountEventKind,
    after_mg: i64,
    confidence: AmountConfidence,
    snapshot: LedgerSnapshot,
) -> Result<AmountEvent, RepositoryError> {
    validate_current_mg(after_mg)?;
    let before_mg: Option<i64> = tx
        .query_row(
            "SELECT after_mg FROM spool_amount_events WHERE spool_id = ?1 ORDER BY sequence DESC LIMIT 1",
            [spool_id],
            |row| row.get(0),
        )
        .optional()?;
    let sequence: i64 = tx.query_row(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM spool_amount_events WHERE spool_id = ?1",
        [spool_id],
        |row| row.get(0),
    )?;
    let id = format!("sae-{}", uuid::Uuid::new_v4());
    let now = crate::printers::now_rfc3339();
    let kind_text = encode_enum(kind);
    let confidence_text = encode_enum(confidence);
    tx.execute(
        "INSERT INTO spool_amount_events(
            id, spool_id, sequence, kind, before_mg, after_mg, confidence_after,
            gross_mg, tare_mg, reservation_id, note, occurred_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            id,
            spool_id,
            sequence,
            kind_text,
            before_mg,
            after_mg,
            confidence_text,
            snapshot.gross_mg,
            snapshot.tare_mg,
            snapshot.reservation_id,
            snapshot.note,
            now,
        ],
    )?;
    let updated = tx.execute(
        "UPDATE spools SET
            current_mg = ?2,
            confidence = ?3,
            updated_at = ?4,
            last_measured_at = CASE WHEN ?3 = 'measured' THEN ?4 ELSE last_measured_at END,
            tare_id = COALESCE(?5, tare_id)
         WHERE id = ?1",
        params![spool_id, after_mg, confidence_text, now, snapshot.tare_id],
    )?;
    if updated == 0 {
        return Err(RepositoryError::NotFound {
            entity_id: spool_id.to_string(),
        });
    }
    Ok(AmountEvent {
        id,
        spool_id: spool_id.to_string(),
        sequence,
        kind,
        before_mg,
        after_mg,
        confidence_after: confidence,
        gross_mg: snapshot.gross_mg,
        tare_mg: snapshot.tare_mg,
        reservation_id: snapshot.reservation_id,
        note: snapshot.note,
        occurred_at: now,
        is_correction: false,
    })
}

/// D7's full ledger for `spool_id`, ordered by `sequence`, with
/// `is_correction` computed per row: true when the row is a `measurement`
/// or `estimate` and an earlier row (any `sequence` strictly before it) is
/// a `consumption`.
pub fn history(tx: &Transaction<'_>, spool_id: &str) -> Result<Vec<AmountEvent>, StorageError> {
    let mut statement = tx.prepare(
        "SELECT id, spool_id, sequence, kind, before_mg, after_mg, confidence_after,
                gross_mg, tare_mg, reservation_id, note, occurred_at
         FROM spool_amount_events WHERE spool_id = ?1 ORDER BY sequence",
    )?;
    let rows = statement
        .query_map([spool_id], decode_event)?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut seen_consumption = false;
    let mut events = Vec::with_capacity(rows.len());
    for mut event in rows {
        event.is_correction = matches!(
            event.kind,
            AmountEventKind::Measurement | AmountEventKind::Estimate
        ) && seen_consumption;
        if matches!(event.kind, AmountEventKind::Consumption) {
            seen_consumption = true;
        }
        events.push(event);
    }
    Ok(events)
}

fn decode_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<AmountEvent> {
    let kind_text: String = row.get(3)?;
    let confidence_text: String = row.get(6)?;
    Ok(AmountEvent {
        id: row.get(0)?,
        spool_id: row.get(1)?,
        sequence: row.get(2)?,
        kind: decode_enum(&kind_text).map_err(|error| from_sql_error(3, error))?,
        before_mg: row.get(4)?,
        after_mg: row.get(5)?,
        confidence_after: decode_enum(&confidence_text).map_err(|error| from_sql_error(6, error))?,
        gross_mg: row.get(7)?,
        tare_mg: row.get(8)?,
        reservation_id: row.get(9)?,
        note: row.get(10)?,
        occurred_at: row.get(11)?,
        is_correction: false,
    })
}

fn from_sql_error(column: usize, error: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_entry_passes_its_confidence_and_amount_through_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let mut connection = rusqlite::Connection::open(temp.path().join("t.sqlite3")).unwrap();
        let tx = connection.transaction().unwrap();
        let (after_mg, confidence, snapshot) = resolve_entry(
            &tx,
            &AmountEntry::Net {
                net_mg: 500_000,
                confidence: AmountConfidence::Estimated,
            },
        )
        .unwrap();
        assert_eq!(after_mg, 500_000);
        assert_eq!(confidence, AmountConfidence::Estimated);
        assert_eq!(snapshot, LedgerSnapshot::default());
    }

    #[test]
    fn net_entry_out_of_range_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let mut connection = rusqlite::Connection::open(temp.path().join("t.sqlite3")).unwrap();
        let tx = connection.transaction().unwrap();
        let error = resolve_entry(
            &tx,
            &AmountEntry::Net {
                net_mg: -1,
                confidence: AmountConfidence::Measured,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            RepositoryError::Validation {
                field_path: "entry.netMg"
            }
        ));
    }

    #[test]
    fn scale_entry_requires_exactly_one_of_tare_id_or_tare_mg() {
        let temp = tempfile::tempdir().unwrap();
        let mut connection = rusqlite::Connection::open(temp.path().join("t.sqlite3")).unwrap();
        let tx = connection.transaction().unwrap();

        let neither = resolve_entry(
            &tx,
            &AmountEntry::Scale {
                gross_mg: 100_000,
                tare_id: None,
                tare_mg: None,
            },
        )
        .unwrap_err();
        assert!(matches!(
            neither,
            RepositoryError::Validation {
                field_path: "entry.tareId"
            }
        ));

        let both = resolve_entry(
            &tx,
            &AmountEntry::Scale {
                gross_mg: 100_000,
                tare_id: Some("tar-missing".to_string()),
                tare_mg: Some(10_000),
            },
        )
        .unwrap_err();
        assert!(matches!(
            both,
            RepositoryError::Validation {
                field_path: "entry.tareId"
            }
        ));
    }

    #[test]
    fn scale_entry_with_ad_hoc_tare_mg_computes_net() {
        let temp = tempfile::tempdir().unwrap();
        let mut connection = rusqlite::Connection::open(temp.path().join("t.sqlite3")).unwrap();
        let tx = connection.transaction().unwrap();
        let (after_mg, confidence, snapshot) = resolve_entry(
            &tx,
            &AmountEntry::Scale {
                gross_mg: 752_300,
                tare_id: None,
                tare_mg: Some(140_000),
            },
        )
        .unwrap();
        assert_eq!(after_mg, 612_300);
        assert_eq!(confidence, AmountConfidence::Measured);
        assert_eq!(snapshot.gross_mg, Some(752_300));
        assert_eq!(snapshot.tare_mg, Some(140_000));
        assert_eq!(snapshot.tare_id, None);
    }
}
