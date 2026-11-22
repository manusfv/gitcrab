//! Git executable discovery and filesystem identity for local worktrees.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::OnceLock,
};

/// Locates Git and checks its version, caching the first result for the process.
///
/// Set `GITCRAB_GIT` to an absolute executable path to override discovery.
/// Unit tests use `GITCRAB_TEST_GIT` instead.
///
/// # Errors
///
/// Returns an error if the override is invalid, Git cannot be found or started,
/// or the executable does not report a supported version (2.43 or newer).
pub fn git_executable() -> Result<&'static Path, String> {
    static GIT: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    #[cfg(test)]
    let override_name = "GITCRAB_TEST_GIT";
    #[cfg(not(test))]
    let override_name = "GITCRAB_GIT";
    GIT.get_or_init(|| {
        let path = resolve_git(std::env::var_os(override_name), override_name)?;
        let mut version = std::process::Command::new(&path);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            version.creation_flags(0x08000000);
        }
        let output = version
            .arg("--version")
            .output()
            .map_err(|e| format!("Could not start Git: {e}"))?;
        let version = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() || !supported_git_version(&version) {
            return Err(format!(
                "Git 2.43 or newer is required. Found {} at {}.",
                version.trim(),
                path.display()
            ));
        }
        Ok(path)
    })
    .as_deref()
    .map_err(Clone::clone)
}

/// Git as started for read-only queries.
#[derive(Debug)]
pub struct ReadGit {
    pub executable: PathBuf,
    /// Variables Git for Windows' launcher would have set, when it is skipped.
    pub env: Vec<(&'static str, OsString)>,
}

/// Locates the Git used for read-only queries.
///
/// Git for Windows puts a launcher, `cmd\git.exe`, on PATH. It only extends
/// PATH, sets `MSYSTEM` and starts the real `git.exe`, which costs a second
/// process for every call. For reads, start that `git.exe` directly with the
/// same additions. Everywhere else this is [`git_executable`].
///
/// # Errors
///
/// Returns the [`git_executable`] error when Git is unavailable.
pub fn git_read_executable() -> Result<&'static ReadGit, String> {
    static GIT: OnceLock<Result<ReadGit, String>> = OnceLock::new();
    GIT.get_or_init(|| {
        let launcher = git_executable()?;
        Ok(skip_windows_launcher(launcher).unwrap_or_else(|| ReadGit {
            executable: launcher.to_owned(),
            env: vec![],
        }))
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// For `<root>\cmd\git.exe`, the `<root>\<toolchain>\bin\git.exe` it launches,
/// with the launcher's PATH and `MSYSTEM` additions.
fn skip_windows_launcher(launcher: &Path) -> Option<ReadGit> {
    if !cfg!(windows) {
        return None;
    }
    let cmd = launcher.parent()?;
    if !launcher.file_name()?.eq_ignore_ascii_case("git.exe")
        || !cmd.file_name()?.eq_ignore_ascii_case("cmd")
    {
        return None;
    }
    let root = cmd.parent()?;
    let (toolchain, bin) = ["mingw64", "clangarm64", "mingw32"]
        .into_iter()
        .map(|toolchain| (toolchain, root.join(toolchain).join("bin")))
        .find(|(_, bin)| bin.join("git.exe").is_file())?;
    let mut path = std::env::join_paths([bin.clone(), root.join("usr").join("bin")]).ok()?;
    if let Some(existing) = std::env::var_os("PATH") {
        path.push(";");
        path.push(existing);
    }
    Some(ReadGit {
        executable: bin.join("git.exe"),
        env: vec![
            ("PATH", path),
            ("MSYSTEM", toolchain.to_ascii_uppercase().into()),
        ],
    })
}

/// Oldest Git release GitCrab runs, as (major, minor).
const MIN_GIT_VERSION: (u32, u32) = (2, 43);

fn supported_git_version(output: &str) -> bool {
    let mut version = output
        .strip_prefix("git version ")
        .unwrap_or_default()
        .trim()
        .split('.');
    match (
        version.next().and_then(|n| n.parse::<u32>().ok()),
        version.next().and_then(|n| n.parse::<u32>().ok()),
    ) {
        (Some(major), Some(minor)) => (major, minor) >= MIN_GIT_VERSION,
        _ => false,
    }
}

fn resolve_git(explicit: Option<OsString>, override_name: &str) -> Result<PathBuf, String> {
    if let Some(path) = explicit.filter(|p| !p.is_empty()).map(PathBuf::from) {
        if path.is_absolute() && path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "{override_name} must name an existing absolute Git executable path."
        ));
    }
    let name = if cfg!(windows) { "git.exe" } else { "git" };
    if let Some(path) = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .filter(|directory| directory.is_absolute())
            .map(|directory| directory.join(name))
            .find(|candidate| candidate.is_file())
    }) {
        return Ok(path);
    }
    #[cfg(target_os = "macos")]
    {
        for path in [
            "/opt/homebrew/bin/git",
            "/usr/local/bin/git",
            "/usr/bin/git",
        ] {
            if Path::new(path).is_file() {
                return Ok(path.into());
            }
        }
    }
    Err(format!(
        "Install Git on an absolute PATH entry or set {override_name} to its executable path."
    ))
}

pub use gitcrab_core::platform::*;

