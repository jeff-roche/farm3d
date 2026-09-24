//! D1: the Project repository — `library_projects` reads/writes and the
//! `project_models` membership primitives later P4 tasks (import, the
//! Model list, commands) compose inside their own transaction. Every
//! function here takes the caller's `&Transaction`/`&Connection` rather
//! than opening its own, matching `spools::repository`/`tares` (P3's
//! pattern for library-style writes: free functions over `&Transaction`
//! that return `RepositoryError`, run through `Storage::write_repo`).

use std::collections::HashMap;

use rusqlite::{params, Connection, Transaction};

use crate::persistence::{RepositoryError, StorageError};
use crate::printers::now_rfc3339;

use super::ProjectRecord;

/// D1: a Project name is unique case-insensitively. 1-128 characters,
/// trimmed — the same rule [`super::validate_project_name`] enforces at the
/// command layer, checked again here so a caller that reaches the
/// repository directly (every test in this module, and any later task that
/// doesn't route through a command) can't write an invalid name.
fn validated_name(name: &str) -> Result<String, RepositoryError> {
    let trimmed = name.trim().to_string();
    let len = trimmed.chars().count();
    if !(1..=128).contains(&len) {
        return Err(RepositoryError::Validation { field_path: "name" });
    }
    Ok(trimmed)
}

