//! Git subprocesses and repository data for a trusted local worktree.
//!
//! [`Git`] passes arguments directly to Git. Reviewed writes check repository
//! state before acting and may verify the result afterward. An external Git
//! process can still change the worktree between inspection and execution.
use crate::platform::RepositoryLocationExt;
use crate::process::{capture, capture_read, ReadScope};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
};
// The parser and git apply consume this format. Presentation changes belong
// in diff::parse or the renderer, after the original patch is cached.
const PATCH_OPTIONS: &[&str] = &[
    "--patch",
    "--no-ext-diff",
    "--no-textconv",
    "--no-color",
    "--no-relative",
    "--src-prefix=a/",
    "--dst-prefix=b/",
    "--line-prefix=",
    "--output-indicator-new=+",
    "--output-indicator-old=-",
    "--output-indicator-context= ",
    "--full-index",
    "--word-diff=none",
    "--diff-algorithm=myers",
    "--no-indent-heuristic",
    "--unified=3",
    "--inter-hunk-context=0",
    "--submodule=short",
    "--ignore-submodules=none",
];

fn patch_args(command: &str) -> Vec<&str> {
    let mut args = vec!["-c", "diff.suppressBlankEmpty=false", command];
    args.extend_from_slice(PATCH_OPTIONS);
    args
}

mod network;
mod read;
#[cfg(test)]
mod tests;
mod write;

#[cfg(test)]
pub(crate) use network::network_tests;

/// Access to Git commands for one trusted local worktree.
///
/// Write methods return [`WriteError`]. Its [`FailureKind`] distinguishes a
/// rejected request, a Git failure, a conflict and an uncertain outcome.
#[derive(Clone, Debug)]
pub struct Git {
    /// Canonical worktree root supplied to Git.
    pub path: PathBuf,
    pub(crate) read_scope: Option<ReadScope>,
    pub(crate) git_dir: Option<PathBuf>,
    pub(crate) common_dir: Option<PathBuf>,
    /// The last index listing and the index file it was read from.
    pub(crate) index_cache: IndexCache,
    #[cfg(test)]
    pub(crate) observe_command: Option<TestCommandObserver>,
}

/// Reuses `ls-files --stage` output while the index file is unchanged.
///
/// The index file ends with a checksum of everything before it, so the same
/// length and trailing checksum mean the same content and the same listing.
/// Shared by clones of a [`Git`], which all read the same index.
#[derive(Clone, Debug, Default)]
pub(crate) struct IndexCache(Arc<Mutex<Option<CachedListing>>>);

/// An index listing and the index file it was read from.
type CachedListing = (IndexStamp, Vec<u8>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IndexStamp {
    len: u64,
    trailer: [u8; 32],
}

impl IndexStamp {
    /// Reads the length and the last 32 bytes, which hold the SHA-1 or
    /// SHA-256 checksum, from one open handle. `None` when the file is
    /// missing or has no checksum (`index.skipHash` writes zeros).
    fn read(index: &Path) -> Option<Self> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(index).ok()?;
        let len = file.metadata().ok()?.len();
        if len < 64 {
            return None;
        }
        file.seek(SeekFrom::End(-32)).ok()?;
        let mut trailer = [0; 32];
        file.read_exact(&mut trailer).ok()?;
        (trailer[12..] != [0; 20]).then_some(Self { len, trailer })
    }
}

#[cfg(test)]
type TestCommandObserver = fn(&Git, &[&str]) -> GitResult<()>;

