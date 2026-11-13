//! Request and response types for the dedicated Git worker.
//!
//! Each [`Service`] owns a worker thread and a repository session. Requests and
//! replies can be serialized for internal diagnostics.
use crate::{
    diff,
    git::{FailureKind, Git, HunkRange, RepoSnapshot, WriteError},
    graph,
    platform::RepositoryLocation,
};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{sync_channel, SyncSender},
    Arc, Mutex, Weak,
};
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
    /// Resolves an operation's target against the reviewed destination before confirmation.
    ReviewOperation {
        op: Operation,
        target: String,
        destination: crate::git::WriteIdentity,
    },
    /// Runs the confirmed operation after revalidating its target and destination.
    Operation { reviewed: ReviewedOperation },
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
            Self::Operation { reviewed } => reviewed.title(),
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

    fn read_kind(&self) -> Option<ReadKind> {
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
    /// Applies the reviewed stash and keeps the stash entry.
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
    pub target_summary: Option<TargetSummary>,
    /// Branch, remote and refs a Pull or Push was reviewed against.
    #[serde(default)]
    pub network: Option<crate::git::NetworkTarget>,
    /// The journal step an Undo or Redo was reviewed against.
    #[serde(default)]
    pub journal: Option<crate::journal::Step>,
}

/// Human-readable identity of a resolved commit or stash.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetSummary {
    pub subject: String,
    /// Committer date in Git's ISO-like format.
    pub date: String,
    pub parents: usize,
}

/// Explains why Cherry-pick and Revert are unavailable for merge commits.
pub const MERGE_COMMIT_UNSUPPORTED: &str =
    "This is a merge commit. Cherry-pick and Revert need a mainline parent, which GitCrab cannot choose yet. Use Git's -m option from a terminal.";

fn short_id(id: &str) -> &str {
    &id[..id.len().min(8)]
}

impl ReviewedOperation {
    fn capture(
        git: &Git,
        op: Operation,
        target: String,
        destination: crate::git::WriteIdentity,
    ) -> Result<Self, String> {
        op.validate_target(&target)?;
        if op.journal_direction().is_some() {
            return Err("Undo and Redo are reviewed against the tab's journal.".into());
        }
        if git.write_identity()? != destination {
            return Err(
                "HEAD or index changed. Close the dialog, refresh, and review again.".into(),
            );
        }
        let checkout_ref = if matches!(op, Operation::Checkout) {
            let full = if target.starts_with("refs/heads/") {
                target.clone()
            } else {
                format!("refs/heads/{target}")
            };
            let local_name = full.strip_prefix("refs/heads/").unwrap_or(&target);
            if Operation::CreateBranch.validate_target(local_name).is_ok()
                && git.ref_exists(&full)?
            {
                Some(full)
            } else {
                None
            }
        } else {
            None
        };
        let network = match &op {
            Operation::Pull { .. } => Some(git.pull_target(&destination)?),
            Operation::Push => Some(git.push_target(&destination)?),
            _ => None,
        };
        let target_id = match &op {
            // The upstream tip as last fetched; Pull fetches again before integrating.
            Operation::Pull { .. } => network
                .as_ref()
                .and_then(|network| git.resolve_commit(&network.tracking).ok()),
            Operation::Push => Some(git.resolve_commit("HEAD")?),
            Operation::CreateBranch => Some(git.resolve_commit("HEAD")?),
            Operation::CreateBranchAt { start } => Some(git.resolve_commit(start)?),
            Operation::Checkout | Operation::Merge | Operation::CherryPick | Operation::Revert => {
                Some(git.resolve_commit(checkout_ref.as_deref().unwrap_or(&target))?)
            }
            Operation::PopStash => Some(git.resolve_commit("refs/stash")?),
            Operation::Stash | Operation::Undo | Operation::Redo => None,
        };
        let target_summary = match &target_id {
            Some(id) => Some(summarize_commit(git, id)?),
            None => None,
        };
        if matches!(op, Operation::CherryPick | Operation::Revert)
            && target_summary.as_ref().is_some_and(|s| s.parents > 1)
        {
            return Err(MERGE_COMMIT_UNSUPPORTED.into());
        }
        Ok(Self {
            op,
            target,
            destination,
            target_id,
            checkout_ref,
            target_summary,
            network,
            journal: None,
        })
    }

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
                        "Push {here} to {} · {commits} ahead as last fetched. If {} has commits that {} does not, the push is rejected; GitCrab never force-pushes.",
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
        }
    }

    fn execute(&self, git: &Git) -> Result<(), WriteError> {
        let current = Self::capture(
            git,
            self.op.clone(),
            self.target.clone(),
            self.destination.clone(),
        )?;
        if current.target_id != self.target_id || current.checkout_ref != self.checkout_ref {
            return Err("The target moved. Close the dialog, refresh, and review again.".into());
        }
        let id = self.target_id.as_deref().unwrap_or_default();
        let result = match self.op {
            Operation::Pull { .. } | Operation::Push => {
                Err("Pull and Push run as network operations.".into())
            }
            Operation::Undo | Operation::Redo => {
                Err("Undo and Redo run through the tab's journal.".into())
            }
            Operation::CreateBranch | Operation::CreateBranchAt { .. } => {
                git.create_branch_at(&self.target, id)
            }
            Operation::Checkout => {
                if let Some(branch) = self
                    .checkout_ref
                    .as_deref()
                    .and_then(|name| name.strip_prefix("refs/heads/"))
                {
                    git.write(&["switch", "--", branch]).map(|_| ())
                } else {
                    git.write(&["switch", "--detach", id]).map(|_| ())
                }
            }
            Operation::Merge => git.merge(id),
            Operation::CherryPick => git.cherry_pick(id),
            Operation::Revert => git.revert(id),
            Operation::Stash => git.stash_with_message(&self.target),
            // Apply the reviewed stash object, then drop its entry only after a
            // clean apply. Like `git stash pop`, conflicts keep the stash.
            Operation::PopStash => git
                .write(&["stash", "apply", id])
                .and_then(|_| git.drop_stash_object(id)),
        };
        result.map_err(|error| operation_failure(git, error))
    }

    /// Runs a reviewed Pull or Push after the same revalidation as other
    /// operations: unchanged HEAD and index, the same upstream and refs, and
    /// the same reviewed tip. `scope` cancels Git and its helpers.
    ///
    /// A successful pull returns HEAD before and after.
    fn execute_network(
        &self,
        git: &Git,
        scope: &crate::process::ReadScope,
    ) -> Result<(String, Option<crate::git::PullOutcome>), WriteError> {
        let current = Self::capture(
            git,
            self.op.clone(),
            self.target.clone(),
            self.destination.clone(),
        )?;
        if current.target_id != self.target_id || current.network != self.network {
            return Err("The target moved. Close the dialog, refresh, and review again.".into());
        }
        let network = self
            .network
            .as_ref()
            .ok_or("Review the operation again before running it.")?;
        match self.op {
            Operation::Pull { mode } => {
                let outcome = git
                    .pull(network, mode, scope)
                    .map_err(|error| operation_failure(git, error))?;
                Ok((outcome.message.clone(), Some(outcome)))
            }
            Operation::Push => Ok((git.push(network, scope)?, None)),
            _ => Err("Only Pull and Push run as network operations.".into()),
        }
    }
}

fn summarize_commit(git: &Git, id: &str) -> Result<TargetSummary, String> {
    let output = git.run(&["show", "-s", "--no-notes", "--format=%P%x00%s%x00%ci", id])?;
    let mut fields = output.trim_end_matches('\n').splitn(3, '\0');
    let parents = fields.next().unwrap_or_default().split_whitespace().count();
    Ok(TargetSummary {
        subject: fields.next().unwrap_or_default().to_owned(),
        date: fields.next().unwrap_or_default().to_owned(),
        parents,
    })
}