/// Finding and rechecking a worktree on disk.
pub trait RepositoryLocationExt: Sized {
    /// Finds the nearest enclosing worktree by inspecting `.git` on disk.
    fn discover(path: &Path) -> Result<Self, String>;
    /// Checks that the paths still resolve the way the user confirmed them.
    fn verify(&self) -> Result<(), String>;
    /// Reads the metadata locations of the worktree at `worktree`.
    fn at_worktree(worktree: &Path) -> Result<Self, String>;
}

impl RepositoryLocationExt for RepositoryLocation {
    /// Finds the nearest enclosing worktree by inspecting `.git` on disk.
    ///
    /// Resolves gitfiles and `commondir` for linked worktrees without launching Git.
    /// Present the resulting paths for trust confirmation before opening the repository.
    ///
    /// # Errors
    ///
    /// Returns an error if `path` is not a directory, no enclosing worktree is
    /// found, or its metadata paths cannot be resolved.
    fn discover(path: &Path) -> Result<Self, String> {
        let selected = path.canonicalize().map_err(|error| error.to_string())?;
        if !selected.is_dir() {
            return Err("Choose a repository folder.".into());
        }
        for worktree in selected.ancestors() {
            let marker = worktree.join(".git");
            match marker.symlink_metadata() {
                Ok(_) => return Self::at_worktree(worktree),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.to_string()),
            }
        }
        Err("Choose a local Git worktree containing .git metadata.".into())
    }

    fn verify(&self) -> Result<(), String> {
        let current = Self::at_worktree(&self.worktree).ok();
        if current.as_ref() != Some(self) {
            return Err(
                "The repository location changed. Choose the folder and review trust again.".into(),
            );
        }
        #[cfg(windows)]
        for path in [&self.worktree, &self.git_dir, &self.common_dir] {
            if let std::borrow::Cow::Owned(argument) = git_path(path) {
                if argument.canonicalize().ok().as_ref() != Some(path) {
                    return Err(format!(
                        "Git for Windows cannot open the repository path {}.",
                        path.display()
                    ));
                }
            }
        }
        Ok(())
    }

    fn at_worktree(worktree: &Path) -> Result<Self, String> {
        let worktree = worktree.canonicalize().map_err(|error| error.to_string())?;
        let marker = worktree.join(".git");
        let metadata = marker.metadata().map_err(|error| error.to_string())?;
        let git_dir = if metadata.is_dir() {
            canonical_directory(&marker)?
        } else {
            read_location(&marker, b"gitdir: ")?
        };
        let common_marker = git_dir.join("commondir");
        let common_dir = match common_marker.symlink_metadata() {
            Ok(_) => read_location(&common_marker, b"")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => git_dir.clone(),
            Err(error) => return Err(error.to_string()),
        };
        Ok(Self {
            worktree,
            git_dir,
            common_dir,
        })
    }
}

/// Git for Windows does not accept Rust's verbatim drive and UNC syntax.
pub(crate) fn git_path(path: &Path) -> std::borrow::Cow<'_, Path> {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let mut components = path.components();
        if let Some(Component::Prefix(prefix)) = components.next() {
            let mut argument = match prefix.kind() {
                Prefix::VerbatimDisk(drive) => PathBuf::from(format!("{}:\\", char::from(drive))),
                Prefix::VerbatimUNC(server, share) => {
                    let mut unc = OsString::from(r"\\");
                    unc.push(server);
                    unc.push(r"\");
                    unc.push(share);
                    PathBuf::from(unc)
                }
                _ => return std::borrow::Cow::Borrowed(path),
            };
            if matches!(components.next(), Some(Component::RootDir)) {
                argument.push(components.as_path());
            }
            return std::borrow::Cow::Owned(argument);
        }
    }
    std::borrow::Cow::Borrowed(path)
}

fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
    let directory = path.canonicalize().map_err(|error| error.to_string())?;
    if !directory.is_dir() {
        return Err(format!(
            "Git metadata location is not a directory. {}",
            path.display()
        ));
    }
    Ok(directory)
}

fn read_location(marker: &Path, prefix: &[u8]) -> Result<PathBuf, String> {
    use std::io::Read;

    const MAX_LOCATION_BYTES: usize = 1024 * 1024;
    let invalid = || format!("Invalid Git metadata location in {}", marker.display());
    let metadata = marker.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_LOCATION_BYTES as u64 {
        return Err(invalid());
    }
    let mut contents = Vec::new();
    std::fs::File::open(marker)
        .and_then(|file| {
            file.take(MAX_LOCATION_BYTES as u64 + 1)
                .read_to_end(&mut contents)
        })
        .map_err(|error| error.to_string())?;
    if contents.len() > MAX_LOCATION_BYTES {
        return Err(invalid());
    }
    while matches!(contents.last(), Some(b'\n' | b'\r')) {
        contents.pop();
    }
    let path = contents
        .strip_prefix(prefix)
        .filter(|path| !path.is_empty() && !path.contains(&0))
        .ok_or_else(invalid)?;
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(path)
    };
    #[cfg(not(unix))]
    let path = std::str::from_utf8(path).map_err(|_| invalid())?;
    canonical_directory(&marker.parent().expect("Git marker has parent").join(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_capability_floor_allows_platform_version_suffixes() {
        assert!(supported_git_version("git version 2.43.0\n"));
        assert!(supported_git_version("git version 2.53.0.windows.3\r\n"));
        assert!(supported_git_version(
            "git version 2.50.1 (Apple Git-155)\n"
        ));
        assert!(!supported_git_version("git version 2.42.9"));
        assert!(!supported_git_version("unknown"));
    }
}
