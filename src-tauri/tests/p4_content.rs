//! P4 Task 3: the content-addressed library content store (spec D2, D3,
//! D4). Covers staging with streaming SHA-256, the source-changed and
//! size/type guards, cancellation, placement and commit under the placement
//! lock, crash recovery through the startup sweep, post-commit blob
//! cleanup (including the re-import race guard), and verified reads.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use farm3d_lib::contracts::command::{CommandError, ErrorCode};
use farm3d_lib::library::content::{
    mark_unreferenced_blobs, CancelFlag, ContentError, ContentFailurePoint, ContentStore,
    StagedFile, MAX_SOURCE_BYTES,
};
use farm3d_lib::persistence::{
    MetadataRootLease, RepositoryError, Storage, StorageError, StoragePaths,
};

const MIB: usize = 1024 * 1024;

struct Fixture {
    temp: tempfile::TempDir,
    _lease: MetadataRootLease,
    storage: Storage,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temporary root");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
        let storage = Storage::open(paths, &lease).expect("storage");
        Self {
            temp,
            _lease: lease,
            storage,
        }
    }

    fn content_root(&self) -> PathBuf {
        self.storage.paths().content_root().to_path_buf()
    }

    fn store(&self) -> ContentStore {
        ContentStore::open(self.storage.paths().content_root()).expect("content store")
    }

    /// Writes `bytes` to a source file outside farm3d's trees.
    fn source(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.temp.path().join(name);
        fs::write(&path, bytes).expect("source file");
        path
    }

    fn blob_path(&self, sha256: &str) -> PathBuf {
        self.content_root()
            .join("blobs/sha256")
            .join(&sha256[..2])
            .join(sha256)
    }

    fn staging_entries(&self, key: &str) -> Vec<PathBuf> {
        match fs::read_dir(self.content_root().join("staging").join(key)) {
            Ok(entries) => entries.map(|entry| entry.expect("entry").path()).collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("staging unreadable: {error}"),
        }
    }

    fn count(&self, sql: &str, sha256: &str) -> i64 {
        self.storage
            .read(|connection| connection.query_row(sql, [sha256], |row| row.get(0)))
            .expect("count")
    }

    fn blob_rows(&self, sha256: &str) -> i64 {
        self.count(
            "SELECT COUNT(*) FROM content_blobs WHERE sha256 = ?1",
            sha256,
        )
    }

    fn pending_rows(&self, sha256: &str) -> i64 {
        self.count(
            "SELECT COUNT(*) FROM pending_blob_cleanup WHERE sha256 = ?1",
            sha256,
        )
    }
}

/// Deterministic, non-repeating-per-MiB bytes so a misplaced chunk changes
/// the hash.
fn pattern(len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| ((index / 7) as u8).wrapping_mul(31) ^ (index as u8))
        .collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn stage(
    store: &ContentStore,
    source: &Path,
    key: &str,
    index: usize,
) -> Result<StagedFile, ContentError> {
    store.stage_from_path(source, key, index, &CancelFlag::never(), &mut |_, _| {})
}

/// Places `staged` with a commit closure that writes nothing beyond the
/// blob rows `place_and_commit` inserts itself.
fn place(fixture: &Fixture, store: &ContentStore, staged: &StagedFile) {
    store
        .place_and_commit(&fixture.storage, &[staged], |_| Ok(()))
        .expect("placement");
}

fn make_writable(path: &Path) {
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions).expect("writable");
}

// 1.
#[test]
fn stage_from_path_hashes_while_copying_into_staging() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let bytes = pattern(3 * MIB);
    let source = fixture.source("model.stl", &bytes);
    let mut calls = Vec::new();

    let staged = store
        .stage_from_path(
            &source,
            "sel-one",
            0,
            &CancelFlag::never(),
            &mut |copied, total| calls.push((copied, total)),
        )
        .expect("staged");

    assert_eq!(staged.sha256, sha256_hex(&bytes));
    assert_eq!(staged.size, bytes.len() as u64);
    assert_eq!(
        staged.path,
        fixture.content_root().join("staging/sel-one/0.part")
    );
    assert_eq!(fs::read(&staged.path).expect("staged bytes"), bytes);
    let total = bytes.len() as u64;
    assert_eq!(
        calls,
        vec![(MIB as u64, total), (2 * MIB as u64, total), (total, total)]
    );
}

