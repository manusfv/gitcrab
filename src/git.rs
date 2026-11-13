//! Git subprocesses and repository data for a trusted local worktree.
//!
//! [`Git`] passes arguments directly to Git. Reviewed writes check repository
//! state before acting and may verify the result afterward. An external Git
//! process can still change the worktree between inspection and execution.
use crate::process::{capture, ReadScope};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
};

/// Git query result with a user-facing error message.
pub type GitResult<T> = Result<T, String>;

/// Failure category used to decide what to show or check before retrying.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FailureKind {
    /// A request failed validation or a required read failed.
    #[default]
    Rejected,
    /// The Git write command reported failure and may have left changes.
    GitFailure,
    /// Git left unmerged entries in the index.
    Conflict,
    /// The write outcome could not be verified and requires inspection.
    Inspect,
    /// A resolution was refused because the working file still has conflict markers.
    ConflictMarkers,
}

/// A failed or unverified write, with a category and user-facing message.
#[derive(Debug)]
pub struct WriteError {
    pub kind: FailureKind,
    pub message: String,
}

impl WriteError {
    /// Reports a Git command failure and prompts the user to check working changes.
    pub fn git(message: String) -> Self {
        Self {
            kind: FailureKind::GitFailure,
            message: format!("{message} Inspect Working changes before retrying."),
        }
    }

    /// Reports an uncertain outcome and prompts the user to refresh and inspect.
    pub fn inspect(message: impl std::fmt::Display) -> Self {
        Self {
            kind: FailureKind::Inspect,
            message: format!("{message} Refresh and inspect the repository before retrying."),
        }
    }
}

impl From<String> for WriteError {
    fn from(message: String) -> Self {
        Self {
            kind: FailureKind::Rejected,
            message,
        }
    }
}

impl From<&str> for WriteError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

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
/// Worktree metadata paths paired with the operation names they indicate.
pub const OPERATION_MARKERS: &[(&str, &str)] = &[
    ("MERGE_HEAD", "merge"),
    ("CHERRY_PICK_HEAD", "cherry-pick"),
    ("REVERT_HEAD", "revert"),
    ("rebase-merge", "rebase"),
    ("rebase-apply", "rebase"),
    ("sequencer", "sequencer"),
];

fn patch_args(command: &str) -> Vec<&str> {
    let mut args = vec!["-c", "diff.suppressBlankEmpty=false", command];
    args.extend_from_slice(PATCH_OPTIONS);
    args
}

/// A commit and its decorations in the loaded history.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct Commit {
    /// Full Git object ID.
    pub id: String,
    /// Parent object IDs in Git's order, with the first parent first.
    pub parents: Vec<String>,
    pub subject: String,
    pub author: String,
    /// Author date in strict ISO 8601 format.
    pub date: String,
    /// Display names of refs pointing at this commit, including the `HEAD` label.
    pub refs: Vec<String>,
    /// Lowercase search text built from the subject, author, object ID and refs.
    #[serde(default)]
    pub search_key: String,
}

impl Commit {
    fn index_search(mut self) -> Self {
        self.search_key = format!(
            "{} {} {} {}",
            self.subject,
            self.author,
            self.id,
            self.refs.join(" ")
        )
        .to_lowercase();
        self
    }

    /// Searches the cached text for a substring prepared by [`normalize_search`].
    pub fn matches_search(&self, normalized_query: &str) -> bool {
        self.search_key.contains(normalized_query)
    }
}

/// Trims and lowercases a query for [`Commit::matches_search`].
pub fn normalize_search(query: &str) -> String {
    query.trim().to_lowercase()
}

/// A changed path from working-tree status or a commit comparison.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct FileChange {
    /// Repository-relative path, using the destination for renames and copies.
    pub path: String,
    /// Repository-relative source path for a rename or copy.
    pub original_path: Option<String>,
    /// Porcelain index status, or the change status for a commit comparison.
    pub index: char,
    /// Porcelain working-tree status, or a space for a commit comparison.
    pub worktree: char,
    /// The paths include non-UTF-8 bytes and are only suitable for display.
    #[serde(default)]
    pub unsupported_path: bool,
}

impl FileChange {
    /// Escapes a supported path for display or returns the stored label for an unsupported path.
    pub fn display_path(&self) -> String {
        if self.unsupported_path {
            self.path.clone()
        } else {
            display_path(&self.path)
        }
    }
    /// Returns the reason file actions are unavailable for a non-UTF-8 path.
    pub fn path_issue(&self) -> Option<&'static str> {
        self.unsupported_path
            .then_some("This Git path contains bytes the file controls cannot address.")
    }
    /// Reports whether the porcelain status describes unresolved index entries.
    pub fn is_unmerged(&self) -> bool {
        matches!(
            (self.index, self.worktree),
            ('D', 'D')
                | ('A', 'U')
                | ('U', 'D')
                | ('U', 'A')
                | ('D', 'U')
                | ('A', 'A')
                | ('U', 'U')
        )
    }

    /// Names the change on one side of working changes, or a commit comparison's change.
    ///
    /// Unmerged entries are named by their conflict type.
    pub fn status_name(&self, staged: bool) -> &'static str {
        if self.is_unmerged() {
            return match (self.index, self.worktree) {
                ('U', 'U') => "Both modified",
                ('A', 'A') => "Both added",
                ('D', 'D') => "Both deleted",
                ('A', 'U') => "Added by us",
                ('U', 'A') => "Added by them",
                ('D', 'U') => "Deleted by us",
                _ => "Deleted by them",
            };
        }
        let status = if self.index == '?' {
            '?'
        } else if staged {
            self.index
        } else {
            self.worktree
        };
        match status {
            '?' => "Untracked",
            'A' => "Added",
            'D' => "Deleted",
            'R' => "Renamed",
            'C' => "Copied",
            'T' => "Type changed",
            'M' => "Modified",
            _ => "Changed",
        }
    }

    /// Describes the path for a tooltip, naming the status and any rename or copy source.
    pub fn describe(&self, staged: bool) -> String {
        let name = self.status_name(staged);
        if let Some(issue) = self.path_issue() {
            return format!("{name}: {} · {issue}", self.display_path());
        }
        match &self.original_path {
            Some(old) if matches!(self.index, 'R' | 'C') => {
                format!("{name}: {} → {}", display_path(old), self.display_path())
            }
            _ => format!("{name}: {}", self.display_path()),
        }
    }

    /// Reports whether this path belongs in an ordinary staged or unstaged list.
    /// Unmerged paths belong in the conflict list, and untracked paths are unstaged.
    pub fn on_side(&self, staged: bool) -> bool {
        if self.is_unmerged() {
            return false;
        }
        if staged {
            self.index != ' ' && self.index != '?'
        } else {
            self.worktree != ' ' || self.index == '?'
        }
    }
}

/// Escapes a path for display using Rust's debug escaping without quotes.
pub fn display_path(path: &str) -> String {
    path.escape_debug().to_string()
}

/// Returns the 1-based line of the first conflict marker in a merged file.
///
/// Recognizes `<<<<<<<`, `|||||||`, `=======` and `>>>>>>>` lines, but only
/// when an opening or closing marker is present so a lone `=======`
/// underline is not reported.
pub fn conflict_marker_line(content: &[u8]) -> Option<usize> {
    let is_marker = |line: &[u8], marker: &[u8], bare: bool| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        line.strip_prefix(marker).is_some_and(|rest| {
            if bare {
                rest.is_empty()
            } else {
                rest.is_empty() || rest[0] == b' '
            }
        })
    };
    let lines = || content.split(|&byte| byte == b'\n');
    let bounded = lines()
        .any(|line| is_marker(line, b"<<<<<<<", false) || is_marker(line, b">>>>>>>", false));
    if !bounded {
        return None;
    }
    lines()
        .position(|line| {
            is_marker(line, b"<<<<<<<", false)
                || is_marker(line, b"|||||||", false)
                || is_marker(line, b"=======", true)
                || is_marker(line, b">>>>>>>", false)
        })
        .map(|index| index + 1)
}

/// Result of a whole-worktree index action.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BulkStage {
    /// Distinct paths whose status entry changed.
    pub changed: usize,
    /// Unmerged paths left for explicit resolution.
    pub conflicts: usize,
}
/// HEAD and index state used to detect changes since a write was reviewed.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct WriteIdentity {
    /// Full HEAD commit object ID, or an empty string for an unborn branch.
    pub head: String,
    /// Full symbolic ref of HEAD, or an empty string for detached HEAD.
    pub symbolic_head: String,
    /// Raw NUL-delimited index entries from `git ls-files --stage -z`.
    pub index: Vec<u8>,
}
/// Local branch and the commit it points to.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct LocalBranch {
    /// Branch name without the `refs/heads/` prefix.
    pub name: String,
    pub tip: String,
}
/// Remote-tracking ref or tag and its target object.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct ReferenceTip {
    /// Full ref name, including `refs/remotes/` or `refs/tags/`.
    pub full_name: String,
    /// Short ref name for display.
    pub label: String,
    /// Target object ID, dereferenced for an annotated tag.
    pub tip: String,
}

/// Reference namespace used for history previews.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceKind {
    LocalBranch,
    Remote,
    Tag,
}

/// A stash list entry as people identify it.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct StashEntry {
    /// Reflog subject, such as `On main: message`.
    pub message: String,
    /// Creation date in Git's ISO-like format.
    pub date: String,
}

/// Repository state read together for a tab refresh.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct RepoSnapshot {
    /// Canonical path used to identify this worktree.
    #[serde(default)]
    pub worktree_id: PathBuf,
    /// Loaded history in child-before-parent date order.
    pub commits: Vec<Commit>,
    pub branches: Vec<LocalBranch>,
    #[serde(default)]
    pub remote_refs: Vec<ReferenceTip>,
    #[serde(default)]
    pub stash_count: usize,
    /// Message and date of `stash@{0}`, the entry Pop stash reviews.
    #[serde(default)]
    pub latest_stash: Option<StashEntry>,
    #[serde(default)]
    pub tags: Vec<ReferenceTip>,
    /// Branch name or abbreviated detached HEAD object ID.
    pub head: String,
    /// Full HEAD object ID, even outside the loaded page. Empty for unborn HEAD.
    pub head_id: String,
    pub status: Vec<FileChange>,
    #[serde(default)]
    pub operations: Vec<String>,
    /// HEAD and index identity captured for subsequent write review.
    #[serde(default)]
    pub reviewed: Option<WriteIdentity>,
    /// Display text for the upstream relationship based on local refs.
    #[serde(default)]
    pub upstream_context: String,
    /// Remotes, the checked-out branch's upstream and the last fetch time.
    #[serde(default)]
    pub remote: RemoteState,
    /// Undo and Redo availability, filled in by the tab's worker.
    #[serde(default)]
    pub journal: crate::journal::JournalStatus,
}

impl RepoSnapshot {
    /// Returns distinct changed destination paths across staged and unstaged changes.
    pub fn unique_changes(&self) -> std::collections::BTreeSet<&str> {
        self.status.iter().map(|f| f.path.as_str()).collect()
    }
    /// Counts paths with unresolved conflicts separately from ordinary changes.
    pub fn conflict_count(&self) -> usize {
        self.status.iter().filter(|file| file.is_unmerged()).count()
    }

    /// Counts ordinary paths as `[modified, added, deleted, renamed_or_copied]`.
    /// Untracked paths count as added.
    pub fn change_counts(&self) -> [usize; 4] {
        let mut paths: std::collections::BTreeMap<&str, Vec<char>> = Default::default();
        for f in self.status.iter().filter(|file| !file.is_unmerged()) {
            paths
                .entry(&f.path)
                .or_default()
                .extend([f.index, f.worktree]);
        }
        let mut counts = [0; 4];
        for status in paths.values() {
            let category = if status.contains(&'R') || status.contains(&'C') {
                3
            } else if status.contains(&'?') || status.contains(&'A') {
                1
            } else if status.contains(&'D') {
                2
            } else {
                0
            };
            counts[category] += 1;
        }
        counts
    }
    /// Reports whether an active operation or unresolved status blocks an ordinary commit.
    pub fn commit_blocked(&self) -> bool {
        !self.operations.is_empty() || self.status.iter().any(FileChange::is_unmerged)
    }

    /// Lists status indices for an ordinary staged or unstaged list in status order.
    /// Unmerged paths are excluded. See [`Self::conflicts`].
    pub fn side(&self, staged: bool) -> Vec<usize> {
        (0..self.status.len())
            .filter(|&index| self.status[index].on_side(staged))
            .collect()
    }

    /// Lists status indices of unmerged paths in status order.
    pub fn conflicts(&self) -> Vec<usize> {
        (0..self.status.len())
            .filter(|&index| self.status[index].is_unmerged())
            .collect()
    }

    /// Reports whether HEAD points at a commit without a checked-out local branch.
    pub fn is_detached(&self) -> bool {
        !self.head_id.is_empty() && self.checked_out_branch().is_none()
    }

    /// Names the active operations for display, such as `merge` or `rebase`.
    pub fn operation_label(&self) -> Option<String> {
        (!self.operations.is_empty()).then(|| {
            self.operations
                .iter()
                .map(|op| match op.as_str() {
                    "sequencer" => "multi-commit sequence",
                    op => op,
                })
                .collect::<Vec<_>>()
                .join(" and ")
        })
    }

    /// Describes how to finish or abandon the active operation with Git.
    pub fn operation_recovery(&self) -> String {
        let command = self
            .operations
            .iter()
            .map(String::as_str)
            .find(|op| *op != "sequencer")
            .or_else(|| (!self.operations.is_empty()).then_some("cherry-pick"));
        let Some(command) = command else {
            return "Edit each conflicted file, mark it resolved, then commit the result.".into();
        };
        let continuation = if command == "merge" {
            "commit the merge or run `git merge --continue`".to_owned()
        } else {
            format!("run `git {command} --continue`")
        };
        format!(
            "Edit each conflicted file and mark it resolved. Then {continuation} in a terminal, or run `git {command} --abort` to return to the state before the {command}. Refresh GitCrab afterwards."
        )
    }

    /// Explains why an ordinary commit is unavailable, without checking the summary.
    pub fn commit_blocker(&self) -> Option<String> {
        let conflicts = self.conflict_count();
        if conflicts > 0 {
            return Some(format!(
                "Resolve {conflicts} conflicted {} before committing.",
                if conflicts == 1 { "file" } else { "files" }
            ));
        }
        if let Some(op) = self.operation_label() {
            return Some(format!(
                "A {op} is in progress. Continue or abort it in a terminal, then refresh."
            ));
        }
        if self.is_detached() {
            return Some(
                "HEAD is detached. Create a branch first so the new commit stays reachable.".into(),
            );
        }
        if self.side(true).is_empty() {
            return Some("Stage changes to create a commit.".into());
        }
        None
    }

    /// Returns the short local branch name from the reviewed symbolic HEAD, if present.
    pub fn checked_out_branch(&self) -> Option<&str> {
        self.reviewed
            .as_ref()?
            .symbolic_head
            .strip_prefix("refs/heads/")
    }

    /// Returns a ref's object ID if it points to a commit in the loaded history.
    /// Use the short name for a local branch and the full ref name for a remote
    /// or tag.
    pub fn loaded_ref_tip(&self, kind: ReferenceKind, name: &str) -> Option<&str> {
        let tip = match kind {
            ReferenceKind::LocalBranch => &self.branches.iter().find(|b| b.name == name)?.tip,
            ReferenceKind::Remote | ReferenceKind::Tag => {
                let refs = if kind == ReferenceKind::Remote {
                    &self.remote_refs
                } else {
                    &self.tags
                };
                &refs
                    .iter()
                    .find(|reference| reference.full_name == name)?
                    .tip
            }
        };
        self.commits
            .iter()
            .any(|c| &c.id == tip)
            .then_some(tip.as_str())
    }

    /// Formats HEAD for display, with a label for unborn or detached HEAD.
    pub fn head_label(&self) -> String {
        if self.head_id.is_empty() {
            format!("{} · no commits", self.head)
        } else if self.checked_out_branch().is_none() {
            format!("Detached HEAD · {}", self.head)
        } else {
            self.head.clone()
        }
    }
}

/// One hunk from a Git patch, with the headers needed to apply it independently.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Hunk {
    /// Hunk header with trailing whitespace removed.
    pub title: String,
    /// Self-contained patch with original file headers and one hunk.
    pub patch: String,
}

/// A machine-format Git patch and the source identity for a rename or copy.
pub struct DiffComparison {
    pub patch: String,
    /// Source path in the before side of this comparison.
    pub original_path: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) enum HistoryRead {
    Full,
    Worktree,
    More,
}

pub(crate) struct HistoryCache {
    refs: String,
    head: String,
    limit: usize,
    commits: Vec<Commit>,
    graph: crate::graph::Graph,
    metadata: u64,
}

struct RepositoryMetadata {
    operations: Vec<String>,
    history_stamp: u64,
}
impl Hunk {
    /// Returns a reason to use a whole-file action based on the patch headers.
    pub fn blocked_reason(&self) -> Option<&'static str> {
        header_blocked_reason(&self.patch)
    }
}

