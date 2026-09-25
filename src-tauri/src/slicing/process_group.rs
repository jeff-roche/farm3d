//! Running an OrcaSlicer process in its own process group, and stopping the
//! whole group (spike Gate E).
//!
//! - **Linux:** the child leads a new process group (`setsid`-style
//!   `process_group(0)`) and gets `PR_SET_PDEATHSIG(SIGTERM)`, so it dies
//!   with farm3d.
//! - **macOS and other Unix:** the same process group, without PDEATHSIG;
//!   startup recovery covers a crashed farm3d (D24).
//! - **Windows:** the child is assigned to a Job Object with
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, owned by the [`GroupChild`], so
//!   dropping it (or farm3d dying) ends every process in the job (D9). No
//!   runtime claim is made there (D24).
//!
//! Stopping sends SIGTERM to the group first, and SIGKILL only after a
//! grace period, because SIGKILL of an AppImage can leave a stale
//! `/tmp/.mount_*` FUSE mount. Windows has no gentle signal, so a stop
//! terminates the job at once. The group is also cleaned up after the
//! leader exits by itself, so a grandchild can't keep the pipes (and the
//! threads reading them) open.
//!
//! The version probe, the AppImage extraction, and the slice supervisor
//! (`process.rs`) use it.

use std::io;
use std::ops::{Deref, DerefMut};
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

/// How long a group gets between SIGTERM and SIGKILL.
pub const TERM_GRACE: Duration = Duration::from_secs(2);

const POLL: Duration = Duration::from_millis(10);

/// A spawned group leader, plus (on Windows) the Job Object that holds it
/// and its descendants. Dereferences to the [`Child`].
#[derive(Debug)]
pub struct GroupChild {
    child: Child,
    #[cfg(windows)]
    job: Option<job::JobObject>,
}

impl Deref for GroupChild {
    type Target = Child;

    fn deref(&self) -> &Child {
        &self.child
    }
}

impl DerefMut for GroupChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}

/// How a stopped or finished group ended.
#[derive(Debug)]
pub struct GroupEnd {
    /// The leader's status from the reap.
    pub status: io::Result<ExitStatus>,
    /// Whether any SIGKILL was sent, which can leave a stale AppImage
    /// mount.
    pub killed: bool,
}