#[test]
fn stage_bytes_stages_a_named_non_part_file() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let bytes = b"\x89PNG thumbnail";

    let staged = store
        .stage_bytes(bytes, "sel-one", "0.thumb.png")
        .expect("staged");

    assert_eq!(staged.sha256, sha256_hex(bytes));
    assert_eq!(staged.size, bytes.len() as u64);
    assert_eq!(
        staged.path,
        fixture.content_root().join("staging/sel-one/0.thumb.png")
    );
    assert_eq!(fs::read(&staged.path).expect("staged bytes"), bytes);
}

#[test]
fn staging_keys_and_names_cannot_escape_staging() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let source = fixture.source("model.stl", b"solid x");

    for key in ["", "..", "../blobs", "a/b", ".hidden"] {
        assert!(matches!(
            stage(&store, &source, key, 0),
            Err(ContentError::Io)
        ));
    }
    for name in ["", "..", "../x.png", "a/b.png", ".x", "0.part"] {
        assert!(matches!(
            store.stage_bytes(b"x", "sel-one", name),
            Err(ContentError::Io)
        ));
    }
}

// 2.
#[test]
fn a_source_changed_between_copy_and_restat_is_rejected_and_unstaged() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let source = fixture.source("model.stl", &pattern(64 * 1024));
    store.before_restat_once(Box::new(|path: &Path| {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(path)
            .expect("source");
        file.write_all(b"appended").expect("append");
    }));

    let result = stage(&store, &source, "sel-changed", 0);

    assert!(matches!(result, Err(ContentError::ChangedDuringRead)));
    assert!(fixture.staging_entries("sel-changed").is_empty());
}

// 3.
#[test]
fn an_oversized_source_is_too_large_before_any_read() {
    let fixture = Fixture::new();
    let store = fixture.store().with_max_bytes(1024);
    let source = fixture.source("big.stl", &pattern(2048));
    let mut progress_calls = 0;

    let result = store.stage_from_path(&source, "sel-big", 0, &CancelFlag::never(), &mut |_, _| {
        progress_calls += 1
    });

    assert!(matches!(result, Err(ContentError::TooLarge)));
    assert_eq!(progress_calls, 0);
    assert!(!fixture.content_root().join("staging/sel-big").exists());
    assert_eq!(MAX_SOURCE_BYTES, 1024 * 1024 * 1024);
}

#[test]
fn a_directory_source_is_not_a_file() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let directory = fixture.temp.path().join("folder.stl");
    fs::create_dir(&directory).expect("directory");

    assert!(matches!(
        stage(&store, &directory, "sel-dir", 0),
        Err(ContentError::NotAFile)
    ));
    assert!(matches!(
        stage(
            &store,
            &fixture.temp.path().join("absent.stl"),
            "sel-dir",
            0
        ),
        Err(ContentError::Unreadable(std::io::ErrorKind::NotFound))
    ));
}

// 4.
#[test]
fn cancelling_after_the_first_chunk_leaves_nothing_staged() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let source = fixture.source("model.stl", &pattern(3 * MIB));
    let (cancel_sender, cancel_receiver) = tokio::sync::watch::channel(false);
    let cancel = CancelFlag::new(cancel_receiver);
    let mut progress_calls = 0;

    let result = store.stage_from_path(&source, "sel-cancel", 0, &cancel, &mut |copied, _| {
        progress_calls += 1;
        assert_eq!(copied, MIB as u64);
        cancel_sender.send(true).expect("cancel");
    });

    assert!(matches!(result, Err(ContentError::Cancelled)));
    assert_eq!(progress_calls, 1);
    assert!(fixture.staging_entries("sel-cancel").is_empty());
}