fn header_blocked_reason(patch: &str) -> Option<&'static str> {
    let mut header = patch.lines();
    if !header
        .next()
        .is_some_and(|line| line.starts_with("diff --git "))
    {
        return Some("Use a whole-file action for unsupported patch formats.");
    }
    for line in header.take_while(|line| !line.starts_with("@@ ")) {
        if let Some(index) = line.strip_prefix("index ") {
            if let Some((_, mode)) = index.split_once(' ') {
                if !matches!(mode, "100644" | "100755") {
                    return Some("Use a whole-file action for symlinks and submodules.");
                }
            }
        } else if !line.starts_with("--- ")
            && !line.starts_with("+++ ")
            && !matches!(line, "new file mode 100644" | "new file mode 100755")
        {
            return Some("Use a whole-file action for changes to file metadata.");
        }
    }
    None
}

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
    #[cfg(test)]
    pub(crate) observe_command: Option<TestCommandObserver>,
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
            #[cfg(test)]
            observe_command: None,
        };
        repo.bytes(&["rev-parse", "--show-toplevel"])?;
        Ok(repo)
    }

    fn command(&self, args: &[&str]) -> GitResult<Command> {
        let mut cmd = Command::new(crate::platform::git_executable()?);
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
        #[cfg(test)]
        {
            crate::test_repo::isolate_config(&mut cmd, &self.path);
            crate::test_repo::SUBPROCESSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        cmd.current_dir(&self.path)
            .args(["--no-pager", "--literal-pathspecs"])
            .args(args)
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
        capture(self.command(args)?, None, 0, Some(&scope))
            .map_err(|e| format!("Could not complete Git command: {e}"))
    }

    fn failed_output(output: &std::process::Output) -> String {
        let stderr = String::from_utf8_lossy(&output.stderr);
        format!("Git exited with {}. {}", output.status, stderr.trim())
            .trim_end()
            .to_owned()
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
    pub fn run(&self, args: &[&str]) -> GitResult<String> {
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

    fn head_id(&self, symbolic: &str) -> GitResult<String> {
        let output = self.output(&["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])?;
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

    /// Reads repository status, refs and history with a fresh Git query.
    ///
    /// The history is capped at `limit.max(1)` commits and ordered with each
    /// child before its parents.
    ///
    /// # Errors
    ///
    /// Returns an error if repository data cannot be read, or if HEAD, the index
    /// or refs change during the snapshot's consistency checks.
    pub fn snapshot(&self, limit: usize) -> GitResult<RepoSnapshot> {
        self.snapshot_cached(limit, HistoryRead::Full, &mut None)
            .map(|(snapshot, _, _)| snapshot)
    }

    fn read_refs(&self) -> GitResult<String> {
        self.run(&[
            "for-each-ref",
            "--format=%(objectname)%00%(refname:short)%00%(*objectname)%00%(refname)",
        ])
    }

    pub(crate) fn snapshot_cached(
        &self,
        limit: usize,
        mode: HistoryRead,
        cache: &mut Option<HistoryCache>,
    ) -> GitResult<(RepoSnapshot, crate::graph::Graph, bool)> {
        let limit = limit.max(1);
        let reviewed = self.write_identity()?;
        let metadata = self.read_metadata()?;
        let head = if let Some(branch) = reviewed.symbolic_head.strip_prefix("refs/heads/") {
            branch.to_string()
        } else {
            self.run(&["rev-parse", "--short", "HEAD"])?
                .trim()
                .to_string()
        };
        let refs = self.read_refs()?;
        let mut labels: HashMap<String, Vec<String>> = HashMap::new();
        let mut branches = Vec::new();
        let mut remote_refs = Vec::new();
        let mut tags = Vec::new();
        for line in refs.lines() {
            let fields: Vec<_> = line.split('\0').collect();
            if fields.len() == 4 {
                let target = if fields[2].is_empty() {
                    fields[0]
                } else {
                    fields[2]
                };
                if let Some(name) = fields[3].strip_prefix("refs/heads/") {
                    branches.push(LocalBranch {
                        name: name.into(),
                        tip: fields[0].into(),
                    });
                } else if let Some(list) = if fields[3].starts_with("refs/remotes/") {
                    Some(&mut remote_refs)
                } else if fields[3].starts_with("refs/tags/") {
                    Some(&mut tags)
                } else {
                    None
                } {
                    list.push(ReferenceTip {
                        full_name: fields[3].into(),
                        label: fields[1].into(),
                        tip: target.into(),
                    });
                }
                labels
                    .entry(target.into())
                    .or_default()
                    .push(fields[1].into());
            }
        }
        let head_exists: GitResult<String> = if reviewed.head.is_empty() {
            Err("Unborn HEAD".into())
        } else {
            Ok(reviewed.head.clone())
        };
        if let Ok(oid) = &head_exists {
            labels
                .entry(oid.trim().into())
                .or_default()
                .insert(0, "HEAD".into());
        }
        let previous = cache.as_ref().filter(|cached| {
            cached.refs == refs
                && cached.head == reviewed.head
                && cached.metadata == metadata.history_stamp
        });
        let reuse = previous.is_some_and(|cached| match mode {
            HistoryRead::Full => false,
            HistoryRead::Worktree => cached.limit == limit,
            HistoryRead::More => cached.limit <= limit && cached.commits.len() < cached.limit,
        });
        let append = previous
            .is_some_and(|cached| matches!(mode, HistoryRead::More) && cached.limit < limit);
        let skip = if append {
            previous.unwrap().commits.len()
        } else {
            0
        };
        let count = format!("--max-count={}", limit.saturating_sub(skip).max(1));
        let skip_arg = format!("--skip={skip}");
        // Date order retains child-before-parent safety while interleaving
        // active branches by commit date instead of grouping each lane.
        let mut log_args = vec![
            "log",
            "--all",
            "--date-order",
            "--no-decorate",
            "--no-show-signature",
            "--no-notes",
            "--no-use-mailmap",
            &count,
            &skip_arg,
            "-z",
            "--format=%H%x00%P%x00%s%x00%an%x00%aI",
        ];
        if head_exists.is_ok() {
            log_args.push("HEAD");
        }
        let log = if reuse {
            String::new()
        } else {
            self.run(&log_args)?
        };
        let fields: Vec<_> = log.split('\0').collect();
        let mut commits: Vec<_> = fields
            .as_chunks::<5>()
            .0
            .iter()
            .filter(|f| !f[0].is_empty())
            .map(|f| Commit {
                id: f[0].into(),
                parents: f[1].split_whitespace().map(String::from).collect(),
                subject: f[2].into(),
                author: f[3].into(),
                date: f[4].into(),
                refs: labels.remove(f[0]).unwrap_or_default(),
                search_key: String::new(),
            })
            .map(Commit::index_search)
            .collect();
        if reuse {
            commits = previous.unwrap().commits.clone();
        } else if append {
            let mut loaded = previous.unwrap().commits.clone();
            loaded.extend(commits);
            commits = loaded;
        }
        let status = parse_status(&self.bytes(&[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ])?);
        let head_id = head_exists
            .as_ref()
            .map(|oid| oid.trim().to_string())
            .unwrap_or_default();
        if self.write_identity()? != reviewed {
            return Err("Repository changed during refresh. Refresh again before writing.".into());
        }
        if self.read_refs()? != refs {
            return Err("References changed during refresh. Refresh again.".into());
        }
        let stashes = self.run(&["stash", "list", "--format=%gs%x00%ci"])?;
        let stash_count = stashes.lines().count();
        let latest_stash = stashes.lines().next().map(|line| {
            let (message, date) = line.split_once('\0').unwrap_or((line, ""));
            StashEntry {
                message: message.to_owned(),
                date: date.to_owned(),
            }
        });
        let remote = self.remote_state(&reviewed)?;
        let upstream_context = remote.context(&reviewed);
        let next = (!reuse).then(|| HistoryCache {
            refs,
            head: reviewed.head.clone(),
            limit,
            graph: crate::graph::layout_with_head(
                &commits,
                (!head_id.is_empty()).then_some(head_id.as_str()),
            ),
            commits: commits.clone(),
            metadata: metadata.history_stamp,
        });
        if let Some(scope) = &self.read_scope {
            scope.check().map_err(|error| error.to_string())?;
        }
        if let Some(next) = next {
            *cache = Some(next);
        }
        let history = cache.as_mut().expect("successful snapshot has history");
        history.limit = limit;
        Ok((
            RepoSnapshot {
                worktree_id: self.path.clone(),
                commits,
                branches,
                stash_count,
                latest_stash,
                remote_refs,
                tags,
                head,
                head_id,
                status,
                operations: metadata.operations,
                upstream_context,
                remote,
                reviewed: Some(reviewed),
                journal: Default::default(),
            },
            history.graph.clone(),
            !reuse,
        ))
    }

    /// Lists the distinct active operation names indicated by Git metadata.
    ///
    /// Git resolves marker paths for the current worktree. To check for
    /// unmerged files, inspect the repository status separately.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot resolve the paths or history metadata cannot be read.
    pub fn operation_markers(&self) -> GitResult<Vec<String>> {
        Ok(self.read_metadata()?.operations)
    }

    fn read_metadata(&self) -> GitResult<RepositoryMetadata> {
        use std::hash::{Hash, Hasher};
        let mut args = vec!["rev-parse", "--path-format=absolute"];
        for (marker, _) in OPERATION_MARKERS {
            args.extend(["--git-path", marker]);
        }
        for path in ["shallow", "info/grafts", "config", "config.worktree"] {
            args.extend(["--git-path", path]);
        }
        let paths = self.run(&args)?;
        let paths: Vec<_> = paths.lines().collect();
        if paths.len() != OPERATION_MARKERS.len() + 4 {
            return Err("Git metadata paths could not be represented.".into());
        }
        let mut active = paths
            .iter()
            .zip(OPERATION_MARKERS)
            .filter(|(p, _)| Path::new(p).exists())
            .map(|(_, (_, name))| name.to_string())
            .collect::<Vec<_>>();
        active.sort();
        active.dedup();
        let mut stamp = std::collections::hash_map::DefaultHasher::new();
        for path in &paths[OPERATION_MARKERS.len()..] {
            path.hash(&mut stamp);
            match std::fs::metadata(path) {
                Ok(metadata) => {
                    if metadata.len() > 1024 * 1024 {
                        return Err("Git history metadata exceeds 1 MiB.".into());
                    }
                    std::fs::read(path)
                        .map_err(|e| e.to_string())?
                        .hash(&mut stamp);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(RepositoryMetadata {
            operations: active,
            history_stamp: stamp.finish(),
        })
    }
    /// Describes the reviewed branch's upstream using local refs only.
    ///
    /// An unavailable upstream comparison produces a display message.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot read the upstream ref or returns malformed counts.
    pub fn upstream_context(&self, identity: &WriteIdentity) -> GitResult<String> {
        if identity.symbolic_head.is_empty() {
            return Ok("Detached HEAD · no upstream".into());
        }
        let upstream = self.run(&[
            "for-each-ref",
            "--format=%(upstream:short)",
            &identity.symbolic_head,
        ])?;
        let upstream = upstream.trim();
        if upstream.is_empty() {
            return Ok("No upstream configured".into());
        }
        match self.run(&[
            "rev-list",
            "--left-right",
            "--count",
            "HEAD...@{upstream}",
            "--",
        ]) {
            Ok(counts) => {
                let counts = counts.split_whitespace().collect::<Vec<_>>();
                if counts.len() != 2 {
                    return Err("Invalid upstream comparison".into());
                }
                Ok(format!(
                    "{} ahead · {} behind · {upstream} · network freshness unknown",
                    counts[0], counts[1]
                ))
            }
            Err(_) => Ok(format!(
                "Upstream {upstream} unavailable · network freshness unknown"
            )),
        }
    }
    /// Lists paths changed by `commit` relative to its first parent.
    /// A root commit is compared with the empty tree.
    ///
    /// # Errors
    ///
    /// Returns an error if the revision is invalid, Git fails, or the file list is malformed.
    pub fn changed_files(&self, commit: &str) -> GitResult<Vec<FileChange>> {
        validate_revision(commit)?;
        let parent = format!("{commit}^1");
        let data = if self.run(&["rev-parse", "--verify", &parent]).is_ok() {
            self.bytes(&[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--find-renames",
                "--name-status",
                "-z",
                &parent,
                commit,
                "--",
            ])?
        } else {
            self.bytes(&[
                "diff-tree",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--root",
                "--no-commit-id",
                "--find-renames",
                "--name-status",
                "-r",
                "-z",
                commit,
                "--",
            ])?
        };
        let mut fields = data.split(|b| *b == 0).filter(|s| !s.is_empty());
        let mut files = Vec::new();
        while let Some(status) = fields.next() {
            let kind = status[0] as char;
            let first = fields.next().ok_or("Missing changed-file path")?;
            let (original_path, path) = if matches!(kind, 'R' | 'C') {
                (
                    Some(first),
                    fields.next().ok_or("Missing rename destination")?,
                )
            } else {
                (None, first)
            };
            files.push(file_change(path, original_path, kind, ' '));
        }
        Ok(files)
    }

    /// Reads the author email and message body for the selected commit.
    pub(crate) fn commit_message_details(&self, commit: &str) -> GitResult<(String, String)> {
        validate_revision(commit)?;
        let metadata = self.run(&[
            "show",
            "-s",
            "--no-notes",
            "--no-show-signature",
            "--no-use-mailmap",
            "--format=%ae%x00%b",
            commit,
            "--",
        ])?;
        let (email, body) = metadata
            .split_once('\0')
            .ok_or("Git returned incomplete commit metadata")?;
        Ok((email.to_owned(), body.trim_end_matches('\n').to_owned()))
    }

    /// Finds the old path for a rename or copy in the requested comparison.
    ///
    /// `path` is the repository-relative destination. In working changes,
    /// `staged` selects the comparison that can contribute the old path.
    /// For example, a staged rename followed by an unstaged edit uses the
    /// new path on both sides of the unstaged comparison.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot read status or the commit's changed files.
    pub fn diff_identity(
        &self,
        commit: Option<&str>,
        path: &str,
        staged: bool,
    ) -> GitResult<Option<String>> {
        let files = if let Some(id) = commit {
            self.changed_files(id)?
        } else {
            parse_status(&self.bytes(&[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
            ])?)
        };
        Ok(files
            .into_iter()
            .find(|f| !f.unsupported_path && f.path == path)
            .and_then(|f| {
                if commit.is_some()
                    || (staged && matches!(f.index, 'R' | 'C'))
                    || (!staged && matches!(f.worktree, 'R' | 'C'))
                {
                    f.original_path
                } else {
                    None
                }
            }))
    }

    /// Returns the raw Git patch for a file comparison.
    ///
    /// See [`Self::compare`] for comparison rules and error conditions.
    pub fn diff(&self, commit: Option<&str>, path: &str, staged: bool) -> GitResult<String> {
        Ok(self.compare(commit, path, staged)?.patch)
    }

    /// Returns a Git patch and the old path if this comparison detects a rename or copy.
    ///
    /// Supply the repository-relative destination as `path`. A selected commit
    /// is compared with its first parent, or the empty tree for a root commit.
    /// With no commit, `staged` selects HEAD versus index when `true` and index
    /// versus working tree otherwise. Untracked files are compared against an
    /// empty file if they resolve inside the worktree.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot produce a UTF-8 patch or an untracked path
    /// resolves outside the worktree.
    pub fn compare(
        &self,
        commit: Option<&str>,
        path: &str,
        staged: bool,
    ) -> GitResult<DiffComparison> {
        let original = self.diff_identity(commit, path, staged)?;
        let patch = self.patch_for_paths(commit, path, staged, original.as_deref())?;
        Ok(DiffComparison {
            patch,
            original_path: original,
        })
    }

    fn patch_for_paths(
        &self,
        commit: Option<&str>,
        path: &str,
        staged: bool,
        original: Option<&str>,
    ) -> GitResult<String> {
        let mut args = patch_args("diff");
        args.push("--find-renames");
        let parent;
        if let Some(id) = commit {
            validate_revision(id)?;
            parent = format!("{id}^1");
            if self.run(&["rev-parse", "--verify", &parent]).is_ok() {
                args.extend([&parent, id]);
            } else {
                let mut args = patch_args("show");
                args.extend(["--format=", "--no-notes", id, "--", path]);
                return self.run(&args);
            }
        } else if staged {
            args.push("--cached");
        }
        args.push("--");
        if let Some(old) = original {
            args.push(old);
        }
        args.push(path);
        let diff = self.run(&args)?;
        if diff.is_empty()
            && commit.is_none()
            && !staged
            && self
                .run(&["ls-files", "--error-unmatch", "--", path])
                .is_err()
        {
            let candidate = self.path.join(path);
            if !candidate.is_file() {
                return Ok(diff);
            }
            let canonical = candidate.canonicalize().map_err(|e| e.to_string())?;
            let root = self.path.canonicalize().map_err(|e| e.to_string())?;
            if !canonical.starts_with(root) {
                return Err("Untracked preview path leaves the repository".into());
            }
            let mut args = patch_args("diff");
            args.extend(["--no-index", "--", "/dev/null", path]);
            let scope = self.read_scope.clone().unwrap_or_default();
            let output =
                capture(self.command(&args)?, None, 1, Some(&scope)).map_err(|e| e.to_string())?;
            // For --no-index, exit 1 means a successful comparison with differences.
            if output.status.success() || output.status.code() == Some(1) {
                return String::from_utf8(output.stdout)
                    .map_err(|_| "Untracked diff is not UTF-8".into());
            }
            return Err(String::from_utf8_lossy(&output.stderr).trim().into());
        }
        Ok(diff)
    }

    fn status(&self) -> GitResult<Vec<FileChange>> {
        Ok(parse_status(&self.bytes(&[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ])?))
    }

    /// Stages or unstages a file after checking its reviewed status entry.
    ///
    /// A rename updates both its source and destination. A directory can be
    /// selected only when it represents a single indexed submodule entry.
    ///
    /// # Errors
    ///
    /// Returns an error if the entry changed or is unresolved, its path cannot
    /// be addressed, or Git cannot update the index.
    pub fn stage_reviewed(&self, reviewed: &FileChange, unstage: bool) -> Result<(), WriteError> {
        if reviewed.is_unmerged() {
            return Err(Self::unmerged_write_error());
        }
        if let Some(issue) = reviewed.path_issue() {
            return Err(issue.into());
        }
        if !reviewed.on_side(unstage) || !self.status()?.contains(reviewed) {
            return Err("File status changed since review. Refresh and review it again.".into());
        }
        let mut paths = vec![reviewed.path.as_str()];
        if (unstage && reviewed.index == 'R') || (!unstage && reviewed.worktree == 'R') {
            paths.push(
                reviewed
                    .original_path
                    .as_deref()
                    .ok_or("Rename source is missing.")?,
            );
        }
        for path in &paths {
            if path.ends_with('/') || self.path.join(path).is_dir() {
                // An indexed gitlink is a single entry even though it has a worktree directory.
                let entries = self.run(&["ls-files", "--stage", "--", path])?;
                let is_gitlink = entries.lines().count() == 1 && entries.starts_with("160000 ");
                if !is_gitlink {
                    return Err(
                        "The reviewed path now names a directory. Refresh and review its files."
                            .into(),
                    );
                }
            }
        }
        self.stage_paths(&paths, unstage)
    }

    /// Stages or unstages all changes across the worktree when Git runs.
    ///
    /// Unstaging preserves working files, including in a repository with no
    /// commits. Unmerged paths keep their conflict entries. Git's own `add` and
    /// `restore` would resolve them even when excluded by a pathspec, so with
    /// conflicts present the other changed paths are read when this runs and
    /// passed to Git explicitly.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot determine HEAD or update the index. An
    /// [`FailureKind::Inspect`] error means the conflict entries changed
    /// while Git ran.
    pub fn stage_all(&self, unstage: bool) -> Result<BulkStage, WriteError> {
        let before = self.status()?;
        let conflicts = self.unmerged_entries()?;
        if conflicts.is_empty() {
            self.stage_paths(&["."], unstage)?;
        } else {
            let mut paths = std::collections::BTreeSet::new();
            for file in before
                .iter()
                .filter(|file| file.on_side(unstage) && !file.unsupported_path)
            {
                paths.insert(file.path.as_str());
                if unstage && file.index == 'R' {
                    paths.extend(file.original_path.as_deref());
                }
            }
            if !paths.is_empty() {
                let mut spec = Vec::new();
                for path in paths {
                    spec.extend_from_slice(path.as_bytes());
                    spec.push(0);
                }
                let args = self.stage_command(unstage, true)?;
                let result = capture(self.command(&args)?, Some(spec.as_slice()), 0, None)
                    .map_err(WriteError::inspect)?;
                if !result.status.success() {
                    return Err(WriteError::git(Self::failed_output(&result)));
                }
            }
            if self.unmerged_entries().map_err(WriteError::inspect)? != conflicts {
                return Err(WriteError::inspect(
                    "Conflict entries changed during the bulk index update. Refresh and review the conflicts.",
                ));
            }
        }
        let after = self.status().map_err(WriteError::inspect)?;
        let changed = before
            .iter()
            .chain(&after)
            .filter(|file| !before.contains(file) || !after.contains(file))
            .map(|file| file.path.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        Ok(BulkStage {
            changed,
            conflicts: after.iter().filter(|file| file.is_unmerged()).count(),
        })
    }

    /// Reads raw unmerged index entries from `git ls-files --unmerged`.
    fn unmerged_entries(&self) -> GitResult<Vec<u8>> {
        self.bytes(&["ls-files", "--unmerged", "-z"])
    }

    fn stage_command(&self, unstage: bool, from_stdin: bool) -> GitResult<Vec<&'static str>> {
        let mut args = if !unstage {
            vec!["add", "-A"]
        } else if !self.head_id(&self.symbolic_head()?)?.is_empty() {
            vec!["restore", "--staged"]
        } else {
            // An unborn index has no HEAD to restore. --cached guarantees that
            // even a partially staged file keeps its working-tree contents.
            vec!["rm", "--cached", "-r", "--force", "--quiet"]
        };
        if from_stdin {
            args.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
        } else {
            args.push("--");
        }
        Ok(args)
    }

    fn stage_paths(&self, paths: &[&str], unstage: bool) -> Result<(), WriteError> {
        let mut args: Vec<&str> = self.stage_command(unstage, false)?;
        args.extend_from_slice(paths);
        self.reject_unmerged_paths(paths)?;
        self.write(&args).map(|_| ())
    }

    /// Marks an unmerged path resolved by staging its working-tree version.
    ///
    /// A missing working file stages the deletion. Unless `allow_markers` is
    /// set, a regular file that still contains a conflict marker line is
    /// refused with [`FailureKind::ConflictMarkers`].
    ///
    /// # Errors
    ///
    /// Returns an error if the path is invalid, no longer unmerged, names a
    /// directory, still has markers, or Git cannot update the index.
    pub fn mark_resolved(&self, path: &str, allow_markers: bool) -> Result<(), WriteError> {
        if path.is_empty()
            || path == "."
            || Path::new(path).is_absolute()
            || path.split(['/', '\\']).any(|s| s == "..")
        {
            return Err("Invalid conflict path".into());
        }
        if !self.has_unmerged_paths(&[path])? {
            return Err(
                "This path no longer has unresolved conflicts. Refresh and review it again.".into(),
            );
        }
        let file = self.path.join(path);
        match std::fs::symlink_metadata(&file) {
            Ok(metadata) if metadata.is_dir() => {
                return Err(
                    "The conflicted path now names a directory. Resolve it with Git.".into(),
                )
            }
            Ok(metadata) if metadata.is_file() && !allow_markers => {
                let content = std::fs::read(&file).map_err(|e| e.to_string())?;
                if let Some(line) = conflict_marker_line(&content) {
                    return Err(WriteError {
                        kind: FailureKind::ConflictMarkers,
                        message: format!(
                            "{} still contains conflict markers (<<<<<<<, =======, >>>>>>>) at line {line}. Edit the file to resolve them, or confirm that the markers are intended.",
                            display_path(path)
                        ),
                    });
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string().into()),
        }
        self.write(&["add", "-A", "--", path])?;
        if self
            .has_unmerged_paths(&[path])
            .map_err(WriteError::inspect)?
        {
            return Err(WriteError::inspect(
                "Git kept unresolved entries for this path. Refresh and inspect it.",
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn stage(&self, path: &str) -> GitResult<()> {
        self.stage_paths(&[path], false)
            .map_err(|error| error.message)
    }

    #[cfg(test)]
    pub(crate) fn unstage(&self, path: &str) -> GitResult<()> {
        if path == "." {
            return self
                .stage_all(true)
                .map(|_| ())
                .map_err(|error| error.message);
        }
        let file = self
            .status()?
            .into_iter()
            .find(|file| file.path == path && file.on_side(true))
            .ok_or("Expected a staged file")?;
        self.stage_reviewed(&file, true)
            .map_err(|error| error.message)
    }

    /// Reads a working changes patch and splits it into individual hunks.
    ///
    /// See [`Self::compare`] for how `path` and `staged` select the comparison.
    /// Check [`Hunk::blocked_reason`] before using a hunk for a write.
    pub fn hunks(&self, path: &str, staged: bool) -> GitResult<Vec<Hunk>> {
        Ok(split_hunks(&self.diff(None, path, staged)?))
    }

    /// Applies a hunk to the index, reversing it when `unstage` is `true`.
    ///
    /// The caller should ensure the hunk is from the current comparison.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported patch headers or if Git cannot apply the patch.
    pub fn apply_hunk(&self, hunk: &Hunk, unstage: bool) -> Result<(), WriteError> {
        if let Some(reason) = hunk.blocked_reason() {
            return Err(reason.into());
        }
        let mut args = vec!["apply", "--cached", "-p1", "--whitespace=nowarn"];
        if unstage {
            args.push("--reverse");
        }
        self.apply_patch(&args, hunk)
    }

    fn apply_patch(&self, args: &[&str], hunk: &Hunk) -> Result<(), WriteError> {
        let result = capture(self.command(args)?, Some(hunk.patch.as_bytes()), 0, None)
            .map_err(WriteError::inspect)?;
        if result.status.success() {
            Ok(())
        } else {
            Err(WriteError::git(Self::failed_output(&result)))
        }
    }

    /// Reverses a hunk in a working file and verifies the resulting bytes.
    ///
    /// Use a repository-relative `path` and a hunk from its current unstaged
    /// patch. Only ordinary UTF-8 files with LF endings and no content filters
    /// are supported. Verification also checks that HEAD and the index stayed
    /// the same. External processes can still change the file during the write.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsupported path or patch, a changed working
    /// file, or a Git failure. [`FailureKind::Inspect`] means the caller should
    /// refresh and inspect the resulting state before retrying.
    pub fn discard_hunk(&self, path: &str, hunk: &Hunk) -> Result<(), WriteError> {
        if let Some(reason) = hunk.blocked_reason() {
            return Err(reason.into());
        }
        if path.is_empty()
            || path == "."
            || PathBuf::from(path).is_absolute()
            || path.split(['/', '\\']).any(|s| s == "..")
        {
            return Err("Invalid discard path".into());
        }
        if self
            .run(&["config", "--get", "core.autocrlf"])
            .is_ok_and(|s| s.trim().eq_ignore_ascii_case("true"))
        {
            return Err(
                "Discard with automatic CRLF conversion is not qualified. Use Git for this file."
                    .into(),
            );
        }
        let attrs = self.run(&[
            "check-attr",
            "-z",
            "filter",
            "working-tree-encoding",
            "eol",
            "--",
            path,
        ])?;
        for fields in attrs.split('\0').collect::<Vec<_>>().as_chunks::<3>().0 {
            if fields[2] != "unspecified"
                && fields[2] != "unset"
                && !(fields[1] == "eol" && fields[2] == "lf")
            {
                return Err("Discard with content filters or non-LF encoding is not qualified. Use Git for this file.".into());
            }
        }
        let file = self.path.join(path);
        if std::fs::symlink_metadata(&file)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("Discard of symlinks is not supported".into());
        }
        let before = std::fs::read(&file).map_err(|e| e.to_string())?;
        let expected = crate::hunk_edit::reversed_worktree(hunk, &before)?;
        let identity = self.write_identity()?;
        if std::fs::read(&file).map_err(|e| e.to_string())? != before {
            return Err("Working file changed before discard".into());
        }
        self.reject_unmerged_paths(&[path])?;
        self.apply_patch(&["apply", "--reverse", "-p1", "--whitespace=nowarn"], hunk)?;
        if std::fs::read(&file).map_err(WriteError::inspect)? != expected
            || self.write_identity().map_err(WriteError::inspect)? != identity
        {
            return Err(WriteError::inspect(
                "Discard outcome uncertain. Git state differs from the reviewed effect.",
            ));
        }
        Ok(())
    }
    /// Reads HEAD and the index entries for use in subsequent write review.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot read HEAD or the index.
    pub fn write_identity(&self) -> GitResult<WriteIdentity> {
        std::thread::scope(|scope| {
            let head = scope.spawn(|| {
                let symbolic = self.symbolic_head()?;
                Ok::<_, String>((self.head_id(&symbolic)?, symbolic))
            });
            let index = self.bytes(&["ls-files", "--stage", "-z"])?;
            let (head, symbolic_head) = head.join().map_err(|_| "HEAD read failed")??;
            Ok(WriteIdentity {
                head,
                symbolic_head,
                index,
            })
        })
    }
    /// Commits the reviewed staged changes and returns the verified commit object ID.
    ///
    /// Checks the reviewed HEAD and index before committing, then verifies the
    /// resulting parent, tree and symbolic HEAD. Normal Git commit hooks can run.
    ///
    /// # Errors
    ///
    /// Rejects a missing summary, active Git operation, stale review or empty
    /// staged diff. Git failures and uncertain outcomes require inspecting the
    /// repository before retrying.
    pub fn commit_reviewed(
        &self,
        message: &str,
        reviewed: &WriteIdentity,
    ) -> Result<String, WriteError> {
        if message.lines().next().is_none_or(|s| s.trim().is_empty()) {
            return Err("Enter a commit summary".into());
        }
        if !self.operation_markers()?.is_empty() {
            return Err("An active Git operation blocks ordinary commit. Complete or abort it in a terminal, then refresh.".into());
        }
        if self.write_identity()? != *reviewed {
            return Err("HEAD or index changed since review. Refresh and review staged changes. Your draft is kept in this tab.".into());
        }
        if self
            .run(&["diff", "--cached", "--name-only", "-z"])?
            .is_empty()
        {
            return Err("No staged changes to commit".into());
        }
        let tree = self.write(&["write-tree"])?;
        if self.write_identity()? != *reviewed {
            return Err("Index changed before commit. Refresh and review again.".into());
        }
        self.commit(message)?;
        let sha = self.resolve_commit("HEAD").map_err(WriteError::inspect)?;
        let state = self
            .run(&[
                "show",
                "-s",
                "--no-show-signature",
                "--no-notes",
                "--format=%P%x00%T",
                &sha,
            ])
            .map_err(WriteError::inspect)?;
        let (parents, actual_tree) = state
            .trim()
            .split_once('\0')
            .ok_or_else(|| WriteError::inspect("Could not verify the commit parent and tree."))?;
        if parents != reviewed.head
            || actual_tree != tree.trim()
            || self.symbolic_head().map_err(WriteError::inspect)? != reviewed.symbolic_head
            || self.resolve_commit("HEAD").map_err(WriteError::inspect)? != sha
        {
            return Err(WriteError::inspect("Commit outcome uncertain. The resulting parent, tree, or branch differs from the reviewed commit. Your draft is kept in this tab."));
        }
        Ok(sha)
    }
    /// Runs `git commit` with the supplied message and currently staged changes.
    ///
    /// Use [`Self::commit_reviewed`] when the user confirmed a specific index state.
    ///
    /// # Errors
    ///
    /// Returns an error for a blank message or a Git commit failure.
    pub fn commit(&self, message: &str) -> Result<(), WriteError> {
        if message.trim().is_empty() {
            return Err("Enter a commit message".into());
        }
        self.write(&["commit", "-m", message]).map(|_| ())
    }
    /// Creates a local branch at HEAD without switching to it.
    ///
    /// # Errors
    ///
    /// Returns an error if the branch name is invalid or Git cannot create the branch.
    pub fn create_branch(&self, name: &str) -> Result<(), WriteError> {
        validate_revision(name)?;
        self.run(&["check-ref-format", "--branch", name])?;
        self.write(&["branch", "--", name]).map(|_| ())
    }
    /// Creates a local branch at `start` without switching to it.
    ///
    /// # Errors
    ///
    /// Returns an error if the branch name or revision is invalid or Git cannot create the branch.
    pub fn create_branch_at(&self, name: &str, start: &str) -> Result<(), WriteError> {
        validate_revision(name)?;
        validate_revision(start)?;
        self.run(&["check-ref-format", "--branch", name])?;
        self.write(&["branch", "--", name, start]).map(|_| ())
    }
    /// Switches to a named local branch or checks out the revision with detached HEAD.
    ///
    /// # Errors
    ///
    /// Returns an error if the target is invalid or Git cannot switch to it.
    pub fn checkout(&self, branch: &str) -> Result<(), WriteError> {
        validate_revision(branch)?;
        let local_ref = format!("refs/heads/{branch}");
        if self
            .run(&["show-ref", "--verify", "--quiet", &local_ref])
            .is_ok()
        {
            self.write(&["switch", branch]).map(|_| ())
        } else {
            let commit_ref = format!("{branch}^{{commit}}");
            self.run(&["rev-parse", "--verify", &commit_ref])?;
            self.write(&["switch", "--detach", branch]).map(|_| ())
        }
    }
    /// Merges the target into the current checkout using Git's default message.
    /// A Git failure may leave an in-progress merge or conflicts.
    pub fn merge(&self, target: &str) -> Result<(), WriteError> {
        validate_revision(target)?;
        self.write(&["merge", "--no-edit", "--", target])
            .map(|_| ())
    }
    /// Cherry-picks the target onto the current checkout.
    /// A Git failure may leave an in-progress operation or conflicts.
    pub fn cherry_pick(&self, target: &str) -> Result<(), WriteError> {
        validate_revision(target)?;
        self.write(&["cherry-pick", target]).map(|_| ())
    }
    /// Reverts the target using Git's default commit message.
    /// A Git failure may leave an in-progress operation or conflicts.
    pub fn revert(&self, target: &str) -> Result<(), WriteError> {
        validate_revision(target)?;
        self.write(&["revert", "--no-edit", target]).map(|_| ())
    }
    /// Stashes tracked changes with the default GitCrab message.
    pub fn stash(&self) -> Result<(), WriteError> {
        self.stash_with_message("GitCrab stash")
    }

    /// Stashes tracked changes with `message`. Untracked files stay in the worktree.
    pub fn stash_with_message(&self, message: &str) -> Result<(), WriteError> {
        self.write(&["stash", "push", "-m", message]).map(|_| ())
    }
    /// Applies the latest stash and drops it if Git applies it successfully.
    pub fn stash_pop(&self) -> Result<(), WriteError> {
        self.write(&["stash", "pop"]).map(|_| ())
    }

    /// Drops the newest stash entry whose commit is `id`.
    ///
    /// Stash entries are addressed by position, so the position is looked up
    /// and confirmed immediately before the drop. If no entry holds `id` any
    /// more, nothing is dropped and the call succeeds.
    pub fn drop_stash_object(&self, id: &str) -> Result<(), WriteError> {
        let list = self
            .run(&["stash", "list", "--format=%H"])
            .map_err(WriteError::inspect)?;
        let Some(index) = list.lines().position(|line| line.trim() == id) else {
            return Ok(());
        };
        let entry = format!("stash@{{{index}}}");
        let current = self
            .run(&["rev-parse", "--verify", "--quiet", &entry])
            .map_err(WriteError::inspect)?;
        if current.trim() != id {
            return Err(WriteError::inspect(
                "The stash list changed while dropping the applied stash. The changes were applied; check the stash list before dropping it manually.",
            ));
        }
        self.write(&["stash", "drop", "--quiet", &entry])
            .map(|_| ())
    }
    /// Rebases the current branch onto `target`.
    /// A Git failure may leave an in-progress rebase or conflicts.
    pub fn rebase(&self, target: &str) -> Result<(), WriteError> {
        validate_revision(target)?;
        self.write(&["rebase", target]).map(|_| ())
    }
    /// Resets HEAD to `target` while preserving working-tree contents.
    /// With `soft`, the index stays as it is. A mixed reset updates the index
    /// to match `target`.
    pub fn reset(&self, target: &str, soft: bool) -> Result<(), WriteError> {
        validate_revision(target)?;
        self.write(&["reset", if soft { "--soft" } else { "--mixed" }, target])
            .map(|_| ())
    }

    // Undo journal support (`crate::journal`). Reads report what the journal's
    // preconditions need; writes are the reversing commands it reviews.

    /// Full symbolic ref of HEAD (empty when detached) and its commit ID
    /// (empty for an unborn branch).
    pub(crate) fn head_state(&self) -> GitResult<(String, String)> {
        let symbolic = self.symbolic_head()?;
        let id = self.head_id(&symbolic)?;
        Ok((symbolic, id))
    }

    /// The commit a full ref points to, or `None` when the ref does not exist.
    pub(crate) fn ref_commit(&self, full_ref: &str) -> GitResult<Option<String>> {
        validate_revision(full_ref)?;
        let commit = format!("{full_ref}^{{commit}}");
        let output = self.output(&[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &commit,
        ])?;
        match output.status.code() {
            Some(0) => String::from_utf8(output.stdout)
                .map(|s| Some(s.trim().to_owned()))
                .map_err(|e| e.to_string()),
            Some(1) if output.stderr.is_empty() => Ok(None),
            _ => Err(Self::failed_output(&output)),
        }
    }

    /// Parent IDs of commit `id`, or `None` when no such commit exists.
    pub(crate) fn commit_parents(&self, id: &str) -> GitResult<Option<Vec<String>>> {
        let Some(id) = self.ref_commit(id)? else {
            return Ok(None);
        };
        let parents = self.run(&["show", "-s", "--no-notes", "--format=%P", &id])?;
        Ok(Some(
            parents.split_whitespace().map(str::to_owned).collect(),
        ))
    }

    /// Stash entries as object ID and reflog subject, newest first.
    pub(crate) fn stash_entries(&self) -> GitResult<Vec<(String, String)>> {
        Ok(self
            .run(&["stash", "list", "--format=%H%x00%gs"])?
            .lines()
            .filter_map(|line| {
                let (id, subject) = line.split_once('\0')?;
                Some((id.trim().to_owned(), subject.to_owned()))
            })
            .collect())
    }

    /// Returns tracked files in the index and working tree to HEAD with
    /// `reset --hard`. Untracked files stay. Callers verify beforehand that
    /// every discarded change is saved elsewhere.
    pub(crate) fn reset_tracked_to_head(&self) -> Result<(), WriteError> {
        self.write(&["reset", "--hard", "--quiet", "HEAD"])
            .map(|_| ())
    }

    /// Whether tracked files differ from HEAD in the index or working tree.
    /// Untracked files are not counted.
    pub(crate) fn has_tracked_changes(&self) -> GitResult<bool> {
        Ok(!self
            .bytes(&["status", "--porcelain=v1", "-z", "--untracked-files=no"])?
            .is_empty())
    }

    /// Whether the working tree and index hold exactly the tracked state that
    /// the stash-shaped commit `state` records: its tree for the working tree
    /// and its second parent's tree for the index.
    pub(crate) fn tracked_state_matches(&self, state: &str) -> GitResult<bool> {
        validate_revision(state)?;
        let index = format!("{state}^2");
        for args in [
            &[
                "diff",
                "--quiet",
                "--no-ext-diff",
                "--no-textconv",
                state,
                "--",
            ][..],
            &[
                "diff",
                "--quiet",
                "--cached",
                "--no-ext-diff",
                "--no-textconv",
                &index,
                "--",
            ],
        ] {
            let output = self.output(args)?;
            match output.status.code() {
                Some(0) => {}
                Some(1) => return Ok(false),
                _ => return Err(Self::failed_output(&output)),
            }
        }
        Ok(true)
    }

    /// Records the tracked index and working tree as a dangling stash-shaped
    /// commit without changing refs or files. `None` when nothing differs from HEAD.
    pub(crate) fn capture_tracked_state(&self) -> Result<Option<String>, WriteError> {
        let id = self.write(&["stash", "create"])?;
        Ok(Some(id.trim().to_owned()).filter(|id| !id.is_empty()))
    }

    /// Applies stash commit `id`, restoring its index state too when `index` is set.
    pub(crate) fn stash_apply(&self, id: &str, index: bool) -> Result<(), WriteError> {
        validate_revision(id)?;
        let mut args = vec!["stash", "apply"];
        if index {
            args.push("--index");
        }
        args.push(id);
        self.write(&args).map(|_| ())
    }

    /// Adds stash commit `id` back to the top of the stash list.
    pub(crate) fn stash_store(&self, message: &str, id: &str) -> Result<(), WriteError> {
        validate_revision(id)?;
        self.write(&["stash", "store", "-m", message, id])
            .map(|_| ())
    }

    /// Moves HEAD (and its branch) to `id` with `reset --keep`, which refuses
    /// to overwrite local changes or untracked files.
    pub(crate) fn reset_keep(&self, id: &str) -> Result<(), WriteError> {
        validate_revision(id)?;
        self.write(&["reset", "--keep", id]).map(|_| ())
    }

    /// Deletes local branch `name`. Git refuses a branch checked out in any worktree.
    pub(crate) fn delete_branch(&self, name: &str) -> Result<(), WriteError> {
        validate_revision(name)?;
        self.write(&["branch", "-D", "--", name]).map(|_| ())
    }
}

fn validate_revision(value: &str) -> GitResult<()> {
    if value.is_empty() || value.starts_with('-') || value.contains('\0') {
        Err("Invalid branch or revision".into())
    } else {
        Ok(())
    }
}

// Network commands: fetch, pull and push.
//
// They run under a cancellable `ReadScope::network` with no time budget, so a
// slow remote or a credential helper's sign-in window never hits the read
// deadline. `GIT_TERMINAL_PROMPT=0` (set for every command) keeps Git from
// waiting on a hidden terminal; credential helpers and SSH agents still work.

/// How Pull integrates the upstream branch into the checked-out branch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullMode {
    /// Fast-forward when possible, otherwise create a merge commit.
    #[default]
    FastForwardIfPossible,
    /// Fast-forward only. A diverged branch is left unchanged.
    FastForwardOnly,
    /// Rebase local commits onto the upstream.
    Rebase,
}

impl PullMode {
    /// Modes in menu order. The first is the toolbar default.
    pub const ALL: [Self; 3] = [
        Self::FastForwardIfPossible,
        Self::FastForwardOnly,
        Self::Rebase,
    ];

    /// Menu label.
    pub fn label(self) -> &'static str {
        match self {
            Self::FastForwardIfPossible => "Pull (fast-forward if possible)",
            Self::FastForwardOnly => "Pull (fast-forward only)",
            Self::Rebase => "Pull (rebase)",
        }
    }

    /// How the dialog describes the integration.
    pub fn explain(self) -> &'static str {
        match self {
            Self::FastForwardIfPossible => {
                "fast-forward if possible, otherwise create a merge commit"
            }
            Self::FastForwardOnly => "fast-forward only; if the branches diverged nothing changes",
            Self::Rebase => "rebase local commits onto it",
        }
    }

    fn args(self) -> &'static [&'static str] {
        match self {
            Self::FastForwardIfPossible => &["--no-rebase", "--ff", "--no-edit"],
            Self::FastForwardOnly => &["--no-rebase", "--ff-only"],
            Self::Rebase => &["--rebase"],
        }
    }
}

/// The checked-out branch's upstream, read from local configuration and refs.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Upstream {
    /// Short name such as `origin/main`.
    pub name: String,
    /// Configured remote, or `.` for a local branch.
    pub remote: String,
    /// Branch ref on the remote, such as `refs/heads/main`.
    pub remote_ref: String,
    /// Local remote-tracking ref, such as `refs/remotes/origin/main`.
    pub tracking: String,
    pub ahead: usize,
    pub behind: usize,
    /// The tracking ref is missing: never fetched, or pruned after the remote deleted it.
    pub gone: bool,
}

/// Remotes and upstream tracking, from local configuration and refs only.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteState {
    /// Configured remote names.
    pub remotes: Vec<String>,
    pub upstream: Option<Upstream>,
    /// When this worktree's `FETCH_HEAD` was last written, in seconds since
    /// the Unix epoch. Fetches and pulls from any client update it.
    pub last_fetch: Option<u64>,
}

impl RemoteState {
    /// An upstream on a remote, as Pull and Push use it. A branch that tracks
    /// another local branch has none.
    pub fn network_upstream(&self) -> Option<&Upstream> {
        self.upstream
            .as_ref()
            .filter(|upstream| upstream.remote != ".")
    }

    /// The remote Push sets as upstream when the branch has none: the only
    /// remote, otherwise `origin`.
    pub fn default_push_remote(&self) -> Option<&str> {
        match self.remotes.as_slice() {
            [only] => Some(only),
            remotes => remotes
                .iter()
                .find(|remote| *remote == "origin")
                .map(String::as_str),
        }
    }

    /// Status text for the upstream relationship.
    pub fn context(&self, identity: &WriteIdentity) -> String {
        if identity.symbolic_head.is_empty() {
            return "Detached HEAD · no upstream".into();
        }
        match &self.upstream {
            None => "No upstream configured".into(),
            Some(upstream) if upstream.gone => format!("Upstream {} unavailable", upstream.name),
            Some(upstream) => format!(
                "{} ahead · {} behind · {}",
                upstream.ahead, upstream.behind, upstream.name
            ),
        }
    }
}

/// The local branch, remote and refs reviewed for Pull or Push.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkTarget {
    /// Checked-out local branch name.
    pub branch: String,
    pub remote: String,
    /// Branch ref on the remote, such as `refs/heads/main`.
    pub remote_ref: String,
    /// Local remote-tracking ref, such as `refs/remotes/origin/main`.
    pub tracking: String,
    /// Push sets this remote branch as the upstream (`git push -u`).
    pub set_upstream: bool,
    /// Local commits the remote branch does not have, as last fetched.
    pub ahead: usize,
    /// Remote commits the local branch does not have, as last fetched.
    pub behind: usize,
}

impl NetworkTarget {
    /// The remote branch as people name it, such as `origin/main`.
    pub fn remote_branch(&self) -> String {
        format!(
            "{}/{}",
            self.remote,
            self.remote_ref
                .strip_prefix("refs/heads/")
                .unwrap_or(&self.remote_ref)
        )
    }
}

/// HEAD before and after a successful pull.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullOutcome {
    pub before: String,
    pub after: String,
    pub message: String,
}

fn count_label(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// Maps a failed network command's diagnostics to a plain explanation.
fn network_explanation(stderr: &str, target: Option<&NetworkTarget>) -> Option<String> {
    let text = stderr.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));
    let remote = target.map_or("the remote".to_owned(), |t| t.remote.clone());
    let remote_branch = target.map_or("the remote branch".to_owned(), |t| t.remote_branch());
    let branch = target.map_or("the branch".to_owned(), |t| t.branch.clone());
    Some(
        if has(&["[rejected]"]) && has(&["non-fast-forward", "fetch first"]) {
            format!("Push rejected: {remote_branch} has commits that {branch} does not have. Pull first, then push again. GitCrab never force-pushes.")
        } else if has(&["[remote rejected]"]) {
            format!("{remote} refused the push, for example because the branch is protected or a server hook declined it.")
        } else if has(&["not possible to fast-forward"]) {
            format!("{branch} and {remote_branch} have diverged, so a fast-forward is not possible. Nothing was changed. Choose Pull (fast-forward if possible) to merge, or Pull (rebase).")
        } else if has(&["permission denied (publickey"]) {
            format!("{remote} refused the SSH key (permission denied, publickey). Add your key to the SSH agent or check your access to the repository, then retry.")
        } else if has(&["host key verification failed"]) {
            "The SSH host key could not be verified. Connect once from a terminal to review and accept the host key, then retry.".into()
        } else if has(&[
            "terminal prompts disabled",
            "could not read username",
            "could not read password",
            "authentication failed",
            "invalid username or password",
            "http basic: access denied",
            "returned error: 401",
            "returned error: 403",
        ]) {
            format!("Authentication failed for {remote}. GitCrab does not ask for passwords itself: sign in through your Git credential helper (for example Git Credential Manager) or load your SSH key into the agent, then retry.")
        } else if has(&[
            "could not resolve host",
            "could not resolve hostname",
            "name or service not known",
            "no such host is known",
            "temporary failure in name resolution",
        ]) {
            format!("Could not resolve the host for {remote}. Check your network connection and the remote URL.")
        } else if has(&[
            "connection refused",
            "connection timed out",
            "operation timed out",
            "failed to connect",
            "network is unreachable",
            "could not connect",
        ]) {
            format!("Could not connect to {remote}. Check your network connection and try again.")
        } else if has(&[
            "repository not found",
            "does not appear to be a git repository",
        ]) {
            format!("{remote} was not found, or you do not have access to it. Check the remote URL and your permissions.")
        } else if has(&["couldn't find remote ref"]) {
            format!("{remote_branch} no longer exists on {remote}. Fetch to prune it, then choose another upstream from a terminal.")
        } else if has(&["no tracking information", "has no upstream branch"]) {
            format!("{branch} has no upstream branch.")
        } else if has(&[
            "would be overwritten by",
            "cannot pull with rebase",
            "please commit or stash them",
            "you have unstaged changes",
            "your index contains uncommitted changes",
        ]) {
            "Local changes would be overwritten. Commit or stash them, then pull again. Nothing was changed.".into()
        } else {
            return None;
        },
    )
}

/// Git's diagnostics without its advice lines.
fn network_diagnostics(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .filter(|line| !line.starts_with("hint:") && !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

impl Git {
    /// Reads remotes, the upstream of `identity`'s branch and the last fetch
    /// time from local configuration and refs. Nothing contacts a remote.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot list remotes or read the branch.
    pub fn remote_state(&self, identity: &WriteIdentity) -> GitResult<RemoteState> {
        let remotes = self
            .run(&["remote"])?
            .lines()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect();
        let upstream = if identity.symbolic_head.starts_with("refs/heads/") {
            let line = self.run(&[
                "for-each-ref",
                "--format=%(refname)%00%(upstream)%00%(upstream:short)%00%(upstream:remotename)%00%(upstream:remoteref)%00%(upstream:track,nobracket)",
                &identity.symbolic_head,
            ])?;
            line.lines().find_map(|line| {
                let fields: Vec<_> = line.split('\0').collect();
                if fields.len() != 6 || fields[0] != identity.symbolic_head || fields[1].is_empty()
                {
                    return None;
                }
                let (mut ahead, mut behind, mut gone) = (0, 0, false);
                for part in fields[5].split(',').map(str::trim) {
                    if let Some(count) = part.strip_prefix("ahead ") {
                        ahead = count.parse().unwrap_or(0);
                    } else if let Some(count) = part.strip_prefix("behind ") {
                        behind = count.parse().unwrap_or(0);
                    } else if part == "gone" {
                        gone = true;
                    }
                }
                Some(Upstream {
                    tracking: fields[1].into(),
                    name: fields[2].into(),
                    remote: fields[3].into(),
                    remote_ref: fields[4].into(),
                    ahead,
                    behind,
                    gone,
                })
            })
        } else {
            None
        };
        // FETCH_HEAD belongs to the worktree's own Git directory.
        let fetch_head = match &self.git_dir {
            Some(directory) => directory.join("FETCH_HEAD"),
            None => PathBuf::from(
                self.run(&[
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-path",
                    "FETCH_HEAD",
                ])?
                .trim(),
            ),
        };
        let last_fetch = std::fs::metadata(fetch_head)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_secs());
        Ok(RemoteState {
            remotes,
            upstream,
            last_fetch,
        })
    }

    fn checked_out_for_network(identity: &WriteIdentity, action: &str) -> GitResult<String> {
        let Some(branch) = identity.symbolic_head.strip_prefix("refs/heads/") else {
            return Err(format!("HEAD is detached. Check out a branch to {action}."));
        };
        if identity.head.is_empty() {
            return Err(format!(
                "{branch} has no commits yet. Make the first commit before you {action}."
            ));
        }
        Ok(branch.to_owned())
    }

    /// Resolves what Pull would integrate: the checked-out branch's upstream.
    ///
    /// # Errors
    ///
    /// Returns a reason when HEAD is detached or unborn, or the branch has no
    /// upstream on a remote.
    pub fn pull_target(&self, identity: &WriteIdentity) -> GitResult<NetworkTarget> {
        let branch = Self::checked_out_for_network(identity, "pull")?;
        let state = self.remote_state(identity)?;
        let upstream = match &state.upstream {
            None => {
                return Err(format!(
                    "{branch} has no upstream branch. Push it to set one, or set one from a terminal with `git branch --set-upstream-to`."
                ))
            }
            Some(upstream) if upstream.remote == "." => {
                return Err(format!(
                    "{branch} tracks the local branch {}. Pull works with a branch on a remote; merge or rebase instead.",
                    upstream.name
                ))
            }
            Some(upstream) => upstream,
        };
        validate_revision(&upstream.remote)?;
        validate_revision(&upstream.remote_ref)?;
        Ok(NetworkTarget {
            branch,
            remote: upstream.remote.clone(),
            remote_ref: upstream.remote_ref.clone(),
            tracking: upstream.tracking.clone(),
            set_upstream: false,
            ahead: upstream.ahead,
            behind: upstream.behind,
        })
    }

    /// Resolves where Push would send the checked-out branch: its upstream,
    /// or the same-named branch on the only remote (or `origin`), set as the
    /// new upstream.
    ///
    /// # Errors
    ///
    /// Returns a reason when HEAD is detached or unborn, there is no remote,
    /// or the branch has no upstream and the remote is ambiguous.
    pub fn push_target(&self, identity: &WriteIdentity) -> GitResult<NetworkTarget> {
        let branch = Self::checked_out_for_network(identity, "push")?;
        let state = self.remote_state(identity)?;
        if state.remotes.is_empty() && state.upstream.is_none() {
            return Err(
                "This repository has no remotes. Add one from a terminal with `git remote add`."
                    .into(),
            );
        }
        if let Some(upstream) = &state.upstream {
            if upstream.remote == "." {
                return Err(format!(
                    "{branch} tracks the local branch {}. Push it from a terminal to choose a remote.",
                    upstream.name
                ));
            }
            validate_revision(&upstream.remote)?;
            validate_revision(&upstream.remote_ref)?;
            return Ok(NetworkTarget {
                branch,
                remote: upstream.remote.clone(),
                remote_ref: upstream.remote_ref.clone(),
                tracking: upstream.tracking.clone(),
                set_upstream: false,
                ahead: upstream.ahead,
                behind: upstream.behind,
            });
        }
        let remote = state.default_push_remote().ok_or_else(|| {
            format!("{branch} has no upstream and there are several remotes, none named origin. Push it once from a terminal with `git push -u <remote> {branch}`.")
        })?;
        validate_revision(remote)?;
        let not_on_remote = format!("--remotes={remote}");
        let ahead = self
            .run(&["rev-list", "--count", "HEAD", "--not", &not_on_remote, "--"])?
            .trim()
            .parse()
            .unwrap_or(0);
        Ok(NetworkTarget {
            remote_ref: format!("refs/heads/{branch}"),
            tracking: format!("refs/remotes/{remote}/{branch}"),
            branch,
            remote: remote.to_owned(),
            set_upstream: true,
            ahead,
            behind: 0,
        })
    }

    /// Runs a network command under `scope`, which can stop it and its helpers.
    fn network(
        &self,
        args: &[&str],
        scope: &ReadScope,
        action: &str,
    ) -> Result<std::process::Output, WriteError> {
        let mut command = self.command(args)?;
        // Never wait on an editor nobody can see. Merge messages use Git's default.
        command
            .env("GIT_EDITOR", ":")
            .env("GIT_SEQUENCE_EDITOR", ":")
            .env("GIT_MERGE_AUTOEDIT", "no");
        #[cfg(test)]
        if let Some(observe) = self.observe_command {
            observe(self, args)?;
        }
        capture(command, None, 0, Some(scope)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::Interrupted {
                format!("{action} was cancelled. Git was stopped.").into()
            } else {
                WriteError::inspect(format!("Could not run Git for {action}: {error}"))
            }
        })
    }

    fn network_refs(&self) -> GitResult<std::collections::BTreeSet<String>> {
        Ok(self
            .run(&[
                "for-each-ref",
                "--format=%(objectname) %(refname)",
                "refs/remotes",
                "refs/tags",
            ])?
            .lines()
            // A remote's HEAD names its default branch; it is not news.
            .filter(|line| !line.ends_with("/HEAD"))
            .map(str::to_owned)
            .collect())
    }

    /// Fetches every remote with `--prune`, updating remote-tracking refs.
    /// Local branches, the index and the working tree are not changed.
    ///
    /// # Errors
    ///
    /// Returns a plain explanation for common authentication and connection
    /// failures, followed by Git's diagnostics, or a cancellation.
    pub(crate) fn fetch(&self, scope: &ReadScope) -> Result<String, WriteError> {
        let remotes = self.run(&["remote"])?.lines().count();
        if remotes == 0 {
            return Err(
                "This repository has no remotes. Add one from a terminal with `git remote add`."
                    .into(),
            );
        }
        let before = self.network_refs()?;
        let output = self.network(&["fetch", "--all", "--prune"], scope, "Fetch")?;
        if !output.status.success() {
            let diagnostics = network_diagnostics(&output.stderr);
            let message = match network_explanation(&diagnostics, None) {
                Some(explanation) => format!("{explanation}\n\nGit said: {diagnostics}"),
                None => format!("Fetch failed. {diagnostics}"),
            };
            return Err(WriteError {
                kind: FailureKind::GitFailure,
                message,
            });
        }
        let after = self.network_refs().map_err(WriteError::inspect)?;
        let names = |refs: &std::collections::BTreeSet<String>| {
            refs.iter()
                .filter_map(|line| line.split_once(' ').map(|(_, name)| name.to_owned()))
                .collect::<std::collections::BTreeSet<_>>()
        };
        let (old_names, new_names) = (names(&before), names(&after));
        let pruned = old_names.difference(&new_names).count();
        let updated = after.difference(&before).count();
        let remotes = count_label(remotes, "remote", "remotes");
        Ok(if updated == 0 && pruned == 0 {
            format!("Fetched {remotes} · already up to date")
        } else {
            let mut message = format!(
                "Fetched {remotes} · {} updated",
                count_label(updated, "reference", "references")
            );
            if pruned > 0 {
                message.push_str(&format!(" · {pruned} pruned"));
            }
            message
        })
    }

    /// Pulls the reviewed upstream into the checked-out branch.
    ///
    /// Returns HEAD before and after. A cancelled pull that had not changed
    /// HEAD, the index or the operation state is a plain rejection; otherwise
    /// it requires inspection.
    ///
    /// # Errors
    ///
    /// Divergence with fast-forward only, local changes in the way, and
    /// authentication or connection failures are rejections with a plain
    /// explanation. Other Git failures, including conflicts, are Git failures.
    pub(crate) fn pull(
        &self,
        target: &NetworkTarget,
        mode: PullMode,
        scope: &ReadScope,
    ) -> Result<PullOutcome, WriteError> {
        validate_revision(&target.remote)?;
        validate_revision(&target.remote_ref)?;
        let before = self.write_identity()?;
        let mut args = vec!["pull"];
        args.extend_from_slice(mode.args());
        args.extend([target.remote.as_str(), target.remote_ref.as_str()]);
        let output = match self.network(&args, scope, "Pull") {
            Ok(output) => output,
            Err(error) if scope.check().is_err() => {
                let unchanged = self.write_identity().is_ok_and(|now| now == before)
                    && self
                        .operation_markers()
                        .is_ok_and(|markers| markers.is_empty());
                return Err(if unchanged {
                    "Pull was cancelled. Git was stopped before it changed the branch.".into()
                } else {
                    WriteError::inspect(format!(
                        "{} It may have stopped while updating the branch.",
                        error.message
                    ))
                });
            }
            Err(error) => return Err(error),
        };
        if !output.status.success() {
            let diagnostics = network_diagnostics(&output.stderr);
            return Err(match network_explanation(&diagnostics, Some(target)) {
                Some(explanation) => WriteError {
                    kind: FailureKind::Rejected,
                    message: format!("{explanation}\n\nGit said: {diagnostics}"),
                },
                None => WriteError::git(format!("Pull failed. {diagnostics}")),
            });
        }
        let after = self.resolve_commit("HEAD").map_err(WriteError::inspect)?;
        if self.symbolic_head().map_err(WriteError::inspect)? != before.symbolic_head {
            return Err(WriteError::inspect(
                "Pull finished but HEAD no longer names the reviewed branch.",
            ));
        }
        let from = target.remote_branch();
        let message = if after == before.head {
            format!("{} is already up to date with {from}.", target.branch)
        } else {
            let range = format!("{}..{after}", before.head);
            let added = self
                .run(&["rev-list", "--count", &range, "--"])
                .ok()
                .and_then(|count| count.trim().parse().ok())
                .unwrap_or(0);
            format!(
                "Pulled {from} into {} ({}) · {} → {} · {}",
                target.branch,
                match mode {
                    PullMode::Rebase => "rebased",
                    // A fast-forward leaves the branch exactly at the fetched tip.
                    _ if self
                        .resolve_commit(&target.tracking)
                        .is_ok_and(|tip| tip == after) =>
                    {
                        "fast-forward"
                    }
                    _ => "merged",
                },
                &before.head[..before.head.len().min(8)],
                &after[..after.len().min(8)],
                count_label(added, "new commit", "new commits"),
            )
        };
        Ok(PullOutcome {
            before: before.head,
            after,
            message,
        })
    }

    /// Pushes the checked-out branch to the reviewed remote branch without
    /// force, setting the upstream when the review said so.
    ///
    /// # Errors
    ///
    /// A non-fast-forward rejection asks the user to pull first. Other common
    /// failures have a plain explanation followed by Git's diagnostics.
    pub(crate) fn push(
        &self,
        target: &NetworkTarget,
        scope: &ReadScope,
    ) -> Result<String, WriteError> {
        validate_revision(&target.remote)?;
        validate_revision(&target.remote_ref)?;
        validate_revision(&target.branch)?;
        let refspec = format!("refs/heads/{}:{}", target.branch, target.remote_ref);
        let mut args = vec!["push"];
        if target.set_upstream {
            args.push("--set-upstream");
        }
        args.extend([target.remote.as_str(), refspec.as_str()]);
        let output = self.network(&args, scope, "Push")?;
        let diagnostics = network_diagnostics(&output.stderr);
        if !output.status.success() {
            return Err(WriteError {
                kind: FailureKind::Rejected,
                message: match network_explanation(&diagnostics, Some(target)) {
                    Some(explanation) => format!("{explanation}\n\nGit said: {diagnostics}"),
                    None => format!("Push failed. {diagnostics}"),
                },
            });
        }
        let to = target.remote_branch();
        let mut message = if diagnostics.contains("Everything up-to-date") {
            format!("{to} already has every commit on {}.", target.branch)
        } else {
            format!(
                "Pushed {} to {to} · {}",
                target.branch,
                count_label(target.ahead, "commit", "commits")
            )
        };
        if target.set_upstream {
            message.push_str(&format!(" · {to} is now the upstream of {}", target.branch));
        }
        Ok(message)
    }
}

#[cfg(test)]
pub(crate) mod network_tests {
    use super::*;
    use crate::test_repo::Fixture;
    use std::sync::{atomic::AtomicBool, Arc};

    pub(crate) fn scope() -> ReadScope {
        ReadScope::network(Arc::new(AtomicBool::new(false)))
    }

    pub(crate) fn commit(fixture: &Fixture, name: &str, text: &str) -> String {
        fixture.write(name, text);
        fixture.git.run(&["add", "--", name]).unwrap();
        fixture.git.run(&["commit", "--quiet", "-m", name]).unwrap();
        fixture.git.resolve_commit("HEAD").unwrap()
    }

    /// A worktree whose `main` tracks `origin/main` in a bare remote, and a
    /// second clone of that remote for writing someone else's commits.
    pub(crate) fn connected() -> (Fixture, Fixture) {
        let local = Fixture::new();
        commit(&local, "base.txt", "base\n");
        let remote = local.bare_remote("remote.git");
        let remote = remote.to_str().unwrap();
        local.git.run(&["remote", "add", "origin", remote]).unwrap();
        local
            .git
            .run(&["push", "--quiet", "--set-upstream", "origin", "main"])
            .unwrap();
        let other = Fixture::new();
        other.git.run(&["remote", "add", "origin", remote]).unwrap();
        other
            .git
            .run(&["pull", "--quiet", "origin", "main"])
            .unwrap();
        other
            .git
            .run(&["branch", "--set-upstream-to=origin/main"])
            .unwrap();
        (local, other)
    }

    /// Commits in `other` and publishes it, so `local` is behind.
    pub(crate) fn publish(other: &Fixture, name: &str) -> String {
        let id = commit(other, name, &format!("{name}\n"));
        other.git.run(&["push", "--quiet"]).unwrap();
        id
    }

    fn state(fixture: &Fixture) -> RemoteState {
        let identity = fixture.git.write_identity().unwrap();
        fixture.git.remote_state(&identity).unwrap()
    }

    fn pull(fixture: &Fixture, mode: PullMode) -> Result<PullOutcome, WriteError> {
        let identity = fixture.git.write_identity().unwrap();
        let target = fixture.git.pull_target(&identity).unwrap();
        fixture.git.pull(&target, mode, &scope())
    }

    fn push_target(fixture: &Fixture) -> NetworkTarget {
        let identity = fixture.git.write_identity().unwrap();
        fixture.git.push_target(&identity).unwrap()
    }

    #[test]
    fn remote_state_reads_remotes_upstream_and_counts_locally() {
        let lonely = Fixture::new();
        commit(&lonely, "a.txt", "a\n");
        let alone = state(&lonely);
        assert!(alone.remotes.is_empty() && alone.upstream.is_none());
        assert_eq!(alone.default_push_remote(), None);
        let identity = lonely.git.write_identity().unwrap();
        assert!(lonely
            .git
            .push_target(&identity)
            .unwrap_err()
            .contains("no remotes"));
        assert!(lonely
            .git
            .pull_target(&identity)
            .unwrap_err()
            .contains("no upstream"));

        let (local, _other) = connected();
        commit(&local, "mine.txt", "mine\n");
        let state = state(&local);
        assert_eq!(state.remotes, ["origin"]);
        let upstream = state.network_upstream().unwrap();
        assert_eq!(upstream.name, "origin/main");
        assert_eq!(upstream.remote, "origin");
        assert_eq!(upstream.remote_ref, "refs/heads/main");
        assert_eq!(upstream.tracking, "refs/remotes/origin/main");
        assert_eq!(
            (upstream.ahead, upstream.behind, upstream.gone),
            (1, 0, false)
        );
        local.git.run(&["switch", "--quiet", "--detach"]).unwrap();
        let identity = local.git.write_identity().unwrap();
        assert!(local
            .git
            .pull_target(&identity)
            .unwrap_err()
            .starts_with("HEAD is detached"));
        assert!(local
            .git
            .remote_state(&identity)
            .unwrap()
            .upstream
            .is_none());
    }

    #[test]
    fn fetch_updates_tracking_refs_and_records_the_fetch_time() {
        let (local, other) = connected();
        let theirs = publish(&other, "theirs.txt");
        assert_eq!(state(&local).upstream.unwrap().behind, 0);
        let message = local.git.fetch(&scope()).unwrap();
        assert!(message.contains("1 reference updated"), "{message}");
        assert_eq!(
            local
                .git
                .resolve_commit("refs/remotes/origin/main")
                .unwrap(),
            theirs
        );
        let state = state(&local);
        assert_eq!(state.upstream.unwrap().behind, 1);
        assert!(state.last_fetch.is_some());
        assert!(local
            .git
            .fetch(&scope())
            .unwrap()
            .ends_with("already up to date"));
    }

    #[test]
    fn fast_forward_pull_moves_the_branch_to_the_upstream() {
        let (local, other) = connected();
        let before = local.git.resolve_commit("HEAD").unwrap();
        let theirs = publish(&other, "theirs.txt");
        let outcome = pull(&local, PullMode::FastForwardOnly).unwrap();
        assert_eq!(outcome.before, before);
        assert_eq!(outcome.after, theirs);
        assert!(
            outcome.message.contains("fast-forward"),
            "{}",
            outcome.message
        );
        assert_eq!(local.git.resolve_commit("HEAD").unwrap(), theirs);
        let again = pull(&local, PullMode::FastForwardOnly).unwrap();
        assert!(again.message.contains("already up to date"));
        assert_eq!(again.before, again.after);
    }

    #[test]
    fn fast_forward_only_refuses_a_diverged_branch_with_a_reason() {
        let (local, other) = connected();
        publish(&other, "theirs.txt");
        let mine = commit(&local, "mine.txt", "mine\n");
        let error = pull(&local, PullMode::FastForwardOnly).unwrap_err();
        assert_eq!(error.kind, FailureKind::Rejected);
        assert!(error.message.contains("have diverged"), "{}", error.message);
        assert!(!error.message.contains("hint:"));
        assert_eq!(local.git.resolve_commit("HEAD").unwrap(), mine);
    }

    #[test]
    fn diverged_pulls_merge_or_rebase() {
        let (local, other) = connected();
        let theirs = publish(&other, "theirs.txt");
        let mine = commit(&local, "mine.txt", "mine\n");
        let merged = pull(&local, PullMode::FastForwardIfPossible).unwrap();
        assert!(merged.message.contains("merged"), "{}", merged.message);
        let parents = local
            .git
            .run(&["show", "-s", "--format=%P", "HEAD"])
            .unwrap();
        assert_eq!(
            parents.split_whitespace().collect::<Vec<_>>(),
            [mine.as_str(), theirs.as_str()]
        );

        let (local, other) = connected();
        let theirs = publish(&other, "theirs.txt");
        commit(&local, "mine.txt", "mine\n");
        let rebased = pull(&local, PullMode::Rebase).unwrap();
        assert!(rebased.message.contains("rebased"), "{}", rebased.message);
        assert_eq!(local.git.resolve_commit("HEAD^").unwrap(), theirs);
        assert_eq!(local.git.symbolic_head().unwrap(), "refs/heads/main");
        assert!(local.git.operation_markers().unwrap().is_empty());
    }

    #[test]
    fn push_updates_the_upstream_or_sets_one() {
        let (local, _other) = connected();
        let mine = commit(&local, "mine.txt", "mine\n");
        let target = push_target(&local);
        assert!(!target.set_upstream);
        assert_eq!(target.ahead, 1);
        assert_eq!(target.remote_branch(), "origin/main");
        let message = local.git.push(&target, &scope()).unwrap();
        assert_eq!(message, "Pushed main to origin/main · 1 commit");
        assert_eq!(
            local
                .git
                .resolve_commit("refs/remotes/origin/main")
                .unwrap(),
            mine
        );

        local
            .git
            .run(&["switch", "--quiet", "-c", "topic"])
            .unwrap();
        let topic = commit(&local, "topic.txt", "topic\n");
        let target = push_target(&local);
        assert!(target.set_upstream);
        assert_eq!(target.remote, "origin");
        assert_eq!(target.remote_ref, "refs/heads/topic");
        assert_eq!(target.ahead, 1);
        let message = local.git.push(&target, &scope()).unwrap();
        assert!(
            message.ends_with("origin/topic is now the upstream of topic"),
            "{message}"
        );
        let upstream = state(&local).upstream.unwrap();
        assert_eq!(
            (upstream.name.as_str(), upstream.ahead),
            ("origin/topic", 0)
        );
        assert_eq!(
            local
                .git
                .resolve_commit("refs/remotes/origin/topic")
                .unwrap(),
            topic
        );
    }

    #[test]
    fn push_without_upstream_needs_one_remote_or_origin() {
        let (local, _other) = connected();
        local
            .git
            .run(&["switch", "--quiet", "-c", "topic"])
            .unwrap();
        local
            .git
            .run(&["remote", "add", "backup", "file:///nowhere"])
            .unwrap();
        assert_eq!(push_target(&local).remote, "origin");
        local
            .git
            .run(&["remote", "rename", "origin", "upstream"])
            .unwrap();
        let identity = local.git.write_identity().unwrap();
        assert!(local
            .git
            .push_target(&identity)
            .unwrap_err()
            .contains("several remotes"));
    }

    #[test]
    fn rejected_push_suggests_pulling_and_never_forces() {
        let (local, other) = connected();
        let theirs = publish(&other, "theirs.txt");
        commit(&local, "mine.txt", "mine\n");
        let error = local.git.push(&push_target(&local), &scope()).unwrap_err();
        assert_eq!(error.kind, FailureKind::Rejected);
        assert!(
            error.message.starts_with("Push rejected"),
            "{}",
            error.message
        );
        assert!(error.message.contains("Pull first"));
        other.git.run(&["fetch", "--quiet"]).unwrap();
        assert_eq!(
            other
                .git
                .resolve_commit("refs/remotes/origin/main")
                .unwrap(),
            theirs
        );
    }

    #[test]
    fn common_failures_have_plain_explanations() {
        let target = NetworkTarget {
            branch: "main".into(),
            remote: "origin".into(),
            remote_ref: "refs/heads/main".into(),
            ..Default::default()
        };
        for (stderr, expected) in [
            (
                "fatal: Authentication failed for 'https://example.invalid/'",
                "Authentication failed for origin",
            ),
            (
                "fatal: could not read Username for 'https://x': terminal prompts disabled",
                "Authentication failed",
            ),
            (
                "git@example.invalid: Permission denied (publickey).",
                "refused the SSH key",
            ),
            (
                "fatal: unable to access 'https://x/': Could not resolve host: x",
                "Could not resolve the host",
            ),
            (
                "ssh: Could not resolve hostname x: No such host is known.",
                "Could not resolve the host",
            ),
            (
                " ! [rejected]        main -> main (non-fast-forward)",
                "Push rejected",
            ),
            (
                "fatal: couldn't find remote ref refs/heads/main",
                "no longer exists",
            ),
            (
                "error: Your local changes to the following files would be overwritten by merge:",
                "Local changes would be overwritten",
            ),
        ] {
            let explanation = network_explanation(stderr, Some(&target)).unwrap();
            assert!(explanation.contains(expected), "{stderr} -> {explanation}");
        }
        assert_eq!(network_explanation("fatal: something new", None), None);
    }

    #[test]
    fn cancelling_a_network_command_stops_git_and_its_helpers() {
        let fixture = Fixture::new();
        commit(&fixture, "a.txt", "a\n");
        // An SSH transport that never answers, run through Git's shell.
        for args in [
            &["config", "core.sshCommand", "sleep 30; exit 1;"][..],
            &["remote", "add", "slow", "ssh://example.invalid/repo.git"],
        ] {
            fixture.git.run(args).unwrap();
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let scope = ReadScope::network(cancelled.clone());
        let start = std::time::Instant::now();
        let error = std::thread::scope(|threads| {
            threads.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(500));
                cancelled.store(true, std::sync::atomic::Ordering::Release);
            });
            fixture.git.fetch(&scope).unwrap_err()
        });
        assert_eq!(error.kind, FailureKind::Rejected, "{}", error.message);
        assert_eq!(error.message, "Fetch was cancelled. Git was stopped.");
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
    }
}

fn parse_status(data: &[u8]) -> Vec<FileChange> {
    let mut out = Vec::new();
    let mut records = data.split(|b| *b == 0);
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let index = record[0] as char;
        let worktree = record[1] as char;
        let original_path = if index == 'R' || index == 'C' || worktree == 'R' || worktree == 'C' {
            records.next()
        } else {
            None
        };
        out.push(file_change(&record[3..], original_path, index, worktree));
    }
    out
}

