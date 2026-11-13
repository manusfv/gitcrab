//! Session undo and redo for GitCrab's own writes.
//!
//! Each repository tab's worker keeps a [`Journal`]: an undo stack and a redo
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
//! [`Journal::record_head_move`] with the destination identity reviewed before
//! the write.
use crate::git::{Git, WriteError, WriteIdentity};
use crate::service::{Operation, ReviewedOperation};
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

    fn verb(self) -> &'static str {
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

/// State read before a write that recording it needs.
#[derive(Clone, Debug, Default)]
pub struct Before {
    stash_top: Option<String>,
    stash_subject: String,
    tracked_clean: bool,
}

impl Before {
    /// Reads what recording `reviewed` will need. Only stash writes read anything.
    pub fn capture(git: &Git, reviewed: &ReviewedOperation) -> Result<Self, String> {
        match reviewed.op {
            Operation::Stash => Ok(Self {
                stash_top: git.ref_commit("refs/stash")?,
                ..Self::default()
            }),
            Operation::PopStash => {
                let id = reviewed.target_id.as_deref().unwrap_or_default();
                let stash_subject = git
                    .stash_entries()?
                    .into_iter()
                    .find(|(entry, _)| entry == id)
                    .map(|(_, subject)| subject)
                    .unwrap_or_default();
                Ok(Self {
                    stash_subject,
                    tracked_clean: !git.has_tracked_changes()?,
                    ..Self::default()
                })
            }
            _ => Ok(Self::default()),
        }
    }
}

/// One tab's undo and redo stacks.
#[derive(Clone, Debug, Default)]
pub struct Journal {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
}

fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

/// `main`, or `detached HEAD at 1234abcd`.
fn place(head_ref: &str, id: &str) -> String {
    match head_ref.strip_prefix("refs/heads/") {
        Some(branch) => branch.to_owned(),
        None if head_ref.is_empty() => format!("detached HEAD at {}", short(id)),
        None => head_ref.to_owned(),
    }
}

fn quoted(text: &str) -> String {
    format!("“{}”", text.lines().next().unwrap_or_default().trim())
}

impl Journal {
    pub fn undo_entries(&self) -> &[Entry] {
        &self.undo
    }

    pub fn redo_entries(&self) -> &[Entry] {
        &self.redo
    }

    fn stack(&self, direction: Direction) -> &Vec<Entry> {
        match direction {
            Direction::Undo => &self.undo,
            Direction::Redo => &self.redo,
        }
    }

    /// The entry the next Undo or Redo acts on.
    pub fn top(&self, direction: Direction) -> Option<&Entry> {
        self.stack(direction).last()
    }

    /// Forgets every entry, for example when the tab opens another repository.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// Records a successful action. A new action clears the redo stack.
    pub fn record(&mut self, entry: Entry) {
        self.redo.clear();
        self.undo.push(entry);
        if self.undo.len() > MAX_ENTRIES {
            self.undo.remove(0);
        }
    }

    /// Records a verified commit made against the `reviewed` HEAD.
    pub fn record_commit(&mut self, reviewed: &WriteIdentity, commit: &str, message: &str) {
        self.record(Entry {
            summary: format!("commit {}", quoted(message)),
            change: Change::Commit {
                head_ref: reviewed.symbolic_head.clone(),
                parent: reviewed.head.clone(),
                commit: commit.to_owned(),
            },
        });
    }

