//! The dedicated Git worker behind [`gitcrab_core::service::GitWorker`].
//!
//! Each [`Service`] owns a worker thread and a repository session.
use crate::{
    diff,
    git::{FailureKind, Git, HunkRange, WriteError},
};
pub use gitcrab_core::service::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{sync_channel, SyncSender},
    Arc, Mutex, Weak,
};
use tokio::sync::oneshot;

/// What a successful local operation reports beyond its effect on HEAD.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Ran {
    /// A popped stash's staged changes could not be restored, so all of
    /// its changes were applied unstaged.
    pub staging_dropped: bool,
}

/// Resolves, revalidates and runs reviewed operations against a repository.
pub(crate) trait OperationRun: Sized {
    fn capture(
        git: &Git,
        op: Operation,
        target: String,
        destination: crate::git::WriteIdentity,
    ) -> Result<Self, String>;
    fn resolve(
        git: &Git,
        op: Operation,
        target: String,
        destination: crate::git::WriteIdentity,
    ) -> Result<Self, String>;
    fn execute(&self, git: &Git) -> Result<Ran, WriteError>;
    fn run_captured(&self, git: &Git) -> Result<Ran, WriteError>;
    fn execute_network(
        &self,
        git: &Git,
        scope: &crate::process::ReadScope,
    ) -> Result<(String, Option<crate::git::PullOutcome>), WriteError>;
}

impl OperationRun for ReviewedOperation {
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
        let finishes = op.finishes_sequence();
        let continues = matches!(op, Operation::Continue { .. });
        let named = crate::git::Sequence::from_name(&target);
        // Reading HEAD, the index and the operation markers does not depend
        // on the target, so they run beside the target lookups. A changed
        // identity is reported first.
        let (identity, markers, resolved) = std::thread::scope(|scope| {
            let identity = scope.spawn(|| git.write_identity());
            let markers = scope.spawn(|| git.operation_markers());
            let resolved = Self::resolve(git, op, target, destination.clone());
            (identity.join(), markers.join(), resolved)
        });
        if identity.map_err(|_| "HEAD and index read failed")?? != destination {
            return Err(
                "HEAD or index changed since the repository was last read. Wait for the refresh, then try again.".into(),
            );
        }
        let markers = markers.map_err(|_| "Git metadata read failed")??;
        let unmerged = !destination.unmerged_entries().is_empty();
        if finishes {
            if crate::git::Sequence::from_operations(&markers) != named {
                return Err("That operation is no longer in progress. Wait for the refresh, then try again.".into());
            }
            if continues && unmerged {
                return Err(gitcrab_core::commands::UNRESOLVED_CONFLICTS.into());
            }
        } else if unmerged || !markers.is_empty() {
            return Err(crate::git::OPERATION_IN_PROGRESS.into());
        }
        resolved
    }

    /// Resolves the target of `op` against the repository, without checking
    /// that HEAD and the index still match `destination`.
    fn resolve(
        git: &Git,
        op: Operation,
        target: String,
        destination: crate::git::WriteIdentity,
    ) -> Result<Self, String> {
        // A branch name a Checkout may switch to, when the target can be one.
        let branch = matches!(op, Operation::Checkout)
            .then(|| {
                if target.starts_with("refs/heads/") {
                    target.clone()
                } else {
                    format!("refs/heads/{target}")
                }
            })
            .filter(|full| {
                let local_name = full.strip_prefix("refs/heads/").unwrap_or(&target);
                Operation::CreateBranch.validate_target(local_name).is_ok()
            });
        // The revision a local operation acts on, when it names one.
        let revision = match &op {
            Operation::CreateBranch => Some("HEAD".to_owned()),
            Operation::CreateBranchAt { start } => Some(start.clone()),
            Operation::Checkout | Operation::Merge | Operation::CherryPick | Operation::Revert => {
                Some(target.clone())
            }
            Operation::PopStash => Some("refs/stash".to_owned()),
            _ => None,
        };
        // Each read is a Git process, so every read the resolution may need
        // starts at once: whether the branch exists, and both candidates'
        // commit and summary. Results are still consulted in the original
        // order, so an unneeded read cannot change the outcome.
        let read = |revision: &str| {
            std::thread::scope(|scope| {
                let summary = scope.spawn(|| git.commit_summary_with_id(revision));
                (
                    git.resolve_commit(revision),
                    summary
                        .join()
                        .unwrap_or_else(|_| Err("Git read failed".into())),
                )
            })
        };
        let (exists, branch_read, revision_read) = std::thread::scope(|scope| {
            let exists = branch
                .as_deref()
                .map(|full| scope.spawn(move || git.ref_exists(full)));
            let branch_read = branch
                .as_deref()
                .map(|full| scope.spawn(move || read(full)));
            let revision_read = revision.as_deref().map(read);
            let failed = |_| "Git read failed".to_owned();
            (
                exists.map(|handle| handle.join().map_err(failed)),
                branch_read.map(|handle| handle.join().map_err(failed)),
                revision_read,
            )
        });
        let mut checkout_ref = match (branch, exists) {
            (Some(full), Some(exists)) => exists??.then_some(full),
            _ => None,
        };
        let remote_branch = match op {
            Operation::Checkout if checkout_ref.is_none() => git.remote_checkout(&target)?,
            _ => None,
        };
        // A remote-tracking branch resolves to the commit of the ref the
        // checkout actually uses, read again by its full name.
        let (branch_read, revision_read) = match &remote_branch {
            Some(remote) => {
                let full = match remote.kind {
                    RemoteCheckoutKind::SwitchTracking => format!("refs/heads/{}", remote.local),
                    _ => remote.tracking.clone(),
                };
                if remote.kind != RemoteCheckoutKind::DetachNameTaken {
                    checkout_ref = Some(format!("refs/heads/{}", remote.local));
                }
                (None, Some(read(&full)))
            }
            None => (branch_read, revision_read),
        };
        let merge_ref = match op {
            Operation::Merge => git.branch_ref(&target)?,
            _ => None,
        };
        if matches!(op, Operation::Stash) && !git.has_tracked_changes()? {
            return Err(crate::git::NOTHING_TO_STASH.into());
        }
        let network = match &op {
            Operation::Pull { .. } => Some(git.pull_target(&destination)?),
            Operation::Push => Some(git.push_target(&destination)?),
            _ => None,
        };
        let (target_id, prefetched) = match &op {
            // The upstream tip as last fetched; Pull fetches again before integrating.
            Operation::Pull { .. } => (
                network
                    .as_ref()
                    .and_then(|network| git.resolve_commit(&network.tracking).ok()),
                None,
            ),
            Operation::Push => (Some(git.resolve_commit("HEAD")?), None),
            Operation::Stash
            | Operation::Undo
            | Operation::Redo
            | Operation::Abort
            | Operation::Continue { .. }
            | Operation::Skip => (None, None),
            _ => {
                let (id, summary) = match (checkout_ref.is_some(), branch_read, revision_read) {
                    (true, Some(branch_read), _) => branch_read?,
                    (_, _, Some(revision_read)) => revision_read,
                    _ => return Err("Enter a target before continuing.".into()),
                };
                (Some(id?), Some(summary))
            }
        };
        let target_summary = match (&target_id, prefetched) {
            // Use the summary read beside the lookup if it names that commit.
            (Some(id), Some(Ok((summary_id, summary)))) if summary_id == *id => Some(summary),
            (Some(id), _) => Some(git.commit_summary(id)?),
            (None, _) => None,
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
            remote_branch,
            merge_ref,
            target_summary,
            network,
            journal: None,
        })
    }

    fn execute(&self, git: &Git) -> Result<Ran, WriteError> {
        let current = Self::capture(
            git,
            self.op.clone(),
            self.target.clone(),
            self.destination.clone(),
        )?;
        if current.target_id != self.target_id
            || current.checkout_ref != self.checkout_ref
            || current.remote_branch != self.remote_branch
            || current.merge_ref != self.merge_ref
        {
            return Err("The target moved since the repository was last read. Wait for the refresh, then try again.".into());
        }
        self.run_captured(git)
    }

    /// Runs an operation whose capture was just taken in this worker request,
    /// so it needs no second revalidation.
    fn run_captured(&self, git: &Git) -> Result<Ran, WriteError> {
        let id = self.target_id.as_deref().unwrap_or_default();
        let sequence = || {
            crate::git::Sequence::from_name(&self.target)
                .ok_or_else(|| WriteError::from(gitcrab_core::commands::NO_SEQUENCE))
        };
        let result = match &self.op {
            Operation::Pull { .. } | Operation::Push => {
                Err("Pull and Push run as network operations.".into())
            }
            Operation::Undo | Operation::Redo => {
                Err("Undo and Redo run through the tab's journal.".into())
            }
            Operation::CreateBranch | Operation::CreateBranchAt { .. } => {
                git.create_branch_at(&self.target, id)
            }
            Operation::Checkout => match &self.remote_branch {
                Some(remote) if remote.kind == RemoteCheckoutKind::CreateTracking => {
                    git.switch_create_tracking(&remote.local, &remote.tracking, id)
                }
                _ => git.switch_to(self.checkout_ref.as_deref(), id),
            },
            // A capture just before this resolved the name to `merge_ref` at
            // `id`. Git words its message from the name as given.
            Operation::Merge if self.merge_ref.is_some() => git.merge(&self.target),
            Operation::Merge => git.merge(id),
            Operation::CherryPick => git.cherry_pick(id),
            Operation::Revert => git.revert(id),
            Operation::Stash => git.stash_with_message(&self.target),
            // Apply the reviewed stash object, then drop its entry only after a
            // clean apply. Like `git stash pop`, conflicts keep the stash.
            Operation::PopStash => {
                return git
                    .stash_apply_with_index(id)
                    .and_then(|restored| {
                        git.drop_stash_object(id).map(|()| Ran {
                            staging_dropped: !restored,
                        })
                    })
                    .map_err(|error| operation_failure(git, error, &self.destination))
            }
            Operation::Abort => sequence().and_then(|sequence| git.abort_sequence(sequence)),
            Operation::Continue { message } => {
                sequence().and_then(|sequence| git.continue_sequence(sequence, message.as_deref()))
            }
            Operation::Skip => sequence().and_then(|sequence| git.skip_sequence(sequence)),
        };
        result
            .map(|()| Ran::default())
            .map_err(|error| operation_failure(git, error, &self.destination))
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
            return Err("The target moved since the repository was last read. Wait for the refresh, then try again.".into());
        }
        let network = self
            .network
            .as_ref()
            .ok_or("Review the operation again before running it.")?;
        match self.op {
            Operation::Pull { mode } => {
                let outcome = git
                    .pull(network, mode, scope)
                    .map_err(|error| operation_failure(git, error, &self.destination))?;
                Ok((outcome.message.clone(), Some(outcome)))
            }
            Operation::Push => Ok((git.push(network, scope)?, None)),
            _ => Err("Only Pull and Push run as network operations.".into()),
        }
    }
}

