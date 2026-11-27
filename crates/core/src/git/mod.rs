//! Repository data and write outcomes shared by the Git worker and the UI.
//!
//! The worker that runs Git lives in the `gitcrab-git` crate. This module
//! holds what it reports: parsed repository types, remote state and write
//! failures.

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

/// The advice that ends every [`FailureKind::GitFailure`] message.
const GIT_FAILURE_ADVICE: &str = "Inspect Working changes before retrying.";

impl WriteError {
    /// Reports a Git command failure and prompts the user to check working changes.
    pub fn git(message: String) -> Self {
        Self {
            kind: FailureKind::GitFailure,
            message: with_advice(&message, GIT_FAILURE_ADVICE),
        }
    }

    /// Reports an uncertain outcome and prompts the user to refresh and inspect.
    pub fn inspect(message: impl std::fmt::Display) -> Self {
        Self {
            kind: FailureKind::Inspect,
            message: with_advice(
                &message.to_string(),
                "Refresh and inspect the repository before retrying.",
            ),
        }
    }

    /// The message without the advice [`Self::git`] added, for wrapping it in
    /// another failure that gives its own.
    pub fn git_text(&self) -> &str {
        self.message
            .strip_suffix(GIT_FAILURE_ADVICE)
            .map_or(self.message.as_str(), str::trim_end)
    }
}

/// Ends `message` as a sentence and adds `advice` after it, on a line of its
/// own when Git's output already spans several lines.
fn with_advice(message: &str, advice: &str) -> String {
    let message = message.trim_end();
    if message.is_empty() {
        return advice.to_owned();
    }
    let stop = if message.ends_with(['.', '!', '?']) {
        ""
    } else {
        "."
    };
    let gap = if message.contains('\n') { "\n" } else { " " };
    format!("{message}{stop}{gap}{advice}")
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

/// Worktree metadata paths paired with the operation names they indicate.
pub const OPERATION_MARKERS: &[(&str, &str)] = &[
    ("MERGE_HEAD", "merge"),
    ("CHERRY_PICK_HEAD", "cherry-pick"),
    ("REVERT_HEAD", "revert"),
    ("rebase-merge", "rebase"),
    ("rebase-apply", "rebase"),
    ("sequencer", "sequencer"),
];

/// Explains why a reviewed write was refused because its target changed.
pub const TARGET_MOVED: &str =
    "The target moved since the repository was last read. Wait for the refresh, then try again.";

/// What to do after a checkout leaves HEAD detached, where commits are blocked.
pub const DETACHED_ADVICE: &str = "Create a branch before committing.";

/// Explains that a popped stash's changes all came back unstaged.
pub const POP_STAGING_DROPPED: &str = "Its staged changes could not be restored on top of the current index, so all of its changes are now unstaged.";

/// Explains why a write is unavailable while Git has stopped part-way.
pub const OPERATION_IN_PROGRESS: &str = "A merge, rebase, cherry-pick or revert is in progress, or the index has conflicts. Finish or abort it first.";

/// A Git command that can stop part-way, leaving the repository to be
/// continued, skipped past or aborted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Sequence {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

impl Sequence {
    const ALL: [Self; 4] = [Self::Rebase, Self::CherryPick, Self::Revert, Self::Merge];

    /// The Git subcommand, which is also the operation's name in
    /// [`RepoSnapshot::operations`].
    pub fn name(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Rebase => "rebase",
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
        }
    }

    /// The name with a capital letter, to start a sentence or a label.
    pub fn title(self) -> &'static str {
        match self {
            Self::Merge => "Merge",
            Self::Rebase => "Rebase",
            Self::CherryPick => "Cherry-pick",
            Self::Revert => "Revert",
        }
    }

    /// The sequence called `name`, as [`Self::name`] spells it.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|sequence| sequence.name() == name)
    }

    /// The sequence to continue or abort for the active operation names.
    /// A rebase can stop on a merge, so it is chosen first.
    pub fn from_operations(operations: &[String]) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|sequence| operations.iter().any(|op| op == sequence.name()))
    }

    /// Whether Git can drop the commit it stopped at and go on with the rest.
    pub fn can_skip(self) -> bool {
        matches!(self, Self::Rebase | Self::CherryPick)
    }

    /// Whether continuing records a commit with a message the user can edit.
    /// A rebase keeps each commit's own message.
    pub fn takes_message(self) -> bool {
        self != Self::Rebase
    }

    /// What Continue does, worded to follow "Continue".
    pub fn continue_effect(self) -> &'static str {
        match self {
            Self::Merge => "commits the merge with the message in the commit box",
            Self::CherryPick => "commits the cherry-pick with the message in the commit box",
            Self::Revert => "commits the revert with the message in the commit box",
            Self::Rebase => "commits the resolution and goes on with the next commit",
        }
    }
}

/// Explains why Stash cannot run when only untracked files changed.
pub const NOTHING_TO_STASH: &str =
    "There are no changes to tracked files to stash. Stash leaves untracked files in the working tree.";

mod model;
mod remote;
mod setup;

pub use model::*;
pub use remote::*;
pub use setup::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_messages_end_git_text_as_a_sentence_and_advise_once() {
        assert_eq!(
            WriteError::git("Git exited with exit status: 1. error: cannot pop".into()).message,
            "Git exited with exit status: 1. error: cannot pop. Inspect Working changes before retrying."
        );
        let multi = WriteError::git(
            "Git exited with exit status: 1. error: Your local changes would be overwritten:\n\tapp.txt\nAborting\n".into(),
        );
        assert_eq!(
            multi.message,
            "Git exited with exit status: 1. error: Your local changes would be overwritten:\n\tapp.txt\nAborting.\nInspect Working changes before retrying."
        );
        assert_eq!(multi.message.matches("Inspect Working changes").count(), 1);
        assert!(multi.git_text().ends_with("Aborting."));
        assert_eq!(
            WriteError::inspect("Outcome uncertain.").message,
            "Outcome uncertain. Refresh and inspect the repository before retrying."
        );
    }

    #[test]
    fn worktree_list_reads_branches_detached_heads_and_prunable_folders() {
        let output = "worktree C:/repo\0HEAD aaa\0branch refs/heads/main\0\0\
            worktree C:/wt/fix\0HEAD bbb\0detached\0\0\
            worktree C:/gone\0HEAD ccc\0branch refs/heads/feat/x\0prunable gitdir file points to non-existent location\0\0";
        let worktrees = parse_worktrees(output);
        assert_eq!(worktrees.len(), 3);
        assert_eq!(worktrees[0].label(), "main");
        assert_eq!(worktrees[1].branch, None);
        assert_eq!(worktrees[1].label(), "fix");
        assert_eq!(worktrees[2].label(), "feat/x");
        assert!(worktrees[2].prunable && !worktrees[0].prunable);
    }

    #[test]
    fn worktree_folder_is_shown_only_when_it_differs_from_the_branch() {
        let worktree = |path: &str, branch: Option<&str>| Worktree {
            path: path.into(),
            branch: branch.map(Into::into),
            ..Default::default()
        };
        assert_eq!(
            worktree("C:/repo", Some("main")).detail(),
            Some("repo".into())
        );
        assert_eq!(worktree("C:/wt/main", Some("main")).detail(), None);
        assert_eq!(worktree("C:/wt/feat__x", Some("feat/x")).detail(), None);
        assert_eq!(worktree("C:/wt/feat/x", Some("feat/x")).detail(), None);
        assert_eq!(
            worktree("C:/wt/fix", Some("feat/x")).detail(),
            Some("fix".into())
        );
        assert_eq!(worktree("C:/wt/fix", None).detail(), None);
    }
}
