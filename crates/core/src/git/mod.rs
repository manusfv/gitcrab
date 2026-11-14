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

/// Explains why Stash cannot run when only untracked files changed.
pub const NOTHING_TO_STASH: &str =
    "There are no changes to tracked files to stash. Stash leaves untracked files in the working tree.";

mod model;
mod remote;

pub use model::*;
pub use remote::*;