/// Words the result of a successful local operation. `head` is the full
/// symbolic ref and commit of HEAD read after the write, so the wording
/// follows where Git actually left HEAD.
fn operation_message(reviewed: &ReviewedOperation, head: &(String, String), ran: Ran) -> String {
    use gitcrab_core::presentation::short_id;
    let (symbolic, head_id) = head;
    let id = reviewed.target_id.as_deref().unwrap_or_default();
    let target = reviewed.target.trim();
    // A target typed as an object ID reads better shortened; a name stays.
    let label = if target.is_empty() || id.starts_with(target) {
        short_id(id).to_owned()
    } else {
        ["refs/heads/", "refs/remotes/", "refs/tags/"]
            .iter()
            .find_map(|prefix| target.strip_prefix(prefix))
            .unwrap_or(target)
            .to_owned()
    };
    let branch = symbolic.strip_prefix("refs/heads/");
    let here = branch.map_or_else(|| "detached HEAD".to_owned(), str::to_owned);
    match reviewed.op {
        Operation::CreateBranch | Operation::CreateBranchAt { .. } => {
            if branch == Some(target) {
                format!("Created branch {target} and switched to it")
            } else {
                format!("Created branch {target} at {}", short_id(id))
            }
        }
        Operation::Checkout => match branch {
            Some(branch) => format!("Checked out {branch}"),
            None if label == short_id(head_id) => format!(
                "HEAD is detached at {label}. Create a branch before committing."
            ),
            None => format!(
                "Checked out {label}. HEAD is detached at {}. Create a branch before committing.",
                short_id(head_id)
            ),
        },
        Operation::Merge if *head_id == reviewed.destination.head => {
            format!("Already up to date: {here} already contains {label}")
        }
        Operation::Merge if head_id == id => format!("Fast-forwarded {here} to {label}"),
        Operation::Merge => format!("Merged {label} into {here}"),
        Operation::CherryPick => format!("Cherry-picked {} onto {here}", short_id(id)),
        Operation::Revert => format!("Reverted {} on {here}", short_id(id)),
        Operation::Stash => {
            "Stashed changes to tracked files; untracked files stay in the working tree".into()
        }
        Operation::PopStash if ran.staging_dropped => "Stash popped. The stash entry was removed. Its staged changes could not be restored on top of the current index, so all of its changes are now unstaged.".into(),
        Operation::PopStash => "Stash popped. The stash entry was removed.".into(),
        Operation::Pull { .. }
        | Operation::Push
        | Operation::Undo
        | Operation::Redo
        | Operation::Abort
        | Operation::Continue { .. }
        | Operation::Skip => {
            format!("{} completed", reviewed.op.label())
        }
    }
}

