//! Real-Git test isolation. Never modify the process environment from a test.
use crate::git::Git;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

pub static SUBPROCESSES: AtomicU64 = AtomicU64::new(0);

pub fn peak_memory_bytes() -> Option<usize> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::{ProcessStatus::*, Threading::GetCurrentProcess};
        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of_val(&counters) as u32;
        if unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) } != 0 {
            return Some(counters.PeakWorkingSetSize);
        }
    }
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let value = status
            .lines()
            .find_map(|line| line.strip_prefix("VmHWM:"))?;
        // Return only on success, like the other platforms, so the shared
        // `None` below stays reachable on Linux too.
        if let Some(kb) = value
            .split_whitespace()
            .next()
            .and_then(|kb| kb.parse::<usize>().ok())
        {
            return Some(kb * 1024);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let mut usage = unsafe { std::mem::zeroed::<libc::rusage>() };
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } == 0 {
            return usize::try_from(usage.ru_maxrss).ok();
        }
    }
    None
}

/// Isolates a fixture command from machine and user Git configuration.
pub fn isolate_config(cmd: &mut Command, repository: &Path) {
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            repository.join(".git/gitcrab-test-global"),
        )
        .env("LC_ALL", "C");
}

/// Owns a temporary Git worktree and removes it when dropped.
pub struct Fixture {
    pub git: Git,
    root: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "gitcrab-test-{}-{nonce}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&root)
            .unwrap_or_else(|err| panic!("Could not create {}: {err}", root.display()));
        let fixture = Self {
            git: Git {
                path: root.join("repo"),
                read_scope: None,
                git_dir: Some(root.join("repo/.git")),
                common_dir: None,
                index_cache: Default::default(),
                observe_command: None,
            },
            root,
        };
        fs::create_dir(&fixture.git.path).unwrap();
        let git = &fixture.git;
        for args in [
            &["init", "--quiet", "--initial-branch=main", "--template="][..],
            &["config", "user.name", "GitCrab Test"],
            &["config", "user.email", "test@example.invalid"],
            &["config", "commit.gpgsign", "false"],
            &["config", "core.autocrlf", "false"],
            &["config", "core.hooksPath", ".git/hooks"],
        ] {
            git.run(args)
                .unwrap_or_else(|err| panic!("Fixture Git {args:?}: {err}"));
        }
        fs::create_dir(git.path.join(".git/hooks")).unwrap();
        fixture
    }

    /// The temporary folder that holds the worktree, removed with the fixture.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn write(&self, name: &str, text: &str) {
        fs::write(self.git.path.join(name), text).unwrap();
    }

    /// Imports linear history on `main` with `refs` additional branches at its tip.
    pub fn history(&self, count: usize, refs: usize) {
        let mut stream = String::new();
        for i in 0..count {
            let message = format!("fixture commit {i}\n");
            stream.push_str(&format!("commit refs/heads/main\ncommitter Fixture <fixture@example.invalid> {} +0000\ndata {}\n{}\n",
                1_700_000_000 + i, message.len(), message));
        }
        self.git
            .fixture_input(&["fast-import", "--quiet"], stream.as_bytes())
            .unwrap();
        let tip = self.git.run(&["rev-parse", "HEAD"]).unwrap();
        let refs = (0..refs)
            .map(|i| format!("update refs/heads/fixture-{i} {}\n", tip.trim()))
            .collect::<String>();
        self.git
            .fixture_input(&["update-ref", "--stdin"], refs.as_bytes())
            .unwrap();
    }

    /// Creates an empty bare repository beside the worktree, removed with the
    /// fixture, for use as a local file-path remote.
    pub fn bare_remote(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        let mut command = Command::new(crate::platform::git_executable().unwrap());
        isolate_config(&mut command, &self.git.path);
        let status = command
            .args(["init", "--quiet", "--bare", "--initial-branch=main"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success(), "Fixture bare remote {name}");
        path
    }

    pub fn hook(&self, name: &str, script: &str) {
        let path = self.git.path.join(".git/hooks").join(name);
        fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(err) = remove_fixture(&self.root) {
            let message = format!("Could not clean fixture {}: {err}", self.root.display());
            if std::thread::panicking() {
                eprintln!("{message}");
            } else {
                panic!("{message}");
            }
        }
    }
}

#[cfg(not(windows))]
fn remove_fixture(root: &Path) -> std::io::Result<()> {
    fs::remove_dir_all(root)
}

#[cfg(windows)]
fn remove_fixture(root: &Path) -> std::io::Result<()> {
    use std::time::{Duration, Instant};

    const SHARING_VIOLATION: i32 = 32;
    // Windows ends processes killed through a job asynchronously, so they can
    // still hold the fixture for a moment after a cancelled command returns.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match clear_readonly(root).and_then(|()| fs::remove_dir_all(root)) {
            Err(err)
                if err.raw_os_error() == Some(SHARING_VIOLATION) && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(50));
            }
            result => return result,
        }
    }
}

#[cfg(windows)]
fn clear_readonly(directory: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::MetadataExt;

    const REPARSE_POINT: u32 = 0x400;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        // Keep traversal and attribute updates inside the owned fixture.
        if metadata.file_attributes() & REPARSE_POINT != 0 {
            continue;
        }
        if metadata.is_dir() {
            clear_readonly(&entry.path())?;
        } else {
            let mut permissions = metadata.permissions();
            if permissions.readonly() {
                #[expect(
                    clippy::permissions_set_readonly_false,
                    reason = "Windows-only: clears the read-only attribute, not Unix world-write"
                )]
                permissions.set_readonly(false);
                fs::set_permissions(entry.path(), permissions)?;
            }
        }
    }
    Ok(())
}
