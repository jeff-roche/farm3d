//! D5 "The lease": one process-wide, exclusive, in-memory lease held by
//! every P9 file operation for its whole run. While it is held, nothing
//! deletes a content blob or a camera image, so every file a backup's
//! database copy lists still exists when the writer reads it.
//!
//! - [`BackupLease::try_acquire`] takes it, or reports the current holder
//!   (`BACKUP_IN_PROGRESS`). It waits for any deletion already running to
//!   finish, so a deletion never straddles the start of a backup.
//! - A deleter asks for a [`DeletionPermit`] first
//!   (`ContentStore::release_unreferenced`, the `MediaJanitor`'s prune
//!   pass). While the lease is held there is none, and the deleter skips
//!   its pass: the pending rows stay and are retried.
//! - A capture that must prune for the disk cap marks its rows pruned as
//!   P8 does and hands the files to [`BackupLease::unlink_or_defer`]; under
//!   the lease they wait in memory.
//! - Dropping the [`LeaseGuard`] unlinks the queued files and runs every
//!   release hook once (the content store's `release_unreferenced`, the
//!   janitor's poke). A crash loses the queue harmlessly: P8's startup
//!   sweep deletes image files with no unpruned row, and P4's deletes blob
//!   files with no row.
//!
//! The lease is a cheap handle (`Clone` shares it). `RuntimeServices`
//! makes one and hands clones to the content store, the media janitor, and
//! `BackupServices`; a store built on its own gets a private lease nobody
//! else takes.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard};

use serde::Serialize;

/// Who holds the lease (`BACKUP_IN_PROGRESS`'s `details.activity`).
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum LeaseActivity {
    /// `create_backup`.
    Backup,
    /// `preview_restore`.
    RestorePreview,
    /// `apply_restore`.
    RestoreApply,
    /// `reset_farm`, every tier.
    Reset,
    /// `clear_storage`.
    StorageCleanup,
    /// `delete_backup`.
    BackupDelete,
}

impl LeaseActivity {
    pub fn as_str(self) -> &'static str {
        match self {
            LeaseActivity::Backup => "backup",
            LeaseActivity::RestorePreview => "restorePreview",
            LeaseActivity::RestoreApply => "restoreApply",
            LeaseActivity::Reset => "reset",
            LeaseActivity::StorageCleanup => "storageCleanup",
            LeaseActivity::BackupDelete => "backupDelete",
        }
    }
}

type ReleaseHook = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct State {
    holder: Option<LeaseActivity>,
    /// Media files whose rows are already pruned, waiting for the release.
    queued: Vec<PathBuf>,
}

#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    /// Read-held by every deletion in progress; `try_acquire` takes (and
    /// immediately drops) the write side, so it waits for them.
    deletions: RwLock<()>,
    hooks: Mutex<Vec<ReleaseHook>>,
}

/// D5's process-wide lease. See the module doc.
#[derive(Clone, Default)]
pub struct BackupLease(Arc<Inner>);

/// Proof that the lease is held; dropping it releases the lease.
#[must_use = "the lease is released when the guard drops"]
pub struct LeaseGuard {
    lease: BackupLease,
    activity: LeaseActivity,
}

