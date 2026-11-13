//! Git child ownership, bounded capture, and read-only request supervision.
use std::{
    io::{self, Read, Write},
    process::{Child, Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

const STDOUT_BYTES: usize = 32 * 1024 * 1024;
const STDERR_BYTES: usize = 1024 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Cancellation flag and deadline shared by the subprocesses in one read request.
///
/// A network scope has no deadline: fetch, pull and push wait on remotes and
/// credential helpers for as long as the user lets them, and stop only when
/// cancelled.
#[derive(Clone, Debug)]
pub(crate) struct ReadScope {
    cancelled: Arc<AtomicBool>,
    deadline: Option<Instant>,
}

impl ReadScope {
    #[cfg(test)]
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Starts a 30-second budget and observes the caller's cancellation flag.
    pub fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            cancelled,
            deadline: Some(Instant::now() + READ_TIMEOUT),
        }
    }

    /// Supervises a network command: no time budget, stopped only by `cancelled`.
    pub(crate) fn network(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            cancelled,
            deadline: None,
        }
    }

    pub(crate) fn check(&self) -> io::Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Git read was superseded.",
            ))
        } else if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Git read exceeded its 30-second request budget.",
            ))
        } else {
            Ok(())
        }
    }
}

impl Default for ReadScope {
    fn default() -> Self {
        Self::new(Arc::new(AtomicBool::new(false)))
    }
}

fn read_pipe(mut pipe: impl Read, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    (&mut pipe).take(limit as u64).read_to_end(&mut bytes)?;
    let truncated = io::copy(&mut pipe, &mut io::sink())? != 0;
    Ok((bytes, truncated))
}

/// Runs Git with bounded output capture and optional read supervision.
///
/// Pass a scope for reads so they can be cancelled as a group. Writes wait
/// for Git to finish. `success_code` is an additional status whose stdout must
/// fit in the capture limit, such as `1` from `git diff --no-index`.
///
/// # Errors
///
/// Returns an I/O error if the command cannot run, capture fails, a read is
/// cancelled or times out, or stdout for a successful status exceeds 32 MiB.
/// Git exit statuses are returned in [`Output`].
pub(crate) fn capture(
    mut command: Command,
    input: Option<&[u8]>,
    success_code: i32,
    scope: Option<&ReadScope>,
) -> io::Result<Output> {
    if let Some(scope) = scope {
        scope.check()?;
    }
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (mut child, mut tree) = ProcessTree::spawn(command, scope.is_some())?;
    let stdout = child.stdout.take().expect("piped Git stdout");
    let stderr = child.stderr.take().expect("piped Git stderr");
    let stdin = child.stdin.take();
    std::thread::scope(|threads| {
        let read_stdout = threads.spawn(|| read_pipe(stdout, STDOUT_BYTES));
        let read_stderr = threads.spawn(|| read_pipe(stderr, STDERR_BYTES));
        let write = input
            .zip(stdin)
            .map(|(input, mut stdin)| threads.spawn(move || stdin.write_all(input)));
        let status = wait(&mut child, scope, &mut tree);
        if status.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Closing supervision also stops auxiliary processes that still own a pipe.
        drop(tree);
        let (stdout, stdout_truncated) = read_stdout.join().expect("Git stdout reader")?;
        let (mut stderr, stderr_truncated) = read_stderr.join().expect("Git stderr reader")?;
        let status = status?;
        if let Some(write) = write {
            let result = write.join().expect("Git stdin writer");
            if status.success() {
                result?;
            }
        }
        if stdout_truncated && (status.success() || status.code() == Some(success_code)) {
            return Err(io::Error::other("Git output exceeded 32 MiB."));
        }
        if stderr_truncated {
            stderr.extend_from_slice(b"\n[Git diagnostics limited to 1 MiB]");
        }
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    })
}

