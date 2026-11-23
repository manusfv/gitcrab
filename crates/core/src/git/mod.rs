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

/// Explains why Stash cannot run when only untracked files changed.
pub const NOTHING_TO_STASH: &str =
    "There are no changes to tracked files to stash. Stash leaves untracked files in the working tree.";

mod model;
mod remote;

pub use model::*;
pub use remote::*;

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
}
