//! P8 D5 "Pruning": the pure [`plan_prune`], and the [`MediaJanitor`]
//! whose one lock serializes every capture and every prune.
//!
//! `plan_prune` decides what to prune from the stored rows alone:
//!
//! 1. every unpinned row older than the retention period goes for `age`;
//! 2. then, while usage (minus what is already planned) plus the incoming
//!    frame is over the cap, the oldest remaining unpinned row (by
//!    `captured_at`, then id) goes for `diskCap`;
//! 3. `fits` says whether usage now fits. When it doesn't (only pinned rows
//!    are left), the `diskCap` actions are dropped, since evidence is never
//!    removed for nothing, and the `age` ones stay.
//!
//! Usage is the stored `byte_len` of every unpruned row, pinned or not;
//! never a filesystem walk. Pinned rows count toward it and are never
//! planned.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{Mutex, MutexGuard, Notify};

use super::PruneReason;

/// The retention settings as bytes: `snapshotRetention.retentionDays` and
/// `snapshotRetention.diskCapMb` (MiB).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RetentionPolicy {
    pub retention_days: i64,
    pub cap_bytes: i64,
}

impl RetentionPolicy {
    /// The migration's defaults: 30 days, 2048 MiB.
    pub const DEFAULT: RetentionPolicy = RetentionPolicy {
        retention_days: 30,
        cap_bytes: 2048 * 1024 * 1024,
    };

    /// From the stored settings (`diskCapMb` in MiB).
    pub fn from_settings(retention_days: i64, disk_cap_mb: i64) -> Self {
        Self {
            retention_days,
            cap_bytes: disk_cap_mb.saturating_mul(1024 * 1024),
        }
    }
}

/// One unpruned `camera_snapshots` row, as the planner sees it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RetainedSnapshot {
    pub id: String,
    pub captured_at: DateTime<Utc>,
    pub byte_len: i64,
    pub pinned: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PruneAction {
    pub id: String,
    pub reason: PruneReason,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct PrunePlan {
    /// `age` actions first (oldest first), then `diskCap` ones.
    pub prune: Vec<PruneAction>,
    /// Whether usage minus `prune` plus the incoming bytes is within the
    /// cap. When false, `prune` holds only `age` actions.
    pub fits: bool,
}

/// D5's pure prune planner over the **unpruned** rows (`rows` in any
/// order), for a frame of `incoming_bytes` about to be stored (0 for the
/// janitor's own pass).
pub fn plan_prune(
    rows: &[RetainedSnapshot],
    retention: RetentionPolicy,
    now: DateTime<Utc>,
    incoming_bytes: i64,
) -> PrunePlan {
    let mut unpinned: Vec<&RetainedSnapshot> = rows.iter().filter(|row| !row.pinned).collect();
    unpinned.sort_by(|left, right| {
        left.captured_at
            .cmp(&right.captured_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    let cutoff = now - chrono::Duration::days(retention.retention_days);
    let mut used: i64 = rows.iter().map(|row| row.byte_len).sum();

    let mut prune = Vec::new();
    let mut remaining = Vec::new();
    for row in unpinned {
        if row.captured_at < cutoff {
            used -= row.byte_len;
            prune.push(PruneAction {
                id: row.id.clone(),
                reason: PruneReason::Age,
            });
        } else {
            remaining.push(row);
        }
    }
    let aged = prune.len();
    for row in remaining {
        if used + incoming_bytes <= retention.cap_bytes {
            break;
        }
        used -= row.byte_len;
        prune.push(PruneAction {
            id: row.id.clone(),
            reason: PruneReason::DiskCap,
        });
    }
    let fits = used + incoming_bytes <= retention.cap_bytes;
    if !fits {
        prune.truncate(aged);
    }
    PrunePlan { prune, fits }
}

/// D5 "`MediaJanitor`": the one lock every capture and every prune pass
/// takes, and the janitor task's wake. The task
/// (`cameras::capture::start`) runs a prune pass at start (after the
/// startup sweep), every `CameraTimings::janitor_every`, and on
/// [`poke`](Self::poke). Contract for callers: a command that changes the
/// retention settings (`save_settings`, `import_settings`; wired by P8
/// Task 9) must call `poke` after its commit.
pub struct MediaJanitor {
    lock: Mutex<()>,
    wake: Arc<Notify>,
    passes: AtomicU64,
    pokes: AtomicU64,
    /// A [`CaptureFault`] the next capture write simulates (test hook).
    fault: AtomicU8,
}

impl Default for MediaJanitor {
    fn default() -> Self {
        Self {
            lock: Mutex::new(()),
            wake: Arc::new(Notify::new()),
            passes: AtomicU64::new(0),
            pokes: AtomicU64::new(0),
            fault: AtomicU8::new(0),
        }
    }
}

/// Where [`MediaJanitor::inject_capture_fault_once`] makes the next
/// capture write stop, as a crash would: nothing after that point runs and
/// nothing is cleaned up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum CaptureFault {
    /// After `tmp/<snp-id>.part` is written and fsynced, before its rename.
    BeforeRename = 1,
    /// After the rename into `snapshots/`, before the row's transaction.
    AfterRename = 2,
}

impl MediaJanitor {
    /// Serializes a capture or a prune with every other one.
    pub async fn lock(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().await
    }

    /// Asks for a prune pass now. Every command that changes the retention
    /// settings calls this after its commit (`save_settings` and
    /// `import_settings`). Pokes coalesce; one before the task starts is
    /// kept for it.
    pub fn poke(&self) {
        self.pokes.fetch_add(1, Ordering::SeqCst);
        self.wake.notify_one();
    }

    /// Test hook: how many times [`poke`](Self::poke) was called.
    pub fn pokes(&self) -> u64 {
        self.pokes.load(Ordering::SeqCst)
    }

    pub(crate) fn wake(&self) -> Arc<Notify> {
        Arc::clone(&self.wake)
    }

    pub(crate) fn pass_done(&self) {
        self.passes.fetch_add(1, Ordering::SeqCst);
    }

    /// Test hook (the crash-injection style of
    /// `Storage::inject_failure_once`): the next capture write stops at
    /// `fault` and returns an error without cleaning up.
    #[doc(hidden)]
    pub fn inject_capture_fault_once(&self, fault: CaptureFault) {
        self.fault.store(fault as u8, Ordering::SeqCst);
    }

    pub(crate) fn take_fault(&self, fault: CaptureFault) -> bool {
        self.fault
            .compare_exchange(fault as u8, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Test hook: how many janitor prune passes have finished.
    pub fn passes(&self) -> u64 {
        self.passes.load(Ordering::SeqCst)
    }
}

/// The janitor's hourly period (D5, `CameraTimings::janitor_every`).
pub const JANITOR_EVERY: Duration = Duration::from_secs(60 * 60);