/// Spawns `command` as the leader of a new process group (see the module
/// docs per platform). Retries briefly while the executable is still busy
/// (`ETXTBSY`, a file just written by another thread).
pub fn spawn_group(command: &mut Command) -> io::Result<GroupChild> {
    configure(command);
    let mut attempts = 0;
    let child = loop {
        match command.spawn() {
            Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy && attempts < 20 => {
                attempts += 1;
                thread::sleep(Duration::from_millis(25));
            }
            result => break result?,
        }
    };
    Ok(adopt(child))
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

#[cfg(not(windows))]
fn adopt(child: Child) -> GroupChild {
    GroupChild { child }
}

/// Puts the child in a new kill-on-close job. A job that can't be made
/// leaves the child plain, as before; the stop path then falls back to
/// `Child::kill`. The child can start processes before it is assigned;
/// those escape the job (std has no suspended spawn).
#[cfg(windows)]
fn adopt(child: Child) -> GroupChild {
    let job = job::JobObject::new()
        .and_then(|job| job.assign(&child).map(|()| job))
        .ok();
    GroupChild { child, job }
}

/// Waits for the group leader until `deadline`. If it is still running,
/// stops the group: SIGTERM, then SIGKILL after `grace`. Either way, any
/// process left in the group afterwards gets the same treatment, and the
/// leader is reaped. Returns whether the leader exited by itself before the
/// deadline.
pub fn wait_or_stop(child: &mut GroupChild, deadline: Instant, grace: Duration) -> bool {
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
    child: &mut GroupChild,
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
/// `grace`, then the straggler cleanup of [`clear_exited_group`].
pub fn stop_group(child: &mut GroupChild, grace: Duration) -> GroupEnd {
    let mut killed = false;
    if !signal_group(child, Signal::Term) || !wait_exit(child, Instant::now() + grace) {
        signal_group(child, Signal::Kill);
        killed = true;
    }
    let mut end = clear_exited_group(child, grace);
    end.killed |= killed;
    end
}

/// The leader has exited: SIGTERM reaches any process still in its group,
/// the leader is reaped, and a group still alive after `grace` gets
/// SIGKILL.
pub fn clear_exited_group(child: &mut GroupChild, grace: Duration) -> GroupEnd {
    reap_and_clear_group(child, grace)
}

#[derive(Clone, Copy)]
enum Signal {
    Term,
    Kill,
}

/// Waits until the leader has exited, without reaping it, so its pid (the
/// group id) stays reserved while the group is signalled.
fn wait_exit(child: &mut GroupChild, deadline: Instant) -> bool {
    wait_until(child, deadline, &|| false) == WaitEnd::Exited
}

/// Whether the leader has exited, without reaping it.
#[cfg(unix)]
fn has_exited(child: &mut GroupChild) -> bool {
    use rustix::process::{waitid, Pid, WaitId, WaitIdOptions};
    let Some(pid) = Pid::from_raw(child.id() as i32) else {
        return true;
    };
    let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
    !matches!(waitid(WaitId::Pid(pid), options), Ok(None))
}

#[cfg(not(unix))]
fn has_exited(child: &mut GroupChild) -> bool {
    !matches!(child.try_wait(), Ok(None))
}

#[cfg(unix)]
fn group_id(child: &GroupChild) -> Option<rustix::process::Pid> {
    rustix::process::Pid::from_raw(child.id() as i32)
}

/// Sends `signal` to the group. Returns whether a gentle stop is pending
/// (always false for [`Signal::Kill`], and for SIGTERM off Unix, where
/// nothing gentle exists).
#[cfg(unix)]
fn signal_group(child: &mut GroupChild, signal: Signal) -> bool {
    use rustix::process::{kill_process_group, Signal as RustixSignal};
    if let Some(group) = group_id(child) {
        let signal = match signal {
            Signal::Term => RustixSignal::TERM,
            Signal::Kill => RustixSignal::KILL,
        };
        let _ = kill_process_group(group, signal);
    }
    matches!(signal, Signal::Term)
}

#[cfg(not(unix))]
fn signal_group(child: &mut GroupChild, signal: Signal) -> bool {
    if matches!(signal, Signal::Kill) {
        #[cfg(windows)]
        if let Some(job) = &child.job {
            if job.terminate().is_ok() {
                return false;
            }
        }
        let _ = child.child.kill();
    }
    false
}

/// The leader has exited (or been killed) but is not reaped yet. SIGTERM
/// reaches any process still in the group; the leader is reaped; and a
/// group still alive after `grace` gets SIGKILL, which is reported. While a
/// member lives, the group id can't be reused, so signalling it after the
/// reap is safe.
#[cfg(unix)]
fn reap_and_clear_group(child: &mut GroupChild, grace: Duration) -> GroupEnd {
    use rustix::process::test_kill_process_group;
    signal_group(child, Signal::Term);
    let status = child.wait();
    let mut killed = false;
    if let Some(group) = group_id(child) {
        let deadline = Instant::now() + grace;
        while test_kill_process_group(group).is_ok() {
            if Instant::now() >= deadline {
                signal_group(child, Signal::Kill);
                killed = true;
                break;
            }
            thread::sleep(POLL);
        }
    }
    GroupEnd { status, killed }
}

/// Off Unix: the leader is reaped, and on Windows whatever is left in its
/// job is terminated.
#[cfg(not(unix))]
fn reap_and_clear_group(child: &mut GroupChild, _grace: Duration) -> GroupEnd {
    let status = child.wait();
    #[cfg(windows)]
    if let Some(job) = &child.job {
        let _ = job.terminate();
    }
    GroupEnd {
        status,
        killed: false,
    }
}

/// D10: the start time of process `pid`, as Linux `/proc/<pid>/stat`
/// field 22 (clock ticks since boot). With the pid, it identifies one
/// process: a pid reused later has a different start time. `None` off
/// Linux, or when the process is gone.
pub fn process_start_time(pid: u32) -> Option<i64> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // Field 2 (the command name) is parenthesised and may hold spaces
        // or parentheses, so count from the last `)`: field 3 comes next.
        let after_name = &stat[stat.rfind(')')? + 1..];
        after_name.split_whitespace().nth(22 - 3)?.parse().ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// D10: the executable process `pid` runs (Linux `/proc/<pid>/exe`).
/// `None` off Linux, or when it can't be read.
pub fn process_executable(pid: u32) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/exe")).ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// D10 startup recovery: stops the process group `group_id`, which is
/// not farm3d's child (it survived a previous run of farm3d): SIGTERM,
/// then SIGKILL if any member outlives `grace`. Returns whether SIGKILL was
/// sent. Does nothing off Unix.
pub fn stop_recorded_group(group_id: i32, grace: Duration) -> bool {
    #[cfg(unix)]
    {
        use rustix::process::{
            kill_process_group, test_kill_process_group, Pid, Signal as RustixSignal,
        };
        let Some(group) = Pid::from_raw(group_id) else {
            return false;
        };
        let _ = kill_process_group(group, RustixSignal::TERM);
        let deadline = Instant::now() + grace;
        while test_kill_process_group(group).is_ok() {
            if Instant::now() >= deadline {
                let _ = kill_process_group(group, RustixSignal::KILL);
                return true;
            }
            thread::sleep(POLL);
        }
        false
    }
    #[cfg(not(unix))]
    {
        let _ = (group_id, grace);
        false
    }
}

/// D9's Windows containment: a Job Object that kills its processes when
/// its last handle closes.
#[cfg(windows)]
mod job {
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// Owns the job handle; dropping it closes the handle, which ends
    /// every process still in the job.
    #[derive(Debug)]
    pub struct JobObject(HANDLE);

    // SAFETY: a job handle is a kernel handle usable from any thread.
    unsafe impl Send for JobObject {}
    unsafe impl Sync for JobObject {}

    fn check(ok: i32) -> io::Result<()> {
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    impl JobObject {
        /// An anonymous job with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
        pub fn new() -> io::Result<Self> {
            // SAFETY: null attributes and name are allowed; the handle is
            // checked before use and owned by the returned value.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(handle);
            // SAFETY: an all-zero JOBOBJECT_EXTENDED_LIMIT_INFORMATION is a
            // valid value (plain integers and counters).
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `info` lives across the call and the size matches it.
            check(unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            })?;
            Ok(job)
        }

        pub fn assign(&self, child: &Child) -> io::Result<()> {
            // SAFETY: both handles are open for the duration of the call.
            check(unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) })
        }

        pub fn terminate(&self) -> io::Result<()> {
            // SAFETY: the handle is open until drop.
            check(unsafe { TerminateJobObject(self.0, 1) })
        }

        #[cfg(test)]
        pub fn contains(&self, child: &Child) -> bool {
            use windows_sys::Win32::System::JobObjects::IsProcessInJob;
            let mut result = 0;
            // SAFETY: both handles are open; `result` is a valid out pointer.
            let ok =
                unsafe { IsProcessInJob(child.as_raw_handle() as HANDLE, self.0, &mut result) };
            ok != 0 && result != 0
        }
    }

    impl Drop for JobObject {
        fn drop(&mut self) {
            // SAFETY: the handle is owned and closed exactly once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::process::Stdio;

    use super::*;

    #[test]
    fn a_child_is_held_in_a_kill_on_close_job_and_a_stop_ends_it() {
        let mut command = Command::new("cmd");
        command
            .args(["/C", "ping -n 30 127.0.0.1"])
            .stdout(Stdio::null());
        let mut child = spawn_group(&mut command).unwrap();
        let job = child.job.as_ref().expect("a job object");
        assert!(job.contains(&child.child));
        let started = Instant::now();
        let end = stop_group(&mut child, Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(end.status.is_ok());
        assert!(!matches!(child.try_wait(), Ok(None)));
    }
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