fn operation_failure(git: &Git, error: WriteError) -> WriteError {
    if error.kind != FailureKind::GitFailure {
        return error;
    }
    let message = error.message;
    match git.has_unmerged_entries() {
        Ok(true) => WriteError {
            kind: FailureKind::Conflict,
            message: format!("Git left conflicts in the index. Inspect Working changes before retrying. {message}"),
        },
        Ok(false) => WriteError::git(message),
        Err(error) => WriteError::inspect(format!("{message} Could not check for conflicts. {error}")),
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
        }
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
            Self::PopStash | Self::Pull { .. } | Self::Push | Self::Undo | Self::Redo => "",
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
                "Git contacts the remote using your credential helper or SSH agent; it never prompts in a hidden terminal. You can cancel while it runs. Conflicts remain visible in Working changes."
            }
            Self::Push => {
                "Git contacts the remote using your credential helper or SSH agent; it never prompts in a hidden terminal. You can cancel while it runs. GitCrab never force-pushes."
            }
            Self::Undo | Self::Redo => {
                "Undo history covers GitCrab's own commits, checkouts, new branches, stashes, merges, cherry-picks and reverts in this tab for this session. GitCrab checks that nothing has changed since, again just before running."
            }
            _ => "Git checks the target and local changes. Failures remain here for correction.",
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
struct Pending {
    request: Request,
    tx: oneshot::Sender<Response>,
    cancelled: Option<Arc<AtomicBool>>,
    queued_at: std::time::Instant,
}
/// Handle to a dedicated Git worker with a bounded request queue.
///
/// Requests execute serially. Dropping the handle cancels tracked reads and
/// closes submissions. Accepted writes can run to completion.
pub struct Service {
    tx: SyncSender<Pending>,
    reads: Mutex<[Option<Weak<AtomicBool>>; 4]>,
    /// Cancellation for the latest Fetch, Pull or Push.
    network: Mutex<Option<Weak<AtomicBool>>>,
}
impl Default for Service {
    fn default() -> Self {
        Self::new()
    }
}
impl Service {
    /// Starts a Git worker with room for eight queued requests.
    ///
    /// # Panics
    ///
    /// Panics if the worker thread cannot be started.
    pub fn new() -> Self {
        let (tx, rx) = sync_channel::<Pending>(8);
        std::thread::Builder::new()
            .name("gitcrab-git".into())
            .spawn(move || {
                let mut session = Session::default();
                for Pending {
                    request,
                    tx,
                    cancelled,
                    queued_at,
                } in rx
                {
                    let queue_ms = queued_at.elapsed().as_secs_f64() * 1000.0;
                    if request.action.is_network() {
                        // Remotes can take minutes. Run beside the worker so
                        // reads continue, with no time budget but cancellable.
                        let scope =
                            crate::process::ReadScope::network(cancelled.unwrap_or_default());
                        match session.network_job(request, scope, queue_ms) {
                            Ok(job) => {
                                // If the thread cannot start, dropping the job
                                // and sender reports a closed worker to the tab.
                                let _ = std::thread::Builder::new()
                                    .name("gitcrab-network".into())
                                    .spawn(move || {
                                        let _ = tx.send(job.run());
                                    });
                            }
                            Err(response) => {
                                let _ = tx.send(*response);
                            }
                        }
                        continue;
                    }
                    let scope = cancelled.map(crate::process::ReadScope::new);
                    let _ = tx.send(session.execute_scoped(request, scope, queue_ms));
                }
            })
            .expect("start Git worker");
        Self {
            tx,
            reads: Default::default(),
            network: Default::default(),
        }
    }
    /// Queues a request without waiting for space or for Git to finish.
    ///
    /// An accepted read cancels the previous read of its kind. An accepted
    /// write cancels all tracked reads. Await the receiver for the worker result.
    ///
    /// # Errors
    ///
    /// Returns an error if the queue is full or the worker has exited. Git
    /// failures are delivered in the response.
    pub fn submit(&self, request: Request) -> Result<oneshot::Receiver<Response>, String> {
        let (tx, rx) = oneshot::channel();
        let kind = request.action.read_kind();
        let network = request.action.is_network();
        let cancelled = (kind.is_some() || network).then(|| Arc::new(AtomicBool::new(false)));
        if network {
            *self.network.lock().unwrap() = cancelled.as_ref().map(Arc::downgrade);
        }
        let mut reads = self.reads.lock().unwrap();
        self.tx
            .try_send(Pending {
                request,
                tx,
                cancelled: cancelled.clone(),
                queued_at: std::time::Instant::now(),
            })
            .map_err(|_| {
                "Git worker is busy. Wait for the current operation and retry.".to_string()
            })?;
        if let Some(kind) = kind {
            let old = reads[kind as usize].replace(Arc::downgrade(cancelled.as_ref().unwrap()));
            if let Some(old) = old.and_then(|old| old.upgrade()) {
                old.store(true, Ordering::Release);
            }
        } else {
            for read in reads.iter().flatten().filter_map(Weak::upgrade) {
                read.store(true, Ordering::Release);
            }
        }
        Ok(rx)
    }

    /// Signals cancellation of the most recently accepted read of this kind.
    /// The request may already have completed when cancellation arrives.
    pub fn cancel_read(&self, kind: ReadKind) {
        if let Some(read) = self.reads.lock().unwrap()[kind as usize]
            .as_ref()
            .and_then(Weak::upgrade)
        {
            read.store(true, Ordering::Release);
        }
    }

    /// Stops the running Fetch, Pull or Push, killing Git and its helper
    /// processes. The response reports the cancellation.
    pub fn cancel_network(&self) {
        if let Some(flag) = self
            .network
            .lock()
            .unwrap()
            .as_ref()
            .and_then(Weak::upgrade)
        {
            flag.store(true, Ordering::Release);
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        for read in self
            .reads
            .get_mut()
            .unwrap()
            .iter()
            .flatten()
            .filter_map(Weak::upgrade)
        {
            read.store(true, Ordering::Release);
        }
        self.cancel_network();
    }
}
#[derive(Default)]
struct Session {
    repo: Option<Git>,
    epoch: u32,
    patch: Option<PatchCache>,
    read_scope: Option<crate::process::ReadScope>,
    history: Option<crate::git::HistoryCache>,
    /// Set while a Fetch, Pull or Push runs beside the worker.
    network_busy: Arc<AtomicBool>,
    /// Undo and redo stacks for this tab's repository. Replaced on open, so a
    /// write finishing for an older repository cannot record into this one.
    journal: Arc<Mutex<crate::journal::Journal>>,
    #[cfg(test)]
    history_layouts: usize,
}

/// A Fetch, Pull or Push ready to run on its own thread.
struct NetworkJob {
    git: Git,
    epoch: u32,
    sequence: u32,
    action: Action,
    scope: crate::process::ReadScope,
    queue_ms: f64,
    busy: Arc<AtomicBool>,
    /// The journal of the repository this job was started for. Opening another
    /// repository replaces the session's journal, so a late pull records here
    /// and never into the new repository's history.
    journal: Arc<Mutex<crate::journal::Journal>>,
}

impl NetworkJob {
    fn run(self) -> Response {
        let start = std::time::Instant::now();
        let result = match &self.action {
            Action::Fetch => self.git.fetch(&self.scope),
            Action::Operation { reviewed } => {
                reviewed
                    .execute_network(&self.git, &self.scope)
                    .map(|(message, pull)| {
                        // A successful pull that moved the branch can be undone
                        // like a merge: Undo resets it back to `before`.
                        if let (Some(pull), Some(target)) = (pull, reviewed.network.as_ref()) {
                            if let Ok(mut journal) = self.journal.lock() {
                                journal.record_branch_move(
                                    "pull",
                                    format!("pull into {}", target.branch),
                                    reviewed.destination.symbolic_head.clone(),
                                    pull.before,
                                    pull.after,
                                );
                            }
                        }
                        message
                    })
            }
            _ => Err("Not a network operation.".into()),
        };
        // Dropping the job clears `busy` before the caller sends this response.
        Response {
            epoch: self.epoch,
            sequence: self.sequence,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
            queue_ms: self.queue_ms,
            result: result
                .map(|message| Reply::Done {
                    message,
                    committed_draft: None,
                })
                .map_err(|error| Failure {
                    message: error.message,
                    kind: error.kind,
                    // Fetch updates remote-tracking refs and a failed pull can
                    // still have fetched or stopped mid-merge.
                    refresh: true,
                }),
        }
    }
}

impl Drop for NetworkJob {
    fn drop(&mut self) {
        self.busy.store(false, Ordering::Release);
    }
}

struct PatchCache {
    token: u32,
    staged: bool,
    path: String,
    source: String,
    hunks: Vec<HunkRange>,
}

const MAX_PREVIEW_BYTES: usize = 8 * 1024 * 1024;
const MAX_PREVIEW_LINES: usize = 50_000;
const MAX_PREVIEW_LINE_BYTES: usize = 8192;

fn preview_fits(patch: &str) -> bool {
    patch.len() <= MAX_PREVIEW_BYTES
        && patch
            .lines()
            .enumerate()
            .all(|(row, line)| row < MAX_PREVIEW_LINES && line.len() <= MAX_PREVIEW_LINE_BYTES)
}

impl Session {
    #[cfg(test)]
    fn execute(&mut self, request: Request) -> Response {
        self.execute_scoped(request, None, 0.0)
    }