/// Classifies a failed Git write. It stopped with conflicts only when it left
/// unmerged entries that the index reviewed `before` it did not already hold;
/// Git refusing to run over existing conflicts is an ordinary failure.
fn operation_failure(
    git: &Git,
    error: WriteError,
    before: &crate::git::WriteIdentity,
) -> WriteError {
    if error.kind != FailureKind::GitFailure {
        return error;
    }
    let introduced = git.unmerged_entries().map(|after| {
        let after: Vec<&[u8]> = after
            .split(|&byte| byte == 0)
            .filter(|entry| !entry.is_empty())
            .collect();
        !after.is_empty() && after != before.unmerged_entries()
    });
    match introduced {
        Ok(true) => WriteError {
            kind: FailureKind::Conflict,
            message: format!("Git left conflicts in the index. {}", error.message),
        },
        Ok(false) => error,
        Err(read) => WriteError::inspect(format!(
            "{} Could not check for conflicts. {read}",
            error.git_text()
        )),
    }
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
    /// Rename or copy source shown in the same comparison.
    original: Option<String>,
    source: String,
    hunks: Vec<HunkRange>,
}

/// Whether `action` leaves the cached comparison as it is: a whole-file
/// index write to a path the comparison does not show, or a commit under an
/// unstaged comparison. The controller applies the same rule before keeping
/// the patch on screen.
fn keeps_patch(action: &Action, patch: &PatchCache) -> bool {
    let shown = |path: &str| path == patch.path || patch.original.as_deref() == Some(path);
    match action {
        Action::StageFile { reviewed, .. } => {
            !shown(&reviewed.path) && !reviewed.original_path.as_deref().is_some_and(shown)
        }
        Action::MarkResolved { path, .. } => !shown(path),
        // A commit leaves the index and working tree, so an unstaged
        // comparison is unchanged; a staged one empties.
        Action::Commit { .. } => !patch.staged,
        _ => false,
    }
}

