//! Running an OrcaSlicer process in its own process group, and stopping the
//! whole group (spike Gate E).
//!
//! On Unix, the child leads a new process group, and on Linux it gets
//! `PR_SET_PDEATHSIG(SIGTERM)`, so it dies with farm3d. Stopping sends
//! SIGTERM to the group first, and SIGKILL only after a grace period,
//! because SIGKILL of an AppImage can leave a stale `/tmp/.mount_*` FUSE
//! mount. The group is also cleaned up after the leader exits by itself, so
//! a grandchild can't keep the pipes (and the threads reading them) open.
//!
//! Other targets make no support claim (D24): they spawn plainly and stop
//! with `Child::kill`.
//!
//! The version probe, the AppImage extraction, and the slice supervisor
//! (`process.rs`) use it.

use std::io;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

/// How long a group gets between SIGTERM and SIGKILL.
pub const TERM_GRACE: Duration = Duration::from_secs(2);

const POLL: Duration = Duration::from_millis(10);

/// Spawns `command` as the leader of a new process group (Unix), with
/// `PR_SET_PDEATHSIG(SIGTERM)` on Linux. Retries briefly while the
/// executable is still busy (`ETXTBSY`, a file just written by another
/// thread).
pub fn spawn_group(command: &mut Command) -> io::Result<Child> {
    configure(command);
    let mut attempts = 0;
    loop {
        match command.spawn() {
            Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy && attempts < 20 => {
                attempts += 1;
                thread::sleep(Duration::from_millis(25));
            }
            result => return result,
        }
    }
}

#[cfg(unix)]
fn configure(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    #[cfg(target_os = "linux")]
    // SAFETY: the closure runs in the forked child before `exec` and only
    // makes one `prctl` syscall, which is async-signal-safe; it allocates
    // nothing and touches no locks.
    unsafe {
        command.pre_exec(|| {
            rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::TERM))
                .map_err(io::Error::from)
        });
    }
}

#[cfg(not(unix))]
fn configure(_command: &mut Command) {}

/// Waits for the group leader until `deadline`. If it is still running,
/// stops the group: SIGTERM, then SIGKILL after `grace`. Either way, any
/// process left in the group afterwards gets the same treatment, and the
/// leader is reaped. Returns whether the leader exited by itself before the
/// deadline.
pub fn wait_or_stop(child: &mut Child, deadline: Instant, grace: Duration) -> bool {
    match wait_until(child, deadline, &|| false) {
        WaitEnd::Exited => {
            clear_exited_group(child, grace);
            true
        }
        WaitEnd::DeadlinePassed | WaitEnd::StopRequested => {
            stop_group(child, grace);
            false
        }
    }
}

/// Why [`wait_until`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitEnd {
    /// The leader exited. It is not reaped yet.
    Exited,
    DeadlinePassed,
    /// `stop_requested` returned true.
    StopRequested,
}

/// Waits for the group leader to exit, until `deadline` or until
/// `stop_requested` returns true (polled every 10 ms). The leader is not
/// reaped, so its pid (the group id) stays reserved; follow with
/// [`clear_exited_group`] or [`stop_group`].
pub fn wait_until(
    child: &mut Child,
    deadline: Instant,
    stop_requested: &dyn Fn() -> bool,
) -> WaitEnd {
    loop {
        if has_exited(child) {
            return WaitEnd::Exited;
        }
        if stop_requested() {
            return WaitEnd::StopRequested;
        }
        if Instant::now() >= deadline {
            return WaitEnd::DeadlinePassed;
        }
        thread::sleep(POLL);
    }
}

/// Stops a running group: SIGTERM, then SIGKILL if the leader outlives
/// `grace`, then the straggler cleanup of [`clear_exited_group`]. Returns
/// whether any SIGKILL was sent, since that can leave a stale AppImage
/// mount.
pub fn stop_group(child: &mut Child, grace: Duration) -> bool {
    signal_group(child, Signal::Term);
    let mut killed = false;
    if !wait_exit(child, Instant::now() + grace) {
        signal_group(child, Signal::Kill);
        killed = true;
    }
    clear_exited_group(child, grace) || killed
}

/// The leader has exited: SIGTERM reaches any process still in its group,
/// the leader is reaped, and a group still alive after `grace` gets
/// SIGKILL. Returns whether that SIGKILL was sent.
pub fn clear_exited_group(child: &mut Child, grace: Duration) -> bool {
    reap_and_clear_group(child, grace)
}

