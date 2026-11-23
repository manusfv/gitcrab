//! Requests, replies and the worker interface for Git work.
//!
//! The UI sends a [`Request`] through a [`GitWorker`] and applies the
//! [`Response`]. The worker that runs Git implements these traits in the
//! `gitcrab-git` crate. Requests and replies can be serialized for internal
//! diagnostics.
use crate::{
    diff,
    git::{FailureKind, RepoSnapshot},
    graph,
    platform::RepositoryLocation,
    presentation::short_id,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::sync::oneshot;

/// Work to perform in the worker's repository session.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "args")]
#[expect(
    clippy::large_enum_variant,
    reason = "one-shot request moved through a channel; boxing buys nothing measurable"
)]
pub enum Action {
    /// Opens a confirmed worktree and reads its initial snapshot in a new epoch.
    Open {
        limit: usize,
        #[serde(default)]
        trust: RepositoryTrust,
    },
    /// Rereads history and working changes at the requested history limit.
    Refresh { limit: usize },
    /// Refreshes working changes and reuses history if its inputs still match.
    RefreshWorktree { limit: usize },
    /// Extends the loaded history to a new total limit, reusing valid cached data.
    LoadMore { limit: usize },
    /// Reads the files changed by the selected commit.
    Details { id: String },
    /// Reads a patch for a commit, or for staged or unstaged changes if `id` is absent.
    Diff {
        id: Option<String>,
        path: String,
        staged: bool,
    },
    /// Updates the index for a file whose status still matches the reviewed entry.
    StageFile {
        reviewed: crate::git::FileChange,
        unstage: bool,
    },
    /// Stages or unstages changes across the worktree, leaving conflict entries in place.
    StageAll { unstage: bool },
    /// Stages the working-tree version of an unmerged path as its resolution.
    /// Conflict markers are refused unless `allow_markers` is set.
    MarkResolved {
        path: String,
        #[serde(default)]
        allow_markers: bool,
    },
    /// Stages or unstages a zero-based hunk from the patch identified by `token`.
    Hunk {
        token: u32,
        index: usize,
        unstage: bool,
    },
    /// Commits the staged changes after verifying `reviewed` is present and current.
    Commit {
        message: String,
        #[serde(default)]
        reviewed: Option<crate::git::WriteIdentity>,
    },
    /// Reverses a zero-based hunk from the current unstaged patch in the working file.
    DiscardHunk { token: u32, index: usize },
    /// Stages or unstages only the selected changed lines of the patch
    /// identified by `token`. `rows` index its displayed lines.
    Lines {
        token: u32,
        rows: Vec<usize>,
        unstage: bool,
    },
    /// Reverses only the selected changed lines of the current unstaged
    /// patch in the working file.
    DiscardLines { token: u32, rows: Vec<usize> },
    /// Resolves an operation's target against the reviewed destination before confirmation.
    ReviewOperation {
        op: Operation,
        target: String,
        destination: crate::git::WriteIdentity,
    },
    /// Runs the confirmed operation after revalidating its target and destination.
    Operation { reviewed: ReviewedOperation },
    /// Resolves a local operation against `destination` and runs it in the
    /// same worker request, so a one-click operation reads its target once.
    /// Pull, Push, Undo and Redo use [`Self::ReviewOperation`] first.
    RunOperation {
        op: Operation,
        target: String,
        destination: crate::git::WriteIdentity,
    },
    /// Fetches every remote with pruning. Runs off the worker and can be cancelled.
    Fetch,
}

/// Worker read category used for cancellation and replacement.
#[derive(Clone, Copy)]
pub enum ReadKind {
    /// Initial open, refresh or history paging.
    Snapshot = 0,
    /// File list for a selected commit.
    Details = 1,
    /// Patch for a selected file.
    Diff = 2,
    /// Target resolution before a write is confirmed.
    OperationReview = 3,
}