#[test]
fn the_pause_hook_holds_a_copy_after_its_first_chunk_until_released() {
    let fixture = Fixture::new();
    let store = Arc::new(fixture.store());
    let bytes = pattern(3 * MIB);
    let source = fixture.source("model.stl", &bytes);
    let pause = store.pause_after_first_chunk_once();

    let worker = {
        let store = Arc::clone(&store);
        std::thread::spawn(move || stage(&store, &source, "sel-pause", 0))
    };
    pause.wait_until_reached();
    let part = fixture.content_root().join("staging/sel-pause/0.part");
    assert_eq!(fs::metadata(&part).expect("part").len(), MIB as u64);
    pause.release();

    let staged = worker.join().expect("worker").expect("staged");
    assert_eq!(staged.sha256, sha256_hex(&bytes));
}

#[test]
fn truncate_staged_once_damages_only_the_next_staged_copy() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let bytes = pattern(4096);
    let source = fixture.source("model.stl", &bytes);
    store.inject_failure_once(ContentFailurePoint::TruncateStagedOnce);

    let damaged = stage(&store, &source, "sel-trunc", 0).expect("staged");
    let intact = stage(&store, &source, "sel-trunc", 1).expect("staged");

    assert_eq!(damaged.size, 4096);
    assert_eq!(fs::metadata(&damaged.path).expect("part").len(), 2048);
    assert_eq!(fs::metadata(&intact.path).expect("part").len(), 4096);
}

// 5.
#[test]
fn placement_creates_a_read_only_blob_and_commits_the_closure_rows() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let bytes = pattern(10_000);
    let source = fixture.source("model.stl", &bytes);
    let first = stage(&store, &source, "sel-place", 0).expect("staged");
    let second = stage(&store, &source, "sel-place", 1).expect("staged");

    let committed = store
        .place_and_commit(&fixture.storage, &[&first], |transaction| {
            transaction.execute(
                "INSERT INTO library_projects (id, revision, name, created_at, updated_at)
                 VALUES ('prj-placed', 1, 'Placed', 'now', 'now')",
                [],
            )?;
            Ok(7)
        })
        .expect("placed");

    assert_eq!(committed, 7);
    let blob = fixture.blob_path(&first.sha256);
    assert_eq!(fs::read(&blob).expect("blob"), bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&blob).expect("blob").permissions().mode();
        assert_eq!(mode & 0o777, 0o400);
    }
    #[cfg(not(unix))]
    assert!(fs::metadata(&blob).expect("blob").permissions().readonly());
    assert_eq!(fixture.blob_rows(&first.sha256), 1);
    let projects: i64 = fixture
        .storage
        .read(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM library_projects WHERE id = 'prj-placed'",
                [],
                |row| row.get(0),
            )
        })
        .expect("projects");
    assert_eq!(projects, 1);
    assert!(!first.path.exists());

    place(&fixture, &store, &second);

    assert!(!second.path.exists());
    let blobs: Vec<_> = fs::read_dir(blob.parent().expect("hh dir"))
        .expect("hh dir")
        .collect();
    assert_eq!(blobs.len(), 1);
    assert_eq!(fixture.blob_rows(&first.sha256), 1);
}

#[test]
fn a_failing_commit_closure_rolls_back_and_surfaces_its_repository_error() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let source = fixture.source("model.stl", &pattern(512));
    let staged = stage(&store, &source, "sel-reject", 0).expect("staged");

    let result: Result<(), ContentError> =
        store.place_and_commit(&fixture.storage, &[&staged], |_| {
            Err(RepositoryError::NotFound {
                entity_id: "prj-missing".to_string(),
            })
        });

    assert!(matches!(
        result,
        Err(ContentError::Repository(RepositoryError::NotFound { ref entity_id }))
            if entity_id == "prj-missing"
    ));
    assert_eq!(fixture.blob_rows(&staged.sha256), 0);
}