impl Git {
    #[cfg(test)]
    pub(crate) fn fixture_input(&self, args: &[&str], input: &[u8]) -> GitResult<()> {
        let output =
            capture(self.command(args)?, Some(input), 0, None).map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(Self::failed_output(&output))
        }
    }

    /// Opens a trusted repository from its root or a directory inside its worktree.
    ///
    /// Call only after the user has confirmed trust in the local Git metadata.
    /// Use [`crate::platform::RepositoryLocation::discover`] to find the paths
    /// for trust confirmation before running Git.
    ///
    /// # Errors
    ///
    /// Returns an error if the worktree cannot be discovered, its metadata
    /// paths have changed, or Git cannot open the repository.
    pub fn open(path: impl AsRef<Path>) -> GitResult<Self> {
        Self::open_confirmed(
            crate::platform::RepositoryLocation::discover(path.as_ref())?,
            None,
        )
    }

    pub(crate) fn open_confirmed(
        location: crate::platform::RepositoryLocation,
        read_scope: Option<ReadScope>,
    ) -> GitResult<Self> {
        location.verify()?;
        let repo = Self {
            path: location.worktree,
            read_scope,
            git_dir: Some(location.git_dir),
            common_dir: Some(location.common_dir),
            index_cache: IndexCache::default(),
            #[cfg(test)]
            observe_command: None,
        };
        repo.bytes(&["rev-parse", "--show-toplevel"])?;
        Ok(repo)
    }

    /// Builds a Git command for writes, hooks and network operations.
    fn command(&self, args: &[&str]) -> GitResult<Command> {
        self.command_with(Command::new(crate::platform::git_executable()?), args)
    }

    /// Builds a Git command for a read-only query, which may skip Git for
    /// Windows' launcher; see [`crate::platform::git_read_executable`].
    fn read_command(&self, args: &[&str]) -> GitResult<Command> {
        let git = crate::platform::git_read_executable()?;
        let mut cmd = Command::new(&git.executable);
        cmd.envs(git.env.iter().map(|(key, value)| (key, value)));
        self.command_with(cmd, args)
    }

    fn command_with(&self, mut cmd: Command, args: &[&str]) -> GitResult<Command> {
        if let Some(directory) = &self.git_dir {
            cmd.arg("--git-dir")
                .arg(crate::platform::git_path(directory).as_ref())
                .arg("--work-tree")
                .arg(crate::platform::git_path(&self.path).as_ref());
        }
        // A trusted repository can still be opened from an unrelated shell or hook.
        // Resolve repo, index and config from the selected worktree and normal settings.
        for (key, _) in std::env::vars_os() {
            if key
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
            {
                cmd.env_remove(key);
            }
        }
        untranslated_messages(&mut cmd);
        #[cfg(test)]
        {
            crate::test_repo::isolate_config(&mut cmd, &self.path);
            crate::test_repo::SUBPROCESSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        cmd.current_dir(&self.path).arg("--no-pager");
        // check-ignore rejects pathspec magic. It matches each path it is
        // given against the ignore rules and never expands globs, so its
        // paths are literal already.
        if args.first() != Some(&"check-ignore") {
            cmd.arg("--literal-pathspecs");
        }
        cmd.args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0");
        if let Some(common_dir) = &self.common_dir {
            cmd.env(
                "GIT_COMMON_DIR",
                crate::platform::git_path(common_dir).as_ref(),
            );
        }
        Ok(cmd)
    }

    fn output(&self, args: &[&str]) -> GitResult<std::process::Output> {
        #[cfg(test)]
        if let Some(observe) = self.observe_command {
            observe(self, args)?;
        }
        let scope = self.read_scope.clone().unwrap_or_default();
        capture_read(self.read_command(args)?, 0, Some(&scope))
            .map_err(|e| format!("Could not complete Git command: {e}"))
    }

    fn failed_output(output: &std::process::Output) -> String {
        let stderr = String::from_utf8_lossy(&output.stderr);
        format!("Git exited with {}. {}", output.status, stderr.trim())
            .trim_end()
            .to_owned()
    }

    /// `git ls-files --stage -z`, reused from the last read while the index
    /// file's length and checksum are unchanged.
    fn index_listing(&self) -> GitResult<Vec<u8>> {
        let index = self.git_dir.as_ref().map(|dir| dir.join("index"));
        let before = index.as_deref().and_then(IndexStamp::read);
        if let Some(stamp) = &before {
            if let Some((cached, listing)) = &*self.index_cache.0.lock().unwrap() {
                if cached == stamp {
                    return Ok(listing.clone());
                }
            }
        }
        let listing = self.bytes(&["ls-files", "--stage", "-z"])?;
        // Keep the listing only if the index did not change while Git read it.
        if let Some(stamp) = before {
            if index.as_deref().and_then(IndexStamp::read).as_ref() == Some(&stamp) {
                *self.index_cache.0.lock().unwrap() = Some((stamp, listing.clone()));
            }
        }
        Ok(listing)
    }

    fn bytes(&self, args: &[&str]) -> GitResult<Vec<u8>> {
        let output = self.output(args)?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(Self::failed_output(&output))
        }
    }

    /// Runs a Git query and returns UTF-8 stdout, including any trailing newline.
    ///
    /// Supply arguments as separate strings. Git runs with paging and terminal
    /// prompts disabled and treats pathspecs literally. Use the write methods
    /// for mutations, which need to run to completion.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot run, fails, or returns non-UTF-8 stdout.
    pub(crate) fn run(&self, args: &[&str]) -> GitResult<String> {
        String::from_utf8(self.bytes(args)?)
            .map_err(|_| "Git returned text that is not UTF-8".into())
    }

    pub(crate) fn write(&self, args: &[&str]) -> Result<String, WriteError> {
        let output = capture(self.command(args)?, None, 0, None).map_err(WriteError::inspect)?;
        if !output.status.success() {
            return Err(WriteError::git(Self::failed_output(&output)));
        }
        #[cfg(test)]
        if let Some(observe) = self.observe_command {
            observe(self, args).map_err(WriteError::inspect)?;
        }
        String::from_utf8(output.stdout).map_err(WriteError::inspect)
    }

    pub(crate) fn resolve_commit(&self, revision: &str) -> GitResult<String> {
        validate_revision(revision)?;
        let commit = format!("{revision}^{{commit}}");
        self.run(&["rev-parse", "--verify", "--end-of-options", &commit])
            .map(|id| id.trim().to_owned())
    }

    pub(crate) fn ref_exists(&self, reference: &str) -> GitResult<bool> {
        let output = self.output(&["show-ref", "--exists", reference])?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(2) => Ok(false),
            _ => Err(Self::failed_output(&output)),
        }
    }

    fn symbolic_head(&self) -> GitResult<String> {
        let output = self.output(&["symbolic-ref", "--quiet", "HEAD"])?;
        match output.status.code() {
            Some(0) => String::from_utf8(output.stdout)
                .map(|s| s.trim().into())
                .map_err(|e| e.to_string()),
            Some(1) if output.stderr.is_empty() => Ok(String::new()),
            _ => Err(Self::failed_output(&output)),
        }
    }

    fn head_commit_output(&self) -> GitResult<std::process::Output> {
        self.output(&["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
    }

    /// Interprets [`Self::head_commit_output`]; `symbolic` tells an unborn
    /// branch apart from a broken HEAD.
    fn head_id_from(&self, output: std::process::Output, symbolic: &str) -> GitResult<String> {
        match output.status.code() {
            Some(0) => String::from_utf8(output.stdout)
                .map(|s| s.trim().into())
                .map_err(|e| e.to_string()),
            Some(1)
                if output.stderr.is_empty()
                    && symbolic.starts_with("refs/heads/")
                    && !self.ref_exists(symbolic)? =>
            {
                Ok(String::new())
            }
            _ => Err(Self::failed_output(&output)),
        }
    }

    pub(crate) fn has_unmerged_entries(&self) -> GitResult<bool> {
        self.has_unmerged_paths(&[])
    }

    fn has_unmerged_paths(&self, paths: &[&str]) -> GitResult<bool> {
        let mut args = vec!["ls-files", "--unmerged", "-z", "--"];
        args.extend_from_slice(paths);
        Ok(!self.bytes(&args)?.is_empty())
    }

    pub(crate) fn reject_unmerged_paths(&self, paths: &[&str]) -> Result<(), WriteError> {
        if self.has_unmerged_paths(paths)? {
            return Err(Self::unmerged_write_error());
        }
        Ok(())
    }

    fn unmerged_write_error() -> WriteError {
        WriteError {
            kind: FailureKind::Conflict,
            message: "The Git index has unresolved conflicts in the requested paths. Resolve them in a terminal, then refresh GitCrab.".into(),
        }
    }

    /// Returns the absolute worktree-specific Git directory and common directory, in that order.
    ///
    /// # Errors
    ///
    /// Returns an error if Git fails or the directories cannot be read as absolute paths.
    pub fn metadata_dirs(&self) -> GitResult<(PathBuf, PathBuf)> {
        let dirs = self.run(&[
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ])?;
        let mut paths = dirs.lines().map(PathBuf::from);
        match (paths.next(), paths.next(), paths.next()) {
            (Some(git), Some(common), None) if git.is_absolute() && common.is_absolute() => {
                Ok((git, common))
            }
            _ => Err("Could not determine Git metadata directories.".into()),
        }
    }

    /// Returns which of the absolute worktree `paths` Git's ignore rules
    /// exclude. Tracked files are never reported, since their changes still
    /// show, and neither are paths outside the worktree or not valid UTF-8.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot run or fails, for example when a path
    /// lies inside a submodule.
    pub fn ignored_paths<'a>(
        &self,
        paths: impl IntoIterator<Item = &'a PathBuf>,
    ) -> GitResult<std::collections::BTreeSet<PathBuf>> {
        let mut asked = std::collections::HashMap::new();
        let mut input = Vec::new();
        for path in paths {
            let Some(relative) = path
                .strip_prefix(&self.path)
                .ok()
                .and_then(Path::to_str)
                .filter(|relative| !relative.is_empty())
            else {
                continue;
            };
            let relative = relative.replace('\\', "/");
            input.extend_from_slice(relative.as_bytes());
            input.push(0);
            asked.insert(relative, path.clone());
        }
        if asked.is_empty() {
            return Ok(Default::default());
        }
        let output = capture(
            self.read_command(&["check-ignore", "--stdin", "-z"])?,
            Some(&input),
            1,
            None,
        )
        .map_err(|e| format!("Could not complete Git command: {e}"))?;
        // Exit status 1 means none of the paths is ignored.
        match output.status.code() {
            Some(0) => Ok(output
                .stdout
                .split(|&byte| byte == 0)
                .filter_map(|path| asked.get(std::str::from_utf8(path).ok()?).cloned())
                .collect()),
            Some(1) => Ok(Default::default()),
            _ => Err(Self::failed_output(&output)),
        }
    }
}