impl Action {
    /// Short name of a write for activity records and error headings.
    pub fn label(&self) -> &'static str {
        match self {
            Self::StageFile { unstage: false, .. } => "Stage file",
            Self::StageFile { unstage: true, .. } => "Unstage file",
            Self::StageAll { unstage: false } => "Stage all",
            Self::StageAll { unstage: true } => "Unstage all",
            Self::MarkResolved { .. } => "Mark resolved",
            Self::Hunk { unstage: false, .. } => "Stage hunk",
            Self::Hunk { unstage: true, .. } => "Unstage hunk",
            Self::Commit { .. } => "Commit",
            Self::DiscardHunk { .. } => "Discard hunk",
            Self::Lines { unstage: false, .. } => "Stage lines",
            Self::Lines { unstage: true, .. } => "Unstage lines",
            Self::DiscardLines { .. } => "Discard lines",
            Self::Operation { reviewed } => reviewed.title(),
            Self::RunOperation { op, .. } => op.label(),
            Self::Fetch => "Fetch",
            Self::Open { .. } => "Open repository",
            Self::Refresh { .. } | Self::RefreshWorktree { .. } => "Refresh",
            Self::LoadMore { .. } => "Load more history",
            Self::Details { .. } => "Load commit details",
            Self::Diff { .. } => "Load changes",
            Self::ReviewOperation { op, .. } => op.label(),
        }
    }

    /// What a write acts on, when the action itself names it.
    pub fn target(&self) -> Option<String> {
        match self {
            Self::StageFile { reviewed, .. } => Some(reviewed.display_path()),
            Self::MarkResolved { path, .. } => Some(crate::git::display_path(path)),
            Self::StageAll { .. } => Some("whole worktree".into()),
            Self::Commit { message, .. } => message.lines().next().map(str::to_owned),
            Self::Fetch => Some("all remotes".into()),
            Self::RunOperation { target, .. } if !target.trim().is_empty() => Some(target.clone()),
            Self::Operation { reviewed } if reviewed.network.is_some() => reviewed.refs(),
            Self::Operation {
                reviewed:
                    ReviewedOperation {
                        journal: Some(step),
                        ..
                    },
            } => Some(step.entry.summary.clone()),
            Self::Operation { reviewed } => Some(match &reviewed.target_id {
                Some(id)
                    if reviewed.target.is_empty() || reviewed.target.starts_with(short_id(id)) =>
                {
                    short_id(id).to_owned()
                }
                Some(id) => format!("{} ({})", reviewed.target, short_id(id)),
                None => reviewed.target.clone(),
            }),
            _ => None,
        }
    }

    /// Whether the action contacts a remote: Fetch, or a reviewed Pull or Push.
    /// These run beside the worker without a time budget and can be cancelled
    /// with [`Service::cancel_network`].
    pub fn is_network(&self) -> bool {
        match self {
            Self::Fetch => true,
            Self::Operation { reviewed } => reviewed.op.is_network(),
            _ => false,
        }
    }

    pub fn read_kind(&self) -> Option<ReadKind> {
        match self {
            Self::Open { .. }
            | Self::Refresh { .. }
            | Self::RefreshWorktree { .. }
            | Self::LoadMore { .. } => Some(ReadKind::Snapshot),
            Self::Details { .. } => Some(ReadKind::Details),
            Self::Diff { .. } => Some(ReadKind::Diff),
            Self::ReviewOperation { .. } => Some(ReadKind::OperationReview),
            _ => None,
        }
    }
}