fn wait(
    child: &mut Child,
    scope: Option<&ReadScope>,
    tree: &mut ProcessTree,
) -> io::Result<std::process::ExitStatus> {
    let Some(scope) = scope else {
        return child.wait();
    };
    loop {
        match exited(child) {
            Ok(true) => {
                tree.stop();
                return child.wait();
            }
            Ok(false) => {}
            Err(error) => {
                tree.stop();
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        }
        if let Err(error) = scope.check() {
            tree.stop();
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(windows)]
fn exited(child: &mut Child) -> io::Result<bool> {
    child.try_wait().map(|status| status.is_some())
}

#[cfg(unix)]
fn exited(child: &mut Child) -> io::Result<bool> {
    // Keep the group leader unreaped until its descendants have been stopped.
    // This prevents a recycled PID from identifying an unrelated process group.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::waitid(
            libc::P_PID,
            child.id(),
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    #[cfg(target_os = "macos")]
    let pid = info.si_pid;
    #[cfg(not(target_os = "macos"))]
    let pid = unsafe { info.si_pid() };
    Ok(pid != 0)
}

#[cfg(unix)]
struct ProcessTree(Option<i32>);

#[cfg(unix)]
impl ProcessTree {
    fn spawn(mut command: Command, supervised: bool) -> io::Result<(Child, Self)> {
        if supervised {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command.spawn()?;
        let group = supervised.then_some(child.id() as i32);
        Ok((child, Self(group)))
    }
    fn stop(&mut self) {
        if let Some(group) = self.0.take() {
            // The unreaped group leader still owns this process group ID.
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
    }
}

#[cfg(unix)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(windows)]
struct ProcessTree(Option<std::os::windows::io::OwnedHandle>);

#[cfg(windows)]
impl ProcessTree {
    fn spawn(mut command: Command, supervised: bool) -> io::Result<(Child, Self)> {
        use std::os::windows::{
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
            process::CommandExt,
        };
        use windows_sys::Win32::System::{
            JobObjects::*,
            Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED},
        };

        if !supervised {
            return Ok((
                command.creation_flags(CREATE_NO_WINDOW).spawn()?,
                Self(None),
            ));
        }

        // SAFETY: Null arguments create an anonymous job with a noninheritable handle.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: The successful call returned a new handle that we own.
        let job = unsafe { OwnedHandle::from_raw_handle(job) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: The job is open and the structure and size match the information class.
        if unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }

        // Git must join the job before it can launch any helpers.
        let mut child = command
            .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED)
            .spawn()?;
        // SAFETY: Both handles stay open through the call.
        let assigned =
            unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) };
        let started = if assigned == 0 {
            Err(io::Error::last_os_error())
        } else {
            resume_suspended_child(&child)
        };
        if let Err(error) = started {
            // Closing the job kills any assigned child. Also handle failure before assignment.
            drop(job);
            if child.kill().is_ok() {
                let _ = child.wait();
            }
            return Err(error);
        }
        Ok((child, Self(Some(job))))
    }

    fn stop(&mut self) {
        drop(self.0.take());
    }
}

#[cfg(windows)]
fn resume_suspended_child(child: &Child) -> io::Result<()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::{ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD,
                THREADENTRY32,
            },
            Threading::{
                GetProcessIdOfThread, OpenThread, ResumeThread, THREAD_QUERY_LIMITED_INFORMATION,
                THREAD_SUSPEND_RESUME,
            },
        },
    };

    // std::process::Child exposes the process handle. Find the initial thread in a snapshot.
    // SAFETY: TH32CS_SNAPTHREAD takes a system-wide snapshot and ignores the process ID.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: The successful call returned a new snapshot handle that we own.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let entry_size = std::mem::size_of::<THREADENTRY32>() as u32;
    let mut entry = THREADENTRY32 {
        dwSize: entry_size,
        ..Default::default()
    };
    // SAFETY: The snapshot is open and entry is a writable THREADENTRY32 with its size set.
    let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
    while found != 0 {
        // The API can return a shorter entry. Read the IDs only when both were written.
        if entry.dwSize >= std::mem::offset_of!(THREADENTRY32, tpBasePri) as u32
            && entry.th32OwnerProcessID == child.id()
        {
            // SAFETY: OpenThread takes an ID from the snapshot and returns a new handle or null.
            let thread = unsafe {
                OpenThread(
                    THREAD_QUERY_LIMITED_INFORMATION | THREAD_SUSPEND_RESUME,
                    0,
                    entry.th32ThreadID,
                )
            };
            if thread.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: The successful call returned a new thread handle that we own.
            let thread = unsafe { OwnedHandle::from_raw_handle(thread) };
            // SAFETY: The handle is open and has the required query and resume rights.
            return unsafe {
                match GetProcessIdOfThread(thread.as_raw_handle()) {
                    0 => Err(io::Error::last_os_error()),
                    pid if pid == child.id() => match ResumeThread(thread.as_raw_handle()) {
                        0 | 1 => Ok(()),
                        u32::MAX => Err(io::Error::last_os_error()),
                        _ => Err(io::Error::other("Git's initial thread is still suspended.")),
                    },
                    _ => Err(io::Error::other(
                        "Git's initial thread no longer belongs to it.",
                    )),
                }
            };
        }
        entry.dwSize = entry_size;
        // SAFETY: The same snapshot is still open and entry's size was reset before reuse.
        found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Git's initial thread was not found.",
        ));
    }
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "subprocess fixture entry point"]
    fn slow_helper() {
        match std::env::var("GITCRAB_PROCESS_HELPER").as_deref() {
            Ok("descendant") => {
                #[expect(
                    clippy::zombie_processes,
                    reason = "the leaf is left running on purpose; the test checks the job reaps it"
                )]
                let child = Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "process::tests::slow_helper", "--ignored"])
                    .env("GITCRAB_PROCESS_HELPER", "leaf")
                    .spawn()
                    .unwrap();
                writeln!(io::stdout(), "spawned descendant {}", child.id()).unwrap();
            }
            Ok(_) => std::thread::sleep(Duration::from_secs(1)),
            Err(_) => {}
        }
    }

    #[test]
    fn read_budget_terminates_and_reaps_a_slow_child() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "process::tests::slow_helper", "--ignored"]);
        command.env("GITCRAB_PROCESS_HELPER", "1");
        let scope = ReadScope {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: Some(Instant::now() + Duration::from_millis(80)),
        };
        let start = Instant::now();
        let error = capture(command, None, 0, Some(&scope)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_millis(800));
    }

    #[test]
    fn superseded_reads_are_rejected_before_spawn() {
        let scope = ReadScope::new(Arc::new(AtomicBool::new(true)));
        let error = capture(Command::new("must-not-start"), None, 0, Some(&scope)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    }

    #[test]
    fn request_cancellation_stops_a_running_child() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let scope = ReadScope::new(cancelled.clone());
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "process::tests::slow_helper", "--ignored"])
            .env("GITCRAB_PROCESS_HELPER", "1");
        std::thread::scope(|threads| {
            threads.spawn(|| {
                std::thread::sleep(Duration::from_millis(80));
                cancelled.store(true, Ordering::Release);
            });
            let error = capture(command, None, 0, Some(&scope)).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        });
    }

    #[test]
    fn a_finished_read_stops_descendants_before_joining_inherited_pipes() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "process::tests::slow_helper", "--ignored"])
            .env("GITCRAB_PROCESS_HELPER", "descendant");
        let start = Instant::now();
        let output = capture(command, None, 0, Some(&ReadScope::default())).unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("spawned descendant"));
        assert!(start.elapsed() < Duration::from_millis(800));
    }

    #[test]
    fn capture_retains_a_bounded_prefix_and_drains_the_rest() {
        let (bytes, truncated) = read_pipe(&b"bounded output"[..], 7).unwrap();
        assert_eq!(bytes, b"bounded");
        assert!(truncated);
    }
}