// 6.
#[test]
fn a_crash_between_placement_and_commit_leaves_an_orphan_the_sweep_removes() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let source = fixture.source("model.stl", &pattern(2048));
    let staged = stage(&store, &source, "sel-crash", 0).expect("staged");
    store.inject_failure_once(ContentFailurePoint::AfterPlacementBeforeCommit);

    let result = store.place_and_commit(&fixture.storage, &[&staged], |_| Ok(()));

    assert!(result.is_err());
    let blob = fixture.blob_path(&staged.sha256);
    assert!(blob.exists());
    assert_eq!(fixture.blob_rows(&staged.sha256), 0);

    let report = store.startup_sweep(&fixture.storage).expect("sweep");

    assert_eq!(report.orphans_removed, 1);
    assert!(!blob.exists());
}

// 7.
#[test]
fn an_existing_blob_with_the_wrong_size_is_corrupt_data() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let source = fixture.source("model.stl", &pattern(4096));
    let first = stage(&store, &source, "sel-corrupt", 0).expect("staged");
    place(&fixture, &store, &first);
    let blob = fixture.blob_path(&first.sha256);
    make_writable(&blob);
    fs::OpenOptions::new()
        .write(true)
        .open(&blob)
        .expect("blob")
        .set_len(100)
        .expect("truncate");
    let second = stage(&store, &source, "sel-corrupt", 1).expect("staged");

    let error = store
        .place_and_commit(&fixture.storage, &[&second], |_| Ok(()))
        .expect_err("size mismatch");

    assert!(matches!(error, ContentError::HashMismatch));
    assert_eq!(CommandError::from(error).code, ErrorCode::CorruptData);
}

// 8.
#[test]
fn unreferenced_blobs_move_to_pending_cleanup_and_are_released() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let source = fixture.source("model.stl", &pattern(2048));
    let staged = stage(&store, &source, "sel-clean", 0).expect("staged");
    place(&fixture, &store, &staged);
    let sha256 = staged.sha256.clone();

    fixture
        .storage
        .write(|transaction| mark_unreferenced_blobs(transaction, std::slice::from_ref(&sha256)))
        .expect("marked");

    assert_eq!(fixture.pending_rows(&sha256), 1);
    assert_eq!(fixture.blob_rows(&sha256), 0);
    assert!(fixture.blob_path(&sha256).exists());

    let released = store
        .release_unreferenced(&fixture.storage)
        .expect("released");

    assert_eq!(released, 1);
    assert!(!fixture.blob_path(&sha256).exists());
    assert_eq!(fixture.pending_rows(&sha256), 0);
}

#[test]
fn mark_unreferenced_blobs_keeps_blobs_a_revision_still_references() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let source = fixture.source("model.stl", &pattern(2048));
    let staged = stage(&store, &source, "sel-kept", 0).expect("staged");
    let sha256 = staged.sha256.clone();
    store
        .place_and_commit(&fixture.storage, &[&staged], |transaction| {
            transaction.execute(
                "INSERT INTO library_models (id, revision, name, format, storage_mode,
                   created_at, updated_at)
                 VALUES ('mdl-kept', 1, 'Kept', 'stl', 'managed', 'now', 'now')",
                [],
            )?;
            transaction.execute(
                "INSERT INTO model_source_revisions (id, model_id, sequence, content_sha256,
                   size_bytes, format, origin, source_file_name, source_path, captured_at,
                   inspector_version, inspection_json)
                 VALUES ('msr-kept', 'mdl-kept', 1, ?1, 2048, 'stl', 'import', 'model.stl',
                   '/tmp/model.stl', 'now', 1, '{}')",
                [&sha256],
            )?;
            Ok(())
        })
        .expect("placed");

    fixture
        .storage
        .write(|transaction| mark_unreferenced_blobs(transaction, std::slice::from_ref(&sha256)))
        .expect("marked");

    assert_eq!(fixture.blob_rows(&sha256), 1);
    assert_eq!(fixture.pending_rows(&sha256), 0);
}