/// User trust decision for the filesystem identity of a local worktree.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum RepositoryTrust {
    /// Approval is still required before the worker can open the repository.
    #[default]
    Unconfirmed,
    /// The user approved these worktree and metadata paths.
    Confirmed(RepositoryLocation),
}
/// Git operation that goes through target review and confirmation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Operation {
    /// Creates a local branch at HEAD.
    CreateBranch,
    /// Creates a local branch at the given revision.
    CreateBranchAt {
        start: String,
    },
    /// Switches to a local branch or checks out a commit with detached HEAD.
    Checkout,
    Merge,
    CherryPick,
    Revert,
    /// Stashes tracked changes with an optional message.
    Stash,
    /// Applies the reviewed stash, then removes its entry from the stash list.
    /// A conflicting apply keeps the entry.
    PopStash,
    /// Pulls the checked-out branch's upstream with the chosen integration.
    Pull {
        #[serde(default)]
        mode: crate::git::PullMode,
    },
    /// Pushes the checked-out branch to its upstream, or sets one. Never forces.
    Push,
    /// Reverses the top entry of the tab's undo journal (`crate::journal`).
    Undo,
    /// Re-applies the top entry of the tab's redo journal.
    Redo,
    /// Abandons the merge, rebase, cherry-pick or revert that the target
    /// names ([`crate::git::Sequence::name`]) and returns to the state
    /// before it started.
    Abort,
    /// Concludes the step the named operation stopped at, without an
    /// editor. A merge, cherry-pick or revert commits with `message`, or
    /// with Git's prepared message when it is `None`.
    Continue {
        #[serde(default)]
        message: Option<String>,
    },
    /// Drops the commit the named rebase or cherry-pick stopped at and goes
    /// on with the rest.
    Skip,
}

/// Operation target and destination resolved by [`Action::ReviewOperation`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewedOperation {
    pub op: Operation,
    /// The original target input, used to revalidate the resolution before writing.
    pub target: String,
    /// HEAD and index state against which the operation was reviewed.
    pub destination: crate::git::WriteIdentity,
    /// Resolved object ID, or `None` for stash creation.
    pub target_id: Option<String>,
    /// Full local branch ref for an attached checkout.
    pub checkout_ref: Option<String>,
    /// Subject and committer date of the resolved target, for people to verify.
    #[serde(default)]
    pub target_summary: Option<crate::git::CommitSummary>,
    /// Branch, remote and refs a Pull or Push was reviewed against.
    #[serde(default)]
    pub network: Option<crate::git::NetworkTarget>,
    /// The journal step an Undo or Redo was reviewed against.
    #[serde(default)]
    pub journal: Option<crate::journal::Step>,
}

/// Explains why Cherry-pick and Revert are unavailable for merge commits.
pub const MERGE_COMMIT_UNSUPPORTED: &str =
    "This is a merge commit. Cherry-pick and Revert need a mainline parent, which GitCrab cannot choose yet. Use Git's -m option from a terminal.";

impl ReviewedOperation {
    /// Source and destination of a reviewed Pull or Push, such as
    /// `origin/main → main`.
    pub fn refs(&self) -> Option<String> {
        let network = self.network.as_ref()?;
        Some(match self.op {
            Operation::Push => format!("{} → {}", network.branch, network.remote_branch()),
            _ => format!("{} → {}", network.remote_branch(), network.branch),
        })
    }