/// Keeps Git's messages in English, which GitCrab matches to explain
/// failures, while leaving the character set and every other locale category
/// as the user set them. Hooks and helpers that Git starts inherit the same
/// environment, so only their message language changes.
///
/// `LC_ALL` overrides `LC_MESSAGES`, so a set `LC_ALL` is moved to `LANG`,
/// the lowest-priority setting, after the other `LC_*` values it was hiding
/// are removed. `LANGUAGE` is ignored while messages use the C locale.
fn untranslated_messages(cmd: &mut Command) {
    let all = std::env::var_os("LC_ALL").filter(|value| !value.is_empty());
    if let Some(all) = all {
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("LC_") {
                cmd.env_remove(key);
            }
        }
        cmd.env("LANG", all);
    }
    cmd.env("LC_MESSAGES", "C").env_remove("LANGUAGE");
}

fn validate_revision(value: &str) -> GitResult<()> {
    if value.is_empty() || value.starts_with('-') || value.contains('\0') {
        Err("Invalid branch or revision".into())
    } else {
        Ok(())
    }
}

pub use gitcrab_core::git::*;

#[derive(Clone, Copy)]
pub(crate) enum HistoryRead {
    Full,
    Worktree,
    More,
}

pub(crate) struct HistoryCache {
    pub(super) refs: String,
    pub(super) head: String,
    pub(super) limit: usize,
    /// History as `git log` printed it, which paging appends to.
    pub(super) commits: Vec<Commit>,
    /// `git stash list` output the rows were built with.
    pub(super) stashes: String,
    /// `commits` with the stash entries placed among them.
    pub(super) rows: Vec<Commit>,
    pub(super) graph: crate::graph::Graph,
    pub(super) metadata: u64,
}

pub(super) struct RepositoryMetadata {
    pub(super) operations: Vec<String>,
    /// Git's prepared commit message for the operation in progress.
    pub(super) message: Option<String>,
    pub(super) history_stamp: u64,
}