/// Permission to delete files right now (the lease isn't held). Hold it
/// for the whole deletion.
pub struct DeletionPermit<'a> {
    _read: RwLockReadGuard<'a, ()>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl BackupLease {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the lease for `activity`, or returns the current holder.
    pub fn try_acquire(&self, activity: LeaseActivity) -> Result<LeaseGuard, LeaseActivity> {
        {
            let mut state = lock(&self.0.state);
            if let Some(holder) = state.holder {
                return Err(holder);
            }
            state.holder = Some(activity);
        }
        // Wait for every deletion that started before the holder was set.
        drop(
            self.0
                .deletions
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        Ok(LeaseGuard {
            lease: self.clone(),
            activity,
        })
    }

    /// The current holder, if any.
    pub fn holder(&self) -> Option<LeaseActivity> {
        lock(&self.0.state).holder
    }

    /// A permit to delete files now, or `None` while the lease is held.
    pub fn deletion_permit(&self) -> Option<DeletionPermit<'_>> {
        let read = self
            .0
            .deletions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lock(&self.0.state).holder.is_some() {
            return None;
        }
        Some(DeletionPermit { _read: read })
    }

    /// Unlinks `paths` (best effort) now, or queues them until the lease
    /// is released.
    pub fn unlink_or_defer(&self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let _read = self
            .0
            .deletions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        {
            let mut state = lock(&self.0.state);
            if state.holder.is_some() {
                state.queued.extend(paths);
                return;
            }
        }
        unlink_all(&paths);
    }

    /// Runs `hook` after every release, outside every lock.
    pub fn on_release(&self, hook: impl Fn() + Send + Sync + 'static) {
        lock(&self.0.hooks).push(Arc::new(hook));
    }

    fn release(&self) {
        let queued = {
            let mut state = lock(&self.0.state);
            state.holder = None;
            std::mem::take(&mut state.queued)
        };
        unlink_all(&queued);
        let hooks: Vec<ReleaseHook> = lock(&self.0.hooks).clone();
        for hook in hooks {
            hook();
        }
    }
}

/// Best effort: a file that can't be removed is an orphan the next
/// startup sweep deletes.
fn unlink_all(paths: &[PathBuf]) {
    for path in paths {
        let _ = fs::remove_file(path);
    }
}

impl LeaseGuard {
    pub fn activity(&self) -> LeaseActivity {
        self.activity
    }

    /// The lease this guard holds.
    pub fn lease(&self) -> &BackupLease {
        &self.lease
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        self.lease.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn the_lease_is_exclusive_and_names_its_holder() {
        let lease = BackupLease::new();
        let guard = lease.try_acquire(LeaseActivity::Backup).unwrap();
        assert_eq!(lease.holder(), Some(LeaseActivity::Backup));
        assert_eq!(
            lease.clone().try_acquire(LeaseActivity::Reset).err(),
            Some(LeaseActivity::Backup)
        );
        drop(guard);
        assert_eq!(lease.holder(), None);
        assert!(lease.try_acquire(LeaseActivity::Reset).is_ok());
    }

    #[test]
    fn no_deletion_permit_while_held() {
        let lease = BackupLease::new();
        assert!(lease.deletion_permit().is_some());
        let guard = lease.try_acquire(LeaseActivity::StorageCleanup).unwrap();
        assert!(lease.deletion_permit().is_none());
        drop(guard);
        assert!(lease.deletion_permit().is_some());
    }

    #[test]
    fn deferred_unlinks_and_hooks_run_once_on_release() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("image.jpg");
        fs::write(&file, b"x").unwrap();
        let lease = BackupLease::new();
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&runs);
        lease.on_release(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let guard = lease.try_acquire(LeaseActivity::Backup).unwrap();
        lease.unlink_or_defer(vec![file.clone()]);
        assert!(file.exists(), "queued while held");
        drop(guard);
        assert!(!file.exists(), "unlinked on release");
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        fs::write(&file, b"x").unwrap();
        lease.unlink_or_defer(vec![file.clone()]);
        assert!(!file.exists(), "unlinked at once when free");
    }

    #[test]
    fn acquiring_waits_for_a_deletion_in_progress() {
        let lease = BackupLease::new();
        let permit = lease.deletion_permit().unwrap();
        let acquired = Arc::new(AtomicUsize::new(0));
        let thread = {
            let lease = lease.clone();
            let acquired = Arc::clone(&acquired);
            std::thread::spawn(move || {
                let guard = lease.try_acquire(LeaseActivity::Backup).unwrap();
                acquired.store(1, Ordering::SeqCst);
                drop(guard);
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(acquired.load(Ordering::SeqCst), 0, "still deleting");
        drop(permit);
        thread.join().unwrap();
        assert_eq!(acquired.load(Ordering::SeqCst), 1);
    }
}