    /// Names the confirmed action, distinguishing a branch switch from a
    /// detached checkout once the target has resolved.
    pub fn title(&self) -> &'static str {
        match self.op {
            Operation::Checkout if self.checkout_ref.is_some() => "Switch to branch",
            Operation::Checkout => "Check out commit in detached HEAD",
            _ => self.op.label(),
        }
    }

    /// Plain-language source and destination, with short IDs to verify.
    ///
    /// `head` is the checked-out branch name, or a short ID when detached.
    pub fn describe(&self, head: &str) -> String {
        let head_id = short_id(&self.destination.head);
        let here = if head_id.is_empty() {
            head.to_owned()
        } else {
            format!("{head} ({head_id})")
        };
        let id = self.target_id.as_deref().map(short_id).unwrap_or_default();
        let target = if self.target.is_empty() || self.target.starts_with(id) {
            id.to_owned()
        } else {
            format!("{} ({id})", self.target)
        };
        match self.op {
            Operation::Checkout => match &self.checkout_ref {
                Some(branch) => format!(
                    "Switch the working tree from {here} to branch {} ({id}).",
                    branch.trim_start_matches("refs/heads/")
                ),
                None => format!(
                    "Check out commit {target} without a branch (detached HEAD). You leave {here}; new commits will not belong to a branch until you create one."
                ),
            },
            Operation::Merge => format!("Merge {target} into {here}."),
            Operation::CherryPick => {
                format!("Apply the changes from commit {target} as a new commit on {here}.")
            }
            Operation::Revert => {
                format!("Create a new commit on {here} that undoes commit {target}.")
            }
            Operation::CreateBranch | Operation::CreateBranchAt { .. } => format!(
                "Create branch {} at {id}. {head} stays checked out.",
                self.target
            ),
            Operation::Pull { mode } => {
                let Some(network) = &self.network else {
                    return String::new();
                };
                format!(
                    "Fetch {} and bring {} into {here}: {}. As last fetched, {} is {} behind and {} ahead.",
                    network.remote,
                    network.remote_branch(),
                    mode.explain(),
                    network.branch,
                    network.behind,
                    network.ahead,
                )
            }
            Operation::Push => {
                let Some(network) = &self.network else {
                    return String::new();
                };
                let commits = match network.ahead {
                    1 => "1 commit".to_owned(),
                    count => format!("{count} commits"),
                };
                if network.set_upstream {
                    format!(
                        "Push {here} to {} as {}. {} has no upstream yet, so this also sets {} as its upstream (git push -u). {commits} not yet on {}. GitCrab never force-pushes.",
                        network.remote,
                        network.remote_branch(),
                        network.branch,
                        network.remote_branch(),
                        network.remote,
                    )
                } else {
                    format!(
                        "Push {here} to {}, {commits} ahead as last fetched. If {} has commits that {} does not, the push is rejected. GitCrab never force-pushes.",
                        network.remote_branch(),
                        network.remote_branch(),
                        network.branch,
                    )
                }
            }
            Operation::Stash => format!("Stash tracked changes from {here}."),
            Operation::PopStash => {
                format!("Pop stash {id}: apply it to the working tree of {here}, then remove it from the stash list. If the changes conflict, the stash stays in the list.")
            }
            Operation::Undo | Operation::Redo => self
                .journal
                .as_ref()
                .map(crate::journal::Step::describe)
                .unwrap_or_default(),
            Operation::Abort => format!(
                "Abort the {} and return {here} to the state before it started.",
                self.target
            ),
            Operation::Continue { .. } => format!("Continue the {} on {here}.", self.target),
            Operation::Skip => format!(
                "Skip the commit the {} stopped at and go on with the rest.",
                self.target
            ),
        }
    }
}

/// Checks a new branch name typed into the branch bar before Git sees it:
/// its syntax for `op`, then whether it clashes with a loaded branch.
///
/// # Errors
///
/// Returns the message to show beside the input.
pub fn validate_new_branch(
    op: &Operation,
    name: &str,
    snapshot: Option<&RepoSnapshot>,
) -> Result<(), String> {
    if let Err(error) = op.validate_target(name) {
        return Err(if name.is_empty() {
            "Type a name for the branch.".into()
        } else {
            error
        });
    }
    match snapshot.and_then(|s| s.branch_name_conflict(name)) {
        Some(conflict) => Err(conflict),
        None => Ok(()),
    }
}