#[derive(Clone, Copy)]
enum Signal {
    Term,
    Kill,
}

/// Waits until the leader has exited, without reaping it, so its pid (the
/// group id) stays reserved while the group is signalled.
fn wait_exit(child: &mut Child, deadline: Instant) -> bool {
    wait_until(child, deadline, &|| false) == WaitEnd::Exited
}

/// Whether the leader has exited, without reaping it.
#[cfg(unix)]
fn has_exited(child: &mut Child) -> bool {
    use rustix::process::{waitid, Pid, WaitId, WaitIdOptions};
    let Some(pid) = Pid::from_raw(child.id() as i32) else {
        return true;
    };
    let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
    !matches!(waitid(WaitId::Pid(pid), options), Ok(None))
}

#[cfg(not(unix))]
fn has_exited(child: &mut Child) -> bool {
    !matches!(child.try_wait(), Ok(None))
}

#[cfg(unix)]
fn group_id(child: &Child) -> Option<rustix::process::Pid> {
    rustix::process::Pid::from_raw(child.id() as i32)
}

#[cfg(unix)]
fn signal_group(child: &Child, signal: Signal) {
    use rustix::process::{kill_process_group, Signal as RustixSignal};
    if let Some(group) = group_id(child) {
        let signal = match signal {
            Signal::Term => RustixSignal::TERM,
            Signal::Kill => RustixSignal::KILL,
        };
        let _ = kill_process_group(group, signal);
    }
}

#[cfg(not(unix))]
fn signal_group(child: &mut Child, signal: Signal) {
    if matches!(signal, Signal::Kill) {
        let _ = child.kill();
    }
}

/// The leader has exited (or been killed) but is not reaped yet. SIGTERM
/// reaches any process still in the group; the leader is reaped; and a
/// group still alive after `grace` gets SIGKILL, which is reported. While a
/// member lives, the group id can't be reused, so signalling it after the
/// reap is safe.
#[cfg(unix)]
fn reap_and_clear_group(child: &mut Child, grace: Duration) -> bool {
    use rustix::process::test_kill_process_group;
    signal_group(child, Signal::Term);
    let _ = child.wait();
    let Some(group) = group_id(child) else {
        return false;
    };
    let deadline = Instant::now() + grace;
    while test_kill_process_group(group).is_ok() {
        if Instant::now() >= deadline {
            signal_group(child, Signal::Kill);
            return true;
        }
        thread::sleep(POLL);
    }
    false
}

#[cfg(not(unix))]
fn reap_and_clear_group(child: &mut Child, _grace: Duration) -> bool {
    let _ = child.wait();
    false
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Stdio;

    use super::*;

    fn alive(pid: i32) -> bool {
        rustix::process::test_kill_process(rustix::process::Pid::from_raw(pid).unwrap()).is_ok()
    }

    #[test]
    fn a_hung_group_is_terminated_including_grandchildren() {
        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("grandchild");
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(format!(
                "sleep 30 & echo $! > '{}'; wait",
                pid_file.display()
            ))
            .stdout(Stdio::null());
        let mut child = spawn_group(&mut command).unwrap();
        let started = Instant::now();
        let exited = wait_or_stop(&mut child, started + Duration::from_millis(300), TERM_GRACE);
        assert!(!exited);
        assert!(started.elapsed() < Duration::from_secs(5));
        let grandchild: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(!alive(grandchild), "the grandchild survived");
    }

    #[test]
    fn a_process_that_ignores_sigterm_is_killed_after_the_grace() {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("trap '' TERM; while :; do sleep 0.05; done");
        let mut child = spawn_group(&mut command).unwrap();
        let pid = child.id() as i32;
        let started = Instant::now();
        assert!(!wait_or_stop(
            &mut child,
            started + Duration::from_millis(100),
            Duration::from_millis(300),
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!alive(pid));
    }

    #[test]
    fn a_leader_that_exits_leaves_no_straggler() {
        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("straggler");
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(format!("sleep 30 & echo $! > '{}'", pid_file.display()));
        let mut child = spawn_group(&mut command).unwrap();
        assert!(wait_or_stop(
            &mut child,
            Instant::now() + Duration::from_secs(5),
            TERM_GRACE
        ));
        let straggler: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(!alive(straggler), "the straggler survived");
    }
}
