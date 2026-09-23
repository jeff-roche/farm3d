//! D2 (host identity): the canonical `host:port` key used to detect two
//! Printer Connections that point at the same physical network endpoint,
//! independent of adapter kind or TLS. See the P2 design spec, section D2.

/// Builds the canonical `host:port` identity for a Connection, or `None`
/// when `host` is empty once trimmed.
///
/// Protocol and TLS are deliberately excluded — two adapters on the same
/// host:port are the same physical endpoint. The identity is not
/// DNS-resolved: `printer.local` and `192.168.1.20` remain distinct even
/// when they name the same device.
pub fn canonical_host_identity(host: &str, port: u16) -> Option<String> {
    let trimmed = host.trim();
    let unbracketed = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(trimmed);
    let lowered = unbracketed.to_ascii_lowercase();
    let host = lowered.strip_suffix('.').unwrap_or(&lowered);
    if host.is_empty() {
        return None;
    }
    Some(match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(v6)) => format!("[{v6}]:{port}"),
        Ok(std::net::IpAddr::V4(v4)) => format!("{v4}:{port}"),
        Err(_) => format!("{host}:{port}"),
    })
}

/// D3's duplicate-host grouping, shared by the Printers import path
/// (`import_printers`, ahead of `PrinterRepository::replace_all`).
///
/// Walks `printers` in the order given — for an import document that's
/// document order, which is what decides "later" here, since a freshly
/// imported row's `created_at` is blank until the repository backfills it
/// during the write that follows. Within that order, the first active
/// (non-archived, non-`None` `archived_at`) Printer to claim a host
/// identity keeps it; every later active Printer sharing that identity is
/// archived in place (`archived_at` set to the same "now" for the whole
/// pass) so the write path never has to reject the import for a duplicate
/// the partial unique index alone would have caught.
///
/// Returns each `(kept printer id, archived printer id)` pair, in the order
/// the duplicates were found — the caller (`import_printers`) turns each
/// pair's archived id into an `OperationWarningCode::DuplicateHostArchived`
/// warning.
///
/// This mirrors the migration's own duplicate-host backfill
/// (`backfill_host_identity` in `persistence::migrations`), but isn't
/// reused by it: the migration operates on raw `(id, host, port)` SQL rows
/// read directly off `printers_json`-less columns (no `StoredPrinter` is
/// ever decoded there), orders by `created_at` — which is meaningful for
/// existing rows — and writes its own `migration_warnings` ledger rows with
/// a different message shape. Refactoring it onto this function would
/// change what it reads and how "oldest wins" is decided, which is outside
/// this task's scope; the SQL (and therefore its checksum) is unaffected
/// either way.
pub fn archive_duplicates(
    printers: &mut [crate::printers::StoredPrinter],
) -> Vec<(String, String)> {
    let mut kept_by_identity: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut archived_pairs = Vec::new();
    let now = crate::printers::now_rfc3339();
    for printer in printers.iter_mut() {
        if printer.archived_at.is_some() {
            continue;
        }
        let Some(identity) = printer
            .connection
            .as_ref()
            .and_then(|connection| canonical_host_identity(&connection.host, connection.port))
        else {
            continue;
        };
        match kept_by_identity.get(&identity) {
            Some(kept_id) => {
                archived_pairs.push((kept_id.clone(), printer.id.clone()));
                printer.archived_at = Some(now.clone());
            }
            None => {
                kept_by_identity.insert(identity, printer.id.clone());
            }
        }
    }
    archived_pairs
}

/// A Printer the v3 migration archived because it shared a host identity
/// with an older one (spec "Pre-existing duplicate hosts"). Read back from
/// the `migration_warnings` ledger so the UI can explain why it's archived.
#[derive(serde::Serialize, Clone, Debug, PartialEq, Eq, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/DuplicateHostArchive.ts")]
pub struct DuplicateHostArchive {
    /// The ledger row's id — stable, so the UI can remember a dismissal.
    pub warning_id: String,
    pub archived_printer_id: String,
    pub kept_printer_id: String,
}