    /// Records that a write moved HEAD's commit without switching branches,
    /// for example a merge, or a pull that updated the checked-out branch.
    ///
    /// `action` is the lowercase action name used in review text, and
    /// `summary` the tooltip summary (`pull into main`). `before` is the
    /// destination identity reviewed before the write. Nothing is recorded
    /// when HEAD did not move. Returns a note for the result message when the
    /// journal could not record the write and was cleared instead.
    pub fn record_head_move(
        &mut self,
        git: &Git,
        action: &str,
        summary: String,
        before: &WriteIdentity,
    ) -> Option<&'static str> {
        let entry = git.head_state().and_then(|(head_ref, after)| {
            if head_ref != before.symbolic_head {
                return Err("HEAD switched branches during the write.".to_owned());
            }
            Ok((after != before.head).then(|| Entry {
                summary,
                change: Change::BranchMoved {
                    action: action.to_owned(),
                    head_ref,
                    before: before.head.clone(),
                    after,
                },
            }))
        });
        self.keep(entry)
    }

    /// Records that `head_ref` (full ref, or empty for detached HEAD) moved
    /// from `before` to `after`, from IDs the caller already verified, for
    /// example a pull's [`crate::git::PullOutcome`]. Does nothing when the
    /// IDs are equal.
    pub fn record_branch_move(
        &mut self,
        action: &str,
        summary: String,
        head_ref: String,
        before: String,
        after: String,
    ) {
        if before != after {
            self.record(Entry {
                summary,
                change: Change::BranchMoved {
                    action: action.to_owned(),
                    head_ref,
                    before,
                    after,
                },
            });
        }
    }

    fn keep(&mut self, entry: Result<Option<Entry>, String>) -> Option<&'static str> {
        match entry {
            Ok(Some(entry)) => {
                self.record(entry);
                None
            }
            Ok(None) => None,
            Err(_) => {
                self.clear();
                Some(JOURNAL_CLEARED)
            }
        }
    }

    /// Records a successful reviewed operation. `before` comes from
    /// [`Before::capture`] before the write ran.
    pub fn record_operation(
        &mut self,
        git: &Git,
        reviewed: &ReviewedOperation,
        before: Result<Before, String>,
    ) -> Option<&'static str> {
        let destination = &reviewed.destination;
        let target_id = reviewed.target_id.clone().unwrap_or_default();
        let label = if reviewed.target.is_empty() || reviewed.target.starts_with(short(&target_id))
        {
            short(&target_id).to_owned()
        } else {
            reviewed.target.clone()
        };
        let here = place(&destination.symbolic_head, &destination.head);
        let moved = |action: &str, summary: String, journal: &mut Self| {
            journal.record_head_move(git, action, summary, destination)
        };
        let entry = match reviewed.op {
            Operation::Merge => {
                return moved("merge", format!("merge of {label} into {here}"), self)
            }
            Operation::CherryPick => {
                return moved("cherry-pick", format!("cherry-pick of {label}"), self)
            }
            Operation::Revert => return moved("revert", format!("revert of {label}"), self),
            Operation::CreateBranch | Operation::CreateBranchAt { .. } => git
                .ref_commit(&format!("refs/heads/{}", reviewed.target))
                .and_then(|tip| match tip {
                    Some(tip) if tip == target_id => Ok(Some(Entry {
                        summary: format!("create branch {}", reviewed.target),
                        change: Change::CreateBranch {
                            name: reviewed.target.clone(),
                            id: target_id.clone(),
                        },
                    })),
                    _ => Err("The new branch could not be verified.".into()),
                }),
            Operation::Checkout => git.head_state().map(|(to_ref, to_id)| {
                (to_ref != destination.symbolic_head || to_id != destination.head).then(|| Entry {
                    summary: format!("checkout of {}", place(&to_ref, &to_id)),
                    change: Change::Checkout {
                        from_ref: destination.symbolic_head.clone(),
                        from_id: destination.head.clone(),
                        to_ref,
                        to_id,
                    },
                })
            }),
            Operation::Stash => before.and_then(|before| {
                let top = git.ref_commit("refs/stash")?;
                Ok(top
                    .filter(|top| Some(top) != before.stash_top.as_ref())
                    .map(|stash| Entry {
                        summary: if reviewed.target.trim().is_empty() {
                            "stash".into()
                        } else {
                            format!("stash {}", quoted(&reviewed.target))
                        },
                        change: Change::Stash {
                            head: destination.head.clone(),
                            stash,
                            message: reviewed.target.clone(),
                        },
                    }))
            }),
            Operation::PopStash => before.and_then(|before| {
                let (_, head) = git.head_state()?;
                let parents = git
                    .commit_parents(&target_id)?
                    .ok_or("The popped stash commit is missing.")?;
                let result = git.capture_tracked_state().map_err(|e| e.message)?;
                Ok(Some(Entry {
                    summary: format!("pop of stash {}", quoted(&before.stash_subject)),
                    change: Change::PopStash {
                        head,
                        stash: target_id.clone(),
                        subject: before.stash_subject,
                        result,
                        restorable: before.tracked_clean && parents.len() <= 2,
                    },
                }))
            }),
            // Undo and Redo move entries between stacks themselves. Writes
            // without an entry are left to the live precondition checks.
            _ => Ok(None),
        };
        self.keep(entry)
    }

    /// Checks the top entry of each stack against the repository.
    pub fn status(&self, git: &Git) -> JournalStatus {
        let preview = |direction| {
            self.top(direction).map(|entry| StepPreview {
                summary: entry.summary.clone(),
                blocked: check(git, &entry.change, direction).unwrap_or(Some(Blocker::Unchecked)),
            })
        };
        // Each check is a few Git calls of its own, so the two run side by side.
        std::thread::scope(|scope| {
            let redo = scope.spawn(|| preview(Direction::Redo));
            let undo = preview(Direction::Undo);
            JournalStatus {
                undo,
                redo: redo.join().unwrap_or(None),
            }
        })
    }

    /// Resolves the top step for the review dialog, after checking that the
    /// repository still matches the reviewed `destination` and the entry.
    pub fn review(
        &self,
        git: &Git,
        direction: Direction,
        destination: WriteIdentity,
    ) -> Result<ReviewedOperation, String> {
        if git.write_identity()? != destination {
            return Err(
                "HEAD or index changed since the repository was last read. Wait for the refresh, then try again.".into(),
            );
        }
        let entry = self.top(direction).ok_or(match direction {
            Direction::Undo => NOTHING_TO_UNDO,
            Direction::Redo => NOTHING_TO_REDO,
        })?;
        if let Some(blocker) = check(git, &entry.change, direction)? {
            return Err(blocker.reason().into());
        }
        Ok(ReviewedOperation {
            op: direction.operation(),
            target: String::new(),
            destination,
            target_id: None,
            checkout_ref: None,
            target_summary: None,
            network: None,
            journal: Some(Step {
                direction,
                entry: entry.clone(),
            }),
        })
    }

    /// Runs a reviewed step after checking the destination, the entry and its
    /// preconditions again, then moves the entry to the other stack.
    pub fn execute(
        &mut self,
        git: &Git,
        reviewed: &ReviewedOperation,
    ) -> Result<String, WriteError> {
        let step = reviewed
            .journal
            .as_ref()
            .ok_or("Review Undo or Redo again.")?;
        if git.write_identity()? != reviewed.destination {
            return Err(
                "HEAD or index changed since the repository was last read. Wait for the refresh, then try again.".into(),
            );
        }
        if self.top(step.direction) != Some(&step.entry) {
            return Err("Undo history changed. Wait for the refresh, then try again.".into());
        }
        if let Some(blocker) = check(git, &step.entry.change, step.direction)? {
            return Err(blocker.reason().into());
        }
        let change = perform(git, &step.entry.change, step.direction)?;
        let entry = Entry {
            summary: step.entry.summary.clone(),
            change,
        };
        match step.direction {
            Direction::Undo => {
                self.undo.pop();
                self.redo.push(entry);
            }
            Direction::Redo => {
                self.redo.pop();
                self.undo.push(entry);
            }
        }
        Ok(format!("{} {}.", step.direction.verb(), step.entry.summary))
    }
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