    fn execute_scoped(
        &mut self,
        request: Request,
        scope: Option<crate::process::ReadScope>,
        queue_ms: f64,
    ) -> Response {
        self.read_scope = scope.clone();
        if let Some(repo) = self.repo.as_mut() {
            repo.read_scope = scope;
        }
        let start = std::time::Instant::now();
        let Request {
            epoch,
            sequence,
            action,
        } = request;
        let mutation = matches!(
            action,
            Action::StageFile { .. }
                | Action::StageAll { .. }
                | Action::MarkResolved { .. }
                | Action::Hunk { .. }
                | Action::DiscardHunk { .. }
                | Action::Commit { .. }
                | Action::Operation { .. }
        );
        let result = if mutation && self.network_busy.load(Ordering::Acquire) {
            // Pull and Push run beside the worker; writes wait for them.
            Err(WriteError::from(
                "A Fetch, Pull or Push is running. Wait for it to finish or cancel it.",
            ))
        } else {
            self.run(epoch, sequence, action)
        }
        .map_err(|error| Failure {
            message: error.message,
            kind: error.kind,
            refresh: mutation && epoch == self.epoch,
        });
        // A failed write can still change the index or worktree.
        if mutation {
            self.patch = None;
        }
        self.read_scope = None;
        if let Some(repo) = self.repo.as_mut() {
            repo.read_scope = None;
        }
        Response {
            epoch,
            sequence,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
            queue_ms,
            result,
        }
    }
    fn snapshot(&mut self, limit: usize, mode: crate::git::HistoryRead) -> Result<Reply, String> {
        let g = self.repo.as_ref().ok_or("Open a local repository first")?;
        let (mut snapshot, graph, _changed) =
            g.snapshot_cached(limit.clamp(1, 100_000), mode, &mut self.history)?;
        snapshot.journal = self.journal.lock().unwrap().status(g);
        #[cfg(test)]
        if _changed {
            self.history_layouts += 1;
        }
        Ok(Reply::Snapshot {
            path: g.path.to_string_lossy().into_owned(),
            snapshot,
            graph,
        })
    }
    fn run(&mut self, epoch: u32, sequence: u32, action: Action) -> Result<Reply, WriteError> {
        if let Action::Open { limit, trust } = action {
            if epoch <= self.epoch {
                return Err("Obsolete repository request".into());
            }
            self.epoch = epoch;
            self.repo = None;
            self.patch = None;
            self.history = None;
            self.journal = Default::default();
            let RepositoryTrust::Confirmed(location) = trust else {
                return Err(
                    "Confirm that you trust this local repository before opening it.".into(),
                );
            };
            self.repo = Some(Git::open_confirmed(location, self.read_scope.clone())?);
            return Ok(self.snapshot(limit, crate::git::HistoryRead::Full)?);
        }
        if epoch != self.epoch {
            return Err(
                "Repository changed. Retry the operation in the current repository.".into(),
            );
        }
        if let Action::Refresh { limit } = action {
            return Ok(self.snapshot(limit, crate::git::HistoryRead::Full)?);
        }
        if let Action::RefreshWorktree { limit } = action {
            return Ok(self.snapshot(limit, crate::git::HistoryRead::Worktree)?);
        }
        if let Action::LoadMore { limit } = action {
            return Ok(self.snapshot(limit, crate::git::HistoryRead::More)?);
        }
        let g = self.repo.as_ref().ok_or("Open a local repository first")?;
        let reply = match action {
            Action::Details { id } => {
                let (author_email, body) = g.commit_message_details(&id)?;
                Reply::Details {
                    files: g.changed_files(&id)?,
                    id,
                    body,
                    author_email,
                }
            }
            Action::Diff { id, path, staged } => {
                let crate::git::DiffComparison {
                    patch,
                    original_path,
                } = g.compare(id.as_deref(), &path, staged)?;
                self.patch = None;
                let fits = preview_fits(&patch);
                let hunks = if fits && id.is_none() {
                    crate::git::hunk_ranges(&patch)
                } else {
                    vec![]
                };
                let titles = hunks.iter().map(|h| h.title(&patch).to_owned()).collect();
                let hunk_blocked = hunks
                    .iter()
                    .find_map(|hunk| hunk.blocked_reason(&patch))
                    .map(str::to_owned);
                let preview = diff::classify(&patch, fits);
                // Only text patches have displayable lines. Other previews are
                // described by the view instead of synthetic code rows.
                let mut lines = if preview == diff::Preview::Text {
                    diff::parse(&patch)
                } else {
                    vec![]
                };
                if !hunks.is_empty() {
                    self.patch = Some(PatchCache {
                        token: sequence,
                        staged,
                        hunks,
                        path: path.clone(),
                        source: patch,
                    });
                }
                if let Some(first) = lines.iter().position(|l| l.text.starts_with("@@ ")) {
                    lines.drain(..first);
                }
                let content = diff::prepare(lines);
                return Ok(Reply::Diff {
                    id,
                    path,
                    staged,
                    original_path,
                    content,
                    hunks: titles,
                    hunk_blocked,
                    token: sequence,
                    preview,
                });
            }
            Action::StageFile { reviewed, unstage } => {
                g.stage_reviewed(&reviewed, unstage)?;
                Reply::Done {
                    message: format!(
                        "{} {}",
                        if unstage { "Unstaged" } else { "Staged" },
                        reviewed.display_path()
                    ),
                    committed_draft: None,
                }
            }
            Action::StageAll { unstage } => {
                let outcome = g.stage_all(unstage)?;
                let mut message = format!(
                    "{} the whole worktree · {} {} changed",
                    if unstage { "Unstaged" } else { "Staged" },
                    outcome.changed,
                    if outcome.changed == 1 {
                        "path"
                    } else {
                        "paths"
                    }
                );
                if outcome.conflicts > 0 {
                    message.push_str(&format!(
                        " · {} conflicted {} left for resolution",
                        outcome.conflicts,
                        if outcome.conflicts == 1 {
                            "path"
                        } else {
                            "paths"
                        }
                    ));
                }
                Reply::Done {
                    message,
                    committed_draft: None,
                }
            }
            Action::MarkResolved {
                path,
                allow_markers,
            } => {
                g.mark_resolved(&path, allow_markers)?;
                Reply::Done {
                    message: format!(
                        "Marked {} resolved. Its merged content is now staged.",
                        crate::git::display_path(&path)
                    ),
                    committed_draft: None,
                }
            }
            Action::Hunk {
                token,
                index,
                unstage,
            } => {
                let patch = self
                    .patch
                    .as_ref()
                    .ok_or("Reopen the diff before staging this hunk")?;
                if patch.token != token || patch.staged != unstage {
                    return Err("Diff changed. Reopen the file and retry.".into());
                }
                if g.diff(None, &patch.path, patch.staged)? != patch.source {
                    return Err("Diff changed since review. Reopen the file and retry.".into());
                }
                let hunk = patch.hunks.get(index).ok_or("Hunk no longer exists")?;
                g.reject_unmerged_paths(&[&patch.path])?;
                g.apply_hunk(&hunk.materialize(&patch.source), unstage)?;
                Reply::Done {
                    message: format!(
                        "{} hunk in {}",
                        if unstage { "Unstaged" } else { "Staged" },
                        crate::git::display_path(&patch.path)
                    ),
                    committed_draft: None,
                }
            }
            Action::DiscardHunk { token, index } => {
                let patch = self
                    .patch
                    .as_ref()
                    .ok_or("Reopen the diff before discarding")?;
                if patch.token != token || patch.staged {
                    return Err("Discard requires a current working-copy diff".into());
                }
                if g.diff(None, &patch.path, false)? != patch.source {
                    return Err("Working file changed since review. Reopen the diff.".into());
                }
                let hunk = patch
                    .hunks
                    .get(index)
                    .ok_or("Hunk no longer exists")?
                    .materialize(&patch.source);
                g.discard_hunk(&patch.path, &hunk)?;
                Reply::Done {
                    message: format!(
                        "Discarded working-copy hunk in {}. Index and HEAD verified unchanged.",
                        patch.path
                    ),
                    committed_draft: None,
                }
            }
            Action::Commit { message, reviewed } => {
                let reviewed = reviewed
                    .as_ref()
                    .ok_or("Refresh and review before committing")?;
                let sha = g.commit_reviewed(&message, reviewed)?;
                self.journal
                    .lock()
                    .unwrap()
                    .record_commit(reviewed, &sha, &message);
                Reply::Done {
                    message: format!("Verified commit {sha} in {}", g.path.display()),
                    committed_draft: Some(message),
                }
            }
            Action::ReviewOperation {
                op,
                target,
                destination,
            } => {
                if let Some(direction) = op.journal_direction() {
                    return Ok(Reply::OperationReview(
                        self.journal
                            .lock()
                            .unwrap()
                            .review(g, direction, destination)?,
                    ));
                }
                return Ok(Reply::OperationReview(ReviewedOperation::capture(
                    g,
                    op,
                    target,
                    destination,
                )?));
            }
            Action::Operation { reviewed } if reviewed.op.journal_direction().is_some() => {
                Reply::Done {
                    message: self.journal.lock().unwrap().execute(g, &reviewed)?,
                    committed_draft: None,
                }
            }
            Action::Operation { reviewed } => {
                let before = crate::journal::Before::capture(g, &reviewed);
                reviewed.execute(g)?;
                let note = self
                    .journal
                    .lock()
                    .unwrap()
                    .record_operation(g, &reviewed, before);
                let message = if matches!(reviewed.op, Operation::PopStash) {
                    "Stash popped. The stash entry was removed."
                } else {
                    "Operation completed"
                };
                Reply::Done {
                    message: match note {
                        Some(note) => format!("{}. {note}", message.trim_end_matches('.')),
                        None => message.into(),
                    },
                    committed_draft: None,
                }
            }
            Action::Fetch => return Err("Fetch runs as a network operation.".into()),
            Action::Open { .. }
            | Action::Refresh { .. }
            | Action::RefreshWorktree { .. }
            | Action::LoadMore { .. } => unreachable!(),
        };
        self.patch = None;
        Ok(reply)
    }

