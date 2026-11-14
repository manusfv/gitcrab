//! Session undo and redo for GitCrab's own writes.
//!
//! Each repository tab's worker keeps a `Journal`: an undo stack and a redo
//! stack of [`Entry`] values recorded after a successful GitCrab write. The
//! journal lives in memory and is cleared when the tab opens a repository.
//! A newly recorded action clears the redo stack.
//!
//! Undo and Redo act only on the top entry of their stack. Before offering a
//! step, before the review dialog shows it, and again immediately before
//! running it, the journal checks that the repository is still exactly as the
//! entry left it ([`Blocker`] names what changed). A step never guesses: when a
//! precondition fails it is disabled with that reason.
//!
//! Recording a write that moved HEAD or a branch, such as a future Pull, uses
//! `Journal::record_head_move` (in `gitcrab-git`) with the destination identity reviewed before
//! the write.
use crate::service::Operation;
use serde::{Deserialize, Serialize};

/// Most entries kept on the undo stack; the oldest are forgotten first.
pub const MAX_ENTRIES: usize = 100;

/// Explains a disabled Undo with an empty stack.
pub const NOTHING_TO_UNDO: &str = "There is nothing to undo. Undo covers commits, checkouts, new branches, stashes, merges, cherry-picks and reverts made in this tab since it opened the repository.";
/// Explains a disabled Redo with an empty stack.
pub const NOTHING_TO_REDO: &str =
    "There is nothing to redo. Redo is available after Undo, until another action is recorded.";
/// Explains why a write left no undo history.
pub const JOURNAL_CLEARED: &str =
    "Undo history for this tab was cleared because GitCrab could not record this action.";

/// Which stack a step comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Undo,
    Redo,
}

impl Direction {
    /// The reviewed operation that runs this step.
    pub fn operation(self) -> Operation {
        match self {
            Self::Undo => Operation::Undo,
            Self::Redo => Operation::Redo,
        }
    }

    pub fn verb(self) -> &'static str {
        match self {
            Self::Undo => "Undid",
            Self::Redo => "Redid",
        }
    }
}

/// What a recorded action changed, with the IDs needed to reverse and re-apply it.
///
/// `head_ref` fields hold the full symbolic ref of HEAD (`refs/heads/main`),
/// or an empty string for detached HEAD.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Change {
    /// A commit on `head_ref`, from `parent` (empty for a root commit) to `commit`.
    Commit {
        head_ref: String,
        parent: String,
        commit: String,
    },
    /// HEAD switched from one branch or commit to another.
    Checkout {
        from_ref: String,
        from_id: String,
        to_ref: String,
        to_id: String,
    },
    /// Local branch `name` was created at `id`.
    CreateBranch { name: String, id: String },
    /// Tracked changes on top of `head` were saved as stash commit `stash`.
    Stash {
        head: String,
        stash: String,
        message: String,
    },
    /// Stash commit `stash`, listed as `subject`, was applied on `head` and dropped.
    ///
    /// `result` records the tracked index and working tree right after the
    /// pop as a stash-shaped commit (`None` when they matched HEAD).
    /// `restorable` is false when tracked files had other changes before the
    /// pop, or the stash carried untracked files, so the pop's changes cannot
    /// be separated from other work.
    PopStash {
        head: String,
        stash: String,
        subject: String,
        result: Option<String>,
        restorable: bool,
    },
    /// HEAD (`head_ref`, or detached) moved from `before` to `after` by
    /// `action`, such as a merge, cherry-pick, revert or pull.
    BranchMoved {
        action: String,
        head_ref: String,
        before: String,
        after: String,
    },
}

/// A recorded action and the summary people see in tooltips and reviews.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Lowercase action summary, such as `commit “Fix parser”`.
    pub summary: String,
    pub change: Change,
}

/// The step a review captured: its direction and the exact top entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub direction: Direction,
    pub entry: Entry,
}

/// Why the top entry cannot be undone or redone now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Blocker {
    HeadMoved,
    BranchMoved,
    BranchMissing,
    BranchExists,
    BranchCheckedOut,
    CommitMissing,
    RootCommit,
    NoPreviousCommit,
    LocalChanges,
    TreeChanged,
    PopNotRestorable,
    StashMissing,
    StashPresent,
    OperationInProgress,
    Unchecked,
}

impl Blocker {
    pub fn reason(self) -> &'static str {
        match self {
            Self::HeadMoved => "HEAD is no longer where this action left it (another commit, checkout or reset happened), so it cannot be reversed safely.",
            Self::BranchMoved => "The branch has moved since this action, so it cannot be reversed safely.",
            Self::BranchMissing => "The branch no longer exists.",
            Self::BranchExists => "A branch with this name exists again.",
            Self::BranchCheckedOut => "The branch is checked out. Switch to another branch first.",
            Self::CommitMissing => "The recorded commit is no longer in this repository.",
            Self::RootCommit => "This is the first commit on its branch. GitCrab does not undo a root commit.",
            Self::NoPreviousCommit => "HEAD had no commit before this action, so there is nothing to return to.",
            Self::LocalChanges => "Tracked files have uncommitted changes. Commit or stash them first, because this step changes the working tree.",
            Self::TreeChanged => "Tracked files changed since this action, so GitCrab cannot tell which changes came from it.",
            Self::PopNotRestorable => "Tracked files had other changes when the stash was popped, or the stash held untracked files, so GitCrab cannot separate the stash's changes from other work.",
            Self::StashMissing => "The stash entry is no longer in the stash list.",
            Self::StashPresent => "The stash entry is already back in the stash list.",
            Self::OperationInProgress => "A merge, rebase, cherry-pick or revert is in progress, or the index has conflicts. Finish or abort it first.",
            Self::Unchecked => "GitCrab could not check the repository. Refresh and try again.",
        }
    }
}