/// Lists the migration's `DUPLICATE_HOST_ARCHIVED` ledger rows, oldest
/// first. A row whose details don't name both Printers is skipped rather
/// than failing the read — the ledger is advisory.
pub fn duplicate_host_archives(
    storage: &std::sync::Arc<crate::persistence::Storage>,
) -> Result<Vec<DuplicateHostArchive>, crate::persistence::StorageError> {
    let rows = storage.read(|connection| {
        let mut statement = connection.prepare(
            "SELECT id, details_json FROM migration_warnings
             WHERE code = 'DUPLICATE_HOST_ARCHIVED'
             ORDER BY created_at, id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })?;
    Ok(rows
        .into_iter()
        .filter_map(|(warning_id, details)| {
            let details: serde_json::Value = serde_json::from_str(&details).ok()?;
            Some(DuplicateHostArchive {
                warning_id,
                archived_printer_id: details["archivedPrinterId"].as_str()?.to_string(),
                kept_printer_id: details["keptPrinterId"].as_str()?.to_string(),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{archive_duplicates, canonical_host_identity};
    use crate::connections::ConnectionConfig;
    use crate::printers::StoredPrinter;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Vector {
        host: String,
        port: u16,
        expected: Option<String>,
    }

    /// The shared Rust/TypeScript conformance vectors for D2 — see
    /// `tests/fixtures/host-identity.json`.
    #[test]
    fn matches_every_fixture_vector() {
        let raw = include_str!("../../tests/fixtures/host-identity.json");
        let vectors: Vec<Vector> = serde_json::from_str(raw).expect("fixture vectors parse");

        for vector in vectors {
            assert_eq!(
                canonical_host_identity(&vector.host, vector.port),
                vector.expected,
                "host={:?} port={}",
                vector.host,
                vector.port
            );
        }
    }

    fn printer(id: &str, host: Option<&str>) -> StoredPrinter {
        StoredPrinter {
            id: id.to_string(),
            name: id.to_string(),
            connection: host.map(|host| ConnectionConfig {
                kind: "moonraker".to_string(),
                host: host.to_string(),
                port: 7125,
                use_tls: false,
                credential_ref: None,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn keeps_the_first_active_printer_in_slice_order_and_archives_the_rest() {
        let mut printers = vec![
            printer("prn-a", Some("dup.invalid")),
            printer("prn-b", Some("other.invalid")),
            printer("prn-c", Some("dup.invalid")),
            printer("prn-d", Some("dup.invalid")),
        ];

        let pairs = archive_duplicates(&mut printers);

        assert_eq!(
            pairs,
            vec![
                ("prn-a".to_string(), "prn-c".to_string()),
                ("prn-a".to_string(), "prn-d".to_string()),
            ]
        );
        assert_eq!(printers[0].archived_at, None);
        assert_eq!(printers[1].archived_at, None);
        assert!(printers[2].archived_at.is_some());
        assert!(printers[3].archived_at.is_some());
    }

    #[test]
    fn ignores_already_archived_printers_and_printers_without_a_connection() {
        let mut archived_dup = printer("prn-archived", Some("dup.invalid"));
        archived_dup.archived_at = Some("2026-09-01T00:00:00.000Z".to_string());
        let mut printers = vec![
            printer("prn-kept", Some("dup.invalid")),
            archived_dup,
            printer("prn-profile-only", None),
        ];

        let pairs = archive_duplicates(&mut printers);

        assert!(pairs.is_empty());
        assert_eq!(printers[0].archived_at, None);
        assert_eq!(
            printers[1].archived_at.as_deref(),
            Some("2026-09-01T00:00:00.000Z")
        );
        assert_eq!(printers[2].archived_at, None);
    }
}