#[test]
fn a_failed_unlink_stays_pending_until_the_startup_sweep() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let source = fixture.source("model.stl", &pattern(2048));
    let staged = stage(&store, &source, "sel-retry", 0).expect("staged");
    place(&fixture, &store, &staged);
    let sha256 = staged.sha256.clone();
    fixture
        .storage
        .write(|transaction| mark_unreferenced_blobs(transaction, std::slice::from_ref(&sha256)))
        .expect("marked");
    store.inject_failure_once(ContentFailurePoint::BeforeUnlink);

    let released = store
        .release_unreferenced(&fixture.storage)
        .expect("released");

    assert_eq!(released, 0);
    assert!(fixture.blob_path(&sha256).exists());
    assert_eq!(fixture.pending_rows(&sha256), 1);
    let attempts = fixture.count(
        "SELECT attempt_count FROM pending_blob_cleanup WHERE sha256 = ?1",
        &sha256,
    );
    assert_eq!(attempts, 1);

    let report = store.startup_sweep(&fixture.storage).expect("sweep");

    assert_eq!(report.pending_released, 1);
    assert!(!fixture.blob_path(&sha256).exists());
    assert_eq!(fixture.pending_rows(&sha256), 0);
}

// 9.
#[test]
fn a_blob_reimported_before_release_is_kept() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let source = fixture.source("model.stl", &pattern(2048));
    let first = stage(&store, &source, "sel-race", 0).expect("staged");
    place(&fixture, &store, &first);
    let sha256 = first.sha256.clone();
    fixture
        .storage
        .write(|transaction| mark_unreferenced_blobs(transaction, std::slice::from_ref(&sha256)))
        .expect("marked");
    let again = stage(&store, &source, "sel-race", 1).expect("staged");
    place(&fixture, &store, &again);

    let released = store
        .release_unreferenced(&fixture.storage)
        .expect("released");

    assert_eq!(released, 0);
    assert!(fixture.blob_path(&sha256).exists());
    assert_eq!(fixture.blob_rows(&sha256), 1);
    assert_eq!(fixture.pending_rows(&sha256), 0);
}

// 10.
#[test]
fn verified_reads_return_intact_bytes_and_reject_damaged_ones() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let bytes = pattern(3 * MIB + 17);
    let source = fixture.source("model.stl", &bytes);
    let staged = stage(&store, &source, "sel-read", 0).expect("staged");
    place(&fixture, &store, &staged);

    let mut intact = Vec::new();
    store
        .open_verified(&staged.sha256)
        .expect("reader")
        .read_to_end(&mut intact)
        .expect("intact read");
    assert_eq!(intact, bytes);

    let blob = fixture.blob_path(&staged.sha256);
    make_writable(&blob);
    let mut damaged = bytes.clone();
    damaged[MIB + 3] ^= 0x01;
    fs::write(&blob, &damaged).expect("flip a byte");

    let error = store
        .open_verified(&staged.sha256)
        .expect("reader")
        .read_to_end(&mut Vec::new())
        .expect_err("hash mismatch");

    assert!(matches!(
        ContentError::from(error),
        ContentError::HashMismatch
    ));
}

#[test]
fn verified_reads_refuse_malformed_hashes() {
    let fixture = Fixture::new();
    let store = fixture.store();

    for hash in ["", "../../etc/passwd", &"A".repeat(64), &"0".repeat(63)] {
        assert!(matches!(
            store.open_verified(hash),
            Err(ContentError::Unreadable(std::io::ErrorKind::InvalidInput))
        ));
    }
}

// 11.
#[test]
fn the_startup_sweep_empties_staging() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let source = fixture.source("model.stl", &pattern(2048));
    stage(&store, &source, "sel-a", 0).expect("staged");
    store
        .stage_bytes(b"thumb", "sel-b", "0.thumb.png")
        .expect("staged");

    let report = store.startup_sweep(&fixture.storage).expect("sweep");

    assert_eq!(report.staging_removed, 2);
    let staging = fixture.content_root().join("staging");
    assert!(staging.is_dir());
    assert_eq!(fs::read_dir(&staging).expect("staging").count(), 0);
}