/// Checks a step's preconditions. `Ok(None)` means it can run.
fn check(git: &Git, change: &Change, direction: Direction) -> Result<Option<Blocker>, String> {
    if !git.operation_markers()?.is_empty() || git.has_unmerged_entries()? {
        return Ok(Some(Blocker::OperationInProgress));
    }
    let undo = direction == Direction::Undo;
    let head = || git.head_state();
    let blocked = match change {
        Change::Commit {
            head_ref,
            parent,
            commit,
        } => {
            let (expected, other) = if undo {
                (commit, parent)
            } else {
                (parent, commit)
            };
            if undo && parent.is_empty() {
                Some(Blocker::RootCommit)
            } else if head()? != (head_ref.clone(), expected.clone()) {
                Some(Blocker::HeadMoved)
            } else if git.commit_parents(other)?.is_none() {
                Some(Blocker::CommitMissing)
            } else {
                None
            }
        }
        Change::Checkout {
            from_ref,
            from_id,
            to_ref,
            to_id,
        } => {
            let ((here_ref, here_id), (there_ref, there_id)) = if undo {
                ((to_ref, to_id), (from_ref, from_id))
            } else {
                ((from_ref, from_id), (to_ref, to_id))
            };
            if there_id.is_empty() {
                Some(Blocker::NoPreviousCommit)
            } else if head()? != (here_ref.clone(), here_id.clone()) {
                Some(Blocker::HeadMoved)
            } else if there_ref.is_empty() {
                git.commit_parents(there_id)?
                    .is_none()
                    .then_some(Blocker::CommitMissing)
            } else {
                match git.ref_commit(there_ref)? {
                    None => Some(Blocker::BranchMissing),
                    Some(tip) if tip != *there_id => Some(Blocker::BranchMoved),
                    Some(_) => None,
                }
            }
        }
        Change::CreateBranch { name, id } => {
            let full = format!("refs/heads/{name}");
            let tip = git.ref_commit(&full)?;
            if undo {
                match tip {
                    None => Some(Blocker::BranchMissing),
                    Some(tip) if tip != *id => Some(Blocker::BranchMoved),
                    Some(_) if head()?.0 == full => Some(Blocker::BranchCheckedOut),
                    Some(_) => None,
                }
            } else if tip.is_some() {
                Some(Blocker::BranchExists)
            } else {
                git.commit_parents(id)?
                    .is_none()
                    .then_some(Blocker::CommitMissing)
            }
        }
        Change::Stash {
            head: at, stash, ..
        } => {
            if head()?.1 != *at {
                Some(Blocker::HeadMoved)
            } else if undo {
                if !git.stash_entries()?.iter().any(|(id, _)| id == stash) {
                    Some(Blocker::StashMissing)
                } else if git.has_tracked_changes()? {
                    Some(Blocker::LocalChanges)
                } else {
                    None
                }
            } else if git.commit_parents(stash)?.is_none() {
                Some(Blocker::CommitMissing)
            } else if !git.tracked_state_matches(stash)? {
                Some(Blocker::TreeChanged)
            } else {
                None
            }
        }
        Change::PopStash {
            head: at,
            stash,
            result,
            restorable,
            ..
        } => {
            let listed = git.stash_entries()?.iter().any(|(id, _)| id == stash);
            if undo && !restorable {
                Some(Blocker::PopNotRestorable)
            } else if head()?.1 != *at {
                Some(Blocker::HeadMoved)
            } else if undo {
                let unchanged = match result {
                    Some(state) => git.tracked_state_matches(state)?,
                    None => !git.has_tracked_changes()?,
                };
                if listed {
                    Some(Blocker::StashPresent)
                } else if git.commit_parents(stash)?.is_none() {
                    Some(Blocker::CommitMissing)
                } else if !unchanged {
                    Some(Blocker::TreeChanged)
                } else {
                    None
                }
            } else if !listed {
                Some(Blocker::StashMissing)
            } else if git.has_tracked_changes()? {
                Some(Blocker::LocalChanges)
            } else {
                None
            }
        }
        Change::BranchMoved {
            head_ref,
            before,
            after,
            ..
        } => {
            let (here, there) = if undo {
                (after, before)
            } else {
                (before, after)
            };
            if there.is_empty() {
                Some(Blocker::NoPreviousCommit)
            } else if head()? != (head_ref.clone(), here.clone()) {
                Some(Blocker::HeadMoved)
            } else if git.commit_parents(there)?.is_none() {
                Some(Blocker::CommitMissing)
            } else if git.has_tracked_changes()? {
                Some(Blocker::LocalChanges)
            } else {
                None
            }
        }
    };
    Ok(blocked)
}