/// D1: "unique, compared case-insensitively" with full Unicode case
/// folding (`str::to_lowercase`), not SQLite's ASCII-only `lower()`. The
/// migration's `library_projects_name` unique index on `lower(name)` is
/// only a backstop against a race between this check and the write (it
/// only catches ASCII-fold collisions on its own).
pub fn project_name_taken(
    tx: &Transaction<'_>,
    name: &str,
    excluding: Option<&str>,
) -> Result<bool, StorageError> {
    let folded = name.to_lowercase();
    let mut statement =
        tx.prepare("SELECT name FROM library_projects WHERE (?1 IS NULL OR id != ?1)")?;
    let names = statement
        .query_map(params![excluding], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(names.iter().any(|other| other.to_lowercase() == folded))
}

fn decode_project_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectRecord> {
    Ok(ProjectRecord {
        id: row.get(0)?,
        revision: row.get(1)?,
        name: row.get(2)?,
        model_count: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

/// The one derivation query behind [`list_projects`] and the
/// insert/rename read-back below: `filter` is a `WHERE` clause over `p`
/// (`library_projects`), and `model_count` is a correlated `COUNT` over
/// `project_models` — the whole of Project membership (D1), so there is no
/// separate membership table to join and de-duplicate.
fn query_project_records<P: rusqlite::Params>(
    connection: &Connection,
    filter: &str,
    params: P,
) -> Result<Vec<ProjectRecord>, StorageError> {
    let query = format!(
        "SELECT p.id, p.revision, p.name,
                (SELECT COUNT(*) FROM project_models m WHERE m.project_id = p.id),
                p.created_at, p.updated_at
         FROM library_projects p
         WHERE {filter}
         ORDER BY lower(p.name), p.id"
    );
    let mut statement = connection.prepare(&query)?;
    let rows = statement
        .query_map(params, decode_project_record)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// D1's Project list, case-insensitive name order, each with its
/// membership count.
pub fn list_projects(connection: &Connection) -> Result<Vec<ProjectRecord>, StorageError> {
    query_project_records(connection, "1 = 1", [])
}

fn load_project_record(
    connection: &Connection,
    id: &str,
) -> Result<Option<ProjectRecord>, StorageError> {
    Ok(query_project_records(connection, "p.id = ?1", [id])?
        .into_iter()
        .next())
}

/// D1: creates a Project. `id` is the caller's (`new_id("prj")` in normal
/// use; a fixed value in tests), so this never allocates one itself — that
/// keeps id generation testable and lets a future import path pick a
/// stable id. Starts at `revision` 1 with no membership.
pub fn insert_project(
    tx: &Transaction<'_>,
    id: &str,
    name: &str,
) -> Result<ProjectRecord, RepositoryError> {
    let name = validated_name(name)?;
    if project_name_taken(tx, &name, None)? {
        return Err(RepositoryError::Validation { field_path: "name" });
    }
    let now = now_rfc3339();
    tx.execute(
        "INSERT INTO library_projects(id, revision, name, created_at, updated_at)
         VALUES (?1, 1, ?2, ?3, ?3)",
        params![id, name, now],
    )?;
    load_project_record(tx, id)?.ok_or_else(|| RepositoryError::NotFound {
        entity_id: id.to_string(),
    })
}

/// D1: renames Project `id`, after the optimistic-concurrency check against
/// `expected_revision` (P2's `CONFLICT` shape) and the case-insensitive
/// uniqueness check (excluding this Project itself).
pub fn rename_project(
    tx: &Transaction<'_>,
    id: &str,
    expected_revision: i64,
    name: &str,
) -> Result<ProjectRecord, RepositoryError> {
    if expected_revision <= 0 {
        return Err(RepositoryError::Validation {
            field_path: "expectedRevision",
        });
    }
    let name = validated_name(name)?;
    let current = load_project_record(tx, id)?.ok_or_else(|| RepositoryError::NotFound {
        entity_id: id.to_string(),
    })?;
    if current.revision != expected_revision {
        return Err(RepositoryError::Conflict {
            entity_id: id.to_string(),
            expected_revision,
            current_revision: current.revision,
        });
    }
    if project_name_taken(tx, &name, Some(id))? {
        return Err(RepositoryError::Validation { field_path: "name" });
    }
    let now = now_rfc3339();
    tx.execute(
        "UPDATE library_projects SET name = ?2, revision = revision + 1, updated_at = ?3
         WHERE id = ?1",
        params![id, name, now],
    )?;
    load_project_record(tx, id)?.ok_or_else(|| RepositoryError::NotFound {
        entity_id: id.to_string(),
    })
}

/// D1: every Project id a Model belongs to, ordered by Project name
/// (case-insensitive) then id — the order `ModelRecord.projectIds` uses.
pub fn project_ids_for(
    connection: &Connection,
    model_id: &str,
) -> Result<Vec<String>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT p.id FROM project_models m
         JOIN library_projects p ON p.id = m.project_id
         WHERE m.model_id = ?1
         ORDER BY lower(p.name), p.id",
    )?;
    let ids = statement
        .query_map([model_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

/// [`project_ids_for`], for every Model in one query — the list-read
/// version, so a Model list doesn't run one membership query per row
/// (N+1).
pub fn project_ids_by_model(
    connection: &Connection,
) -> Result<HashMap<String, Vec<String>>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT m.model_id, p.id FROM project_models m
         JOIN library_projects p ON p.id = m.project_id
         ORDER BY m.model_id, lower(p.name), p.id",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut by_model: HashMap<String, Vec<String>> = HashMap::new();
    for (model_id, project_id) in rows {
        by_model.entry(model_id).or_default().push(project_id);
    }
    Ok(by_model)
}

/// D1: whether Project `id` exists, checked up front by [`apply_membership`]
/// so an unknown `add` id fails as [`RepositoryError::NotFound`] rather than
/// as an opaque foreign-key error, and so the whole call — including any
/// other, valid, adds/removes — writes nothing (the caller's
/// `Storage::write_repo` rolls the transaction back on `Err`).
fn project_exists(tx: &Transaction<'_>, id: &str) -> Result<bool, StorageError> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM library_projects WHERE id = ?1)",
        [id],
        |row| row.get(0),
    )
    .map_err(StorageError::from)
}

/// D1: the result of [`apply_membership`] — whether any `project_models`
/// row actually changed, and which Project ids were touched (added to or
/// removed from), for a caller that needs to emit `library.project.changed`
/// per gained/lost Project.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MembershipChange {
    pub changed: bool,
    pub touched_projects: Vec<String>,
}

/// D1: applies a set of Project-membership adds and removes for one Model
/// in a single call. Membership is a set (`INSERT OR IGNORE`): adding to a
/// Project the Model already belongs to is a successful no-op. Every `add`
/// id must name an existing Project, checked before any write — an unknown
/// one is [`RepositoryError::NotFound`] and the whole call writes nothing.
/// Removing from a Project the Model doesn't belong to (or that doesn't
/// exist) is also a no-op, since `DELETE` on a missing row is harmless.
pub fn apply_membership(
    tx: &Transaction<'_>,
    model_id: &str,
    add: &[String],
    remove: &[String],
) -> Result<MembershipChange, RepositoryError> {
    for project_id in add {
        if !project_exists(tx, project_id)? {
            return Err(RepositoryError::NotFound {
                entity_id: project_id.clone(),
            });
        }
    }
    let now = now_rfc3339();
    let mut changed = false;
    let mut touched_projects = Vec::new();
    for project_id in add {
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO project_models(project_id, model_id, added_at)
             VALUES (?1, ?2, ?3)",
            params![project_id, model_id, now],
        )?;
        if inserted > 0 {
            changed = true;
            touched_projects.push(project_id.clone());
        }
    }
    for project_id in remove {
        let deleted = tx.execute(
            "DELETE FROM project_models WHERE project_id = ?1 AND model_id = ?2",
            params![project_id, model_id],
        )?;
        if deleted > 0 {
            changed = true;
            touched_projects.push(project_id.clone());
        }
    }
    Ok(MembershipChange {
        changed,
        touched_projects,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert_project(
        storage: &crate::persistence::Storage,
        id: &str,
        name: &str,
    ) -> ProjectRecord {
        storage
            .write_repo(|tx| super::insert_project(tx, id, name))
            .expect("insert project")
    }

    /// `Storage::read`'s closure must return a plain `rusqlite::Result<T>`,
    /// but every read helper above returns `Result<T, StorageError>` (the
    /// brief's `list_projects(&Connection) -> Result<_, StorageError>`
    /// shape). Nests the call and flattens it, matching
    /// `printers::repository::list`/`get`'s precedent for calling a
    /// `StorageError`-returning free function through `Storage::read`.
    fn read<T>(
        storage: &crate::persistence::Storage,
        operation: impl FnOnce(&Connection) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        storage
            .read(|connection| Ok(operation(connection)))
            .and_then(|inner| inner)
    }

    /// Seeds a `library_models` row directly with raw SQL — Task 2 has no
    /// Model repository yet (that's later tasks' job), and
    /// `apply_membership` needs a real Model to reference.
    fn seed_model(storage: &crate::persistence::Storage, id: &str) {
        storage
            .write(|tx| {
                tx.execute(
                    "INSERT INTO library_models(
                        id, revision, name, format, storage_mode, created_at, updated_at
                     ) VALUES (?1, 1, 'Model', 'stl', 'managed', '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
                    [id],
                )?;
                Ok(())
            })
            .expect("seed model");
    }

    #[test]
    fn insert_project_then_list_projects_returns_model_count_zero() {
        let (_temp, _lease, storage) = crate::test_storage();
        insert_project(&storage, "prj-a", "Miniatures");

        let projects = read(&storage, super::list_projects).expect("list projects");

        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, "prj-a");
        assert_eq!(projects[0].name, "Miniatures");
        assert_eq!(projects[0].model_count, 0);
        assert_eq!(projects[0].revision, 1);
    }

    #[test]
    fn rename_project_with_a_stale_revision_is_a_conflict() {
        let (_temp, _lease, storage) = crate::test_storage();
        insert_project(&storage, "prj-a", "Miniatures");

        let error = storage
            .write_repo(|tx| super::rename_project(tx, "prj-a", 7, "Terrain"))
            .expect_err("stale revision must conflict");

        assert!(matches!(
            error,
            RepositoryError::Conflict {
                entity_id,
                expected_revision: 7,
                current_revision: 1,
            } if entity_id == "prj-a"
        ));
    }

    #[test]
    fn rename_project_succeeds_and_bumps_the_revision() {
        let (_temp, _lease, storage) = crate::test_storage();
        insert_project(&storage, "prj-a", "Miniatures");

        let renamed = storage
            .write_repo(|tx| super::rename_project(tx, "prj-a", 1, "Terrain"))
            .expect("rename");

        assert_eq!(renamed.name, "Terrain");
        assert_eq!(renamed.revision, 2);
    }

    #[test]
    fn a_unicode_case_fold_duplicate_name_is_rejected() {
        let (_temp, _lease, storage) = crate::test_storage();
        insert_project(&storage, "prj-a", "Ärger");

        let error = storage
            .write_repo(|tx| super::insert_project(tx, "prj-b", "ärger"))
            .expect_err("unicode case-fold duplicate must be rejected");

        assert!(matches!(
            error,
            RepositoryError::Validation { field_path: "name" }
        ));
    }

    #[test]
    fn insert_project_trims_the_name_before_writing_it() {
        let (_temp, _lease, storage) = crate::test_storage();

        let created = insert_project(&storage, "prj-a", " x ");

        assert_eq!(created.name, "x");
    }

    #[test]
    fn insert_project_rejects_a_name_that_is_blank_after_trimming() {
        let (_temp, _lease, storage) = crate::test_storage();

        let error = storage
            .write_repo(|tx| super::insert_project(tx, "prj-a", "   "))
            .expect_err("a blank name must be rejected");

        assert!(matches!(
            error,
            RepositoryError::Validation { field_path: "name" }
        ));
    }

    #[test]
    fn apply_membership_adding_an_existing_membership_is_a_no_op() {
        let (_temp, _lease, storage) = crate::test_storage();
        insert_project(&storage, "prj-a", "Miniatures");
        seed_model(&storage, "mdl-a");
        storage
            .write_repo(|tx| super::apply_membership(tx, "mdl-a", &["prj-a".to_string()], &[]))
            .expect("first add");

        let change = storage
            .write_repo(|tx| super::apply_membership(tx, "mdl-a", &["prj-a".to_string()], &[]))
            .expect("second add is a no-op");

        assert_eq!(
            change,
            MembershipChange {
                changed: false,
                touched_projects: vec![],
            }
        );
    }

    #[test]
    fn apply_membership_add_and_remove_in_one_call_applies_both() {
        let (_temp, _lease, storage) = crate::test_storage();
        insert_project(&storage, "prj-a", "A");
        insert_project(&storage, "prj-b", "B");
        seed_model(&storage, "mdl-a");
        storage
            .write_repo(|tx| super::apply_membership(tx, "mdl-a", &["prj-a".to_string()], &[]))
            .expect("seed membership in prj-a");

        let change = storage
            .write_repo(|tx| {
                super::apply_membership(tx, "mdl-a", &["prj-b".to_string()], &["prj-a".to_string()])
            })
            .expect("add and remove");

        assert!(change.changed);
        let mut touched = change.touched_projects.clone();
        touched.sort();
        assert_eq!(touched, vec!["prj-a".to_string(), "prj-b".to_string()]);

        let ids =
            read(&storage, |conn| super::project_ids_for(conn, "mdl-a")).expect("project ids");
        assert_eq!(ids, vec!["prj-b".to_string()]);
    }

    #[test]
    fn apply_membership_with_an_unknown_project_id_writes_nothing() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_model(&storage, "mdl-a");

        let error = storage
            .write_repo(|tx| {
                super::apply_membership(tx, "mdl-a", &["prj-missing".to_string()], &[])
            })
            .expect_err("unknown project must be rejected");

        assert!(matches!(
            error,
            RepositoryError::NotFound { entity_id } if entity_id == "prj-missing"
        ));
        let ids =
            read(&storage, |conn| super::project_ids_for(conn, "mdl-a")).expect("project ids");
        assert!(ids.is_empty(), "nothing must have been written");
    }

    #[test]
    fn project_ids_for_orders_by_project_name() {
        let (_temp, _lease, storage) = crate::test_storage();
        insert_project(&storage, "prj-z", "Zeta");
        insert_project(&storage, "prj-a", "Alpha");
        insert_project(&storage, "prj-m", "Mid");
        seed_model(&storage, "mdl-a");
        storage
            .write_repo(|tx| {
                super::apply_membership(
                    tx,
                    "mdl-a",
                    &[
                        "prj-z".to_string(),
                        "prj-a".to_string(),
                        "prj-m".to_string(),
                    ],
                    &[],
                )
            })
            .expect("add all three");

        let ids =
            read(&storage, |conn| super::project_ids_for(conn, "mdl-a")).expect("project ids");

        assert_eq!(
            ids,
            vec![
                "prj-a".to_string(),
                "prj-m".to_string(),
                "prj-z".to_string()
            ]
        );
    }

    #[test]
    fn project_ids_by_model_matches_project_ids_for_with_one_query() {
        let (_temp, _lease, storage) = crate::test_storage();
        insert_project(&storage, "prj-a", "A");
        seed_model(&storage, "mdl-a");
        seed_model(&storage, "mdl-b");
        storage
            .write_repo(|tx| super::apply_membership(tx, "mdl-a", &["prj-a".to_string()], &[]))
            .expect("membership for mdl-a only");

        let by_model = read(&storage, super::project_ids_by_model).expect("project ids by model");

        assert_eq!(by_model.get("mdl-a"), Some(&vec!["prj-a".to_string()]));
        assert_eq!(by_model.get("mdl-b"), None);
    }

    #[test]
    fn new_id_prefixed_project_id_round_trips_through_insert() {
        let (_temp, _lease, storage) = crate::test_storage();
        let id = crate::library::new_id("prj");

        let created = insert_project(&storage, &id, "Generated Id");

        assert_eq!(created.id, id);
    }
}