#[test]
fn discard_staging_removes_one_key_only() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let source = fixture.source("model.stl", &pattern(2048));
    stage(&store, &source, "sel-a", 0).expect("staged");
    stage(&store, &source, "sel-b", 0).expect("staged");

    store.discard_staging("sel-a");

    assert!(fixture.staging_entries("sel-a").is_empty());
    assert_eq!(fixture.staging_entries("sel-b").len(), 1);
}

#[test]
fn info_counts_blobs_bytes_and_pending_cleanup() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let first = stage(&store, &fixture.source("a.stl", &pattern(1000)), "sel-i", 0).expect("a");
    let second = stage(&store, &fixture.source("b.stl", &pattern(3000)), "sel-i", 1).expect("b");
    store
        .place_and_commit(&fixture.storage, &[&first, &second], |_| Ok(()))
        .expect("placed");
    fixture
        .storage
        .write(|transaction| {
            mark_unreferenced_blobs(transaction, std::slice::from_ref(&first.sha256))
        })
        .expect("marked");

    let info = store.info(&fixture.storage).expect("info");

    assert_eq!(info.blob_count, 1);
    assert_eq!(info.total_bytes, 3000);
    assert_eq!(info.pending_cleanup_count, 1);
}

#[cfg(unix)]
#[test]
fn a_symlinked_blob_directory_is_a_path_collision() {
    let fixture = Fixture::new();
    let content_root = fixture.content_root();
    let elsewhere = fixture.temp.path().join("elsewhere");
    fs::create_dir(&elsewhere).expect("elsewhere");
    std::os::unix::fs::symlink(&elsewhere, content_root.join("blobs")).expect("symlink");

    assert!(matches!(
        ContentStore::open(&content_root),
        Err(StorageError::PathCollision)
    ));
}

/// Review fix: a file the sweep cannot delete must not block startup. The
/// pending blob stays pending (and is not retried as an orphan), and the
/// orphan and staging leftovers are counted as failures for the next sweep.
#[cfg(unix)]
#[test]
fn undeletable_leftovers_do_not_fail_the_startup_sweep() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let store = fixture.store();
    let pending = stage(&store, &fixture.source("a.stl", &pattern(1000)), "sel-p", 0)
        .expect("pending staged");
    place(&fixture, &store, &pending);
    fixture
        .storage
        .write(|transaction| {
            mark_unreferenced_blobs(transaction, std::slice::from_ref(&pending.sha256))
        })
        .expect("marked");
    let orphan =
        stage(&store, &fixture.source("b.stl", &pattern(3000)), "sel-o", 0).expect("orphan staged");
    store.inject_failure_once(ContentFailurePoint::AfterPlacementBeforeCommit);
    assert!(store
        .place_and_commit(&fixture.storage, &[&orphan], |_| Ok(()))
        .is_err());
    stage(
        &store,
        &fixture.source("c.stl", &pattern(500)),
        "sel-stuck",
        0,
    )
    .expect("staged");

    let locked: Vec<PathBuf> = vec![
        fixture
            .blob_path(&pending.sha256)
            .parent()
            .unwrap()
            .to_path_buf(),
        fixture
            .blob_path(&orphan.sha256)
            .parent()
            .unwrap()
            .to_path_buf(),
        fixture.content_root().join("staging/sel-stuck"),
    ];
    let set_mode = |mode: u32| {
        for directory in &locked {
            fs::set_permissions(directory, fs::Permissions::from_mode(mode)).expect("chmod");
        }
    };
    set_mode(0o500);
    if fs::write(locked[2].join("probe"), b"").is_ok() {
        set_mode(0o700);
        eprintln!("skipped: read-only directories are writable here (running as root?)");
        return;
    }

    let report = store.startup_sweep(&fixture.storage);
    set_mode(0o700);
    let report = report.expect("an undeletable file must not fail the sweep");

    // The emptied `sel-p` and `sel-o` keys go; only `sel-stuck` is stuck.
    assert_eq!(report.staging_removed, 2);
    assert_eq!(report.staging_failed, 1);
    assert_eq!(report.pending_released, 0);
    assert_eq!(report.orphans_removed, 0);
    assert_eq!(report.orphans_failed, 1);
    assert!(fixture.blob_path(&pending.sha256).exists());
    assert!(fixture.blob_path(&orphan.sha256).exists());
    assert_eq!(fixture.pending_rows(&pending.sha256), 1);
    let attempts = fixture.count(
        "SELECT attempt_count FROM pending_blob_cleanup WHERE sha256 = ?1",
        &pending.sha256,
    );
    assert_eq!(attempts, 1);

    let retried = store.startup_sweep(&fixture.storage).expect("retry sweep");

    assert_eq!(retried.staging_removed, 1);
    assert_eq!(retried.pending_released, 1);
    assert_eq!(retried.orphans_removed, 1);
    assert!(!fixture.blob_path(&pending.sha256).exists());
    assert!(!fixture.blob_path(&orphan.sha256).exists());
    assert_eq!(fixture.pending_rows(&pending.sha256), 0);
}