impl Operation {
    /// Dialog title and action name before the target resolves.
    pub fn label(&self) -> &'static str {
        match self {
            Self::CreateBranch | Self::CreateBranchAt { .. } => "Create branch",
            Self::Checkout => "Checkout",
            Self::Merge => "Merge",
            Self::CherryPick => "Cherry-pick",
            Self::Revert => "Revert",
            Self::Stash => "Stash",
            Self::PopStash => "Pop stash",
            Self::Pull { mode } => mode.label(),
            Self::Push => "Push",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Abort => "Abort",
            Self::Continue { .. } => "Continue",
            Self::Skip => "Skip commit",
        }
    }

    /// Whether the operation finishes or abandons the merge, rebase,
    /// cherry-pick or revert its target names.
    pub fn finishes_sequence(&self) -> bool {
        matches!(self, Self::Abort | Self::Continue { .. } | Self::Skip)
    }

    /// The journal stack an Undo or Redo acts on.
    pub fn journal_direction(&self) -> Option<crate::journal::Direction> {
        match self {
            Self::Undo => Some(crate::journal::Direction::Undo),
            Self::Redo => Some(crate::journal::Direction::Redo),
            _ => None,
        }
    }

    /// Whether the operation contacts a remote.
    pub fn is_network(&self) -> bool {
        matches!(self, Self::Pull { .. } | Self::Push)
    }

    /// Whether the dialog asks for a target. Pop stash, Pull and Push act on
    /// the latest stash or the checked-out branch instead.
    pub fn takes_target(&self) -> bool {
        !matches!(
            self,
            Self::PopStash | Self::Pull { .. } | Self::Push | Self::Undo | Self::Redo
        )
    }

    /// Whether the target is a branch or revision the user may pick from a list.
    pub fn takes_revision(&self) -> bool {
        matches!(
            self,
            Self::Checkout | Self::Merge | Self::CherryPick | Self::Revert
        )
    }

    /// Checks the syntax of the dialog input before Git resolves the target.
    ///
    /// # Errors
    ///
    /// Returns a message if a required target is blank, a new branch name
    /// fails the local syntax checks, or a revision looks like a command option
    /// or contains a NUL byte.
    pub fn validate_target(&self, target: &str) -> Result<(), String> {
        if matches!(
            self,
            Self::Stash | Self::PopStash | Self::Pull { .. } | Self::Push | Self::Undo | Self::Redo
        ) {
            return Ok(());
        }
        if self.finishes_sequence() {
            return match crate::git::Sequence::from_name(target) {
                Some(sequence) if matches!(self, Self::Skip) && !sequence.can_skip() => {
                    Err(crate::commands::SKIP_UNSUPPORTED.into())
                }
                Some(_) => Ok(()),
                None => Err(crate::commands::NO_SEQUENCE.into()),
            };
        }
        if target.trim().is_empty() {
            return Err("Enter a target before continuing.".into());
        }
        if matches!(self, Self::CreateBranch | Self::CreateBranchAt { .. }) {
            if target == "@"
                || target.starts_with('-')
                || target.starts_with('/')
                || target.ends_with('/')
                || target.ends_with('.')
                || target.contains("..")
                || target.contains("@{")
                || target.contains("//")
                || target
                    .chars()
                    .any(|c| c.is_control() || " ~^:?*[\\".contains(c))
                || target
                    .split('/')
                    .any(|part| part.starts_with('.') || part.ends_with(".lock"))
            {
                return Err(
                    "Use a valid Git branch name; spaces and revision syntax are not allowed."
                        .into(),
                );
            }
        } else if target.starts_with('-') || target.contains('\0') {
            return Err("Enter a branch or revision, without command options.".into());
        }
        Ok(())
    }
    /// Returns the placeholder for this operation's target input.
    pub fn placeholder(&self) -> &'static str {
        match self {
            Self::CreateBranch | Self::CreateBranchAt { .. } => "feature/branch-name",
            Self::Stash => "Optional message",
            Self::PopStash
            | Self::Pull { .. }
            | Self::Push
            | Self::Undo
            | Self::Redo
            | Self::Abort
            | Self::Continue { .. }
            | Self::Skip => "",
            _ => "Branch name or commit revision",
        }
    }
    /// Returns the explanatory text shown in the confirmation dialog.
    pub fn help(&self) -> &'static str {
        match self {
            Self::CreateBranch | Self::CreateBranchAt { .. } => {
                "Creates a local branch without switching your working tree."
            }
            Self::Stash => "Stashes tracked changes. Untracked files stay in the working tree.",
            Self::PopStash => {
                "Applies the reviewed stash and removes it from the stash list. If the changes conflict, the stash is kept and the conflicts remain visible in Working changes."
            }
            Self::Pull { .. } => {
                "Git signs in through your credential helper or SSH agent. You can cancel while it runs. Conflicts appear in Working changes."
            }
            Self::Push => {
                "Git signs in through your credential helper or SSH agent. You can cancel while it runs. GitCrab never force-pushes."
            }
            Self::Undo | Self::Redo => {
                "Undo history covers GitCrab's own commits, checkouts, new branches, stashes, merges, cherry-picks and reverts in this tab for this session. GitCrab checks that nothing has changed since, again just before running."
            }
            _ => "Git checks the target and your local changes first. If it fails, the error appears here.",
        }
    }
}
/// Request paired with the identifiers needed to reconcile its response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    /// Repository generation. Opening a worktree requires a strictly newer epoch.
    pub epoch: u32,
    /// Caller-assigned request sequence, echoed in the response.
    pub sequence: u32,
    pub action: Action,
}
/// Successful result of an [`Action`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
#[expect(
    clippy::large_enum_variant,
    reason = "one-shot reply moved through a channel; boxing buys nothing measurable"
)]
pub enum Reply {
    /// Canonical worktree path, repository snapshot and its matching graph.
    Snapshot {
        path: String,
        snapshot: RepoSnapshot,
        graph: graph::Graph,
    },
    /// Changed files and message details for the echoed commit ID.
    Details {
        id: String,
        files: Vec<crate::git::FileChange>,
        #[serde(default)]
        body: String,
        #[serde(default)]
        author_email: String,
    },
    /// Resolved operation ready to present for confirmation.
    OperationReview(ReviewedOperation),
    /// Display-ready patch and hunk controls for the echoed selection.
    Diff {
        id: Option<String>,
        path: String,
        staged: bool,
        original_path: Option<String>,
        content: diff::Content,
        /// Titles in patch order for actions addressed by hunk index.
        hunks: Vec<String>,
        /// User-facing reason hunk actions are unavailable for this patch.
        hunk_blocked: Option<String>,
        /// Token required by subsequent actions on this cached patch.
        token: u32,
        /// What the comparison can show. Non-text previews carry no patch lines.
        #[serde(default)]
        preview: diff::Preview,
    },
    /// Completed write and the submitted message if a commit was verified.
    Done {
        message: String,
        committed_draft: Option<String>,
    },
    /// Completed index or working-tree write, with what it changed read
    /// again right after it, so no snapshot refresh has to follow.
    IndexDone {
        message: String,
        /// Working changes and the identity they belong to, or `None` when
        /// they could not be read consistently.
        worktree: Option<WorktreeState>,
        /// For a hunk or line write, a [`Reply::Diff`] of the comparison it
        /// started from. Its token addresses the worker's cached hunks, so
        /// further hunk actions need no reload. `None` when nothing is left.
        diff: Option<Box<Reply>>,
    },
}
/// The parts of a snapshot an index or working-tree write can change.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorktreeState {
    pub status: Vec<crate::git::FileChange>,
    /// HEAD and index identity that `status` was read against.
    pub reviewed: crate::git::WriteIdentity,
    pub journal: crate::journal::JournalStatus,
    /// When the worker started reading. File watcher events from before
    /// then are already reflected.
    #[serde(skip)]
    pub read_started: Option<std::time::Instant>,
}