fn file_change(path: &[u8], original: Option<&[u8]>, index: char, worktree: char) -> FileChange {
    let display = |raw: &[u8]| match std::str::from_utf8(raw) {
        Ok(path) => path.to_owned(),
        Err(_) => format!(
            "b\"{}\"",
            raw.iter()
                .flat_map(|byte| std::ascii::escape_default(*byte))
                .map(char::from)
                .collect::<String>()
        ),
    };
    FileChange {
        path: display(path),
        original_path: original.map(display),
        index,
        worktree,
        unsupported_path: std::str::from_utf8(path).is_err()
            || original.is_some_and(|raw| std::str::from_utf8(raw).is_err()),
    }
}

/// Splits a unified Git patch into individual hunks in patch order.
/// Each hunk includes the original headers for its file.
pub fn split_hunks(diff: &str) -> Vec<Hunk> {
    hunk_ranges(diff)
        .iter()
        .map(|hunk| hunk.materialize(diff))
        .collect()
}

pub(crate) struct HunkRange {
    header: std::ops::Range<usize>,
    body: std::ops::Range<usize>,
}

impl HunkRange {
    pub fn title<'a>(&self, patch: &'a str) -> &'a str {
        patch[self.body.clone()]
            .lines()
            .next()
            .unwrap_or_default()
            .trim_end()
    }
    pub fn blocked_reason(&self, patch: &str) -> Option<&'static str> {
        header_blocked_reason(&patch[self.header.clone()])
    }
    pub fn materialize(&self, patch: &str) -> Hunk {
        Hunk {
            title: self.title(patch).to_owned(),
            patch: format!(
                "{}{}",
                &patch[self.header.clone()],
                &patch[self.body.clone()]
            ),
        }
    }
}