/// Two files of one selection are staged at once (D13), so two threads
/// create the same `staging/<key>` directory concurrently. Neither may
/// fail because the other created it first.
#[test]
fn concurrent_staging_into_one_key_never_races_on_the_directory() {
    let fixture = Fixture::new();
    let store = Arc::new(fixture.store());
    for round in 0..50 {
        let key = format!("race-{round}");
        let barrier = Arc::new(std::sync::Barrier::new(4));
        let threads: Vec<_> = (0..4)
            .map(|index| {
                let (store, barrier, key) = (Arc::clone(&store), Arc::clone(&barrier), key.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    store.stage_bytes(b"bytes", &key, &format!("{index}.thumb.png"))
                })
            })
            .collect();
        for thread in threads {
            thread
                .join()
                .unwrap()
                .unwrap_or_else(|error| panic!("round {round}: {error}"));
        }
    }
}

// Task 6: the commit's cancel check and the recorded source stat.

#[test]
fn a_raised_cancel_stops_placement_before_anything_is_placed_or_written() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let bytes = pattern(4096);
    let staged = stage(
        &store,
        &fixture.source("model.stl", &bytes),
        "sel-cancel",
        0,
    )
    .expect("staged");
    let (cancel, raised) = tokio::sync::watch::channel(true);
    let mut ran = false;

    let error = store
        .place_and_commit_unless_cancelled(
            &fixture.storage,
            &[&staged],
            &CancelFlag::new(raised),
            |_| {
                ran = true;
                Ok(())
            },
        )
        .expect_err("a raised cancel stops the commit");

    assert!(matches!(error, ContentError::Cancelled), "{error:?}");
    assert!(!ran, "the commit closure never runs");
    assert!(staged.path.is_file(), "the staged copy is not moved");
    assert!(!fixture.blob_path(&staged.sha256).exists());
    assert_eq!(
        fixture.count(
            "SELECT COUNT(*) FROM content_blobs WHERE sha256 = ?1",
            &staged.sha256
        ),
        0
    );
    drop(cancel);
}

#[test]
fn a_staged_source_records_the_stat_it_was_copied_under() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let source = fixture.source("model.stl", &pattern(4096));
    let metadata = fs::metadata(&source).expect("metadata");

    let staged = stage(&store, &source, "sel-stat", 0).expect("staged");
    let thumbnail = store
        .stage_bytes(b"png", "sel-stat", "0.thumb.png")
        .expect("thumbnail");

    let stat = staged.source.expect("a copied source has a stat");
    assert_eq!(stat.size, 4096);
    assert_eq!(stat.modified, Some(metadata.modified().expect("mtime")));
    let since_epoch = metadata
        .modified()
        .expect("mtime")
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after the epoch");
    assert_eq!(stat.modified_ns(), Some(since_epoch.as_nanos() as i64));
    assert!(stat
        .modified_rfc3339()
        .is_some_and(|text| text.ends_with('Z')));
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            stat.file_id,
            Some(format!("{}:{}", metadata.dev(), metadata.ino()))
        );
    }
    assert_eq!(thumbnail.source, None, "in-memory bytes have no source");
}