    /// Prepares a Fetch, Pull or Push to run on its own thread with the
    /// session's repository, or returns the response that rejects it.
    fn network_job(
        &mut self,
        request: Request,
        scope: crate::process::ReadScope,
        queue_ms: f64,
    ) -> Result<NetworkJob, Box<Response>> {
        let Request {
            epoch,
            sequence,
            action,
        } = request;
        let reject = |message: &str, refresh: bool| {
            Box::new(Response {
                epoch,
                sequence,
                elapsed_ms: 0.0,
                queue_ms,
                result: Err(Failure {
                    message: message.into(),
                    kind: FailureKind::Rejected,
                    refresh,
                }),
            })
        };
        if epoch != self.epoch {
            return Err(reject(
                "Repository changed. Retry the operation in the current repository.",
                false,
            ));
        }
        let Some(repo) = self.repo.as_ref() else {
            return Err(reject("Open a local repository first", false));
        };
        if self.network_busy.swap(true, Ordering::AcqRel) {
            return Err(reject(
                "A Fetch, Pull or Push is already running. Wait for it or cancel it.",
                false,
            ));
        }
        // A pull rewrites the worktree, so cached hunk patches expire now.
        self.patch = None;
        let mut git = repo.clone();
        git.read_scope = None;
        Ok(NetworkJob {
            git,
            epoch,
            sequence,
            action,
            scope,
            queue_ms,
            busy: self.network_busy.clone(),
            journal: self.journal.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_repo::Fixture;
    use std::path::Path;

    fn reviewed(op: Operation, target: &str, checkout_ref: Option<&str>) -> ReviewedOperation {
        ReviewedOperation {
            op,
            target: target.into(),
            destination: crate::git::WriteIdentity {
                head: "0123456789abcdef".into(),
                ..Default::default()
            },
            target_id: Some("fedcba9876543210".into()),
            checkout_ref: checkout_ref.map(str::to_owned),
            target_summary: None,
            journal: None,
            network: None,
        }
    }

    #[test]
    fn operation_review_names_the_consequence_with_short_ids() {
        let branch = reviewed(Operation::Checkout, "topic", Some("refs/heads/topic"));
        assert_eq!(branch.title(), "Switch to branch");
        assert_eq!(
            branch.describe("main"),
            "Switch the working tree from main (01234567) to branch topic (fedcba98)."
        );
        let detached = reviewed(Operation::Checkout, "fedcba9876543210", None);
        assert_eq!(detached.title(), "Check out commit in detached HEAD");
        assert!(detached.describe("main").starts_with(
            "Check out commit fedcba98 without a branch (detached HEAD). You leave main (01234567)"
        ));
        assert_eq!(
            reviewed(Operation::Merge, "topic", None).describe("main"),
            "Merge topic (fedcba98) into main (01234567)."
        );
        assert_eq!(
            reviewed(Operation::CherryPick, "fedcba98", None).describe("main"),
            "Apply the changes from commit fedcba98 as a new commit on main (01234567)."
        );
        assert_eq!(
            reviewed(Operation::Revert, "fedcba98", None).describe("main"),
            "Create a new commit on main (01234567) that undoes commit fedcba98."
        );
        let action = Action::Operation {
            reviewed: reviewed(Operation::Merge, "topic", None),
        };
        assert_eq!(action.label(), "Merge");
        assert_eq!(action.target().as_deref(), Some("topic (fedcba98)"));
    }

    #[test]
    fn merge_commits_cannot_be_cherry_picked_or_reverted() {
        let fixture = Fixture::new();
        fixture.history(2, 0);
        let git = &fixture.git;
        git.write(&["switch", "-c", "side", "HEAD~1"]).unwrap();
        fixture.write("side.txt", "side\n");
        git.write(&["add", "side.txt"]).unwrap();
        git.write(&["commit", "-m", "side"]).unwrap();
        git.write(&["switch", "-"]).unwrap();
        git.write(&["merge", "--no-ff", "--no-edit", "side"])
            .unwrap();
        let destination = git.write_identity().unwrap();
        for op in [Operation::CherryPick, Operation::Revert] {
            let error = ReviewedOperation::capture(git, op, "HEAD".into(), destination.clone())
                .unwrap_err();
            assert_eq!(error, MERGE_COMMIT_UNSUPPORTED);
        }
        let merge =
            ReviewedOperation::capture(git, Operation::Checkout, "HEAD".into(), destination)
                .unwrap();
        let summary = merge.target_summary.unwrap();
        assert_eq!(summary.parents, 2);
        assert!(summary.subject.starts_with("Merge branch 'side'"));
    }

    #[test]
    fn worktree_refresh_reuses_history_but_checks_new_refs_and_loads_only_more_commits() {
        let fixture = Fixture::new();
        fixture.history(12, 0);
        let git = &fixture.git;
        let mut session = Session::default();
        let open = session
            .run(
                1,
                1,
                Action::Open {
                    limit: 5,
                    trust: RepositoryTrust::Confirmed(
                        RepositoryLocation::discover(&git.path).unwrap(),
                    ),
                },
            )
            .unwrap();
        let first = match open {
            Reply::Snapshot { snapshot, .. } => snapshot.commits,
            _ => panic!("expected snapshot"),
        };
        assert_eq!(first.len(), 5);
        assert_eq!(session.history_layouts, 1);
        fixture.write("arrived.txt", "worktree edit\n");
        let refreshed = session
            .run(1, 2, Action::RefreshWorktree { limit: 5 })
            .unwrap();
        assert!(
            matches!(refreshed, Reply::Snapshot { snapshot, .. } if snapshot.status.iter().any(|file| file.path == "arrived.txt"))
        );
        assert_eq!(session.history_layouts, 1);
        let more = session.run(1, 3, Action::LoadMore { limit: 10 }).unwrap();
        let ids = match more {
            Reply::Snapshot { snapshot, .. } => snapshot.commits,
            _ => panic!("expected snapshot"),
        };
        assert_eq!(ids.len(), 10);
        assert_eq!(
            ids[..5].iter().map(|c| &c.id).collect::<Vec<_>>(),
            first.iter().map(|c| &c.id).collect::<Vec<_>>()
        );
        assert_eq!(session.history_layouts, 2);
        let mut expected = None;
        let complete = git
            .snapshot_cached(10, crate::git::HistoryRead::Full, &mut expected)
            .unwrap()
            .0;
        assert_eq!(
            ids.iter().map(|c| &c.id).collect::<Vec<_>>(),
            complete.commits.iter().map(|c| &c.id).collect::<Vec<_>>()
        );
        git.create_branch_at("external", "HEAD~5").unwrap();
        session
            .run(1, 4, Action::RefreshWorktree { limit: 10 })
            .unwrap();
        assert_eq!(session.history_layouts, 3);
        session.run(1, 5, Action::Refresh { limit: 10 }).unwrap();
        assert_eq!(session.history_layouts, 4);
    }

    #[test]
    fn late_refresh_failure_or_cancellation_preserves_the_history_generation() {
        for (limit, cancel) in [(2, false), (5, false), (2, true), (5, true)] {
            let fixture = Fixture::new();
            fixture.history(2, 0);
            let git = &fixture.git;
            let mut session = Session::default();
            let open = session
                .run(
                    1,
                    1,
                    Action::Open {
                        limit,
                        trust: RepositoryTrust::Confirmed(
                            RepositoryLocation::discover(&git.path).unwrap(),
                        ),
                    },
                )
                .unwrap();
            let Reply::Snapshot {
                graph: original_graph,
                ..
            } = open
            else {
                panic!("expected snapshot")
            };
            git.create_branch_at("old-history", "HEAD").unwrap();
            let new_root = git
                .write(&["commit-tree", "HEAD^{tree}", "-m", "new root"])
                .unwrap();
            git.write(&["update-ref", "refs/heads/main", new_root.trim()])
                .unwrap();
            session.repo.as_mut().unwrap().observe_command = Some(if cancel {
                |git, args| {
                    if args.starts_with(&["stash", "list"]) {
                        git.read_scope.as_ref().unwrap().cancel();
                    }
                    Ok(())
                }
            } else {
                |_, args| {
                    if args.starts_with(&["stash", "list"]) {
                        return Err("injected late stash-list failure".into());
                    }
                    Ok(())
                }
            });
            let failed = session
                .execute_scoped(
                    Request {
                        epoch: 1,
                        sequence: 2,
                        action: Action::Refresh { limit },
                    },
                    Some(crate::process::ReadScope::default()),
                    0.0,
                )
                .result
                .unwrap_err();
            assert!(
                failed.message.contains(if cancel {
                    "superseded"
                } else {
                    "injected late"
                }),
                "{failed:?}"
            );
            session.repo.as_mut().unwrap().observe_command = None;
            for (sequence, requested, action) in [
                (3, limit, Action::RefreshWorktree { limit }),
                (4, 10, Action::LoadMore { limit: 10 }),
            ] {
                let Reply::Snapshot {
                    snapshot, graph, ..
                } = session.run(1, sequence, action).unwrap()
                else {
                    panic!("expected snapshot")
                };
                let expected = git.snapshot(requested).unwrap();
                assert_eq!(
                    snapshot
                        .commits
                        .iter()
                        .map(|commit| &commit.id)
                        .collect::<Vec<_>>(),
                    expected
                        .commits
                        .iter()
                        .map(|commit| &commit.id)
                        .collect::<Vec<_>>(),
                );
                let expected_graph =
                    graph::layout_with_head(&expected.commits, Some(&expected.head_id));
                let encode = |graph: &graph::Graph| serde_json::to_value(graph).unwrap();
                assert_ne!(encode(&expected_graph), encode(&original_graph));
                assert_eq!(encode(&graph), encode(&expected_graph));
            }
        }
    }

    #[test]
    fn preview_limits_disable_partial_writes_without_changing_git_state() {
        let fixture = Fixture::new();
        fixture.write("large.txt", &"x".repeat(MAX_PREVIEW_LINE_BYTES + 1));
        let git = &fixture.git;
        let before = git.write_identity().unwrap();
        let mut session = Session {
            repo: Some(git.clone()),
            epoch: 1,
            ..Default::default()
        };
        let reply = session
            .run(
                1,
                1,
                Action::Diff {
                    id: None,
                    path: "large.txt".into(),
                    staged: false,
                },
            )
            .unwrap();
        assert!(
            matches!(reply, Reply::Diff { content, hunks, preview, .. } if hunks.is_empty() && content.lines.is_empty() && preview == diff::Preview::Oversized)
        );
        assert!(session.patch.is_none());
        assert!(session
            .run(
                1,
                2,
                Action::Hunk {
                    token: 1,
                    index: 0,
                    unstage: false
                }
            )
            .is_err());
        assert_eq!(git.write_identity().unwrap(), before);
        assert_eq!(
            std::fs::read(git.path.join("large.txt")).unwrap().len(),
            MAX_PREVIEW_LINE_BYTES + 1
        );
    }

    #[test]
    #[ignore = "repeatable snapshot timing fixture"]
    fn benchmark_snapshot_refresh() {
        use std::sync::atomic::Ordering::Relaxed;
        let fixture = Fixture::new();
        fixture.history(20_000, 200);
        let linked = std::env::var_os("GITCRAB_BENCH_LINKED").is_some();
        let full = std::env::var_os("GITCRAB_BENCH_FULL_REFRESH").is_some();
        let git = if linked {
            let path = fixture.git.path.parent().unwrap().join("linked");
            fixture
                .git
                .run(&[
                    "worktree",
                    "add",
                    "--detach",
                    path.to_str().unwrap(),
                    "HEAD",
                ])
                .unwrap();
            Git::open(path).unwrap()
        } else {
            fixture.git.clone()
        };
        for i in 0..2000 {
            std::fs::write(git.path.join(format!("untracked-{i}")), "fixture\n").unwrap();
        }
        let service = Service::new();
        service
            .submit(Request {
                epoch: 1,
                sequence: 1,
                action: Action::Open {
                    limit: 20_000,
                    trust: RepositoryTrust::Confirmed(
                        RepositoryLocation::discover(&git.path).unwrap(),
                    ),
                },
            })
            .unwrap()
            .blocking_recv()
            .unwrap()
            .result
            .unwrap();
        let mut samples = vec![];
        let mut queues = vec![];
        let mut counts = vec![];
        for sequence in 2..=12 {
            let before = crate::test_repo::SUBPROCESSES.load(Relaxed);
            let response = service
                .submit(Request {
                    epoch: 1,
                    sequence,
                    action: if full {
                        Action::Refresh { limit: 20_000 }
                    } else {
                        Action::RefreshWorktree { limit: 20_000 }
                    },
                })
                .unwrap()
                .blocking_recv()
                .unwrap();
            samples.push(response.elapsed_ms);
            queues.push(response.queue_ms);
            counts.push(crate::test_repo::SUBPROCESSES.load(Relaxed) - before);
            assert!(
                matches!(response.result, Ok(Reply::Snapshot { snapshot, .. }) if snapshot.commits.len() == 20_000)
            );
        }
        samples.sort_by(f64::total_cmp);
        queues.sort_by(f64::total_cmp);
        println!("20,000 commits, 200 extra refs, 2000 untracked, linked={linked}, full={full}. Median refresh {:.2} ms, queue {:.2} ms, Git counts {counts:?}, process peak {:?} bytes", samples[5], queues[5], crate::test_repo::peak_memory_bytes());
    }

    #[test]
    #[ignore = "repeatable real-Git diff timing fixture"]
    fn benchmark_worktree_diff() {
        use std::sync::atomic::Ordering::Relaxed;
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("tracked.txt", "base\n");
        git.stage("tracked.txt").unwrap();
        git.commit("base").unwrap();
        for i in 0..24 {
            git.run(&[
                "commit",
                "--allow-empty",
                "-m",
                &format!("fixture commit {i}"),
            ])
            .unwrap();
        }
        fixture.write("tracked.txt", "edit\n");
        std::fs::create_dir(git.path.join("untracked")).unwrap();
        for i in 0..2000 {
            fixture.write(&format!("untracked/{i}"), "fixture\n");
        }
        let service = Service::new();
        let send = |sequence, action| {
            service
                .submit(Request {
                    epoch: 1,
                    sequence,
                    action,
                })
                .unwrap()
                .blocking_recv()
                .unwrap()
        };
        send(
            1,
            Action::Open {
                limit: 500,
                trust: RepositoryTrust::Confirmed(RepositoryLocation::discover(&git.path).unwrap()),
            },
        )
        .result
        .unwrap();
        let mut samples = vec![];
        let mut delays = vec![];
        let mut processes = vec![];
        for sequence in 2..=12 {
            let start = std::time::Instant::now();
            let before = crate::test_repo::SUBPROCESSES.load(Relaxed);
            let response = send(
                sequence,
                Action::Diff {
                    id: None,
                    path: "tracked.txt".into(),
                    staged: false,
                },
            );
            let total = start.elapsed().as_secs_f64() * 1000.0;
            processes.push(crate::test_repo::SUBPROCESSES.load(Relaxed) - before);
            samples.push(response.elapsed_ms);
            delays.push(total - response.elapsed_ms);
            assert!(matches!(response.result, Ok(Reply::Diff { .. })));
        }
        samples.sort_by(f64::total_cmp);
        delays.sort_by(f64::total_cmp);
        println!("25 commits, 2000 untracked files, 11 requests. Git process counts {processes:?}. Median worker {:.2} ms, queue and handoff {:.2} ms", samples[5], delays[5]);
    }

    #[test]
    fn operation_rejects_a_new_destination_even_at_the_same_commit() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("tracked.txt", "base\n");
        git.stage("tracked.txt").unwrap();
        git.commit("base").unwrap();
        git.create_branch("other").unwrap();
        let second = Git::open(&git.path).unwrap();
        let reviewed = ReviewedOperation::capture(
            git,
            Operation::CherryPick,
            "HEAD".into(),
            git.write_identity().unwrap(),
        )
        .unwrap();
        second.checkout("other").unwrap();
        let actual = second.write_identity().unwrap();
        assert!(reviewed
            .execute(git)
            .unwrap_err()
            .message
            .contains("HEAD or index changed"));
        assert_eq!(git.write_identity().unwrap(), actual);
        assert_eq!(actual.symbolic_head, "refs/heads/other");
        assert_eq!(
            std::fs::read(git.path.join("tracked.txt")).unwrap(),
            b"base\n"
        );
    }

    #[test]
    fn worker_records_writes_and_runs_reviewed_undo_and_redo() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("tracked.txt", "base\n");
        git.stage("tracked.txt").unwrap();
        git.commit("base").unwrap();
        let parent = git.resolve_commit("HEAD").unwrap();
        let mut session = Session {
            repo: Some(git.clone()),
            epoch: 1,
            ..Default::default()
        };
        fn run(session: &mut Session, action: Action) -> Result<Reply, Failure> {
            session
                .execute(Request {
                    epoch: 1,
                    sequence: 1,
                    action,
                })
                .result
        }
        fn journal(session: &mut Session) -> crate::journal::JournalStatus {
            match run(session, Action::Refresh { limit: 10 }) {
                Ok(Reply::Snapshot { snapshot, .. }) => snapshot.journal,
                _ => panic!("refresh failed"),
            }
        }
        fn review(session: &mut Session, op: Operation, target: &str) -> ReviewedOperation {
            let destination = session.repo.as_ref().unwrap().write_identity().unwrap();
            match run(
                session,
                Action::ReviewOperation {
                    op,
                    target: target.into(),
                    destination,
                },
            ) {
                Ok(Reply::OperationReview(reviewed)) => reviewed,
                other => panic!("review failed: {other:?}"),
            }
        }
        fixture.write("tracked.txt", "second\n");
        git.stage("tracked.txt").unwrap();
        let reviewed = Some(git.write_identity().unwrap());
        assert!(run(
            &mut session,
            Action::Commit {
                message: "second".into(),
                reviewed,
            }
        )
        .is_ok());
        let status = journal(&mut session);
        assert_eq!(
            status.summary(crate::journal::Direction::Undo),
            Some("commit “second”")
        );
        assert_eq!(
            status.state(crate::journal::Direction::Undo),
            crate::journal::StepState::Ready
        );

        let undo = review(&mut session, Operation::Undo, "");
        let action = Action::Operation { reviewed: undo };
        assert_eq!(action.label(), "Undo");
        assert_eq!(action.target().as_deref(), Some("commit “second”"));
        let Ok(Reply::Done { message, .. }) = run(&mut session, action) else {
            panic!("undo failed");
        };
        assert_eq!(message, "Undid commit “second”.");
        assert_eq!(git.resolve_commit("HEAD").unwrap(), parent);
        let status = journal(&mut session);
        assert_eq!(
            status.state(crate::journal::Direction::Redo),
            crate::journal::StepState::Ready
        );

        let redo = review(&mut session, Operation::Redo, "");
        assert!(run(&mut session, Action::Operation { reviewed: redo }).is_ok());
        assert_ne!(git.resolve_commit("HEAD").unwrap(), parent);

        // A reviewed operation is recorded too, and clears Redo.
        let undo = review(&mut session, Operation::Undo, "");
        assert!(run(&mut session, Action::Operation { reviewed: undo }).is_ok());
        let branch = review(&mut session, Operation::CreateBranch, "topic");
        assert!(run(&mut session, Action::Operation { reviewed: branch }).is_ok());
        let status = journal(&mut session);
        assert_eq!(
            status.summary(crate::journal::Direction::Undo),
            Some("create branch topic")
        );
        assert_eq!(
            status.state(crate::journal::Direction::Redo),
            crate::journal::StepState::Empty
        );
    }

    #[test]
    fn checkout_rejects_a_moved_branch_and_keeps_symbolic_head_on_retry() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("tracked.txt", "base\n");
        git.stage("tracked.txt").unwrap();
        git.commit("base").unwrap();
        git.create_branch("feature").unwrap();
        fixture.write("tracked.txt", "tip\n");
        git.stage("tracked.txt").unwrap();
        git.commit("tip").unwrap();
        let actual = git.write_identity().unwrap();
        let reviewed =
            ReviewedOperation::capture(git, Operation::Checkout, "feature".into(), actual.clone())
                .unwrap();
        let second = Git::open(&git.path).unwrap();
        second.run(&["branch", "-f", "feature", "HEAD"]).unwrap();
        assert!(reviewed
            .execute(git)
            .unwrap_err()
            .message
            .contains("target moved"));
        assert_eq!(git.write_identity().unwrap(), actual);
        assert_eq!(
            std::fs::read(git.path.join("tracked.txt")).unwrap(),
            b"tip\n"
        );
        let reviewed =
            ReviewedOperation::capture(git, Operation::Checkout, "feature".into(), actual.clone())
                .unwrap();
        reviewed.execute(git).unwrap();
        let after = git.write_identity().unwrap();
        assert_eq!(after.symbolic_head, "refs/heads/feature");
        assert_eq!(after.head, actual.head);
        assert_eq!(after.index, actual.index);
    }