/// Worker failure reported through [`Response::result`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Failure {
    pub message: String,
    #[serde(default)]
    pub kind: FailureKind,
    /// Whether the current repository should be refreshed after this failure.
    pub refresh: bool,
}
/// Worker result with the request identifiers and timing measurements.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub epoch: u32,
    pub sequence: u32,
    /// Worker execution time in milliseconds, measured after dequeuing.
    pub elapsed_ms: f64,
    /// Time from submission to dequeuing, in milliseconds.
    #[serde(default)]
    pub queue_ms: f64,
    pub result: Result<Reply, Failure>,
}

/// A tab's connection to its Git worker.
///
/// Requests execute serially. Dropping the worker cancels tracked reads and
/// closes submissions; accepted writes can run to completion.
pub trait GitWorker: Send + Sync {
    /// Queues a request without waiting for space or for Git to finish.
    ///
    /// An accepted read cancels the previous read of its kind. An accepted
    /// write cancels all tracked reads. Await the receiver for the result.
    ///
    /// # Errors
    ///
    /// Returns an error if the queue is full or the worker has exited. Git
    /// failures are delivered in the response.
    fn submit(&self, request: Request) -> Result<oneshot::Receiver<Response>, String>;

    /// Signals cancellation of the most recently accepted read of this kind.
    fn cancel_read(&self, kind: ReadKind);