fn verify_head(git: &Git, head_ref: &str, id: &str) -> Result<(), WriteError> {
    let actual = git.head_state().map_err(WriteError::inspect)?;
    if actual != (head_ref.to_owned(), id.to_owned()) {
        return Err(WriteError::inspect(
            "Outcome uncertain. HEAD is not where this step should have left it. Inspect the repository before continuing.",
        ));
    }
    Ok(())
}

/// Runs a checked step and returns the change to file on the other stack.
fn perform(git: &Git, change: &Change, direction: Direction) -> Result<Change, WriteError> {
    let undo = direction == Direction::Undo;
    match change {
        Change::Commit {
            head_ref,
            parent,
            commit,
        } => {
            let target = if undo { parent } else { commit };
            git.reset(target, true)?;
            verify_head(git, head_ref, target)?;
        }
        Change::Checkout {
            from_ref,
            from_id,
            to_ref,
            to_id,
        } => {
            let (there_ref, there_id) = if undo {
                (from_ref, from_id)
            } else {
                (to_ref, to_id)
            };
            git.switch_to(Some(there_ref), there_id)?;
            verify_head(git, there_ref, there_id)?;
        }
        Change::CreateBranch { name, id } => {
            let full = format!("refs/heads/{name}");
            if undo {
                git.delete_branch(name)?;
            } else {
                git.create_branch_at(name, id)?;
            }
            let tip = git.ref_commit(&full).map_err(WriteError::inspect)?;
            if tip.is_some() == undo || tip.is_some_and(|tip| tip != *id) {
                return Err(WriteError::inspect(
                    "Outcome uncertain. The branch is not as this step should have left it.",
                ));
            }
        }
        Change::Stash {
            head,
            stash,
            message,
        } => {
            if undo {
                git.stash_apply(stash, true)?;
                git.drop_stash_object(stash)?;
            } else {
                let before = git.ref_commit("refs/stash").map_err(WriteError::inspect)?;
                git.stash_with_message(message)?;
                let after = git.ref_commit("refs/stash").map_err(WriteError::inspect)?;
                match after {
                    Some(stash) if Some(&stash) != before.as_ref() => {
                        return Ok(Change::Stash {
                            head: head.clone(),
                            stash,
                            message: message.clone(),
                        })
                    }
                    _ => {
                        return Err(WriteError::inspect(
                            "Outcome uncertain. Git did not create a new stash entry.",
                        ))
                    }
                }
            }
        }
        Change::PopStash {
            head,
            stash,
            subject,
            result,
            ..
        } => {
            if undo {
                // Save the changes first, so a failure below never loses them.
                git.stash_store(subject, stash)?;
                let unchanged = match result {
                    Some(state) => git.tracked_state_matches(state),
                    None => git.has_tracked_changes().map(|changed| !changed),
                }
                .map_err(WriteError::inspect)?;
                if !unchanged {
                    return Err(WriteError::inspect(
                        "The stash entry is back in the stash list, but tracked files changed during Undo, so they were left as they are.",
                    ));
                }
                if result.is_some() {
                    git.reset_tracked_to_head()?;
                }
            } else {
                git.stash_apply(stash, false)?;
                git.drop_stash_object(stash)?;
                let result = git.capture_tracked_state()?;
                return Ok(Change::PopStash {
                    head: head.clone(),
                    stash: stash.clone(),
                    subject: subject.clone(),
                    result,
                    restorable: true,
                });
            }
        }
        Change::BranchMoved {
            head_ref,
            before,
            after,
            ..
        } => {
            let target = if undo { before } else { after };
            git.reset_keep(target)?;
            verify_head(git, head_ref, target)?;
        }
    }
    Ok(change.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_repo::Fixture;

    fn base(fixture: &Fixture) -> String {
        fixture.write("tracked.txt", "base\n");
        fixture.git.stage("tracked.txt").unwrap();
        fixture.git.commit("base").unwrap();
        fixture.git.resolve_commit("HEAD").unwrap()
    }

    fn commit(fixture: &Fixture, name: &str, text: &str, message: &str) -> String {
        fixture.write(name, text);
        fixture.git.stage(name).unwrap();
        fixture.git.commit(message).unwrap();
        fixture.git.resolve_commit("HEAD").unwrap()
    }

    fn read(fixture: &Fixture, name: &str) -> String {
        std::fs::read_to_string(fixture.git.path.join(name)).unwrap_or_default()
    }

    fn reviewed(
        op: Operation,
        target: &str,
        destination: WriteIdentity,
        id: Option<String>,
    ) -> ReviewedOperation {
        ReviewedOperation {
            op,
            target: target.into(),
            destination,
            target_id: id,
            checkout_ref: None,
            target_summary: None,
            network: None,
            journal: None,
        }
    }

    fn state(journal: &Journal, git: &Git, direction: Direction) -> StepState {
        journal.status(git).state(direction)
    }

    fn step(journal: &mut Journal, git: &Git, direction: Direction) -> Result<String, String> {
        let reviewed = journal.review(git, direction, git.write_identity().unwrap())?;
        journal
            .execute(git, &reviewed)
            .map_err(|error| error.message)
    }

    fn head(git: &Git) -> (String, String) {
        git.head_state().unwrap()
    }

    #[test]
    fn commit_round_trips_and_a_new_action_clears_redo() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let parent = base(&fixture);
        let mut journal = Journal::default();
        assert_eq!(state(&journal, git, Direction::Undo), StepState::Empty);
        fixture.write("tracked.txt", "second\n");
        git.stage("tracked.txt").unwrap();
        let identity = git.write_identity().unwrap();
        let sha = git.commit_reviewed("second\n\nbody", &identity).unwrap();
        journal.record_commit(&identity, &sha, "second\n\nbody");
        let status = journal.status(git);
        assert_eq!(status.summary(Direction::Undo), Some("commit “second”"));
        assert_eq!(status.state(Direction::Undo), StepState::Ready);

        let reviewed = journal
            .review(git, Direction::Undo, git.write_identity().unwrap())
            .unwrap();
        assert_eq!(reviewed.title(), "Undo");
        assert!(reviewed
            .describe("main")
            .contains("move main back to its parent"));
        assert_eq!(
            journal.execute(git, &reviewed).unwrap(),
            "Undid commit “second”."
        );
        assert_eq!(head(git), ("refs/heads/main".into(), parent.clone()));
        // The commit's changes stay staged; the working tree is untouched.
        assert_eq!(
            git.run(&["diff", "--cached", "--name-only"]).unwrap(),
            "tracked.txt\n"
        );
        assert_eq!(read(&fixture, "tracked.txt"), "second\n");
        assert_eq!(state(&journal, git, Direction::Undo), StepState::Empty);
        assert_eq!(state(&journal, git, Direction::Redo), StepState::Ready);

        step(&mut journal, git, Direction::Redo).unwrap();
        assert_eq!(head(git), ("refs/heads/main".into(), sha.clone()));
        assert!(!git.has_tracked_changes().unwrap());

        // Undo again, then record a new action: Redo is gone.
        step(&mut journal, git, Direction::Undo).unwrap();
        let identity = git.write_identity().unwrap();
        let other = git.commit_reviewed("other", &identity).unwrap();
        journal.record_commit(&identity, &other, "other");
        assert_eq!(state(&journal, git, Direction::Redo), StepState::Empty);
        assert_eq!(journal.undo_entries().len(), 1);
        assert_eq!(
            journal
                .review(git, Direction::Redo, git.write_identity().unwrap())
                .unwrap_err(),
            NOTHING_TO_REDO
        );
    }

    #[test]
    fn external_changes_block_undo_at_availability_and_execution() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        // A root commit is recorded but not undone.
        fixture.write("tracked.txt", "base\n");
        git.stage("tracked.txt").unwrap();
        let unborn = git.write_identity().unwrap();
        let root = git.commit_reviewed("root", &unborn).unwrap();
        let mut journal = Journal::default();
        journal.record_commit(&unborn, &root, "root");
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::RootCommit)
        );

        fixture.write("tracked.txt", "second\n");
        git.stage("tracked.txt").unwrap();
        let identity = git.write_identity().unwrap();
        let sha = git.commit_reviewed("second", &identity).unwrap();
        journal.record_commit(&identity, &sha, "second");
        assert_eq!(state(&journal, git, Direction::Undo), StepState::Ready);
        // Another client moves the branch: Undo is disabled with the reason.
        let other = Git::open(&git.path).unwrap();
        other
            .write(&["commit", "--allow-empty", "-m", "external"])
            .unwrap();
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::HeadMoved)
        );
        assert_eq!(
            journal
                .review(git, Direction::Undo, git.write_identity().unwrap())
                .unwrap_err(),
            Blocker::HeadMoved.reason()
        );
        let moved = git.resolve_commit("HEAD").unwrap();
        assert_ne!(moved, sha);

        // A branch moved between review and execution is caught again.
        let mut journal = Journal::default();
        git.create_branch("topic").unwrap();
        journal.record(Entry {
            summary: "create branch topic".into(),
            change: Change::CreateBranch {
                name: "topic".into(),
                id: moved.clone(),
            },
        });
        let reviewed = journal
            .review(git, Direction::Undo, git.write_identity().unwrap())
            .unwrap();
        other.write(&["branch", "-f", "topic", &sha]).unwrap();
        assert_eq!(
            journal.execute(git, &reviewed).unwrap_err().message,
            Blocker::BranchMoved.reason()
        );
        assert_eq!(git.ref_commit("refs/heads/topic").unwrap(), Some(sha));
        assert_eq!(journal.undo_entries().len(), 1);
        // A HEAD or index change since review needs a fresh review.
        other.write(&["branch", "-f", "topic", &moved]).unwrap();
        fixture.write("tracked.txt", "staged\n");
        git.stage("tracked.txt").unwrap();
        assert!(journal
            .execute(git, &reviewed)
            .unwrap_err()
            .message
            .contains("HEAD or index changed"));
    }

    #[test]
    fn checkout_round_trips_and_needs_the_previous_branch_unchanged() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let start = base(&fixture);
        git.create_branch("feature").unwrap();
        let tip = commit(&fixture, "tracked.txt", "main\n", "main");
        let before = git.write_identity().unwrap();
        git.checkout("feature").unwrap();
        let mut journal = Journal::default();
        let mut op = reviewed(Operation::Checkout, "feature", before, Some(start.clone()));
        op.checkout_ref = Some("refs/heads/feature".into());
        assert_eq!(
            journal.record_operation(git, &op, Before::capture(git, &op)),
            None
        );
        assert_eq!(
            journal.status(git).summary(Direction::Undo),
            Some("checkout of feature")
        );

        step(&mut journal, git, Direction::Undo).unwrap();
        assert_eq!(head(git), ("refs/heads/main".into(), tip.clone()));
        assert_eq!(read(&fixture, "tracked.txt"), "main\n");
        step(&mut journal, git, Direction::Redo).unwrap();
        assert_eq!(head(git), ("refs/heads/feature".into(), start.clone()));
        assert_eq!(read(&fixture, "tracked.txt"), "base\n");

        // The branch to return to moved: Undo is disabled, not redirected.
        git.write(&["branch", "-f", "main", &start]).unwrap();
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::BranchMoved)
        );
        git.write(&["branch", "-D", "main"]).unwrap();
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::BranchMissing)
        );
    }

    #[test]
    fn created_branch_round_trips_unless_checked_out() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let id = base(&fixture);
        let identity = git.write_identity().unwrap();
        git.create_branch_at("topic", &id).unwrap();
        let mut journal = Journal::default();
        let op = reviewed(Operation::CreateBranch, "topic", identity, Some(id.clone()));
        journal.record_operation(git, &op, Before::capture(git, &op));
        assert_eq!(
            journal.status(git).summary(Direction::Undo),
            Some("create branch topic")
        );
        step(&mut journal, git, Direction::Undo).unwrap();
        assert_eq!(git.ref_commit("refs/heads/topic").unwrap(), None);
        // Another client recreates it: Redo is disabled.
        git.write(&["branch", "topic"]).unwrap();
        assert_eq!(
            state(&journal, git, Direction::Redo),
            StepState::Blocked(Blocker::BranchExists)
        );
        git.write(&["branch", "-D", "topic"]).unwrap();
        step(&mut journal, git, Direction::Redo).unwrap();
        assert_eq!(git.ref_commit("refs/heads/topic").unwrap(), Some(id));
        git.checkout("topic").unwrap();
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::BranchCheckedOut)
        );
    }

    #[test]
    fn stash_round_trips_with_its_index_state() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let start = base(&fixture);
        fixture.write("staged.txt", "staged\n");
        git.stage("staged.txt").unwrap();
        fixture.write("tracked.txt", "unstaged\n");
        let identity = git.write_identity().unwrap();
        let op = reviewed(Operation::Stash, "wip", identity, None);
        let before = Before::capture(git, &op);
        git.stash_with_message("wip").unwrap();
        let mut journal = Journal::default();
        journal.record_operation(git, &op, before);
        assert_eq!(
            journal.status(git).summary(Direction::Undo),
            Some("stash “wip”")
        );

        step(&mut journal, git, Direction::Undo).unwrap();
        assert!(git.stash_entries().unwrap().is_empty());
        assert_eq!(read(&fixture, "tracked.txt"), "unstaged\n");
        assert_eq!(
            git.run(&["diff", "--cached", "--name-only"]).unwrap(),
            "staged.txt\n"
        );
        assert_eq!(head(git).1, start);

        step(&mut journal, git, Direction::Redo).unwrap();
        let entries = git.stash_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].1.ends_with("wip"), "{entries:?}");
        assert!(!git.has_tracked_changes().unwrap());
        // The redone entry tracks the new stash object.
        let Change::Stash { stash, .. } = &journal.top(Direction::Undo).unwrap().change else {
            panic!("stash entry expected");
        };
        assert_eq!(*stash, entries[0].0);

        fixture.write("tracked.txt", "later\n");
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::LocalChanges)
        );
    }

    #[test]
    fn popped_stash_returns_only_when_the_tree_holds_exactly_its_changes() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let start = base(&fixture);
        fixture.write("tracked.txt", "stashed\n");
        git.stash_with_message("saved").unwrap();
        let pop = |journal: &mut Journal| {
            let id = git.resolve_commit("refs/stash").unwrap();
            let op = reviewed(
                Operation::PopStash,
                "",
                git.write_identity().unwrap(),
                Some(id.clone()),
            );
            let before = Before::capture(git, &op);
            git.stash_apply(&id, false).unwrap();
            git.drop_stash_object(&id).unwrap();
            journal.record_operation(git, &op, before);
        };
        let mut journal = Journal::default();
        pop(&mut journal);
        assert_eq!(
            journal.status(git).summary(Direction::Undo),
            Some("pop of stash “On main: saved”")
        );
        step(&mut journal, git, Direction::Undo).unwrap();
        let entries = git.stash_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].1, "On main: saved");
        assert_eq!(read(&fixture, "tracked.txt"), "base\n");
        assert!(!git.has_tracked_changes().unwrap());
        assert_eq!(head(git).1, start);

        step(&mut journal, git, Direction::Redo).unwrap();
        assert!(git.stash_entries().unwrap().is_empty());
        assert_eq!(read(&fixture, "tracked.txt"), "stashed\n");

        // Editing after the pop means the changes can no longer be told apart.
        fixture.write("tracked.txt", "stashed and edited\n");
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::TreeChanged)
        );

        // A pop onto other tracked changes is recorded but cannot be undone.
        fixture.write("tracked.txt", "base\n");
        fixture.write("other.txt", "other\n");
        git.stage("other.txt").unwrap();
        git.commit("other").unwrap();
        fixture.write("tracked.txt", "second stash\n");
        git.stash_with_message("second").unwrap();
        fixture.write("other.txt", "local\n");
        pop(&mut journal);
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::PopNotRestorable)
        );
    }

    #[test]
    fn merge_and_revert_reset_back_only_on_a_clean_tree() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        base(&fixture);
        git.create_branch("feature").unwrap();
        git.checkout("feature").unwrap();
        let feature = commit(&fixture, "feature.txt", "feature\n", "feature");
        git.checkout("main").unwrap();
        commit(&fixture, "main.txt", "main\n", "main");
        let before = git.write_identity().unwrap();
        git.merge(&feature).unwrap();
        let merged = git.resolve_commit("HEAD").unwrap();
        let mut journal = Journal::default();
        let op = reviewed(
            Operation::Merge,
            "feature",
            before.clone(),
            Some(feature.clone()),
        );
        journal.record_operation(git, &op, Before::capture(git, &op));
        assert_eq!(
            journal.status(git).summary(Direction::Undo),
            Some("merge of feature into main")
        );
        let review = journal
            .review(git, Direction::Undo, git.write_identity().unwrap())
            .unwrap();
        assert!(review.describe("main").contains("none are discarded"));

        fixture.write("main.txt", "uncommitted\n");
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::LocalChanges)
        );
        fixture.write("main.txt", "main\n");
        step(&mut journal, git, Direction::Undo).unwrap();
        assert_eq!(head(git), ("refs/heads/main".into(), before.head.clone()));
        assert!(!git.path.join("feature.txt").exists());
        step(&mut journal, git, Direction::Redo).unwrap();
        assert_eq!(head(git).1, merged);
        assert_eq!(read(&fixture, "feature.txt"), "feature\n");

        // Revert records the same kind of entry.
        let before = git.write_identity().unwrap();
        git.revert(&feature).unwrap();
        let op = reviewed(
            Operation::Revert,
            &feature,
            before.clone(),
            Some(feature.clone()),
        );
        journal.record_operation(git, &op, Before::capture(git, &op));
        assert_eq!(
            journal.status(git).summary(Direction::Undo),
            Some(format!("revert of {}", &feature[..8]).as_str())
        );
        step(&mut journal, git, Direction::Undo).unwrap();
        assert_eq!(head(git).1, before.head);
        assert_eq!(read(&fixture, "feature.txt"), "feature\n");
    }

    #[test]
    fn a_verified_branch_move_such_as_a_pull_can_be_recorded_and_undone() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let before = base(&fixture);
        let after = commit(&fixture, "pulled.txt", "pulled\n", "upstream");
        let mut journal = Journal::default();
        journal.record_branch_move(
            "pull",
            "pull into main".into(),
            "refs/heads/main".into(),
            before.clone(),
            after.clone(),
        );
        assert_eq!(
            journal.status(git).summary(Direction::Undo),
            Some("pull into main")
        );
        step(&mut journal, git, Direction::Undo).unwrap();
        assert_eq!(head(git).1, before);
        assert!(!git.path.join("pulled.txt").exists());
        step(&mut journal, git, Direction::Redo).unwrap();
        assert_eq!(head(git).1, after);
        // An unchanged branch records nothing.
        journal.record_branch_move(
            "pull",
            "pull".into(),
            "refs/heads/main".into(),
            after.clone(),
            after,
        );
        assert_eq!(journal.undo_entries().len(), 1);
    }
}