    #[test]
    fn pop_stash_rejects_a_new_stash_and_drops_only_the_reviewed_entry_on_retry() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("tracked.txt", "base\n");
        git.stage("tracked.txt").unwrap();
        git.commit("base").unwrap();
        fixture.write("tracked.txt", "old stash\n");
        git.stash_with_message("old").unwrap();
        let actual = git.write_identity().unwrap();
        let reviewed =
            ReviewedOperation::capture(git, Operation::PopStash, String::new(), actual.clone())
                .unwrap();
        let second = Git::open(&git.path).unwrap();
        fixture.write("tracked.txt", "new stash\n");
        second.stash_with_message("new").unwrap();
        let stash = second.run(&["stash", "list"]).unwrap();
        assert!(reviewed
            .execute(git)
            .unwrap_err()
            .message
            .contains("target moved"));
        assert_eq!(git.write_identity().unwrap(), actual);
        assert_eq!(git.run(&["stash", "list"]).unwrap(), stash);
        assert_eq!(
            std::fs::read(git.path.join("tracked.txt")).unwrap(),
            b"base\n"
        );
        let latest =
            ReviewedOperation::capture(git, Operation::PopStash, String::new(), actual.clone())
                .unwrap();
        latest.execute(git).unwrap();
        assert_eq!(git.write_identity().unwrap(), actual);
        // The popped "new" entry is gone; the older "old" entry remains.
        let remaining = git.run(&["stash", "list", "--format=%gs"]).unwrap();
        assert_eq!(stash.lines().count(), 2);
        assert_eq!(remaining.lines().count(), 1);
        assert!(remaining.contains("old"), "{remaining}");
        assert_eq!(
            std::fs::read(git.path.join("tracked.txt")).unwrap(),
            b"new stash\n"
        );
    }
    #[test]
    fn pop_stash_drops_one_duplicate_entry_and_keeps_a_concurrent_list_update() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("tracked.txt", "base\n");
        git.stage("tracked.txt").unwrap();
        git.commit("base").unwrap();
        fixture.write("tracked.txt", "reviewed stash\n");
        git.stash_with_message("reviewed").unwrap();
        let id = git.resolve_commit("refs/stash").unwrap();
        fixture.write("tracked.txt", "another stash\n");
        git.stash_with_message("another").unwrap();
        let another = git.resolve_commit("refs/stash").unwrap();
        git.write(&["update-ref", "refs/tags/late-stash", &another])
            .unwrap();
        git.write(&["stash", "store", "-m", "duplicate object", &id])
            .unwrap();
        let before = git.run(&["stash", "list", "--format=%H %gs"]).unwrap();
        assert_eq!(
            before.lines().filter(|line| line.starts_with(&id)).count(),
            2
        );
        let identity = git.write_identity().unwrap();
        let reviewed =
            ReviewedOperation::capture(git, Operation::PopStash, String::new(), identity.clone())
                .unwrap();
        let mut observed = git.clone();
        observed.observe_command = Some(|git, args| {
            if args.starts_with(&["stash", "apply"]) {
                let other = Git::open(&git.path)?;
                let another = other.resolve_commit("refs/tags/late-stash")?;
                other
                    .write(&["stash", "store", "-m", "concurrent entry", &another])
                    .unwrap();
            }
            Ok(())
        });
        let result = reviewed.execute(&observed);
        // Only the newest entry holding the reviewed ID is dropped; the other
        // duplicate and the concurrently stored entry stay.
        let mut expected = vec![format!("{another} concurrent entry")];
        let mut dropped = false;
        for line in before.lines() {
            if !dropped && line.starts_with(&id) {
                dropped = true;
                continue;
            }
            expected.push(line.to_owned());
        }
        assert_eq!(
            git.run(&["stash", "list", "--format=%H %gs"]).unwrap(),
            format!("{}\n", expected.join("\n")),
        );
        assert_eq!(git.write_identity().unwrap(), identity);
        assert_eq!(
            std::fs::read(git.path.join("tracked.txt")).unwrap(),
            b"reviewed stash\n"
        );
        result.unwrap();
    }

    #[test]
    fn conflicting_reviewed_merge_reports_unmerged_index_and_preserves_head() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("conflict.txt", "base\n");
        git.stage("conflict.txt").unwrap();
        git.commit("base").unwrap();
        git.create_branch("feature").unwrap();
        git.checkout("feature").unwrap();
        fixture.write("conflict.txt", "feature\n");
        git.stage("conflict.txt").unwrap();
        git.commit("feature").unwrap();
        git.checkout("main").unwrap();
        fixture.write("conflict.txt", "main\n");
        git.stage("conflict.txt").unwrap();
        git.commit("main").unwrap();
        let before = git.write_identity().unwrap();
        let reviewed =
            ReviewedOperation::capture(git, Operation::Merge, "feature".into(), before.clone())
                .unwrap();
        let mut session = Session {
            repo: Some(git.clone()),
            epoch: 1,
            ..Default::default()
        };
        let response = session.execute(Request {
            epoch: 1,
            sequence: 1,
            action: Action::Operation { reviewed },
        });
        let failure = response.result.unwrap_err();
        assert_eq!(failure.kind, FailureKind::Conflict);
        assert!(failure.refresh);
        let after = git.write_identity().unwrap();
        assert_eq!(after.head, before.head);
        assert_eq!(after.symbolic_head, before.symbolic_head);
        assert_ne!(after.index, before.index);
        assert_eq!(git.run(&["show", ":2:conflict.txt"]).unwrap(), "main\n");
        assert_eq!(git.run(&["show", ":3:conflict.txt"]).unwrap(), "feature\n");
        let file = std::fs::read_to_string(git.path.join("conflict.txt")).unwrap();
        assert!(file.contains("<<<<<<<"));
        assert!(file.contains(">>>>>>>"));

        // Generic whole-worktree actions keep the conflict entries.
        let unmerged = git.run(&["ls-files", "-u"]).unwrap();
        fixture.write("late.txt", "arrived\n");
        let mut sequence = 1;
        let mut run = |action| {
            sequence += 1;
            session
                .execute(Request {
                    epoch: 1,
                    sequence,
                    action,
                })
                .result
        };
        for unstage in [false, true] {
            let Ok(Reply::Done { message, .. }) = run(Action::StageAll { unstage }) else {
                panic!("bulk index action failed");
            };
            assert!(message.contains("1 conflicted path left for resolution"));
            assert_eq!(git.run(&["ls-files", "-u"]).unwrap(), unmerged);
        }
        let refused = run(Action::MarkResolved {
            path: "conflict.txt".into(),
            allow_markers: false,
        })
        .unwrap_err();
        assert_eq!(refused.kind, FailureKind::ConflictMarkers);
        assert!(refused.refresh);
        assert_eq!(git.run(&["ls-files", "-u"]).unwrap(), unmerged);
        fixture.write("conflict.txt", "main and feature\n");
        assert!(run(Action::MarkResolved {
            path: "conflict.txt".into(),
            allow_markers: false,
        })
        .is_ok());
        assert!(git.run(&["ls-files", "-u"]).unwrap().is_empty());
    }

    #[test]
    fn operation_targets_match_git_branch_syntax_and_optional_messages() {
        for name in [
            "",
            " ",
            "-bad",
            "a..b",
            "a.lock",
            "a/.hidden",
            "a/b.lock",
            "a b",
            "a@{b",
            "@",
            "a/",
            "/a",
            "a//b",
            "a.",
            "a?b",
            "a[b",
            "a\\b",
        ] {
            assert!(
                Operation::CreateBranch.validate_target(name).is_err(),
                "{name}"
            );
        }
        for name in ["feature/日本語", "topic-1", "a.b", "release/v1"] {
            assert!(
                Operation::CreateBranch.validate_target(name).is_ok(),
                "{name}"
            );
        }
        for op in [
            Operation::Checkout,
            Operation::Merge,
            Operation::CherryPick,
            Operation::Revert,
        ] {
            assert!(op.validate_target("").is_err());
            assert!(op.validate_target("--bad").is_err());
            assert!(op.validate_target("HEAD~1").is_ok());
        }
        assert!(Operation::Stash.validate_target("").is_ok());
        assert!(Operation::PopStash.validate_target("").is_ok());
    }
    #[test]
    fn hunk_tokens_expire_after_mutations_and_failures_refresh() {
        let fixture = crate::test_repo::Fixture::new();
        let git = &fixture.git;
        let path = &git.path;
        std::fs::write(path.join("new ü file.txt"), "new content\n").unwrap();
        let mut session = Session::default();
        session
            .run(
                1,
                1,
                Action::Open {
                    limit: 500,
                    trust: RepositoryTrust::Confirmed(RepositoryLocation::discover(path).unwrap()),
                },
            )
            .unwrap();
        session
            .run(
                1,
                2,
                Action::Diff {
                    id: None,
                    path: "new ü file.txt".into(),
                    staged: false,
                },
            )
            .unwrap();
        let stale = session.execute(Request {
            epoch: 1,
            sequence: 3,
            action: Action::Hunk {
                token: 1,
                index: 0,
                unstage: false,
            },
        });
        assert!(stale.result.unwrap_err().refresh);
        assert!(git.run(&["diff", "--cached"]).unwrap().is_empty());
        session
            .run(
                1,
                4,
                Action::Diff {
                    id: None,
                    path: "new ü file.txt".into(),
                    staged: false,
                },
            )
            .unwrap();
        session
            .execute(Request {
                epoch: 1,
                sequence: 5,
                action: Action::Hunk {
                    token: 4,
                    index: 0,
                    unstage: false,
                },
            })
            .result
            .unwrap();
        assert!(session.patch.is_none());
        assert!(git
            .run(&["diff", "--cached"])
            .unwrap()
            .contains("new content"));
        assert_eq!(
            std::fs::read_to_string(path.join("new ü file.txt")).unwrap(),
            "new content\n"
        );
        session
            .run(
                1,
                6,
                Action::Diff {
                    id: None,
                    path: "new ü file.txt".into(),
                    staged: true,
                },
            )
            .unwrap();
        assert!(session
            .execute(Request {
                epoch: 1,
                sequence: 7,
                action: Action::DiscardHunk { token: 6, index: 0 }
            })
            .result
            .is_err());
        std::fs::write(path.join("new ü file.txt"), "unstaged content\n").unwrap();
        session
            .run(
                1,
                8,
                Action::Diff {
                    id: None,
                    path: "new ü file.txt".into(),
                    staged: false,
                },
            )
            .unwrap();
        std::fs::write(path.join("new ü file.txt"), "external edit\n").unwrap();
        let identity = git.write_identity().unwrap();
        assert!(
            session
                .execute(Request {
                    epoch: 1,
                    sequence: 9,
                    action: Action::DiscardHunk { token: 8, index: 0 }
                })
                .result
                .unwrap_err()
                .refresh
        );
        assert_eq!(git.write_identity().unwrap(), identity);
        assert_eq!(
            std::fs::read(path.join("new ü file.txt")).unwrap(),
            b"external edit\n"
        );
        session
            .run(
                1,
                10,
                Action::Diff {
                    id: None,
                    path: "new ü file.txt".into(),
                    staged: false,
                },
            )
            .unwrap();
        session
            .execute(Request {
                epoch: 1,
                sequence: 11,
                action: Action::DiscardHunk {
                    token: 10,
                    index: 0,
                },
            })
            .result
            .unwrap();
        assert_eq!(
            std::fs::read(path.join("new ü file.txt")).unwrap(),
            b"new content\n"
        );
        assert_eq!(git.write_identity().unwrap(), identity);
        let failed = session.execute(Request {
            epoch: 1,
            sequence: 6,
            action: Action::Commit {
                message: String::new(),
                reviewed: None,
            },
        });
        assert!(failed.result.unwrap_err().refresh);
        std::fs::create_dir(path.join("nested")).unwrap();
        std::fs::write(path.join("nested/another.txt"), "keep worktree\n").unwrap();
        git.stage(".").unwrap();
        std::fs::write(path.join("nested/another.txt"), "newer worktree\n").unwrap();
        git.unstage(".").unwrap();
        assert!(git.run(&["diff", "--cached"]).unwrap().is_empty());
        assert_eq!(
            std::fs::read_to_string(path.join("nested/another.txt")).unwrap(),
            "newer worktree\n"
        );
    }
    #[test]
    fn rejects_cross_repository_mutation_and_obsolete_patch() {
        let mut session = Session {
            epoch: 5,
            ..Default::default()
        };
        let response = session.execute(Request {
            epoch: 4,
            sequence: 1,
            action: Action::Commit {
                message: "must never run".into(),
                reviewed: None,
            },
        });
        assert!(!response.result.unwrap_err().refresh);
        assert!(session
            .run(
                4,
                2,
                Action::Open {
                    limit: 500,
                    trust: RepositoryTrust::Unconfirmed,
                }
            )
            .unwrap_err()
            .message
            .contains("Obsolete"));
        assert_eq!(session.epoch, 5);
    }
    #[test]
    fn protocol_round_trip_preserves_repository_identity() {
        let location = RepositoryLocation::test_path("repository ü 日本語");
        let request = Request {
            epoch: 42,
            sequence: 93,
            action: Action::Open {
                limit: 10000,
                trust: RepositoryTrust::Confirmed(location.clone()),
            },
        };
        let json = serde_json::to_string(&request).unwrap();
        let decoded: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.epoch, 42);
        assert_eq!(decoded.sequence, 93);
        assert!(
            matches!(decoded.action, Action::Open { limit: 10000, trust: RepositoryTrust::Confirmed(decoded) } if decoded == location)
        );
    }
    #[test]
    fn worker_rejects_an_open_request_without_repository_trust() {
        let fixture = Fixture::new();
        let mut session = Session::default();
        let path = fixture.git.path.to_str().unwrap().to_owned();
        let request = serde_json::json!({
            "epoch": 1, "sequence": 1,
            "action": { "kind": "Open", "args": { "path": path, "limit": 5 } }
        });
        let rejected = session.execute(serde_json::from_value(request).unwrap());
        assert!(rejected.result.unwrap_err().message.contains("trust"));
        assert!(session.repo.is_none());
        assert!(session.run(1, 2, Action::Refresh { limit: 5 }).is_err());
        let accepted = session
            .run(
                2,
                3,
                Action::Open {
                    limit: 5,
                    trust: RepositoryTrust::Confirmed(
                        RepositoryLocation::discover(&fixture.git.path).unwrap(),
                    ),
                },
            )
            .unwrap();
        assert!(matches!(accepted, Reply::Snapshot { .. }));
    }

    #[test]
    fn subdirectory_open_stays_bound_to_the_confirmed_linked_worktree() {
        let fixture = Fixture::new();
        fixture.history(2, 0);
        let root = &fixture.git.path;
        let linked = root.join("nested-worktree");
        fixture
            .git
            .run(&["worktree", "add", "-b", "linked", linked.to_str().unwrap()])
            .unwrap();
        let selected = linked.join("subdirectory");
        std::fs::create_dir(&selected).unwrap();
        let location = RepositoryLocation::discover(&selected).unwrap();
        assert_eq!(location.worktree, linked.canonicalize().unwrap());
        assert_eq!(
            location.common_dir,
            root.join(".git").canonicalize().unwrap()
        );
        assert_ne!(location.git_dir, location.common_dir);

        let mut session = Session::default();
        let open = || Action::Open {
            limit: 5,
            trust: RepositoryTrust::Confirmed(location.clone()),
        };
        let reply = session.run(1, 1, open()).unwrap();
        assert!(matches!(reply, Reply::Snapshot { path, snapshot, .. }
            if Path::new(&path) == location.worktree
                && snapshot.worktree_id == location.worktree && snapshot.head == "linked"));

        let marker = linked.join(".git");
        std::fs::write(&marker, "gitdir: ../.git\n").unwrap();
        assert!(session
            .run(2, 2, open())
            .unwrap_err()
            .message
            .contains("location changed"));
        std::fs::remove_file(marker).unwrap();
        assert_eq!(
            RepositoryLocation::discover(&selected).unwrap().worktree,
            root.canonicalize().unwrap()
        );
        assert!(session
            .run(3, 3, open())
            .unwrap_err()
            .message
            .contains("location changed"));
        assert!(session.repo.is_none());
    }
    #[test]
    fn superseded_queued_reads_are_cancelled_and_mutations_cancel_all_reads() {
        let (tx, rx) = sync_channel::<Pending>(8);
        let service = Service {
            tx,
            reads: Default::default(),
            network: Default::default(),
        };
        let first = service
            .submit(Request {
                epoch: 1,
                sequence: 1,
                action: Action::Details { id: "old".into() },
            })
            .unwrap();
        let old = rx.recv().unwrap().cancelled.unwrap();
        let second = service
            .submit(Request {
                epoch: 1,
                sequence: 2,
                action: Action::Details { id: "new".into() },
            })
            .unwrap();
        assert!(old.load(Ordering::Acquire));
        let current = rx.recv().unwrap().cancelled.unwrap();
        assert!(!current.load(Ordering::Acquire));
        let mutation = service
            .submit(Request {
                epoch: 1,
                sequence: 3,
                action: Action::StageAll { unstage: false },
            })
            .unwrap();
        assert!(current.load(Ordering::Acquire));
        assert!(rx.recv().unwrap().cancelled.is_none());
        drop((first, second, mutation));
    }

    #[test]
    fn bounded_queue_reports_busy_without_blocking() {
        let (tx, _rx) = sync_channel::<Pending>(8);
        let service = Service {
            tx,
            reads: Default::default(),
            network: Default::default(),
        };
        let mut receivers = vec![];
        for sequence in 0..8 {
            receivers.push(
                service
                    .submit(Request {
                        epoch: 1,
                        sequence,
                        action: Action::Refresh { limit: 500 },
                    })
                    .unwrap(),
            );
        }
        assert!(service
            .submit(Request {
                epoch: 1,
                sequence: 9,
                action: Action::Refresh { limit: 500 }
            })
            .unwrap_err()
            .contains("busy"));
    }
    #[test]
    fn dropped_window_receiver_does_not_stop_worker() {
        let service = Service::new();
        let first = service
            .submit(Request {
                epoch: 1,
                sequence: 1,
                action: Action::Refresh { limit: 1 },
            })
            .unwrap();
        drop(first);
        let second = service
            .submit(Request {
                epoch: 1,
                sequence: 2,
                action: Action::Refresh { limit: 1 },
            })
            .unwrap()
            .blocking_recv()
            .unwrap();
        assert_eq!(second.sequence, 2);
        assert!(second.result.is_err());
    }

    mod network {
        use super::*;
        use crate::git::network_tests::{commit, connected, publish, scope};
        use crate::git::PullMode;

        fn session(git: &Git) -> Session {
            Session {
                repo: Some(git.clone()),
                epoch: 1,
                ..Default::default()
            }
        }

        #[test]
        fn pull_review_names_its_refs_and_conflicts_keep_their_kind() {
            let (local, other) = connected();
            commit(&other, "base.txt", "theirs\n");
            other.git.run(&["push", "--quiet"]).unwrap();
            commit(&local, "base.txt", "mine\n");
            let identity = local.git.write_identity().unwrap();
            let reviewed = ReviewedOperation::capture(
                &local.git,
                Operation::Pull {
                    mode: PullMode::FastForwardIfPossible,
                },
                String::new(),
                identity,
            )
            .unwrap();
            assert_eq!(reviewed.refs().as_deref(), Some("origin/main → main"));
            assert_eq!(reviewed.title(), "Pull (fast-forward if possible)");
            assert!(reviewed
                .describe("main")
                .starts_with("Fetch origin and bring origin/main into main"));
            let mut session = session(&local.git);
            let job = session
                .network_job(
                    Request {
                        epoch: 1,
                        sequence: 2,
                        action: Action::Operation { reviewed },
                    },
                    scope(),
                    0.0,
                )
                .unwrap();
            // Other writes wait for the pull instead of racing it.
            let blocked = session.execute(Request {
                epoch: 1,
                sequence: 3,
                action: Action::StageAll { unstage: false },
            });
            assert!(blocked
                .result
                .unwrap_err()
                .message
                .contains("Pull or Push is running"));
            let failure = job.run().result.unwrap_err();
            assert_eq!(failure.kind, FailureKind::Conflict, "{}", failure.message);
            assert!(failure.refresh);
            assert!(!session.network_busy.load(Ordering::Acquire));
            assert!(local.git.has_unmerged_entries().unwrap());
        }

        #[test]
        fn push_review_says_it_sets_the_upstream_and_rechecks_the_branch() {
            let (local, other) = connected();
            local
                .git
                .run(&["switch", "--quiet", "-c", "topic"])
                .unwrap();
            commit(&local, "topic.txt", "topic\n");
            let capture = || {
                let identity = local.git.write_identity().unwrap();
                ReviewedOperation::capture(&local.git, Operation::Push, String::new(), identity)
                    .unwrap()
            };
            let reviewed = capture();
            assert_eq!(reviewed.refs().as_deref(), Some("topic → origin/topic"));
            assert!(reviewed.describe("topic").contains("(git push -u)"));
            // A commit after the review makes it stale.
            commit(&local, "late.txt", "late\n");
            assert!(reviewed
                .execute_network(&local.git, &scope())
                .unwrap_err()
                .message
                .contains("changed"));
            let reviewed = capture();
            let action = Action::Operation {
                reviewed: reviewed.clone(),
            };
            assert!(action.is_network());
            assert_eq!(action.target().as_deref(), Some("topic → origin/topic"));
            let (message, pull) = reviewed.execute_network(&local.git, &scope()).unwrap();
            assert!(pull.is_none());
            assert!(message.contains("2 commits"), "{message}");

            // The pulled branch reports HEAD before and after for the journal.
            let theirs = publish(&other, "theirs.txt");
            local.git.run(&["switch", "--quiet", "main"]).unwrap();
            let before = local.git.resolve_commit("HEAD").unwrap();
            let identity = local.git.write_identity().unwrap();
            let reviewed = ReviewedOperation::capture(
                &local.git,
                Operation::Pull {
                    mode: PullMode::FastForwardOnly,
                },
                String::new(),
                identity,
            )
            .unwrap();
            let (_, pull) = reviewed.execute_network(&local.git, &scope()).unwrap();
            let pull = pull.unwrap();
            assert_eq!((pull.before, pull.after), (before, theirs));
        }

        #[test]
        fn a_successful_pull_is_recorded_and_can_be_undone_and_redone() {
            use crate::journal::{Direction, StepState};
            let (local, other) = connected();
            let theirs = publish(&other, "theirs.txt");
            let before = local.git.resolve_commit("HEAD").unwrap();
            let identity = local.git.write_identity().unwrap();
            let reviewed = ReviewedOperation::capture(
                &local.git,
                Operation::Pull {
                    mode: PullMode::FastForwardIfPossible,
                },
                String::new(),
                identity,
            )
            .unwrap();
            let mut session = session(&local.git);
            let job = session
                .network_job(
                    Request {
                        epoch: 1,
                        sequence: 2,
                        action: Action::Operation { reviewed },
                    },
                    scope(),
                    0.0,
                )
                .unwrap();
            assert!(job.run().result.is_ok());
            assert_eq!(local.git.resolve_commit("HEAD").unwrap(), theirs);

            let status = session.journal.lock().unwrap().status(&local.git);
            assert_eq!(status.summary(Direction::Undo), Some("pull into main"));
            assert_eq!(status.state(Direction::Undo), StepState::Ready);

            let step = |direction| {
                let destination = local.git.write_identity().unwrap();
                let mut journal = session.journal.lock().unwrap();
                let reviewed = journal.review(&local.git, direction, destination).unwrap();
                journal.execute(&local.git, &reviewed).unwrap();
            };
            step(Direction::Undo);
            assert_eq!(local.git.resolve_commit("HEAD").unwrap(), before);
            step(Direction::Redo);
            assert_eq!(local.git.resolve_commit("HEAD").unwrap(), theirs);

            // Redo moved the entry back onto the undo stack.
            let status = session.journal.lock().unwrap().status(&local.git);
            assert_eq!(status.state(Direction::Redo), StepState::Empty);
        }

        #[test]
        fn network_actions_run_beside_the_worker_and_can_be_cancelled() {
            let fixture = Fixture::new();
            commit(&fixture, "a.txt", "a\n");
            for args in [
                &["config", "core.sshCommand", "sleep 30; exit 1;"][..],
                &["remote", "add", "slow", "ssh://example.invalid/repo.git"],
            ] {
                fixture.git.run(args).unwrap();
            }
            let service = Service::new();
            let open = service
                .submit(Request {
                    epoch: 1,
                    sequence: 1,
                    action: Action::Open {
                        limit: 5,
                        trust: RepositoryTrust::Confirmed(
                            RepositoryLocation::discover(&fixture.git.path).unwrap(),
                        ),
                    },
                })
                .unwrap()
                .blocking_recv()
                .unwrap();
            assert!(open.result.is_ok());
            let start = std::time::Instant::now();
            let fetch = service
                .submit(Request {
                    epoch: 1,
                    sequence: 2,
                    action: Action::Fetch,
                })
                .unwrap();
            // Reads keep working while the remote hangs.
            let refresh = service
                .submit(Request {
                    epoch: 1,
                    sequence: 3,
                    action: Action::Refresh { limit: 5 },
                })
                .unwrap()
                .blocking_recv()
                .unwrap();
            assert!(refresh.result.is_ok());
            service.cancel_network();
            let failure = fetch.blocking_recv().unwrap().result.unwrap_err();
            assert_eq!(failure.message, "Fetch was cancelled. Git was stopped.");
            assert!(failure.refresh);
            assert!(start.elapsed() < std::time::Duration::from_secs(10));
        }
    }
}