/// Whether a stack's top step can run, for the command model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepState {
    #[default]
    Empty,
    Ready,
    Blocked(Blocker),
}

/// The top step of one stack, checked against the repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepPreview {
    pub summary: String,
    pub blocked: Option<Blocker>,
}

/// Undo and Redo availability carried with each snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalStatus {
    pub undo: Option<StepPreview>,
    pub redo: Option<StepPreview>,
}

impl JournalStatus {
    fn preview(&self, direction: Direction) -> Option<&StepPreview> {
        match direction {
            Direction::Undo => self.undo.as_ref(),
            Direction::Redo => self.redo.as_ref(),
        }
    }

    pub fn state(&self, direction: Direction) -> StepState {
        match self.preview(direction) {
            None => StepState::Empty,
            Some(StepPreview {
                blocked: Some(blocker),
                ..
            }) => StepState::Blocked(*blocker),
            Some(_) => StepState::Ready,
        }
    }

    /// Summary of the top step, such as `commit “Fix parser”`.
    pub fn summary(&self, direction: Direction) -> Option<&str> {
        self.preview(direction).map(|p| p.summary.as_str())
    }
}

pub fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

/// `main`, or `detached HEAD at 1234abcd`.
pub fn place(head_ref: &str, id: &str) -> String {
    match head_ref.strip_prefix("refs/heads/") {
        Some(branch) => branch.to_owned(),
        None if head_ref.is_empty() => format!("detached HEAD at {}", short(id)),
        None => head_ref.to_owned(),
    }
}

pub fn quoted(text: &str) -> String {
    format!("“{}”", text.lines().next().unwrap_or_default().trim())
}

impl Step {
    /// What running this step will do, in plain language with short IDs.
    pub fn describe(&self) -> String {
        let undo = self.direction == Direction::Undo;
        match &self.entry.change {
            Change::Commit {
                head_ref,
                parent,
                commit,
            } => {
                let branch = place(head_ref, commit);
                if undo {
                    format!("Undo commit {} on {branch}: move {branch} back to its parent {} with a soft reset. The commit's changes stay staged and the working tree is not touched, so nothing is discarded. Redo restores the same commit.", short(commit), short(parent))
                } else {
                    format!("Redo commit {} on {branch}: move {branch} from {} forward to that commit again with a soft reset. The index and working tree are not touched; anything staged now that differs from the commit stays staged.", short(commit), short(parent))
                }
            }
            Change::Checkout {
                from_ref,
                from_id,
                to_ref,
                to_id,
            } => {
                let from = place(from_ref, from_id);
                let to = place(to_ref, to_id);
                let (leave, enter) = if undo { (to, from) } else { (from, to) };
                format!("Switch the working tree from {leave} back to {enter}. Git's normal checks for local changes apply: if switching would overwrite them, nothing changes.")
            }
            Change::CreateBranch { name, id } => {
                if undo {
                    format!("Delete branch {name}, which still points at {} where it was created. The checked-out branch does not change, and Redo recreates the branch at the same commit.", short(id))
                } else {
                    format!(
                        "Create branch {name} again at {}. The checked-out branch does not change.",
                        short(id)
                    )
                }
            }
            Change::Stash {
                head,
                stash,
                message,
            } => {
                let message = if message.trim().is_empty() {
                    String::new()
                } else {
                    format!(" {}", quoted(message))
                };
                if undo {
                    format!("Apply stash {}{message} back to the working tree and index on {}, then remove it from the stash list. Tracked files have no other changes, so nothing is overwritten.", short(stash), short(head))
                } else {
                    format!("Stash the tracked changes again{message}. They still match the stash this action created, so the same changes are saved; the new stash entry has a new object ID.")
                }
            }
            Change::PopStash {
                head,
                stash,
                subject,
                ..
            } => {
                if undo {
                    format!("Put stash {} {} back at the top of the stash list, then return tracked files to HEAD {}, removing the changes the pop applied. Tracked files hold exactly what the pop produced, so nothing else is discarded. Untracked files are kept.", short(stash), quoted(subject), short(head))
                } else {
                    format!("Pop stash {} {} again: apply it to the working tree, then remove it from the stash list.", short(stash), quoted(subject))
                }
            }
            Change::BranchMoved {
                action,
                head_ref,
                before,
                after,
            } => {
                let branch = place(head_ref, after);
                if undo {
                    format!("Reset {branch} from {} back to {}, where it was before the {action}. The {action}'s commits leave {branch}; Redo can restore them during this session and Git's reflog keeps them. Tracked files have no uncommitted changes, so none are discarded. The working tree changes to match {}, and Git stops if an untracked file would be overwritten.", short(after), short(before), short(before))
                } else {
                    format!("Move {branch} from {} forward to {} again, as after the {action}. Tracked files have no uncommitted changes. The working tree changes to match {}, and Git stops if an untracked file would be overwritten.", short(before), short(after), short(after))
                }
            }
        }
    }
}
