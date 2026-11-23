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

/// Worktree metadata paths paired with the operation names they indicate.
pub const OPERATION_MARKERS: &[(&str, &str)] = &[
    ("MERGE_HEAD", "merge"),
    ("CHERRY_PICK_HEAD", "cherry-pick"),
    ("REVERT_HEAD", "revert"),
    ("rebase-merge", "rebase"),
    ("rebase-apply", "rebase"),
    ("sequencer", "sequencer"),
];

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
}

/// Explains why Stash cannot run when only untracked files changed.
pub const NOTHING_TO_STASH: &str =
    "There are no changes to tracked files to stash. Stash leaves untracked files in the working tree.";

mod model;
mod remote;

pub use model::*;
pub use remote::*;