pub(crate) fn hunk_ranges(diff: &str) -> Vec<HunkRange> {
    let mut ranges = Vec::new();
    let (mut offset, mut file_start) = (0, 0);
    let mut header_end = None;
    let mut body_start = None;
    for line in diff.split_inclusive('\n') {
        let file = line.starts_with("diff --git ");
        let hunk = line.starts_with("@@ ");
        if file || hunk {
            if let (Some(header_end), Some(start)) = (header_end, body_start.take()) {
                ranges.push(HunkRange {
                    header: file_start..header_end,
                    body: start..offset,
                });
            }
            if file {
                file_start = offset;
                header_end = None;
            } else {
                header_end.get_or_insert(offset);
                body_start = Some(offset);
            }
        }
        offset += line.len();
    }
    if let (Some(header_end), Some(start)) = (header_end, body_start) {
        ranges.push(HunkRange {
            header: file_start..header_end,
            body: start..offset,
        });
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_repo::Fixture;

    #[test]
    fn history_search_matches_reference_only_with_normalized_whitespace() {
        let commit = Commit {
            subject: "Implement feature".into(),
            refs: vec!["Release/October".into()],
            ..Default::default()
        }
        .index_search();
        assert!(commit.matches_search(&normalize_search("  RELEASE/october  ")));
        assert!(!commit.matches_search(&normalize_search("november")));
    }

    #[test]
    #[ignore = "repeatable history search timing fixture"]
    fn benchmark_history_search() {
        let commits: Vec<_> = (0..20_000)
            .map(|i| Commit {
                subject: format!("Add fixture feature number {i}"),
                author: "Fixture Author".into(),
                id: format!("{i:040x}"),
                refs: vec![format!("feature/group-{}", i % 30)],
                ..Default::default()
            })
            .collect();
        let index_start = std::time::Instant::now();
        let indexed: Vec<_> = commits.iter().cloned().map(Commit::index_search).collect();
        let index_ms = index_start.elapsed().as_secs_f64() * 1000.0;
        let mut before = vec![];
        let mut after = vec![];
        for i in 0..11 {
            let query = normalize_search(&format!(" FEATURE/GROUP-{i} "));
            let start = std::time::Instant::now();
            let old = commits
                .iter()
                .filter(|c| {
                    format!("{} {} {} {}", c.subject, c.author, c.id, c.refs.join(" "))
                        .to_lowercase()
                        .contains(&query)
                })
                .count();
            before.push(start.elapsed().as_secs_f64() * 1000.0);
            let start = std::time::Instant::now();
            let new = indexed.iter().filter(|c| c.matches_search(&query)).count();
            after.push(start.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(old, new);
        }
        before.sort_by(f64::total_cmp);
        after.sort_by(f64::total_cmp);
        println!("20,000 commits: median search {:.2} ms before, {:.2} ms indexed. One-time index build (including cloned fixture) {index_ms:.2} ms.", before[5], after[5]);
    }

    #[test]
    fn canonical_tag_identity_is_separate_from_a_same_named_branch() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        git.run(&["commit", "--allow-empty", "-m", "tagged"])
            .unwrap();
        let tagged = git.resolve_commit("HEAD").unwrap();
        git.run(&["tag", "-a", "release", "-m", "release"]).unwrap();
        git.run(&["commit", "--allow-empty", "-m", "branch tip"])
            .unwrap();
        let tip = git.resolve_commit("HEAD").unwrap();
        git.create_branch("release").unwrap();
        let snapshot = git.snapshot(100).unwrap();
        assert_eq!(
            snapshot.loaded_ref_tip(ReferenceKind::Tag, "refs/tags/release"),
            Some(tagged.as_str())
        );
        assert_eq!(
            snapshot.loaded_ref_tip(ReferenceKind::LocalBranch, "release"),
            Some(tip.as_str())
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_are_distinct_disabled_rows_in_status_and_commit_details() {
        use std::os::unix::ffi::OsStrExt;
        let fixture = Fixture::new();
        let git = &fixture.git;
        for raw in [b"name-\xff.txt", b"name-\xfe.txt"] {
            std::fs::write(git.path.join(std::ffi::OsStr::from_bytes(raw)), b"data\n").unwrap();
        }
        let status = git.status().unwrap();
        assert_eq!(status.len(), 2);
        assert_ne!(status[0].path, status[1].path);
        assert!(status.iter().all(|entry| entry.path_issue().is_some()));
        assert!(git.stage_reviewed(&status[0], false).is_err());
        assert!(git.write_identity().unwrap().index.is_empty());
        git.stage_all(false).unwrap();
        git.commit("non-Unicode paths").unwrap();
        let details = git.changed_files("HEAD").unwrap();
        assert_eq!(details.len(), 2);
        assert!(details.iter().all(|entry| entry.path_issue().is_some()));
    }

    #[test]
    fn gitlink_preview_pins_short_output_and_rejects_partial_updates() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        git.run(&["commit", "--allow-empty", "-m", "old target"])
            .unwrap();
        let old = git.resolve_commit("HEAD").unwrap();
        git.run(&["commit", "--allow-empty", "-m", "new target"])
            .unwrap();
        let new = git.resolve_commit("HEAD").unwrap();
        git.run(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{old},module"),
        ])
        .unwrap();
        git.commit("old gitlink").unwrap();
        git.run(&[
            "update-index",
            "--cacheinfo",
            &format!("160000,{new},module"),
        ])
        .unwrap();
        git.run(&["config", "diff.submodule", "diff"]).unwrap();
        let hunks = git.hunks("module", true).unwrap();
        assert_eq!(hunks.len(), 1);
        assert!(hunks[0]
            .patch
            .contains(&format!("+Subproject commit {new}")));
        let reviewed = git.write_identity().unwrap();
        assert!(git.apply_hunk(&hunks[0], true).is_err());
        assert_eq!(git.write_identity().unwrap(), reviewed);
        assert!(git.run(&["ls-files", "-s"]).unwrap().starts_with("160000 "));
    }

    #[test]
    fn capture_preserves_exit_status_when_git_closes_large_stdin_early() {
        let fixture = Fixture::new();
        let cmd = fixture
            .git
            .command(&["gitcrab-no-such-subcommand"])
            .unwrap();
        let result = capture(cmd, Some(&vec![b'x'; 512 * 1024]), 0, None).unwrap();
        assert!(!result.status.success());
        assert!(!result.stderr.is_empty());
    }
    #[test]
    fn reviewed_commit_rejects_index_head_hooks_and_active_markers() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("one.txt", "one\n");
        git.stage("one.txt").unwrap();
        let unborn = git.write_identity().unwrap();
        let sha = git.commit_reviewed("first", &unborn).unwrap();
        assert_eq!(sha, git.run(&["rev-parse", "HEAD"]).unwrap().trim());
        fixture.write("one.txt", "two\n");
        git.stage("one.txt").unwrap();
        let reviewed = git.write_identity().unwrap();
        fixture.write("extra.txt", "extra\n");
        git.stage("extra.txt").unwrap();
        assert!(git
            .commit_reviewed("stale index", &reviewed)
            .unwrap_err()
            .message
            .contains("changed"));
        let reviewed = git.write_identity().unwrap();
        git.run(&["commit", "--allow-empty", "-m", "external"])
            .unwrap();
        assert!(git.commit_reviewed("stale HEAD", &reviewed).is_err());
        fixture.write("one.txt", "three\n");
        git.stage("one.txt").unwrap();
        let reviewed = git.write_identity().unwrap();
        fixture.hook(
            "pre-commit",
            "#!/bin/sh\necho controlled-hook-failure >&2\nexit 1\n",
        );
        let failed = git.commit_reviewed("hook fails", &reviewed).unwrap_err();
        assert_eq!(failed.kind, FailureKind::GitFailure);
        assert!(
            failed.message.contains("controlled-hook-failure"),
            "{failed:?}"
        );
        assert_eq!(git.write_identity().unwrap(), reviewed);
        fixture.hook("pre-commit", "#!/bin/sh\nexit 0\n");
        let marker = git.run(&["rev-parse", "--git-path", "MERGE_HEAD"]).unwrap();
        std::fs::write(git.path.join(marker.trim()), format!("{}\n", reviewed.head)).unwrap();
        let snapshot = git.snapshot(100).unwrap();
        assert!(snapshot.commit_blocked());
        assert!(!snapshot.status.iter().any(|f| f.index == 'U'));
        assert!(git
            .commit_reviewed("ordinary commit", &reviewed)
            .unwrap_err()
            .message
            .contains("active Git operation"));
    }
    #[test]
    fn commit_with_hook_modified_index_requires_inspection_and_keeps_actual_commit() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("a.txt", "base\n");
        git.stage("a.txt").unwrap();
        git.commit("base").unwrap();
        fixture.write("a.txt", "reviewed\n");
        git.stage("a.txt").unwrap();
        let reviewed = git.write_identity().unwrap();
        fixture.hook(
            "pre-commit",
            "#!/bin/sh\nprintf 'hook edit\\n' > a.txt\ngit add -- a.txt\n",
        );
        let failure = git
            .commit_reviewed("changed by hook", &reviewed)
            .unwrap_err();
        assert_eq!(failure.kind, FailureKind::Inspect);
        let after = git.write_identity().unwrap();
        assert_ne!(after.head, reviewed.head);
        assert_ne!(after.index, reviewed.index);
        assert_eq!(git.resolve_commit("HEAD^").unwrap(), reviewed.head);
        assert_eq!(git.run(&["show", "HEAD:a.txt"]).unwrap(), "hook edit\n");
        assert_eq!(git.run(&["show", ":a.txt"]).unwrap(), "hook edit\n");
        assert_eq!(
            std::fs::read(git.path.join("a.txt")).unwrap(),
            b"hook edit\n"
        );
    }

    #[test]
    fn reviewed_commit_verifies_a_signed_commit_with_signature_display_enabled() {
        let executable = if cfg!(windows) {
            "ssh-keygen.exe"
        } else {
            "ssh-keygen"
        };
        let Some(ssh_keygen) = std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .filter(|directory| directory.is_absolute())
                .map(|directory| directory.join(executable))
                .find(|path| path.is_file())
        }) else {
            eprintln!("Skipping SSH signing test because ssh-keygen is unavailable");
            return;
        };
        let fixture = Fixture::new();
        let git = &fixture.git;
        let key = git.path.join(".git/signing-key");
        let output = std::process::Command::new(&ssh_keygen)
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let public_key = std::fs::read_to_string(key.with_extension("pub")).unwrap();
        fixture.write(
            ".git/allowed-signers",
            &format!("test@example.invalid {public_key}"),
        );
        fixture.write("a.txt", "base\n");
        git.stage("a.txt").unwrap();
        git.commit("base").unwrap();
        for (setting, value) in [
            ("gpg.format", "ssh"),
            ("gpg.ssh.program", ssh_keygen.to_str().unwrap()),
            ("gpg.ssh.allowedSignersFile", ".git/allowed-signers"),
            ("user.signingKey", key.to_str().unwrap()),
            ("commit.gpgSign", "true"),
            ("log.showSignature", "true"),
        ] {
            git.run(&["config", setting, value]).unwrap();
        }
        fixture.write("a.txt", "reviewed\n");
        git.stage("a.txt").unwrap();
        let reviewed = git.write_identity().unwrap();
        let sha = git.commit_reviewed("signed change", &reviewed).unwrap();
        assert_eq!(git.write_identity().unwrap().head, sha);
        let display = git.run(&["show", "-s", "--format=%P%x00%T", &sha]).unwrap();
        assert!(display.lines().count() > 1);
    }

    #[test]
    fn failed_read_after_a_successful_commit_requires_inspection() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("a.txt", "reviewed\n");
        git.stage("a.txt").unwrap();
        let reviewed = git.write_identity().unwrap();
        fixture.hook(
            "post-commit",
            "#!/bin/sh\ngit symbolic-ref HEAD refs/heads/empty\n",
        );
        let failure = git
            .commit_reviewed("committed on main", &reviewed)
            .unwrap_err();
        assert_eq!(failure.kind, FailureKind::Inspect);
        let after = git.write_identity().unwrap();
        assert_eq!(after.symbolic_head, "refs/heads/empty");
        assert!(after.head.is_empty());
        assert_eq!(after.index, reviewed.index);
        assert_eq!(
            git.run(&["show", "refs/heads/main:a.txt"]).unwrap(),
            "reviewed\n"
        );
        assert_eq!(
            std::fs::read(git.path.join("a.txt")).unwrap(),
            b"reviewed\n"
        );
    }

    #[test]
    fn discard_only_reviewed_hunk_preserves_index_head_and_other_hunk() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let original = (0..40).map(|i| format!("line {i}\n")).collect::<String>();
        fixture.write("日本語.txt", &original);
        git.stage(".").unwrap();
        git.commit("base").unwrap();
        let staged = original.replace("line 1\n", "staged one\n");
        fixture.write("日本語.txt", &staged);
        git.stage(".").unwrap();
        let working = staged
            .replace("line 10\n", "work ten\n")
            .replace("line 30\n", "work thirty\n");
        fixture.write("日本語.txt", &working);
        let identity = git.write_identity().unwrap();
        let hunks = git.hunks("日本語.txt", false).unwrap();
        assert_eq!(hunks.len(), 2);
        git.discard_hunk("日本語.txt", &hunks[0]).unwrap();
        assert_eq!(git.write_identity().unwrap(), identity);
        let expected = staged.replace("line 30\n", "work thirty\n");
        assert_eq!(
            std::fs::read(git.path.join("日本語.txt")).unwrap(),
            expected.as_bytes()
        );
        assert!(git.discard_hunk("日本語.txt", &hunks[0]).is_err());
        fixture.write("new.txt", "untracked\n");
        let added = git.hunks("new.txt", false).unwrap();
        assert!(git.discard_hunk("new.txt", &added[0]).is_err());
    }
    #[test]
    fn unique_path_counts_and_linked_worktree_markers_stay_local() {
        let snapshot = RepoSnapshot {
            status: vec![
                FileChange {
                    path: "new".into(),
                    index: 'R',
                    worktree: 'M',
                    original_path: Some("old".into()),
                    unsupported_path: false,
                },
                FileChange {
                    path: "new".into(),
                    index: 'M',
                    worktree: 'M',
                    original_path: None,
                    unsupported_path: false,
                },
            ],
            ..Default::default()
        };
        assert_eq!(snapshot.unique_changes().len(), 1);
        assert_eq!(snapshot.change_counts(), [0, 0, 0, 1]);
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("a", "a");
        git.stage(".").unwrap();
        git.commit("root").unwrap();
        let linked_path = git.path.join("linked");
        git.run(&[
            "worktree",
            "add",
            "-b",
            "linked",
            linked_path.to_str().unwrap(),
        ])
        .unwrap();
        let linked = Git::open(&linked_path).unwrap();
        let marker = linked
            .run(&["rev-parse", "--git-path", "CHERRY_PICK_HEAD"])
            .unwrap();
        let marker = PathBuf::from(marker.trim());
        std::fs::write(
            if marker.is_absolute() {
                marker
            } else {
                linked.path.join(marker)
            },
            git.run(&["rev-parse", "HEAD"]).unwrap(),
        )
        .unwrap();
        assert_eq!(linked.operation_markers().unwrap(), ["cherry-pick"]);
        assert!(git.operation_markers().unwrap().is_empty());
    }
    #[test]
    fn rename_comparisons_keep_paths_and_blobs_in_the_correct_namespace() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let old = "legacy 日本語 [1].txt";
        let new = "archive 日本語 [2].txt";
        let text = (0..30)
            .map(|n| format!("stable line {n}\n"))
            .collect::<String>();
        fixture.write(old, &text);
        git.stage(old).unwrap();
        git.commit("root").unwrap();
        let before = git.run(&["rev-parse", &format!("HEAD:{old}")]).unwrap();
        git.run(&["mv", "--", old, new]).unwrap();
        let staged = git.diff(None, new, true).unwrap();
        assert!(staged.contains("rename from "));
        assert!(!staged.contains("@@"));
        assert_eq!(before, git.run(&["rev-parse", &format!(":{new}")]).unwrap());
        git.commit("pure rename").unwrap();
        let id = git.run(&["rev-parse", "HEAD"]).unwrap();
        let files = git.changed_files(id.trim()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].original_path.as_deref(), Some(old));
        assert_eq!(files[0].path, new);
        let committed = git.diff(Some(id.trim()), new, false).unwrap();
        assert!(committed.contains("similarity index 100%"));
        assert!(!committed.contains("@@"));
        // An unstaged edit after a staged rename compares against the new
        // index path, while the staged comparison still uses HEAD's old path.
        git.run(&["mv", "--", new, old]).unwrap();
        fixture.write(old, &text.replace("stable line 15", "edited line 15"));
        let working = git.diff(None, old, false).unwrap();
        assert!(working.contains("-stable line 15"));
        assert!(working.contains("+edited line 15"));
        assert!(!working.contains("rename from"));
        git.stage(old).unwrap();
        let edited = git.diff(None, old, true).unwrap();
        assert!(edited.contains("rename from"));
        assert!(edited.contains("-stable line 15"));
        git.commit("rename with edit").unwrap();
        let id = git.run(&["rev-parse", "HEAD"]).unwrap();
        let mut expected_args = patch_args("diff");
        expected_args.extend(["--find-renames", "HEAD^1", "HEAD", "--", new, old]);
        assert_eq!(
            git.diff(Some(id.trim()), old, false).unwrap(),
            git.run(&expected_args).unwrap()
        );
        assert_eq!(
            git.run(&["show", &format!("HEAD:{old}")]).unwrap(),
            text.replace("stable line 15", "edited line 15")
        );
    }

    #[test]
    fn real_git_create_branch_at_keeps_head_and_index() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("file", "one");
        git.stage("file").unwrap();
        git.commit("first").unwrap();
        let first = git.run(&["rev-parse", "HEAD"]).unwrap();
        fixture.write("file", "two");
        git.stage("file").unwrap();
        git.commit("second").unwrap();
        let head = git.run(&["rev-parse", "HEAD"]).unwrap();
        let tree = git.run(&["write-tree"]).unwrap();
        git.create_branch_at("feature/日本語", first.trim())
            .unwrap();
        assert_eq!(git.run(&["rev-parse", "feature/日本語"]).unwrap(), first);
        assert_eq!(git.run(&["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(git.run(&["write-tree"]).unwrap(), tree);
        assert!(git.create_branch_at("--bad", first.trim()).is_err());
    }
    #[test]
    fn real_git_unicode_paths_and_branch_names() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let name = "notes 日本語 [draft].txt";
        fixture.write(name, "First version\n");
        git.stage(name).unwrap();
        git.commit("Unicode path").unwrap();
        git.create_branch("feature/日本語").unwrap();
        git.checkout("feature/日本語").unwrap();
        fixture.write(name, "Second version\n");
        let state = git.snapshot(10).unwrap();
        assert_eq!(state.head, "feature/日本語");
        assert_eq!(state.status[0].path, name);
        assert!(git
            .diff(None, name, false)
            .unwrap()
            .contains("+Second version"));
        let nested = git.path.join("folder with spaces 日本語");
        std::fs::create_dir(&nested).unwrap();
        assert_eq!(
            Git::open(nested).unwrap().path.canonicalize().unwrap(),
            git.path.canonicalize().unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn untracked_symlink_preview_cannot_leave_repository() {
        let fixture = Fixture::new();
        let outside = Fixture::new();
        outside.write("outside.txt", "outside repository\n");
        std::os::unix::fs::symlink(
            outside.git.path.join("outside.txt"),
            fixture.git.path.join("link.txt"),
        )
        .unwrap();
        let error = fixture.git.diff(None, "link.txt", false).unwrap_err();
        assert!(error.contains("leaves the repository"));
        assert_eq!(
            std::fs::read_to_string(outside.git.path.join("outside.txt")).unwrap(),
            "outside repository\n"
        );
    }

    #[test]
    fn status_keeps_rename_destination_and_literal_names() {
        let rows = parse_status(b"R  new name\0old name\0 M [literal]*.rs\0?? line\nbreak\0");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].path, "new name");
        assert_eq!(rows[0].original_path.as_deref(), Some("old name"));
        assert_eq!(rows[1].path, "[literal]*.rs");
        assert_eq!(rows[2].path, "line\nbreak");
    }
    #[test]
    fn split_patch_keeps_context_and_headers() {
        let patches = split_hunks("diff --git a/a b/a\nindex 1..2 100644\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n@@ -20 +20 @@\n-before\n+after\n");
        assert_eq!(patches.len(), 2);
        assert!(patches[1].patch.contains("--- a/a\n+++ b/a\n@@ -20"));
        assert!(!patches[1].patch.contains("+new"));
    }
    #[test]
    fn real_git_unborn_literal_paths_and_independent_hunks() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        assert!(git.snapshot(100).unwrap().commits.is_empty());
        let original: String = (0..40).map(|i| format!("line {i}\n")).collect();
        fixture.write("[literal].txt", &original);
        assert!(git
            .diff(None, "[literal].txt", false)
            .unwrap()
            .contains("+line 2"));
        git.stage("[literal].txt").unwrap();
        assert_eq!(git.snapshot(100).unwrap().status[0].index, 'A');
        git.unstage("[literal].txt").unwrap();
        assert_eq!(git.snapshot(100).unwrap().status[0].index, '?');
        git.stage("[literal].txt").unwrap();
        git.commit("Initial content").unwrap();
        fixture.write(
            "[literal].txt",
            &original
                .replace("line 2\n", "first edit\n")
                .replace("line 32\n", "second edit\n"),
        );
        let hunks = git.hunks("[literal].txt", false).unwrap();
        assert_eq!(hunks.len(), 2);
        git.apply_hunk(&hunks[0], false).unwrap();
        let cached = git.diff(None, "[literal].txt", true).unwrap();
        assert!(cached.contains("+first edit"));
        assert!(!cached.contains("+second edit"));
        let remaining = git.diff(None, "[literal].txt", false).unwrap();
        assert!(remaining.contains("+second edit"));
        assert!(!remaining.contains("+first edit"));
        git.apply_hunk(&git.hunks("[literal].txt", true).unwrap()[0], true)
            .unwrap();
        assert!(git.diff(None, "[literal].txt", true).unwrap().is_empty());
        assert_eq!(git.hunks("[literal].txt", false).unwrap().len(), 2);
    }
    #[test]
    fn real_git_merge_refs_root_diff_and_detached_head() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("root.txt", "root\n");
        git.stage("root.txt").unwrap();
        git.commit("Root").unwrap();
        let root = git.snapshot(100).unwrap().commits[0].id.clone();
        assert_eq!(
            git.changed_files(&root)
                .unwrap()
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec!["root.txt"]
        );
        assert!(git
            .diff(Some(&root), "root.txt", false)
            .unwrap()
            .contains("+root"));
        git.create_branch("feature").unwrap();
        git.checkout("feature").unwrap();
        fixture.write("feature.txt", "feature\n");
        git.stage("feature.txt").unwrap();
        git.commit("Feature").unwrap();
        git.checkout("main").unwrap();
        fixture.write("main.txt", "main\n");
        git.stage("main.txt").unwrap();
        git.commit("Main").unwrap();
        git.merge("feature").unwrap();
        git.run(&["tag", "-a", "v1", "-m", "Version one"]).unwrap();
        let state = git.snapshot(100).unwrap();
        assert_eq!(state.commits[0].parents.len(), 2);
        assert!(state.commits[0].refs.contains(&"v1".into()));
        assert!(state.commits[0].refs.contains(&"HEAD".into()));
        assert_eq!(
            git.changed_files(&state.commits[0].id)
                .unwrap()
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec!["feature.txt"]
        );
        git.checkout(&root).unwrap();
        assert!(git.run(&["symbolic-ref", "--quiet", "HEAD"]).is_err());
        let detached = git.snapshot(100).unwrap();
        assert_eq!(detached.checked_out_branch(), None);
        assert!(detached
            .commits
            .iter()
            .any(|c| c.id == root && c.refs.contains(&"HEAD".into())));
    }
    #[test]
    fn real_git_cherry_pick_revert_stash_and_conflict_feedback() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("shared.txt", "original\n");
        git.stage("shared.txt").unwrap();
        git.commit("Root").unwrap();
        git.create_branch("feature").unwrap();
        git.checkout("feature").unwrap();
        fixture.write("feature.txt", "feature\n");
        git.stage("feature.txt").unwrap();
        git.commit("Feature addition").unwrap();
        let feature = git.run(&["rev-parse", "HEAD"]).unwrap().trim().to_string();
        git.checkout("main").unwrap();
        git.cherry_pick(&feature).unwrap();
        assert!(git.path.join("feature.txt").exists());
        git.revert("HEAD").unwrap();
        assert!(!git.path.join("feature.txt").exists());
        fixture.write("shared.txt", "work in progress\n");
        git.stash().unwrap();
        assert!(git.snapshot(100).unwrap().status.is_empty());
        git.stash_pop().unwrap();
        assert_eq!(
            std::fs::read_to_string(git.path.join("shared.txt")).unwrap(),
            "work in progress\n"
        );
        git.stage("shared.txt").unwrap();
        git.commit("Main line changes").unwrap();
        git.checkout("feature").unwrap();
        fixture.write("shared.txt", "feature conflicting version\n");
        git.stage("shared.txt").unwrap();
        git.commit("Feature line changes").unwrap();
        git.checkout("main").unwrap();
        assert!(git.merge("feature").is_err());
        assert!(git
            .snapshot(100)
            .unwrap()
            .status
            .iter()
            .any(|f| f.index == 'U' && f.worktree == 'U'));
        assert!(git.path.join(".git/MERGE_HEAD").exists());
    }

    /// Leaves `shared.txt` as a `UU` merge conflict on `main`.
    fn conflicted_fixture() -> Fixture {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("shared.txt", "base\n");
        fixture.write("other.txt", "other\n");
        git.stage("shared.txt").unwrap();
        git.stage("other.txt").unwrap();
        git.commit("Root").unwrap();
        git.create_branch("feature").unwrap();
        git.checkout("feature").unwrap();
        fixture.write("shared.txt", "feature\n");
        git.stage("shared.txt").unwrap();
        git.commit("Feature").unwrap();
        git.checkout("main").unwrap();
        fixture.write("shared.txt", "main\n");
        git.stage("shared.txt").unwrap();
        git.commit("Main").unwrap();
        assert!(git.merge("feature").is_err());
        fixture
    }

    #[test]
    fn generic_staging_never_clears_unmerged_entries() {
        let fixture = conflicted_fixture();
        let git = &fixture.git;
        fixture.write("other.txt", "other edited\n");
        fixture.write("new.txt", "arrived\n");
        let unmerged = git.run(&["ls-files", "-u"]).unwrap();
        assert!(!unmerged.is_empty());
        let conflict = git
            .status()
            .unwrap()
            .into_iter()
            .find(|file| file.path == "shared.txt")
            .unwrap();
        assert!(conflict.is_unmerged());
        assert!(!conflict.on_side(true) && !conflict.on_side(false));
        for unstage in [false, true] {
            let error = git.stage_reviewed(&conflict, unstage).unwrap_err();
            assert_eq!(error.kind, FailureKind::Conflict);
        }
        assert!(git.stage("shared.txt").is_err());
        assert_eq!(git.run(&["ls-files", "-u"]).unwrap(), unmerged);

        let staged = git.stage_all(false).unwrap();
        assert_eq!(staged.conflicts, 1);
        assert_eq!(staged.changed, 2);
        assert_eq!(git.run(&["ls-files", "-u"]).unwrap(), unmerged);
        assert_eq!(git.run(&["show", ":other.txt"]).unwrap(), "other edited\n");
        assert_eq!(git.run(&["show", ":new.txt"]).unwrap(), "arrived\n");
        let snapshot = git.snapshot(10).unwrap();
        let staged_paths: Vec<_> = snapshot
            .side(true)
            .into_iter()
            .map(|i| snapshot.status[i].path.as_str())
            .collect();
        assert_eq!(staged_paths, ["new.txt", "other.txt"]);
        assert_eq!(snapshot.conflicts().len(), 1);
        assert_eq!(snapshot.operation_label().as_deref(), Some("merge"));
        assert!(snapshot
            .commit_blocker()
            .is_some_and(|reason| reason.contains("1 conflicted file")));
        assert!(snapshot.operation_recovery().contains("git merge --abort"));

        let unstaged = git.stage_all(true).unwrap();
        assert_eq!(unstaged.conflicts, 1);
        assert_eq!(git.run(&["ls-files", "-u"]).unwrap(), unmerged);
        assert_eq!(git.run(&["show", ":other.txt"]).unwrap(), "other\n");
        assert!(git.run(&["ls-files", "--", "new.txt"]).unwrap().is_empty());
        assert!(git
            .snapshot(10)
            .unwrap()
            .status
            .iter()
            .any(|f| f.path == "shared.txt" && f.index == 'U' && f.worktree == 'U'));
    }

    #[test]
    fn mark_resolved_refuses_conflict_markers_unless_confirmed() {
        let fixture = conflicted_fixture();
        let git = &fixture.git;
        let unmerged = git.run(&["ls-files", "-u"]).unwrap();
        let refused = git.mark_resolved("shared.txt", false).unwrap_err();
        assert_eq!(refused.kind, FailureKind::ConflictMarkers);
        assert!(refused.message.contains("line 1"));
        assert_eq!(git.run(&["ls-files", "-u"]).unwrap(), unmerged);
        assert!(git.mark_resolved("other.txt", false).is_err());
        assert!(git.mark_resolved("../shared.txt", false).is_err());

        fixture.write("shared.txt", "main and feature\n");
        git.mark_resolved("shared.txt", false).unwrap();
        assert!(git.run(&["ls-files", "-u"]).unwrap().is_empty());
        assert_eq!(
            git.run(&["show", ":shared.txt"]).unwrap(),
            "main and feature\n"
        );
        assert!(git.mark_resolved("shared.txt", false).is_err());

        let fixture = conflicted_fixture();
        let git = &fixture.git;
        git.mark_resolved("shared.txt", true).unwrap();
        assert!(git.run(&["ls-files", "-u"]).unwrap().is_empty());
        assert!(git
            .run(&["show", ":shared.txt"])
            .unwrap()
            .contains("<<<<<<<"));

        let fixture = conflicted_fixture();
        let git = &fixture.git;
        std::fs::remove_file(git.path.join("shared.txt")).unwrap();
        git.mark_resolved("shared.txt", false).unwrap();
        assert!(git
            .run(&["ls-files", "--", "shared.txt"])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn conflict_markers_need_an_opening_or_closing_line() {
        assert_eq!(conflict_marker_line(b"Title\n=======\nbody\n"), None);
        assert_eq!(
            conflict_marker_line(b"a\r\n<<<<<<< HEAD\r\nb\r\n=======\r\nc\r\n>>>>>>> topic\r\n"),
            Some(2)
        );
        assert_eq!(
            conflict_marker_line(b"x\n=======\ny\n>>>>>>> topic\n"),
            Some(2)
        );
        assert_eq!(conflict_marker_line(b"<<<<<<<< not a marker\n"), None);
    }

    #[test]
    fn statuses_have_names_and_rename_sources() {
        let renamed = FileChange {
            path: "new.txt".into(),
            original_path: Some("old.txt".into()),
            index: 'R',
            worktree: 'M',
            unsupported_path: false,
        };
        assert_eq!(renamed.status_name(true), "Renamed");
        assert_eq!(renamed.status_name(false), "Modified");
        assert_eq!(renamed.describe(true), "Renamed: old.txt → new.txt");
        let conflict = FileChange {
            path: "c.txt".into(),
            index: 'U',
            worktree: 'D',
            ..Default::default()
        };
        assert_eq!(conflict.status_name(false), "Deleted by them");
        let untracked = FileChange {
            path: "u.txt".into(),
            index: '?',
            worktree: '?',
            ..Default::default()
        };
        assert_eq!(untracked.status_name(false), "Untracked");
    }

    #[test]
    fn detached_and_unborn_snapshots_explain_commit_routes() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("first.txt", "first\n");
        let unborn = git.snapshot(10).unwrap();
        assert_eq!(unborn.head_label(), "main · no commits");
        assert!(!unborn.is_detached());
        git.stage("first.txt").unwrap();
        assert_eq!(git.snapshot(10).unwrap().commit_blocker(), None);
        git.commit("First").unwrap();
        git.run(&["switch", "--detach", "HEAD"]).unwrap();
        fixture.write("first.txt", "changed\n");
        git.stage("first.txt").unwrap();
        let detached = git.snapshot(10).unwrap();
        assert!(detached.is_detached());
        assert!(detached
            .commit_blocker()
            .is_some_and(|reason| reason.contains("Create a branch")));
    }

    #[test]
    fn real_git_rename_unstage_restores_both_index_paths_and_stale_stage_all_is_safe() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let original="Original component\nKeep this content to identify the renamed file.\nMore stable content.\n";
        fixture.write("old [name].txt", original);
        git.stage("old [name].txt").unwrap();
        git.commit("Track original name").unwrap();
        git.run(&["mv", "--", "old [name].txt", "new [name].txt"])
            .unwrap();
        let changed = format!("{original}Additional unstaged line\n");
        fixture.write("new [name].txt", &changed);
        git.stage("new [name].txt").unwrap();
        let state = git.snapshot(100).unwrap();
        assert_eq!(state.status.len(), 1);
        assert_eq!(state.status[0].index, 'R');
        assert_eq!(state.status[0].worktree, ' ');
        assert_eq!(
            state.status[0].original_path.as_deref(),
            Some("old [name].txt")
        );
        git.unstage("new [name].txt").unwrap();
        assert!(git
            .run(&["diff", "--cached", "--name-only"])
            .unwrap()
            .is_empty());
        assert!(!git.path.join("old [name].txt").exists());
        assert_eq!(
            std::fs::read_to_string(git.path.join("new [name].txt")).unwrap(),
            changed
        );
        for _ in 0..2 {
            git.stage_all(false).unwrap();
        }
        assert_eq!(git.snapshot(100).unwrap().status[0].index, 'R');
        git.unstage("new [name].txt").unwrap();
        assert!(git
            .run(&["diff", "--cached", "--name-only"])
            .unwrap()
            .is_empty());
    }
    #[test]
    fn single_file_rejects_directory_replacement_but_bulk_uses_live_scope() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("tracked", "base\n");
        git.stage("tracked").unwrap();
        git.commit("base").unwrap();
        fixture.write("tracked", "reviewed edit\n");
        let review = git.status().unwrap().remove(0);
        let head = git.write_identity().unwrap();
        std::fs::remove_file(git.path.join("tracked")).unwrap();
        std::fs::create_dir(git.path.join("tracked")).unwrap();
        fixture.write("tracked/arrived.txt", "new nested file\n");
        fixture.write("late.txt", "appeared after review\n");
        assert!(git.stage_reviewed(&review, false).is_err());
        assert_eq!(git.write_identity().unwrap(), head);
        assert_eq!(git.run(&["show", ":tracked"]).unwrap(), "base\n");
        git.stage_all(false).unwrap();
        assert_eq!(
            git.run(&["show", ":tracked/arrived.txt"]).unwrap(),
            "new nested file\n"
        );
        assert_eq!(
            git.run(&["show", ":late.txt"]).unwrap(),
            "appeared after review\n"
        );
        git.stage_all(true).unwrap();
        assert_eq!(git.write_identity().unwrap(), head);
        assert_eq!(
            std::fs::read(git.path.join("tracked/arrived.txt")).unwrap(),
            b"new nested file\n"
        );
    }

    #[test]
    fn rename_source_can_be_staged_as_an_independent_worktree_path() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("old.txt", "original\n");
        git.stage("old.txt").unwrap();
        git.commit("base").unwrap();
        let head = git.write_identity().unwrap().head;
        git.run(&["mv", "old.txt", "new.txt"]).unwrap();
        fixture.write("new.txt", "unrelated change in rename destination\n");
        fixture.write("old.txt", "independent worktree file\n");
        let review = git
            .status()
            .unwrap()
            .into_iter()
            .find(|file| file.path == "old.txt" && file.on_side(false))
            .unwrap();
        git.stage_reviewed(&review, false).unwrap();
        assert_eq!(git.write_identity().unwrap().head, head);
        assert_eq!(
            git.run(&["show", ":old.txt"]).unwrap(),
            "independent worktree file\n"
        );
        assert_eq!(git.run(&["show", ":new.txt"]).unwrap(), "original\n");
        assert_eq!(
            std::fs::read(git.path.join("new.txt")).unwrap(),
            b"unrelated change in rename destination\n"
        );
    }

    #[test]
    fn staged_mode_and_rename_hunks_cannot_change_the_index() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let before = (0..40).map(|i| format!("line {i}\n")).collect::<String>();
        let after = before
            .replace("line 1\n", "first edit\n")
            .replace("line 35\n", "second edit\n");
        fixture.write("original.txt", &before);
        git.stage("original.txt").unwrap();
        git.commit("base").unwrap();
        fixture.write("original.txt", &after);
        git.stage("original.txt").unwrap();
        git.run(&["update-index", "--chmod=+x", "original.txt"])
            .unwrap();
        let hunks = git.hunks("original.txt", true).unwrap();
        assert_eq!(hunks.len(), 2);
        let reviewed = git.write_identity().unwrap();
        assert!(git
            .apply_hunk(&hunks[0], true)
            .unwrap_err()
            .message
            .contains("whole-file"));
        assert_eq!(git.write_identity().unwrap(), reviewed);
        assert_eq!(git.run(&["show", ":original.txt"]).unwrap(), after);
        assert!(git.run(&["ls-files", "-s"]).unwrap().starts_with("100755 "));

        git.run(&["mv", "original.txt", "renamed.txt"]).unwrap();
        let hunks = git.hunks("renamed.txt", true).unwrap();
        assert_eq!(hunks.len(), 2);
        let reviewed = git.write_identity().unwrap();
        assert!(git
            .apply_hunk(&hunks[0], true)
            .unwrap_err()
            .message
            .contains("whole-file"));
        assert_eq!(git.write_identity().unwrap(), reviewed);
        assert!(git.run(&["show", ":original.txt"]).is_err());
        assert_eq!(git.run(&["show", ":renamed.txt"]).unwrap(), after);
        assert!(!git.path.join("original.txt").exists());
        assert_eq!(
            std::fs::read_to_string(git.path.join("renamed.txt")).unwrap(),
            after
        );
    }

    #[test]
    fn deletion_hunks_require_whole_file_actions_in_both_directions() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("deleted.txt", "delete me\n");
        git.stage("deleted.txt").unwrap();
        git.commit("base").unwrap();
        std::fs::remove_file(git.path.join("deleted.txt")).unwrap();
        let head = git.write_identity().unwrap();
        let hunk = git.hunks("deleted.txt", false).unwrap().remove(0);
        assert!(git
            .apply_hunk(&hunk, false)
            .unwrap_err()
            .message
            .contains("whole-file"));
        assert_eq!(git.write_identity().unwrap(), head);
        assert_eq!(git.run(&["show", ":deleted.txt"]).unwrap(), "delete me\n");
        git.stage("deleted.txt").unwrap();
        let staged = git.write_identity().unwrap();
        let hunk = git.hunks("deleted.txt", true).unwrap().remove(0);
        assert!(git
            .apply_hunk(&hunk, true)
            .unwrap_err()
            .message
            .contains("whole-file"));
        assert_eq!(git.write_identity().unwrap(), staged);
        assert!(git.run(&["show", ":deleted.txt"]).is_err());
        assert!(!git.path.join("deleted.txt").exists());
    }

    #[test]
    fn machine_patches_round_trip_under_user_diff_settings() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let old = "first\n\nold\n\nlast\n";
        let new = "first\n\nnew\n\nlast\n";
        fixture.write(".gitattributes", "*.txt diff=custom\n");
        fixture.write("tracked.txt", old);
        git.stage(".").unwrap();
        git.commit("base").unwrap();
        let head = git.write_identity().unwrap();
        git.run(&["config", "--global", "diff.noprefix", "true"])
            .unwrap();
        for (key, value) in [
            ("diff.noprefix", "true"),
            ("diff.srcprefix", "custom-source/"),
            ("diff.dstprefix", "custom-destination/"),
            ("diff.context", "0"),
            ("diff.suppressBlankEmpty", "true"),
            ("diff.submodule", "diff"),
            ("diff.custom.textconv", "gitcrab-test-command-must-not-run"),
        ] {
            git.run(&["config", key, value]).unwrap();
        }
        let root = git.diff(Some(&head.head), "tracked.txt", false).unwrap();
        assert!(
            root.starts_with("diff --git a/tracked.txt b/tracked.txt\n"),
            "{root}"
        );
        fixture.write("tracked.txt", new);
        fixture.write("untracked.txt", new);
        for path in ["tracked.txt", "untracked.txt"] {
            let hunks = git.hunks(path, false).unwrap();
            assert_eq!(hunks.len(), 1);
            assert!(hunks[0].patch.contains(&format!("+++ b/{path}\n")));
            if path == "tracked.txt" {
                assert!(hunks[0].patch.contains("\n \n-old\n+new\n \n"));
            }
            git.apply_hunk(&hunks[0], false).unwrap();
            assert_eq!(git.run(&["show", &format!(":{path}")]).unwrap(), new);
            let staged = git.hunks(path, true).unwrap();
            git.apply_hunk(&staged[0], true).unwrap();
            assert_eq!(git.write_identity().unwrap(), head);
            assert_eq!(std::fs::read_to_string(git.path.join(path)).unwrap(), new);
        }
    }

    #[test]
    fn broken_head_and_corrupt_index_are_errors_instead_of_empty_identities() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let original = std::fs::read(git.path.join(".git/HEAD")).unwrap();
        fixture.write(".git/HEAD", "not a reference\n");
        assert!(git.write_identity().is_err());
        std::fs::write(git.path.join(".git/HEAD"), original).unwrap();
        fixture.write(".git/index", "invalid index\n");
        assert!(git.write_identity().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn unstaged_mode_and_content_hunks_preserve_the_executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("file.txt", "base\n");
        git.stage("file.txt").unwrap();
        git.commit("base").unwrap();
        fixture.write("file.txt", "edited\n");
        std::fs::set_permissions(
            git.path.join("file.txt"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let before = git.write_identity().unwrap();
        let hunk = git.hunks("file.txt", false).unwrap().remove(0);
        assert!(git
            .apply_hunk(&hunk, false)
            .unwrap_err()
            .message
            .contains("whole-file"));
        assert_eq!(git.write_identity().unwrap(), before);
        assert_eq!(
            std::fs::metadata(git.path.join("file.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(git.run(&["show", ":file.txt"]).unwrap(), "base\n");
        assert_eq!(
            std::fs::read(git.path.join("file.txt")).unwrap(),
            b"edited\n"
        );
    }

    #[test]
    fn real_git_new_file_hunk_can_be_staged_and_unstaged_without_touching_worktree() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let name = "new [file] with spaces.txt";
        let content = "First untracked line\nSecond untracked line\n";
        fixture.write(name, content);
        let before = git.write_identity().unwrap();
        let hunks = git.hunks(name, false).unwrap();
        assert_eq!(hunks.len(), 1);
        git.apply_hunk(&hunks[0], false).unwrap();
        assert_eq!(git.snapshot(100).unwrap().status[0].index, 'A');
        assert_eq!(git.run(&["show", &format!(":{name}")]).unwrap(), content);
        assert!(git
            .run(&["ls-files", "--stage"])
            .unwrap()
            .starts_with("100644 "));
        assert_eq!(git.write_identity().unwrap().head, before.head);
        let staged = git.hunks(name, true).unwrap();
        git.apply_hunk(&staged[0], true).unwrap();
        assert_eq!(git.snapshot(100).unwrap().status[0].index, '?');
        assert_eq!(git.write_identity().unwrap(), before);
        assert_eq!(
            std::fs::read_to_string(git.path.join(name)).unwrap(),
            content
        );
    }
}