    /// Stops the running Fetch, Pull or Push. The response reports it.
    fn cancel_network(&self);
}

/// Starts Git workers and finds repositories on disk.
pub trait GitBackend: Send + Sync {
    /// Starts a worker for one tab.
    fn worker(&self) -> Box<dyn GitWorker>;

    /// Finds the worktree enclosing `path` for trust confirmation, without
    /// running Git.
    ///
    /// # Errors
    ///
    /// Returns a message if `path` is not inside a Git worktree.
    fn discover(&self, path: &Path) -> Result<RepositoryLocation, String>;

    /// Opens a check of Git's ignore rules for the trusted worktree at
    /// `location`, which the file watcher uses to skip changes to ignored
    /// files such as build output. Blocks while it runs Git, so call it off
    /// the UI thread. `None` means every change refreshes.
    fn ignore_check(&self, _location: &RepositoryLocation) -> Option<Box<dyn IgnoreCheck>> {
        None
    }

    /// Removes and returns the Git calls recorded since the last drain.
    /// Empty unless the backend was built with tracing.
    fn drain_git_calls(&self) -> Vec<GitCall> {
        Vec::new()
    }
}

/// Tells which worktree paths Git ignores, for one repository.
pub trait IgnoreCheck: Send {
    /// Returns the subset of the absolute `paths` that Git's ignore rules
    /// exclude. Tracked files are never in it. Blocks while Git runs.
    ///
    /// # Errors
    ///
    /// Returns a message if Git cannot answer; treat every path as relevant then.
    fn ignored(
        &self,
        paths: &std::collections::BTreeSet<std::path::PathBuf>,
    ) -> Result<std::collections::BTreeSet<std::path::PathBuf>, String>;
}

/// One Git invocation recorded by a diagnostic build: when it started, how
/// long the spawn took and how long the whole call took, in milliseconds
/// since the trace epoch.
#[derive(Clone, Debug)]
pub struct GitCall {
    pub args: String,
    pub thread: String,
    pub start_ms: f64,
    pub spawn_ms: f64,
    pub total_ms: f64,
    pub stdout_bytes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::LocalBranch;

    fn snapshot(branches: &[&str]) -> RepoSnapshot {
        RepoSnapshot {
            branches: branches
                .iter()
                .map(|name| LocalBranch {
                    name: (*name).to_owned(),
                    tip: String::new(),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn new_branch_names_are_checked_for_syntax_then_clashes() {
        let s = snapshot(&["main", "feature/a"]);
        let check = |name: &str| validate_new_branch(&Operation::CreateBranch, name, Some(&s));
        assert_eq!(check("").unwrap_err(), "Type a name for the branch.");
        assert!(check("has space")
            .unwrap_err()
            .contains("valid Git branch name"));
        assert_eq!(
            check("main").unwrap_err(),
            "A branch named main already exists."
        );
        // Refs are files: `feature` cannot coexist with `feature/a`, nor `main/x` with `main`.
        assert!(check("feature").unwrap_err().contains("feature/a"));
        assert!(check("main/x")
            .unwrap_err()
            .contains("existing branch main"));
        assert!(check("feature/b").is_ok());
        assert!(validate_new_branch(&Operation::CreateBranch, "main", None).is_ok());
    }
}