/// Reads a comparison and prepares it for display. Returns the hunk cache
/// for later hunk and line actions when the patch has addressable hunks.
fn read_diff(
    g: &Git,
    id: Option<String>,
    path: String,
    staged: bool,
    token: u32,
) -> Result<(Reply, Option<PatchCache>), String> {
    let crate::git::DiffComparison {
        patch,
        original_path,
    } = g.compare(id.as_deref(), &path, staged)?;
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
    let discard_blocked = (!staged && !hunks.is_empty())
        .then(|| crate::hunk_edit::discard_blocked_reason(&patch))
        .flatten()
        .map(str::to_owned);
    let preview = diff::classify(&patch, fits);
    // Only text patches have displayable lines. Other previews are
    // described by the view instead of synthetic code rows.
    let mut lines = if preview == diff::Preview::Text {
        diff::parse(&patch)
    } else {
        vec![]
    };
    let cache = (!hunks.is_empty()).then(|| PatchCache {
        token,
        staged,
        hunks,
        path: path.clone(),
        original: original_path.clone(),
        source: patch,
    });
    if let Some(first) = lines.iter().position(|l| l.text.starts_with("@@ ")) {
        lines.drain(..first);
    }
    let content = diff::prepare(lines);
    Ok((
        Reply::Diff {
            id,
            path,
            staged,
            original_path,
            content,
            hunks: titles,
            hunk_blocked,
            discard_blocked,
            token,
            preview,
        },
        cache,
    ))
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
                | Action::Lines { .. }
                | Action::DiscardLines { .. }
                | Action::Commit { .. }
                | Action::Operation { .. }
                | Action::RunOperation { .. }
        );
        let keeps_patch = self
            .patch
            .as_ref()
            .is_some_and(|patch| keeps_patch(&action, patch));
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
        // A failed write can still change the index or worktree. A hunk or
        // line write that read its comparison again keeps that fresh cache.
        // A write to another file cannot change the cached comparison.
        if mutation && !keeps_patch && !matches!(result, Ok(Reply::IndexDone { diff: Some(_), .. }))
        {
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
        let journal = &self.journal;
        let history = &mut self.history;
        // The journal check reads the repository on its own, so it runs beside
        // the snapshot rather than after it.
        let (snapshot, journal) = std::thread::scope(|scope| {
            let journal = scope.spawn(|| journal.lock().unwrap().status(g));
            let snapshot = g.snapshot_cached(
                limit.clamp(1, gitcrab_core::controller::HISTORY_MAX),
                mode,
                history,
            );
            (snapshot, journal.join())
        });
        let (mut snapshot, graph, _changed) = snapshot?;
        snapshot.journal = journal.map_err(|_| "Undo history check failed")?;
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
                // The message and the file list are independent reads.
                let (message, files) = std::thread::scope(|scope| {
                    let files = scope.spawn(|| g.changed_files(&id));
                    (g.commit_message_details(&id), files.join())
                });
                let (author_email, body) = message?;
                Reply::Details {
                    files: files.map_err(|_| "Git changed-files read failed")??,
                    id,
                    body,
                    author_email,
                }
            }
            Action::Diff { id, path, staged } => {
                return Ok(self.diff_reply(id, path, staged, sequence)?);
            }
            Action::StageFile { reviewed, unstage } => {
                g.stage_reviewed(&reviewed, unstage)?;
                let message = format!(
                    "{} {}",
                    if unstage { "Unstaged" } else { "Staged" },
                    reviewed.display_path()
                );
                return Ok(self.index_done(message, None, sequence));
            }
            Action::StageAll { unstage } => {
                let outcome = g.stage_all(unstage)?;
                let mut message = format!(
                    "{} the whole worktree, {} {} changed",
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
                        ". {} conflicted {} left for resolution",
                        outcome.conflicts,
                        if outcome.conflicts == 1 {
                            "path"
                        } else {
                            "paths"
                        }
                    ));
                }
                return Ok(self.index_done(message, None, sequence));
            }
            Action::MarkResolved {
                path,
                allow_markers,
            } => {
                g.mark_resolved(&path, allow_markers)?;
                let message = format!(
                    "Marked {} resolved. Its merged content is now staged.",
                    crate::git::display_path(&path)
                );
                return Ok(self.index_done(message, None, sequence));
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
                // The conflict check runs beside the diff comparison.
                let (diff, unmerged) = std::thread::scope(|scope| {
                    let unmerged = scope.spawn(|| g.reject_unmerged_paths(&[&patch.path]));
                    (g.diff(None, &patch.path, patch.staged), unmerged.join())
                });
                if diff? != patch.source {
                    return Err("Diff changed since review. Reopen the file and retry.".into());
                }
                let hunk = patch.hunks.get(index).ok_or("Hunk no longer exists")?;
                unmerged.map_err(|_| WriteError::from("Git conflict check failed"))??;
                g.apply_hunk(&hunk.materialize(&patch.source), unstage)?;
                let message = format!(
                    "{} hunk in {}",
                    if unstage { "Unstaged" } else { "Staged" },
                    crate::git::display_path(&patch.path)
                );
                let path = patch.path.clone();
                return Ok(self.index_done(message, Some((path, unstage)), sequence));
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
                let message = format!("Discarded a hunk in {}.", patch.path);
                let path = patch.path.clone();
                return Ok(self.index_done(message, Some((path, false)), sequence));
            }
            Action::Lines {
                token,
                rows,
                unstage,
            } => {
                let patch = self
                    .patch
                    .as_ref()
                    .ok_or("Reopen the diff before staging these lines")?;
                if patch.token != token || patch.staged != unstage {
                    return Err("Diff changed. Reopen the file and retry.".into());
                }
                if g.diff(None, &patch.path, patch.staged)? != patch.source {
                    return Err("Diff changed since review. Reopen the file and retry.".into());
                }
                let keep = if unstage {
                    crate::hunk_edit::Keep::After
                } else {
                    crate::hunk_edit::Keep::Before
                };
                let partial = crate::hunk_edit::select_lines(&patch.source, &rows, keep)?;
                g.reject_unmerged_paths(&[&patch.path])?;
                g.apply_hunk(&partial, unstage)?;
                let message = format!(
                    "{} selected lines in {}",
                    if unstage { "Unstaged" } else { "Staged" },
                    crate::git::display_path(&patch.path)
                );
                let path = patch.path.clone();
                return Ok(self.index_done(message, Some((path, unstage)), sequence));
            }
            Action::DiscardLines { token, rows } => {
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
                let partial = crate::hunk_edit::select_lines(
                    &patch.source,
                    &rows,
                    crate::hunk_edit::Keep::After,
                )?;
                g.discard_hunk(&patch.path, &partial)?;
                let message = format!(
                    "Discarded the selected lines in {}.",
                    crate::git::display_path(&patch.path)
                );
                let path = patch.path.clone();
                return Ok(self.index_done(message, Some((path, false)), sequence));
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
                    message: format!(
                        "Verified commit {sha} in {}",
                        gitcrab_core::presentation::display_location(&g.path)
                    ),
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
            Action::RunOperation {
                op,
                target,
                destination,
                expected,
            } => {
                if op.is_network() || op.journal_direction().is_some() {
                    return Err("Review this operation before running it.".into());
                }
                // The capture is this write's revalidation: it checks HEAD and
                // the index against `destination` and resolves the target now.
                let reviewed = ReviewedOperation::capture(g, op, target, destination)?;
                // Switching to the branch that tracks a remote branch does not
                // use the remote branch's commit, so its tip may move.
                let switches = reviewed
                    .remote_branch
                    .as_ref()
                    .is_some_and(|remote| remote.kind == RemoteCheckoutKind::SwitchTracking);
                if expected.is_some() && !switches && reviewed.target_id != expected {
                    return Err("The target moved since the repository was last read. Wait for the refresh, then try again.".into());
                }
                if reviewed.op.finishes_sequence() {
                    reviewed.run_captured(g)?;
                    self.sequence_done(g, &reviewed)
                } else {
                    let before = crate::journal::Before::capture(g, &reviewed);
                    let ran = reviewed.run_captured(g)?;
                    self.operation_done(g, &reviewed, before, ran)
                }
            }
            Action::Operation { reviewed } if reviewed.op.finishes_sequence() => {
                reviewed.execute(g)?;
                self.sequence_done(g, &reviewed)
            }
            Action::Operation { reviewed } => {
                let before = crate::journal::Before::capture(g, &reviewed);
                let ran = reviewed.execute(g)?;
                self.operation_done(g, &reviewed, before, ran)
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

    /// Reads a comparison, caches its hunks under `token` for later hunk and
    /// line actions, and returns it ready for display.
    fn diff_reply(
        &mut self,
        id: Option<String>,
        path: String,
        staged: bool,
        token: u32,
    ) -> Result<Reply, String> {
        let g = self.repo.as_ref().ok_or("Open a local repository first")?;
        self.patch = None;
        let (reply, cache) = read_diff(g, id, path, staged, token)?;
        self.patch = cache;
        Ok(reply)
    }

    /// Finishes an index or working-tree write by reading what it changed.
    ///
    /// `diff` names the comparison a hunk or line write started from. Only
    /// that comparison is read again: it is what the user waits for, and a
    /// consistent read of the working changes takes longer, so the file lists
    /// follow in the usual refresh. A whole-file write has no comparison to
    /// show, so its reply carries the working changes and no refresh follows.
    /// Either part is left out when it cannot be read consistently, or when
    /// the comparison is now empty, and the controller refreshes as before.
    fn index_done(&mut self, message: String, diff: Option<(String, bool)>, token: u32) -> Reply {
        if diff.is_some() {
            // Replaced below by the comparison read again, if any is left.
            self.patch = None;
        }
        let Some(g) = self.repo.as_ref() else {
            return Reply::Done {
                message,
                committed_draft: None,
            };
        };
        let read_started = std::time::Instant::now();
        let (worktree, diff) = match diff {
            Some((path, staged)) => (None, Some(read_diff(g, None, path, staged, token))),
            None => {
                let journal = &self.journal;
                let worktree = std::thread::scope(|scope| {
                    let undo = scope.spawn(|| journal.lock().unwrap().status(g));
                    let state = g.worktree_state();
                    Some((state.ok()?, undo.join().ok()?))
                });
                (worktree, None)
            }
        };
        let diff = match diff {
            Some(Ok((reply, Some(cache)))) => {
                self.patch = Some(cache);
                Some(Box::new(reply))
            }
            _ => None,
        };
        Reply::IndexDone {
            message,
            worktree: worktree.map(|((status, reviewed), journal)| WorktreeState {
                conflict_markers: g.conflict_marker_paths(&status),
                status,
                reviewed,
                journal,
                read_started: Some(read_started),
            }),
            diff,
        }
    }

    /// Records a completed local operation in the journal and words its result.
    fn operation_done(
        &self,
        g: &Git,
        reviewed: &ReviewedOperation,
        before: Result<crate::journal::Before, String>,
        ran: Ran,
    ) -> Reply {
        let message = match g.head_state() {
            Ok(head) => operation_message(reviewed, &head, ran),
            Err(_) => format!("{} completed", reviewed.op.label()),
        };
        let note = self
            .journal
            .lock()
            .unwrap()
            .record_operation(g, reviewed, before);
        let message = match &reviewed.remote_branch {
            Some(remote) => remote.outcome(),
            None => message,
        };
        Reply::Done {
            message: match note {
                Some(note) => format!("{}. {note}", message.trim_end_matches('.')),
                None => message,
            },
            committed_draft: None,
        }
    }

    /// Words the result of Abort, Continue or Skip.
    ///
    /// None of them is recorded for Undo. Continue and Skip make new commits,
    /// so like any new action they clear the redo stack. Abort returns to the
    /// state the journal's entries were checked against before the operation
    /// started, so the stacks stay as they are.
    fn sequence_done(&self, g: &Git, reviewed: &ReviewedOperation) -> Reply {
        let title = crate::git::Sequence::from_name(&reviewed.target)
            .map_or("Operation", crate::git::Sequence::title);
        if !matches!(reviewed.op, Operation::Abort) {
            self.journal.lock().unwrap().clear_redo();
        }
        let finished = g
            .operation_markers()
            .is_ok_and(|markers| markers.is_empty());
        let message = match (&reviewed.op, finished) {
            (Operation::Abort, _) => format!("{title} aborted."),
            (Operation::Skip, true) => format!("Commit skipped. {title} finished."),
            (Operation::Skip, false) => format!("Commit skipped. The {} goes on.", reviewed.target),
            (_, true) => format!("{title} finished."),
            (_, false) => format!("{title} continued. Git stopped again."),
        };
        let committed_draft = match &reviewed.op {
            Operation::Continue { message } => message.clone(),
            _ => None,
        };
        Reply::Done {
            message,
            committed_draft,
        }
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
    use crate::{
        graph,
        platform::{RepositoryLocation, RepositoryLocationExt},
    };
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
            remote_branch: None,
            merge_ref: None,
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
    fn hunk_writes_return_the_diff_that_the_next_hunk_write_uses() {
        let fixture = Fixture::new();
        let lines = |edited: &[usize]| {
            (0..40)
                .map(|i| {
                    if edited.contains(&i) {
                        format!("edited {i}\n")
                    } else {
                        format!("line {i}\n")
                    }
                })
                .collect::<String>()
        };
        fixture.write("a.txt", &lines(&[]));
        let git = &fixture.git;
        git.run(&["add", "a.txt"]).unwrap();
        git.run(&["commit", "-qm", "base"]).unwrap();
        fixture.write("a.txt", &lines(&[2, 20, 38]));
        let mut session = Session {
            repo: Some(git.clone()),
            epoch: 1,
            ..Default::default()
        };
        let hunks = |reply: &Reply| match reply {
            Reply::Diff { hunks, token, .. } => (hunks.len(), *token),
            other => panic!("expected a diff, got {other:?}"),
        };
        let diff = Action::Diff {
            id: None,
            path: "a.txt".into(),
            staged: false,
        };
        let (count, mut token) = hunks(
            &session
                .execute(Request {
                    epoch: 1,
                    sequence: 1,
                    action: diff,
                })
                .result
                .unwrap(),
        );
        assert_eq!(count, 3);
        // Each write returns the remaining hunks under its own token, which
        // the next write uses without reading the diff again.
        for (sequence, remaining) in [(2, 2), (3, 1)] {
            let reply = session
                .execute(Request {
                    epoch: 1,
                    sequence,
                    action: Action::Hunk {
                        token,
                        index: 0,
                        unstage: false,
                    },
                })
                .result
                .unwrap();
            // Only the diff: the file lists follow in the refresh.
            let Reply::IndexDone {
                message,
                worktree: None,
                diff: Some(diff),
            } = reply
            else {
                panic!("expected the write to carry its diff");
            };
            assert_eq!(message, "Staged hunk in a.txt");
            (_, token) = hunks(&diff);
            assert_eq!((hunks(&diff).0, token), (remaining, sequence));
        }
        // Staging the last hunk leaves no diff; nothing is read again.
        let reply = session
            .execute(Request {
                epoch: 1,
                sequence: 4,
                action: Action::Hunk {
                    token,
                    index: 0,
                    unstage: false,
                },
            })
            .result
            .unwrap();
        assert!(matches!(
            reply,
            Reply::IndexDone {
                worktree: None,
                diff: None,
                ..
            }
        ));
        assert!(session.patch.is_none());
        assert_eq!(
            git.run(&["diff", "--cached", "--stat"]).unwrap().trim(),
            "a.txt | 6 +++---\n 1 file changed, 3 insertions(+), 3 deletions(-)"
        );
        // A whole-file write carries the working changes instead, matching
        // a fresh identity.
        let reply = session
            .execute(Request {
                epoch: 1,
                sequence: 5,
                action: Action::StageAll { unstage: true },
            })
            .result
            .unwrap();
        let Reply::IndexDone {
            worktree: Some(worktree),
            diff: None,
            ..
        } = reply
        else {
            panic!("expected working changes, got {reply:?}");
        };
        assert!(matches!(&worktree.status[..], [f] if (f.index, f.worktree) == (' ', 'M')));
        assert_eq!(worktree.reviewed, git.write_identity().unwrap());
    }

    #[test]
    fn writing_another_file_keeps_the_open_diffs_hunks() {
        let fixture = Fixture::new();
        fixture.write("a.txt", "a\n");
        fixture.write("b.txt", "b\n");
        let git = &fixture.git;
        git.run(&["add", "."]).unwrap();
        git.run(&["commit", "-qm", "base"]).unwrap();
        fixture.write("a.txt", "a edited\n");
        fixture.write("b.txt", "b edited\n");
        let mut session = Session {
            repo: Some(git.clone()),
            epoch: 1,
            ..Default::default()
        };
        let mut run = |sequence, action| {
            session
                .execute(Request {
                    epoch: 1,
                    sequence,
                    action,
                })
                .result
        };
        run(
            1,
            Action::Diff {
                id: None,
                path: "a.txt".into(),
                staged: false,
            },
        )
        .unwrap();
        let b = git
            .worktree_state()
            .unwrap()
            .0
            .into_iter()
            .find(|f| f.path == "b.txt")
            .unwrap();
        run(
            2,
            Action::StageFile {
                reviewed: b,
                unstage: false,
            },
        )
        .unwrap();
        // a.txt's hunk from before b was staged still applies.
        run(
            3,
            Action::Hunk {
                token: 1,
                index: 0,
                unstage: false,
            },
        )
        .unwrap();
        assert_eq!(
            git.run(&["diff", "--cached", "--name-only"]).unwrap(),
            "a.txt\nb.txt\n"
        );
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
    fn local_operations_resolve_and_run_in_one_request() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("tracked.txt", "base\n");
        git.stage("tracked.txt").unwrap();
        git.commit("base").unwrap();
        let mut session = Session {
            repo: Some(git.clone()),
            epoch: 1,
            ..Default::default()
        };
        let mut run = |op, target: &str, destination| {
            session
                .execute(Request {
                    epoch: 1,
                    sequence: 1,
                    action: Action::RunOperation {
                        op,
                        target: target.into(),
                        destination,
                        expected: None,
                    },
                })
                .result
        };
        let destination = git.write_identity().unwrap();
        assert!(run(Operation::CreateBranch, "topic", destination.clone()).is_ok());
        assert!(git.ref_exists("refs/heads/topic").unwrap());
        // The capture still rejects a HEAD or index that moved since the snapshot.
        fixture.write("tracked.txt", "staged\n");
        git.stage("tracked.txt").unwrap();
        let moved = run(Operation::Checkout, "topic", destination.clone());
        assert!(moved.is_err_and(|failure| failure.message.contains("changed")));
        assert!(git
            .write_identity()
            .unwrap()
            .symbolic_head
            .ends_with("/main"));
        // Network and journal operations keep their separate review.
        let current = git.write_identity().unwrap();
        assert!(run(Operation::Push, "", current.clone()).is_err());
        assert!(run(Operation::Undo, "", current).is_err());
    }

    /// Runs `op` on `target` against the current HEAD and index and returns
    /// the result message or the failure.
    fn run_local(session: &mut Session, op: Operation, target: &str) -> Result<String, WriteError> {
        let destination = session.repo.as_ref().unwrap().write_identity().unwrap();
        match session.run(
            1,
            1,
            Action::RunOperation {
                op,
                target: target.into(),
                destination,
                expected: None,
            },
        )? {
            Reply::Done { message, .. } => Ok(message),
            other => panic!("expected a result, got {other:?}"),
        }
    }

    #[test]
    fn local_operations_word_their_result_from_head_after_the_write() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("tracked.txt", "base\n");
        git.stage("tracked.txt").unwrap();
        git.commit("base").unwrap();
        let mut session = Session {
            repo: Some(git.clone()),
            epoch: 1,
            ..Default::default()
        };
        let short = |id: &str| gitcrab_core::presentation::short_id(id).to_owned();
        // Each message may end with a note about Undo, so only its start is fixed.
        let starts = |message: Result<String, WriteError>, expected: &str| {
            let message = message.unwrap();
            assert!(message.starts_with(expected), "{message:?}");
        };
        starts(
            run_local(&mut session, Operation::CreateBranch, "topic"),
            &format!(
                "Created branch topic at {}",
                short(&git.resolve_commit("HEAD").unwrap())
            ),
        );
        starts(
            run_local(&mut session, Operation::Checkout, "topic"),
            "Checked out topic",
        );
        fixture.write("topic.txt", "topic\n");
        git.stage("topic.txt").unwrap();
        git.commit("topic").unwrap();
        let topic = git.resolve_commit("HEAD").unwrap();
        starts(
            run_local(&mut session, Operation::Checkout, "main"),
            "Checked out main",
        );
        starts(
            run_local(&mut session, Operation::Merge, "topic"),
            "Fast-forwarded main to topic",
        );
        starts(
            run_local(&mut session, Operation::Merge, "topic"),
            "Already up to date: main already contains topic",
        );
        starts(
            run_local(&mut session, Operation::Revert, &topic),
            &format!("Reverted {} on main", short(&topic)),
        );
        starts(
            run_local(&mut session, Operation::CherryPick, &topic),
            &format!("Cherry-picked {} onto main", short(&topic)),
        );
        git.checkout("topic").unwrap();
        fixture.write("other.txt", "other\n");
        git.stage("other.txt").unwrap();
        git.commit("other").unwrap();
        git.checkout("main").unwrap();
        starts(
            run_local(&mut session, Operation::Merge, "topic"),
            "Merged topic into main",
        );
        starts(
            run_local(&mut session, Operation::Checkout, &topic),
            &format!(
                "HEAD is detached at {}. Create a branch before committing.",
                short(&topic)
            ),
        );
        starts(
            run_local(&mut session, Operation::Checkout, "main~1"),
            "Checked out main~1. HEAD is detached at ",
        );
        fixture.write("tracked.txt", "edited\n");
        starts(
            run_local(&mut session, Operation::Stash, ""),
            "Stashed changes to tracked files; untracked files stay in the working tree",
        );
    }

    #[test]
    fn diffs_say_up_front_when_discard_cannot_act_on_their_hunks() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("tracked.txt", "base\n");
        git.stage("tracked.txt").unwrap();
        git.commit("base").unwrap();
        fixture.write("tracked.txt", "edited\n");
        fixture.write("new.txt", "new\n");
        let mut session = Session {
            repo: Some(git.clone()),
            epoch: 1,
            ..Default::default()
        };
        let mut blocked = |path: &str| {
            let reply = session
                .run(
                    1,
                    1,
                    Action::Diff {
                        id: None,
                        path: path.into(),
                        staged: false,
                    },
                )
                .unwrap();
            let Reply::Diff {
                hunks,
                hunk_blocked,
                discard_blocked,
                ..
            } = reply
            else {
                panic!("expected a diff, got {reply:?}")
            };
            assert!(!hunks.is_empty() && hunk_blocked.is_none(), "{path}");
            discard_blocked
        };
        assert_eq!(blocked("tracked.txt"), None);
        assert!(blocked("new.txt").is_some_and(|reason| reason.contains("new")));
        let hunk = git
            .compare(None, "new.txt", false)
            .map(|comparison| {
                crate::git::hunk_ranges(&comparison.patch)[0].materialize(&comparison.patch)
            })
            .unwrap();
        let refused = git.discard_hunk("new.txt", &hunk).unwrap_err();
        assert_eq!(
            Some(refused.message.as_str()),
            crate::hunk_edit::discard_blocked_reason(&hunk.patch)
        );
    }

    #[test]
    fn pop_stash_restores_the_staged_split_and_conflicts_keep_the_stash() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("app.txt", "base\n");
        git.stage("app.txt").unwrap();
        git.commit("base").unwrap();
        let mut session = Session {
            repo: Some(git.clone()),
            epoch: 1,
            ..Default::default()
        };
        fixture.write("app.txt", "staged\n");
        git.stage("app.txt").unwrap();
        fixture.write("app.txt", "staged and edited\n");
        run_local(&mut session, Operation::Stash, "").unwrap();
        let popped = run_local(&mut session, Operation::PopStash, "").unwrap();
        assert!(
            popped.starts_with("Stash popped. The stash entry was removed.")
                && !popped.contains("staged changes could not"),
            "{popped:?}"
        );
        let status = git.snapshot(1).unwrap().status;
        assert_eq!((status[0].index, status[0].worktree), ('M', 'M'));
        assert_eq!(git.run(&["show", ":app.txt"]).unwrap(), "staged\n");
        assert!(git.stash_entries().unwrap().is_empty());

        run_local(&mut session, Operation::Stash, "").unwrap();
        fixture.write("app.txt", "conflicting\n");
        git.stage("app.txt").unwrap();
        git.commit("conflicting").unwrap();
        let failed = run_local(&mut session, Operation::PopStash, "").unwrap_err();
        assert_eq!(failed.kind, FailureKind::Conflict, "{failed:?}");
        assert_eq!(
            failed.message.matches("Inspect Working changes").count(),
            1,
            "{failed:?}"
        );
        assert_eq!(git.stash_entries().unwrap().len(), 1);
    }

    #[test]
    fn merging_a_branch_by_name_names_it_and_needs_it_where_it_was_chosen() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("tracked.txt", "base\n");
        git.stage("tracked.txt").unwrap();
        git.commit("base").unwrap();
        git.create_branch("feature/clean").unwrap();
        git.checkout("feature/clean").unwrap();
        fixture.write("feature.txt", "feature\n");
        git.stage("feature.txt").unwrap();
        git.commit("feature").unwrap();
        let chosen = git.resolve_commit("HEAD").unwrap();
        git.checkout("main").unwrap();
        fixture.write("main.txt", "main\n");
        git.stage("main.txt").unwrap();
        git.commit("main").unwrap();
        let mut session = Session {
            repo: Some(git.clone()),
            epoch: 1,
            ..Default::default()
        };
        let mut merge = |expected: &str| {
            session
                .execute(Request {
                    epoch: 1,
                    sequence: 1,
                    action: Action::RunOperation {
                        op: Operation::Merge,
                        target: "feature/clean".into(),
                        destination: git.write_identity().unwrap(),
                        expected: Some(expected.into()),
                    },
                })
                .result
        };
        let other = Git::open(&git.path).unwrap();
        other
            .write(&["branch", "-f", "feature/clean", "main"])
            .unwrap();
        assert_eq!(
            git.branch_ref("feature/clean").unwrap().as_deref(),
            Some("refs/heads/feature/clean")
        );
        let moved = merge(&chosen).unwrap_err();
        assert!(moved.message.contains("target moved"), "{}", moved.message);
        other
            .write(&["branch", "-f", "feature/clean", &chosen])
            .unwrap();
        assert!(merge(&chosen).is_ok());
        assert_eq!(
            git.run(&["log", "-1", "--format=%s"]).unwrap().trim(),
            "Merge branch 'feature/clean'"
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
    fn a_stopped_merge_allows_only_its_own_steps_and_only_new_conflicts_count() {
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
        let clean = git.write_identity().unwrap();
        let mut session = Session {
            repo: Some(git.clone()),
            epoch: 1,
            ..Default::default()
        };
        let mut run = |op, target: &str| {
            session
                .execute(Request {
                    epoch: 1,
                    sequence: 1,
                    action: Action::RunOperation {
                        op,
                        target: target.into(),
                        destination: git.write_identity().unwrap(),
                        expected: None,
                    },
                })
                .result
        };
        let failure = run(Operation::Merge, "feature").unwrap_err();
        assert_eq!(failure.kind, FailureKind::Conflict);

        let refused = run(Operation::Stash, "").unwrap_err();
        assert_eq!(refused.kind, FailureKind::Rejected);
        assert_eq!(refused.message, crate::git::OPERATION_IN_PROGRESS);
        let conflicted = git.write_identity().unwrap();
        let error = git.stash_with_message("during merge").unwrap_err();
        assert_eq!(
            operation_failure(git, error, &conflicted).kind,
            FailureKind::GitFailure
        );
        let error = WriteError::git("Git exited with 1.".into());
        assert_eq!(
            operation_failure(git, error, &clean).kind,
            FailureKind::Conflict
        );

        let wrong = run(Operation::Abort, "rebase").unwrap_err();
        assert!(wrong.message.contains("no longer in progress"));
        let unresolved = run(Operation::Continue { message: None }, "merge").unwrap_err();
        assert_eq!(
            unresolved.message,
            gitcrab_core::commands::UNRESOLVED_CONFLICTS
        );
        let Ok(Reply::Done { message, .. }) = run(Operation::Abort, "merge") else {
            panic!("abort failed");
        };
        assert_eq!(message, "Merge aborted.");
        assert_eq!(git.write_identity().unwrap(), clean);

        assert!(run(Operation::Merge, "feature").is_err());
        fixture.write("conflict.txt", "resolved\n");
        git.mark_resolved("conflict.txt", false).unwrap();
        let draft = "Merge feature by hand".to_owned();
        let continued = run(
            Operation::Continue {
                message: Some(draft.clone()),
            },
            "merge",
        );
        let Ok(Reply::Done {
            message,
            committed_draft,
        }) = continued
        else {
            panic!("continue failed: {continued:?}");
        };
        assert_eq!(message, "Merge finished.");
        assert_eq!(committed_draft, Some(draft.clone()));
        let merge = git.run(&["show", "-s", "--format=%P%n%s", "HEAD"]).unwrap();
        let (parents, subject) = merge.trim().split_once('\n').unwrap();
        assert_eq!(parents.split_whitespace().count(), 2);
        assert_eq!(subject, draft);
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
            let Ok(Reply::IndexDone { message, .. }) = run(Action::StageAll { unstage }) else {
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

        fn checkout(session: &mut Session, target: &str) -> Result<String, String> {
            let destination = session.repo.as_ref().unwrap().write_identity().unwrap();
            match session
                .execute(Request {
                    epoch: 1,
                    sequence: 2,
                    action: Action::RunOperation {
                        op: Operation::Checkout,
                        target: target.into(),
                        destination,
                        expected: None,
                    },
                })
                .result
            {
                Ok(Reply::Done { message, .. }) => Ok(message),
                Ok(other) => Err(format!("unexpected reply {other:?}")),
                Err(failure) => Err(failure.message),
            }
        }

        fn journal_step(session: &Session, direction: crate::journal::Direction) {
            let git = session.repo.as_ref().unwrap();
            let mut journal = session.journal.lock().unwrap();
            let reviewed = journal
                .review(git, direction, git.write_identity().unwrap())
                .unwrap();
            journal.execute(git, &reviewed).unwrap();
        }

        fn head(git: &Git) -> (String, String) {
            git.head_state().unwrap()
        }

        /// Publishes branch `name` from `other` and fetches it into `local`.
        fn remote_branch(local: &Fixture, other: &Fixture, name: &str) -> String {
            other.git.run(&["switch", "--quiet", "-c", name]).unwrap();
            let tip = commit(other, &format!("{name}.txt"), "remote\n");
            other
                .git
                .run(&["push", "--quiet", "-u", "origin", name])
                .unwrap();
            local.git.run(&["fetch", "--quiet"]).unwrap();
            tip
        }

        #[test]
        fn checking_out_a_tracked_remote_branch_switches_to_its_local_branch() {
            use crate::journal::Direction;
            let (local, _other) = connected();
            let git = &local.git;
            let main = git.resolve_commit("HEAD").unwrap();
            git.run(&["switch", "--quiet", "-c", "topic"]).unwrap();
            let topic = commit(&local, "topic.txt", "topic\n");
            let mut session = session(git);
            assert_eq!(
                checkout(&mut session, "origin/main").unwrap(),
                "Switched to main, which tracks origin/main."
            );
            assert_eq!(head(git), ("refs/heads/main".into(), main));
            let status = session.journal.lock().unwrap().status(git);
            assert_eq!(status.summary(Direction::Undo), Some("checkout of main"));
            journal_step(&session, Direction::Undo);
            assert_eq!(head(git), ("refs/heads/topic".into(), topic));
        }

        #[test]
        fn checking_out_an_untracked_remote_branch_creates_a_tracking_branch() {
            use crate::journal::{Blocker, Direction, StepState};
            let (local, other) = connected();
            let git = &local.git;
            let main = git.resolve_commit("HEAD").unwrap();
            let tip = remote_branch(&local, &other, "feature");
            let mut session = session(git);
            assert_eq!(
                checkout(&mut session, "origin/feature").unwrap(),
                "Created feature tracking origin/feature and switched to it."
            );
            assert_eq!(head(git), ("refs/heads/feature".into(), tip.clone()));
            assert_eq!(
                git.upstream_of("refs/heads/feature").unwrap().as_deref(),
                Some("refs/remotes/origin/feature")
            );
            let status = session.journal.lock().unwrap().status(git);
            assert_eq!(
                status.summary(Direction::Undo),
                Some("checkout of origin/feature as new branch feature")
            );

            journal_step(&session, Direction::Undo);
            assert_eq!(head(git), ("refs/heads/main".into(), main.clone()));
            assert_eq!(git.ref_commit("refs/heads/feature").unwrap(), None);
            journal_step(&session, Direction::Redo);
            assert_eq!(head(git), ("refs/heads/feature".into(), tip.clone()));
            assert_eq!(
                git.upstream_of("refs/heads/feature").unwrap().as_deref(),
                Some("refs/remotes/origin/feature")
            );

            git.write(&["commit", "--quiet", "--allow-empty", "-m", "more"])
                .unwrap();
            let status = |session: &Session| session.journal.lock().unwrap().status(git);
            assert_eq!(
                status(&session).state(Direction::Undo),
                StepState::Blocked(Blocker::HeadMoved)
            );
            git.write(&["reset", "--quiet", "--hard", &tip]).unwrap();
            git.write(&["branch", "-f", "main", &tip]).unwrap();
            assert_eq!(
                status(&session).state(Direction::Undo),
                StepState::Blocked(Blocker::BranchMoved)
            );
            git.write(&["branch", "-f", "main", &main]).unwrap();
            assert_eq!(status(&session).state(Direction::Undo), StepState::Ready);

            journal_step(&session, Direction::Undo);
            assert!(checkout(&mut session, "refs/remotes/origin/feature")
                .unwrap()
                .starts_with("Created feature tracking origin/feature"));
            assert_eq!(head(git), ("refs/heads/feature".into(), tip));
        }

        #[test]
        fn redo_of_a_tracking_checkout_needs_the_branch_name_free() {
            use crate::journal::{Blocker, Direction, StepState};
            let (local, other) = connected();
            let git = &local.git;
            let tip = remote_branch(&local, &other, "feature");
            let mut session = session(git);
            checkout(&mut session, "origin/feature").unwrap();
            journal_step(&session, Direction::Undo);
            git.write(&["branch", "feature", &tip]).unwrap();
            assert_eq!(
                session
                    .journal
                    .lock()
                    .unwrap()
                    .status(git)
                    .state(Direction::Redo),
                StepState::Blocked(Blocker::BranchExists)
            );
        }

        #[test]
        fn a_remote_branch_whose_name_is_taken_locally_still_detaches() {
            let (local, other) = connected();
            let git = &local.git;
            let tip = remote_branch(&local, &other, "feature");
            git.write(&["branch", "feature"]).unwrap();
            let mut session = session(git);
            assert_eq!(
                checkout(&mut session, "origin/feature").unwrap(),
                "Checked out origin/feature in detached HEAD, because local branch feature already exists and does not track it."
            );
            assert_eq!(head(git), (String::new(), tip));
            let main = git.resolve_commit("main").unwrap();
            assert_eq!(
                checkout(&mut session, &main).unwrap(),
                format!(
                    "HEAD is detached at {}. Create a branch before committing.",
                    gitcrab_core::presentation::short_id(&main)
                )
            );
            assert_eq!(head(git), (String::new(), main));
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
