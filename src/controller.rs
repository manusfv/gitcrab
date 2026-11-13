//! Repository tab state and request coordination for the desktop client.
//!
//! Each lane tracks an active request and the latest pending replacement.
//! The caller dispatches returned requests to the worker and feeds completions
//! back to the controller, including completions for superseded requests.
use crate::{
    diff,
    git::{ReferenceKind, RepoSnapshot},
    graph::Graph,
    platform::RepositoryLocation,
    service::*,
};

/// Request lane used to track completions and coalesce replacements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Snapshot = 0,
    Details = 1,
    Diff = 2,
    Mutation = 3,
    OperationReview = 4,
}
#[derive(Default)]
struct Slot {
    active: Option<(u32, u32)>,
    pending: Option<Request>,
    latest: u32,
}
/// Displayable patch and its worker-provided hunk controls.
#[derive(Clone, Debug)]
pub struct Patch {
    /// Commit object ID, or `None` for a working changes comparison.
    pub id: Option<String>,
    /// Repository-relative destination path.
    pub path: String,
    /// Source path when this comparison detects a rename or copy.
    pub original_path: Option<String>,
    /// Whether a working changes comparison is against the index.
    pub staged: bool,
    pub content: diff::Content,
    /// Hunk titles in patch order. Their indices identify hunk actions.
    pub hunks: Vec<String>,
    /// User-facing reason to disable hunk actions, when applicable.
    pub hunk_blocked: Option<String>,
    /// Worker token tying a hunk action to this exact cached patch.
    pub token: u32,
    /// What the comparison can show. Only text previews have lines.
    pub preview: diff::Preview,
}

/// File comparison selected by the user, including while its patch is loading.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffSelection {
    /// Commit object ID, or `None` for a working changes comparison.
    pub id: Option<String>,
    pub path: String,
    /// Whether a working changes comparison is against the index.
    pub staged: bool,
}

struct ReopenDiff {
    selection: DiffSelection,
    generation: u64,
}

/// Worker actions for an accepted refresh or history paging request.
pub struct RefreshPlan {
    /// Read kinds to cancel before dispatching the replacement.
    pub cancel: [ReadKind; 2],
    /// Request to send now. `None` means the replacement was queued in its lane.
    pub dispatch: Option<(Lane, Request)>,
}

/// Owns a tab's repository data, selections, draft and in-flight requests.
///
/// Pass each submitted request's lane and response to [`Self::complete`].
/// Use [`Self::accepts_response`] first when the response may also trigger
/// effects outside the controller.
pub struct Controller {
    /// Repository generation used to identify obsolete responses.
    pub epoch: u32,
    sequence: u32,
    slots: [Slot; 5],
    pub path: String,
    pub snapshot: Option<RepoSnapshot>,
    pub graph: Graph,
    /// Selected commit object ID, or `None` for working changes.
    pub selected: Option<String>,
    previewed_ref: Option<(ReferenceKind, String)>,
    pub files: Vec<crate::git::FileChange>,
    pub commit_body: String,
    pub commit_author_email: String,
    pub details_failed: bool,
    /// Commit whose details are loaded. A commit's details never change, so a
    /// refresh only reads them again when they are missing.
    details_loaded: Option<String>,
    pub patch: Option<Patch>,
    /// Selected review context survives asynchronous reads and refreshes.
    pub diff_context: Option<DiffSelection>,
    pub draft: String,
    /// Revision of the commit input used to preserve edits made during a commit.
    pub draft_revision: u64,
    submitted_revision: Option<u64>,
    /// A repository mutation is in flight.
    pub busy: bool,
    /// The write retired a snapshot read in flight, so it refreshes when done.
    refresh_after_write: bool,
    /// The write in flight changes only the index or worktree.
    index_only_write: bool,
    pub operation_review: Option<Result<ReviewedOperation, String>>,
    pub notice: String,
    pub error: bool,
    operation_notice: Option<(String, bool)>,
    /// Limit of the installed history snapshot.
    pub limit: usize,
    requested_limit: Option<usize>,
    /// Worker execution time for the last accepted response, in milliseconds.
    pub worker_ms: f64,
    /// Time that request waited in the worker queue, in milliseconds.
    pub queue_ms: f64,
    reopen: Option<ReopenDiff>,
    selection_generation: u64,
    // Some(true) requires a full refresh because Git metadata changed.
    pending_watcher_refresh: Option<bool>,
    drafts: std::collections::HashMap<std::path::PathBuf, String>,
    /// Unmerged paths whose resolution GitCrab refused because of conflict markers.
    pub marker_refusals: std::collections::BTreeSet<String>,
    resolving: Option<String>,
    /// A commit or reference the user activated outside loaded history.
    reveal: Option<Reveal>,
    /// Set when a [`Reveal`] selected its commit, so the view can scroll to it.
    pub revealed: bool,
}

/// A target that is loaded page by page until it appears in history.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Reveal {
    Commit(String),
    Reference(ReferenceKind, String),
}

/// Result of activating a commit or reference that may be outside loaded history.
pub enum Seek {
    /// The target was loaded and is now selected. Dispatch its details read.
    Selected(Option<(Lane, Request)>),
    /// More history was requested. The target is selected when it arrives.
    Loading(Option<RefreshPlan>),
    /// The target cannot be reached. [`Controller::notice`] explains why.
    NotFound,
}

/// History depth at which automatic paging for a [`Reveal`] stops.
pub const REVEAL_LIMIT: usize = 20_000;
impl Default for Controller {
    fn default() -> Self {
        Self {
            epoch: 0,
            sequence: 0,
            slots: Default::default(),
            path: String::new(),
            snapshot: None,
            graph: Graph::default(),
            selected: None,
            previewed_ref: None,
            files: vec![],
            commit_body: String::new(),
            commit_author_email: String::new(),
            details_failed: false,
            details_loaded: None,
            patch: None,
            diff_context: None,
            draft: String::new(),
            draft_revision: 0,
            submitted_revision: None,
            busy: false,
            refresh_after_write: false,
            index_only_write: false,
            operation_review: None,
            notice: "Open a local repository to begin".into(),
            error: false,
            operation_notice: None,
            limit: 500,
            requested_limit: None,
            worker_ms: 0.,
            queue_ms: 0.,
            reopen: None,
            selection_generation: 0,
            pending_watcher_refresh: None,
            drafts: Default::default(),
            marker_refusals: Default::default(),
            resolving: None,
            reveal: None,
            revealed: false,
        }
    }
}
impl Controller {
    /// Clears the current notice and its retained operation feedback.
    pub fn dismiss_notice(&mut self) {
        self.notice.clear();
        self.error = false;
        self.operation_notice = None;
    }

    /// Updates the commit message and advances its revision when the text changes.
    pub fn set_draft(&mut self, value: String) {
        if self.draft != value {
            self.draft = value;
            self.draft_revision += 1;
        }
    }

    /// Reports whether this tab has a draft for the current or a previous worktree.
    pub fn has_drafts(&self) -> bool {
        !self.draft.is_empty() || self.drafts.values().any(|draft| !draft.is_empty())
    }

    fn draft_key(path: &str) -> std::path::PathBuf {
        // The desktop lifecycle supplies the worker's canonical worktree path.
        std::path::PathBuf::from(path)
    }
    fn invalidate(&mut self, lane: Lane) {
        self.sequence += 1;
        let s = &mut self.slots[lane as usize];
        s.latest = self.sequence;
        s.pending = None;
    }
    /// Starts opening a worktree after the user confirms trust in `location`.
    ///
    /// Saves the old worktree's draft in this tab and restores any draft for
    /// the new one. A mutation in flight or a worktree path that cannot be
    /// represented in UTF-8 prevents opening. If the snapshot lane is active,
    /// the open request is queued and this method returns `None`.
    pub fn open_trusted(&mut self, location: RepositoryLocation) -> Option<(Lane, Request)> {
        if self.busy {
            return None;
        }
        let path = location.worktree.to_str()?.to_owned();
        self.epoch += 1;
        self.drafts
            .insert(Self::draft_key(&self.path), self.draft.clone());
        self.draft = self
            .drafts
            .remove(&Self::draft_key(&path))
            .unwrap_or_default();
        self.path = path.clone();
        for l in [
            Lane::Snapshot,
            Lane::Details,
            Lane::Diff,
            Lane::OperationReview,
        ] {
            self.invalidate(l);
        }
        self.snapshot = None;
        self.graph = Graph::default();
        self.selected = None;
        self.previewed_ref = None;
        self.files.clear();
        self.commit_body.clear();
        self.commit_author_email.clear();
        self.details_failed = false;
        self.details_loaded = None;
        self.patch = None;
        self.diff_context = None;
        self.reopen = None;
        self.pending_watcher_refresh = None;
        self.marker_refusals.clear();
        self.reveal = None;
        self.limit = 500;
        self.requested_limit = None;
        self.notice = "Opening repository…".into();
        self.error = false;
        self.operation_notice = None;
        self.read(
            Lane::Snapshot,
            Action::Open {
                limit: 500,
                trust: RepositoryTrust::Confirmed(location),
            },
        )
    }
    /// Selects a commit by object ID, or returns to working changes with `None`.
    ///
    /// Clears the previous diff and requests details for a selected commit.
    pub fn select(&mut self, id: Option<String>) -> Option<(Lane, Request)> {
        self.reveal = None;
        self.invalidate(Lane::Details);
        self.invalidate(Lane::Diff);
        self.selected = id.clone();
        self.previewed_ref = None;
        self.files.clear();
        self.commit_body.clear();
        self.commit_author_email.clear();
        self.details_failed = false;
        self.details_loaded = None;
        self.patch = None;
        self.diff_context = None;
        self.reopen = None;
        id.and_then(|id| self.read(Lane::Details, Action::Details { id }))
    }

    /// Selects a reference's tip when it is in the loaded history.
    ///
    /// Uses the naming conventions of [`RepoSnapshot::loaded_ref_tip`]. The
    /// preview records the reference so refs at the same commit remain distinct.
    pub fn preview_ref(&mut self, kind: ReferenceKind, name: &str) -> Option<(Lane, Request)> {
        let Some(id) = self
            .snapshot
            .as_ref()
            .and_then(|s| s.loaded_ref_tip(kind, name))
        else {
            self.notice =
                "Reference tip is outside loaded history. Load more commits to reveal it.".into();
            return None;
        };
        let job = self.select(Some(id.to_owned()));
        self.previewed_ref = Some((kind, name.to_owned()));
        job
    }

    /// Selects a commit, paging through history until it appears.
    pub fn seek_commit(&mut self, id: &str) -> Seek {
        let Some(snapshot) = &self.snapshot else {
            return Seek::NotFound;
        };
        if snapshot.commits.iter().any(|c| c.id == id) {
            return Seek::Selected(self.select(Some(id.to_owned())));
        }
        self.start_reveal(Reveal::Commit(id.to_owned()))
    }

    /// Previews a reference, paging through history until its tip appears.
    pub fn seek_ref(&mut self, kind: ReferenceKind, name: &str) -> Seek {
        let Some(snapshot) = &self.snapshot else {
            return Seek::NotFound;
        };
        if snapshot.loaded_ref_tip(kind, name).is_some() {
            return Seek::Selected(self.preview_ref(kind, name));
        }
        let exists = match kind {
            ReferenceKind::LocalBranch => snapshot.branches.iter().any(|b| b.name == name),
            ReferenceKind::Remote => snapshot.remote_refs.iter().any(|r| r.full_name == name),
            ReferenceKind::Tag => snapshot.tags.iter().any(|r| r.full_name == name),
        };
        if !exists {
            self.notice = format!("{name} no longer exists. Refresh to update references.");
            return Seek::NotFound;
        }
        self.start_reveal(Reveal::Reference(kind, name.to_owned()))
    }

    /// Reports whether a history page beyond the loaded commits may exist.
    pub fn more_history(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|s| s.commits.len() >= self.limit && self.limit < 100_000)
    }

    fn start_reveal(&mut self, reveal: Reveal) -> Seek {
        let target = match &reveal {
            Reveal::Commit(id) => format!("commit {}", id.chars().take(8).collect::<String>()),
            Reveal::Reference(_, name) => name.clone(),
        };
        if !self.more_history() || self.limit >= REVEAL_LIMIT {
            let loaded = self.snapshot.as_ref().map_or(0, |s| s.commits.len());
            self.notice = if self.more_history() {
                format!("{target} is not in the first {loaded} commits. Use Load more to keep searching.")
            } else {
                format!("{target} is not reachable in this repository's history.")
            };
            return Seek::NotFound;
        }
        self.notice = format!("Loading more history to find {target}…");
        let plan = self.load_more();
        self.reveal = Some(reveal);
        Seek::Loading(plan)
    }

    /// Reports whether this exact reference is being previewed.
    pub fn is_previewing(&self, kind: ReferenceKind, name: &str) -> bool {
        self.previewed_ref
            .as_ref()
            .is_some_and(|(k, n)| *k == kind && n == name)
    }
    /// Selects a repository-relative path and requests its patch in the current context.
    ///
    /// For working changes, `staged` chooses the index comparison when `true`
    /// and the working-tree comparison otherwise.
    ///
    /// Reloading the comparison already on screen keeps its patch visible
    /// until the replacement arrives; see [`Self::patch_ready`].
    pub fn diff(&mut self, path: String, staged: bool) -> Option<(Lane, Request)> {
        self.selection_generation += 1;
        self.invalidate(Lane::Diff);
        let selection = DiffSelection {
            id: self.selected.clone(),
            path: path.clone(),
            staged,
        };
        if !self.patch.as_ref().is_some_and(|p| {
            p.id == selection.id && p.path == selection.path && p.staged == selection.staged
        }) {
            self.patch = None;
        }
        self.diff_context = Some(selection);
        self.read(
            Lane::Diff,
            Action::Diff {
                id: self.selected.clone(),
                path,
                staged,
            },
        )
    }
    /// Clears the diff selection and invalidates any pending or in-flight patch.
    pub fn close_diff(&mut self) {
        self.invalidate(Lane::Diff);
        self.patch = None;
        self.diff_context = None;
        self.reopen = None;
    }
    /// Requests a full refresh at the current or pending history limit.
    ///
    /// A snapshot must already be loaded. Apply the cancellation and dispatch
    /// instructions in the returned plan when a refresh is accepted.
    pub fn refresh(&mut self) -> Option<RefreshPlan> {
        self.refresh_with(Action::Refresh {
            limit: self.requested_limit.unwrap_or(self.limit),
        })
    }
    /// Records a watcher notification and starts its refresh if the tab is ready.
    ///
    /// Metadata changes require a full refresh. Working-tree changes can reuse
    /// history. An explicit snapshot request already queued covers the notification.
    pub fn watcher_refresh(&mut self, metadata_dirty: bool) -> Option<RefreshPlan> {
        self.snapshot.as_ref()?;
        // A queued explicit request will start after this notification. Keep
        // other notifications outside the lane so its active reply stays current.
        if self.slots[Lane::Snapshot as usize].pending.is_none() {
            self.pending_watcher_refresh =
                Some(metadata_dirty || self.pending_watcher_refresh.unwrap_or(false));
        }
        self.take_watcher_refresh()
    }
    /// Takes a deferred watcher refresh once snapshot loading and writes are idle.
    pub fn take_watcher_refresh(&mut self) -> Option<RefreshPlan> {
        if self.loading() || self.busy {
            return None;
        }
        if self.pending_watcher_refresh.take()? {
            self.refresh()
        } else {
            self.refresh_with(Action::RefreshWorktree { limit: self.limit })
        }
    }
    /// Requests up to 500 more commits, with a ceiling of 100,000.
    ///
    /// Requires a loaded snapshot. The installed limit advances after a
    /// successful snapshot response.
    pub fn load_more(&mut self) -> Option<RefreshPlan> {
        self.snapshot.as_ref()?;
        let limit = (self.requested_limit.unwrap_or(self.limit) + 500).min(100_000);
        self.requested_limit = Some(limit);
        self.refresh_with(Action::LoadMore { limit })
    }
    fn refresh_with(&mut self, action: Action) -> Option<RefreshPlan> {
        self.snapshot.as_ref()?;
        self.pending_watcher_refresh = None;
        self.remember_patch();
        self.invalidate(Lane::Diff);
        Some(RefreshPlan {
            cancel: [ReadKind::Snapshot, ReadKind::Diff],
            dispatch: self.read(Lane::Snapshot, action),
        })
    }
    /// Schedules the selected diff to reload after the next snapshot. The
    /// patch on screen stays until then, with its actions unavailable.
    fn remember_patch(&mut self) {
        self.reopen = self.diff_context.clone().map(|selection| ReopenDiff {
            selection,
            generation: self.selection_generation,
        });
    }
    /// Tests whether a file belongs in the requested working changes list.
    pub fn on_side(f: &crate::git::FileChange, staged: bool) -> bool {
        f.on_side(staged)
    }
    /// Reports whether the patch on screen is current, so its hunk and line
    /// actions address the worker's cached comparison.
    pub fn patch_ready(&self) -> bool {
        self.patch.is_some() && !self.diff_loading()
    }
    /// Reports whether a selected diff is waiting on a read, refresh or write.
    pub fn diff_loading(&self) -> bool {
        self.diff_context.is_some()
            && (self.slots[Lane::Diff as usize].active.is_some()
                || self.slots[Lane::Snapshot as usize].active.is_some()
                || self.busy)
    }
    /// Returns the working-changes file whose selected side has no changes left.
    ///
    /// This is the review context after its last hunk or the whole file moved
    /// to the other side. The selection is kept so the user chooses what to
    /// review next instead of being moved to a different file.
    pub fn completed_review(&self) -> Option<&DiffSelection> {
        let selection = self.diff_context.as_ref()?;
        let snapshot = self.snapshot.as_ref()?;
        (selection.id.is_none()
            && self.patch.is_none()
            && !self.diff_loading()
            && !snapshot.status.iter().any(|file| {
                file.path == selection.path
                    && (file.on_side(selection.staged) || file.is_unmerged())
            }))
        .then_some(selection)
    }

    /// Reports whether `path` has changes on the requested working-changes side.
    pub fn has_side(&self, path: &str, staged: bool) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .status
                .iter()
                .any(|file| file.path == path && file.on_side(staged))
        })
    }

    /// Finds the next addressable path on a working-changes side after `after`,
    /// wrapping to the start of the list and skipping `after` itself.
    pub fn next_file(&self, staged: bool, after: &str) -> Option<String> {
        let snapshot = self.snapshot.as_ref()?;
        let paths: Vec<_> = snapshot
            .side(staged)
            .into_iter()
            .map(|index| &snapshot.status[index])
            .filter(|file| !file.unsupported_path && file.path != after)
            .map(|file| file.path.as_str())
            .collect();
        paths
            .iter()
            .find(|path| **path > after)
            .or(paths.first())
            .map(|path| (*path).to_owned())
    }

    /// Reports whether the selected commit's files and message are still loading.
    pub fn details_loading(&self) -> bool {
        let lane = &self.slots[Lane::Details as usize];
        self.selected.is_some()
            && (lane.active == Some((self.epoch, lane.latest)) || lane.pending.is_some())
    }
    fn request(&mut self, lane: Lane, action: Action) -> Option<(Lane, Request)> {
        self.sequence += 1;
        let request = Request {
            epoch: self.epoch,
            sequence: self.sequence,
            action,
        };
        let slot = &mut self.slots[lane as usize];
        slot.latest = request.sequence;
        if slot.active.is_some() {
            slot.pending = Some(request);
            None
        } else {
            slot.active = Some((request.epoch, request.sequence));
            Some((lane, request))
        }
    }
    fn read(&mut self, lane: Lane, action: Action) -> Option<(Lane, Request)> {
        self.request(lane, action)
    }
    /// Requests target resolution against the supplied repository identity.
    ///
    /// Returns the review sequence and a request to dispatch if the review lane
    /// is idle. Use the sequence to associate the result with the current dialog.
    pub fn review_operation(
        &mut self,
        op: Operation,
        target: String,
        destination: crate::git::WriteIdentity,
    ) -> (u32, Option<(Lane, Request)>) {
        self.operation_review = None;
        let dispatch = self.read(
            Lane::OperationReview,
            Action::ReviewOperation {
                op,
                target,
                destination,
            },
        );
        (self.slots[Lane::OperationReview as usize].latest, dispatch)
    }
    /// Clears the review result and invalidates its requests in this controller.
    /// To interrupt the worker read, also cancel [`ReadKind::OperationReview`].
    pub fn cancel_operation_review(&mut self) {
        self.invalidate(Lane::OperationReview);
        self.operation_review = None;
    }
    /// Starts a write from a loaded snapshot when no mutation is active.
    ///
    /// Supply a write action. A commit captures the current draft revision so
    /// its completion can preserve any edits made while Git was running.
    pub fn mutate(&mut self, action: Action) -> Option<(Lane, Request)> {
        if self.busy || self.snapshot.is_none() {
            return None;
        }
        self.submitted_revision =
            matches!(action, Action::Commit { .. }).then_some(self.draft_revision);
        self.resolving = match &action {
            Action::MarkResolved { path, .. } => Some(path.clone()),
            _ => None,
        };
        self.index_only_write = matches!(
            action,
            Action::StageFile { .. }
                | Action::StageAll { .. }
                | Action::MarkResolved { .. }
                | Action::Hunk { .. }
                | Action::DiscardHunk { .. }
                | Action::Lines { .. }
                | Action::DiscardLines { .. }
        );
        // Submitting a write cancels every read in the worker. Retire a
        // snapshot read in flight so its cancellation is not reported as a
        // failed refresh, and replace it once the write finishes.
        let snapshot = &self.slots[Lane::Snapshot as usize];
        if snapshot.active.is_some() || snapshot.pending.is_some() {
            self.invalidate(Lane::Snapshot);
            self.refresh_after_write = true;
        }
        self.busy = true;
        self.operation_notice = None;
        self.error = false;
        self.notice = "Running Git operation…".into();
        self.invalidate(Lane::Details);
        self.invalidate(Lane::Diff);
        self.remember_patch();
        self.request(Lane::Mutation, action)
    }
    /// Checks whether a response is current enough to apply external effects.
    ///
    /// Check before updating dialogs, focus, input buffers or persisted state.
    /// Pass every response to [`Self::complete`] so queued work can proceed.
    pub fn accepts_response(&self, lane: Lane, response: &Response) -> bool {
        let slot = &self.slots[lane as usize];
        response.epoch == self.epoch
            && slot.latest == response.sequence
            && slot.active == Some((response.epoch, response.sequence))
    }

    /// Reconciles a worker response and returns follow-up requests to dispatch.
    ///
    /// An obsolete active response can release its lane and start a queued
    /// replacement. Only a current response updates repository data and selections.
    pub fn complete(&mut self, lane: Lane, response: Response) -> Vec<(Lane, Request)> {
        let slot = &mut self.slots[lane as usize];
        if slot.active != Some((response.epoch, response.sequence)) {
            return vec![];
        }
        slot.active = None;
        let current = response.epoch == self.epoch && slot.latest == response.sequence;
        let refresh = response
            .result
            .as_ref()
            .map_or_else(|failure| failure.refresh, |_| true);
        let mut jobs = vec![];
        if current {
            let requested_limit = if lane == Lane::Snapshot {
                self.requested_limit.take()
            } else {
                None
            };
            self.worker_ms = response.elapsed_ms;
            self.queue_ms = response.queue_ms;
            match response.result {
                Ok(Reply::Snapshot {
                    path,
                    snapshot,
                    graph,
                }) => {
                    if let Some(limit) = requested_limit {
                        self.limit = limit;
                    }
                    if self.previewed_ref.as_ref().is_some_and(|(kind, name)| {
                        snapshot.loaded_ref_tip(*kind, name) != self.selected.as_deref()
                    }) {
                        self.previewed_ref = None;
                    }
                    self.marker_refusals.retain(|path| {
                        snapshot
                            .status
                            .iter()
                            .any(|file| &file.path == path && file.is_unmerged())
                    });
                    self.path = path;
                    self.snapshot = Some(snapshot);
                    self.graph = graph;
                    if let Some(id) = self.selected.clone() {
                        if self
                            .snapshot
                            .as_ref()
                            .unwrap()
                            .commits
                            .iter()
                            .any(|c| c.id == id)
                        {
                            if self.details_loaded.as_ref() != Some(&id) {
                                if let Some(job) = self.read(Lane::Details, Action::Details { id })
                                {
                                    jobs.push(job);
                                }
                            }
                        } else {
                            self.select(None);
                        }
                    }
                    if !self.busy {
                        (self.notice, self.error) =
                            self.operation_notice.clone().unwrap_or_default();
                    }
                    if let Some(reveal) = self.reveal.take() {
                        let snapshot = self.snapshot.as_ref().unwrap();
                        let found = match &reveal {
                            Reveal::Commit(id) => snapshot
                                .commits
                                .iter()
                                .any(|c| &c.id == id)
                                .then(|| id.clone()),
                            Reveal::Reference(kind, name) => {
                                snapshot.loaded_ref_tip(*kind, name).map(str::to_owned)
                            }
                        };
                        match found {
                            Some(id) => {
                                jobs.extend(self.select(Some(id)));
                                if let Reveal::Reference(kind, name) = reveal {
                                    self.previewed_ref = Some((kind, name));
                                }
                                self.revealed = true;
                                self.notice.clear();
                            }
                            None => {
                                if let Seek::Loading(plan) = self.start_reveal(reveal) {
                                    jobs.extend(plan.and_then(|plan| plan.dispatch));
                                }
                            }
                        }
                    }
                    if let Some(reopen) = self
                        .reopen
                        .take()
                        .filter(|reopen| reopen.generation == self.selection_generation)
                    {
                        let DiffSelection { id, path, staged } = reopen.selection;
                        let can_reopen = id.is_some()
                            || self.snapshot.as_ref().is_some_and(|snapshot| {
                                snapshot.status.iter().any(|file| {
                                    file.path == path
                                        && !file.unsupported_path
                                        && (file.on_side(staged) || file.is_unmerged())
                                })
                            });
                        if id == self.selected && can_reopen {
                            if let Some(job) = self.diff(path, staged) {
                                jobs.push(job);
                            }
                        } else {
                            // Nothing is left on this side; the view offers
                            // the next step instead of a stale patch.
                            self.patch = None;
                        }
                    }
                }
                Ok(Reply::Details {
                    id,
                    files,
                    body,
                    author_email,
                }) if self.selected.as_ref() == Some(&id) => {
                    self.details_loaded = Some(id);
                    self.files = files;
                    self.commit_body = body;
                    self.commit_author_email = author_email;
                    self.details_failed = false;
                }
                Ok(Reply::Diff {
                    id,
                    path,
                    staged,
                    original_path,
                    content,
                    hunks,
                    hunk_blocked,
                    token,
                    preview,
                }) if self.selected == id => {
                    self.patch = Some(Patch {
                        id,
                        path,
                        staged,
                        original_path,
                        content,
                        hunks,
                        hunk_blocked,
                        token,
                        preview,
                    });
                }
                Ok(Reply::Done {
                    message,
                    committed_draft,
                }) => {
                    self.operation_notice = Some((message.clone(), false));
                    self.notice = message;
                    if committed_draft.as_ref() == Some(&self.draft)
                        && self.submitted_revision == Some(self.draft_revision)
                    {
                        self.draft.clear();
                        self.draft_revision += 1;
                    }
                }
                Ok(Reply::OperationReview(reviewed)) => self.operation_review = Some(Ok(reviewed)),
                Err(e) if lane == Lane::OperationReview => {
                    self.operation_review = Some(Err(e.message))
                }
                Err(e) => {
                    if lane == Lane::Details {
                        self.details_failed = true;
                    }
                    if lane == Lane::Diff {
                        self.patch = None;
                    }
                    if lane == Lane::Snapshot {
                        self.reveal = None;
                    }
                    if lane == Lane::Mutation {
                        self.operation_notice = Some((e.message.clone(), true));
                        if e.kind == crate::git::FailureKind::ConflictMarkers {
                            self.marker_refusals.extend(self.resolving.take());
                        }
                    }
                    self.notice = e.message;
                    self.error = true;
                }
                _ => {}
            }
        }
        if lane == Lane::Mutation {
            self.busy = false;
            let replaced = std::mem::take(&mut self.refresh_after_write);
            let index_only = std::mem::take(&mut self.index_only_write);
            if current && (refresh || replaced) {
                // A write to the index or worktree cannot change history, so
                // the worktree refresh reuses the loaded commits. It still
                // reads references, HEAD and metadata, and reloads history if
                // any of them changed. A retired full refresh stays full.
                let plan = if index_only && !replaced && self.requested_limit.is_none() {
                    self.refresh_with(Action::RefreshWorktree { limit: self.limit })
                } else {
                    self.refresh()
                };
                if let Some(job) = plan.and_then(|plan| plan.dispatch) {
                    jobs.push(job);
                }
            } else if let Some(reopen) = self
                .reopen
                .take()
                .filter(|reopen| reopen.generation == self.selection_generation)
            {
                // Without a refresh, reload the diff directly so its actions
                // address a current worker patch again.
                let DiffSelection { path, staged, .. } = reopen.selection;
                jobs.extend(self.diff(path, staged));
            }
        }
        let slot = &mut self.slots[lane as usize];
        if let Some(request) = slot.pending.take().filter(|r| r.epoch == self.epoch) {
            slot.active = Some((request.epoch, request.sequence));
            jobs.push((lane, request));
        }
        jobs
    }
    /// Reports whether the snapshot lane has an active request.
    pub fn loading(&self) -> bool {
        self.slots[0].active.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refresh_triggers_cannot_supersede_a_delayed_initial_open() {
        let mut controller = Controller::default();
        let (_, open) = controller
            .open_trusted(RepositoryLocation::test_path("remembered"))
            .unwrap();
        // F5 and activation both enter refresh. Watcher refresh and paging
        // also have no replacement to offer during the initial open.
        for _ in 0..2 {
            assert!(controller.refresh().is_none());
        }
        assert!(controller.watcher_refresh(false).is_none());
        assert!(controller.watcher_refresh(true).is_none());
        assert!(controller.load_more().is_none());
        let response = reply(
            &open,
            Ok(Reply::Snapshot {
                path: "remembered".into(),
                snapshot: Default::default(),
                graph: Default::default(),
            }),
        );
        assert!(controller.accepts_response(Lane::Snapshot, &response));
        assert!(controller.complete(Lane::Snapshot, response).is_empty());
        assert!(controller.snapshot.is_some());
        assert_eq!(controller.path, "remembered");
        assert_eq!(controller.limit, 500);
    }

    #[test]
    fn failed_load_more_keeps_the_installed_limit_and_can_be_retried() {
        let mut controller = Controller {
            snapshot: Some(Default::default()),
            ..Default::default()
        };
        let (_, page) = controller.load_more().unwrap().dispatch.unwrap();
        assert!(matches!(page.action, Action::LoadMore { limit: 1000 }));
        assert_eq!(controller.limit, 500);
        let failure = Failure {
            message: "could not load page".into(),
            kind: Default::default(),
            refresh: false,
        };
        controller.complete(Lane::Snapshot, reply(&page, Err(failure.clone())));
        assert_eq!(controller.limit, 500);
        assert!(controller.error);

        let (_, retry) = controller.load_more().unwrap().dispatch.unwrap();
        assert!(matches!(retry.action, Action::LoadMore { limit: 1000 }));
        assert!(controller.refresh().unwrap().dispatch.is_none());
        let jobs = controller.complete(Lane::Snapshot, reply(&retry, Err(failure)));
        let [(Lane::Snapshot, refresh)] = &jobs[..] else {
            panic!("expected a replacement snapshot request");
        };
        assert!(matches!(refresh.action, Action::Refresh { limit: 1000 }));
        assert_eq!(controller.limit, 500);
        controller.complete(
            Lane::Snapshot,
            reply(
                refresh,
                Ok(Reply::Snapshot {
                    path: "repo".into(),
                    snapshot: Default::default(),
                    graph: Default::default(),
                }),
            ),
        );
        assert_eq!(controller.limit, 1000);
        assert!(!controller.error);
    }

    #[test]
    fn explicit_paging_keeps_its_replacement_when_watcher_notifications_arrive() {
        let mut controller = Controller {
            snapshot: Some(Default::default()),
            ..Default::default()
        };
        let (_, first) = controller.refresh().unwrap().dispatch.unwrap();
        assert!(controller.watcher_refresh(true).is_none());
        let replacement = controller.load_more().unwrap();
        assert!(matches!(
            replacement.cancel,
            [ReadKind::Snapshot, ReadKind::Diff]
        ));
        assert!(replacement.dispatch.is_none());
        assert!(controller.watcher_refresh(true).is_none());
        let old = reply(
            &first,
            Err(Failure {
                message: "superseded".into(),
                kind: Default::default(),
                refresh: false,
            }),
        );
        assert!(!controller.accepts_response(Lane::Snapshot, &old));
        let jobs = controller.complete(Lane::Snapshot, old);
        assert!(matches!(
            &jobs[..],
            [(
                Lane::Snapshot,
                Request {
                    action: Action::LoadMore { limit: 1000 },
                    ..
                }
            )]
        ));
        controller.complete(
            Lane::Snapshot,
            reply(
                &jobs[0].1,
                Ok(Reply::Snapshot {
                    path: "repo".into(),
                    snapshot: Default::default(),
                    graph: Default::default(),
                }),
            ),
        );
        assert!(controller.take_watcher_refresh().is_none());
        assert!(!controller.error);
    }

    #[test]
    fn watcher_notifications_keep_the_current_snapshot_and_coalesce_one_follow_up() {
        for metadata_dirty in [false, true] {
            let mut controller = Controller {
                snapshot: Some(Default::default()),
                ..Default::default()
            };
            let (_, first) = controller.watcher_refresh(false).unwrap().dispatch.unwrap();
            assert!(matches!(first.action, Action::RefreshWorktree { .. }));
            let response = reply(
                &first,
                Ok(Reply::Snapshot {
                    path: "repo".into(),
                    snapshot: RepoSnapshot {
                        status: vec![change("arrived.txt", ' ', 'M')],
                        ..Default::default()
                    },
                    graph: Default::default(),
                }),
            );
            for dirty in [false, metadata_dirty, false, false] {
                assert!(controller.watcher_refresh(dirty).is_none());
                assert!(controller.accepts_response(Lane::Snapshot, &response));
            }
            assert!(controller.complete(Lane::Snapshot, response).is_empty());
            assert_eq!(
                controller.snapshot.as_ref().unwrap().status[0].path,
                "arrived.txt"
            );
            let (_, follow_up) = controller.take_watcher_refresh().unwrap().dispatch.unwrap();
            assert!(matches!(
                (metadata_dirty, &follow_up.action),
                (false, Action::RefreshWorktree { limit: 500 })
                    | (true, Action::Refresh { limit: 500 })
            ));
            controller.complete(
                Lane::Snapshot,
                reply(
                    &follow_up,
                    Ok(Reply::Snapshot {
                        path: "repo".into(),
                        snapshot: Default::default(),
                        graph: Default::default(),
                    }),
                ),
            );
            assert!(controller.take_watcher_refresh().is_none());
        }
    }
    #[test]
    fn native_effects_require_a_current_active_response() {
        let mut c = Controller::default();
        let (_, old) = c
            .open_trusted(RepositoryLocation::test_path("old"))
            .unwrap();
        let old_reply = reply(
            &old,
            Err(Failure {
                message: "obsolete open failure".into(),
                kind: Default::default(),
                refresh: false,
            }),
        );
        assert!(c.accepts_response(Lane::Snapshot, &old_reply));
        c.set_draft("keep this draft".into());
        assert!(c
            .open_trusted(RepositoryLocation::test_path("new"))
            .is_none());
        assert!(!c.accepts_response(Lane::Snapshot, &old_reply));
        let next = c.complete(Lane::Snapshot, old_reply);
        assert_eq!(next.len(), 1);
        let current = reply(
            &next[0].1,
            Ok(Reply::Snapshot {
                path: "new".into(),
                snapshot: Default::default(),
                graph: Default::default(),
            }),
        );
        assert!(c.accepts_response(Lane::Snapshot, &current));
        c.complete(Lane::Snapshot, current.clone());
        assert!(!c.accepts_response(Lane::Snapshot, &current));
        c.open_trusted(RepositoryLocation::test_path("old"));
        assert_eq!(c.draft, "keep this draft");
    }

    #[test]
    fn repeated_mutation_completion_cannot_repeat_native_effects() {
        let mut c = Controller {
            snapshot: Some(Default::default()),
            ..Default::default()
        };
        let (_, request) = c.mutate(Action::StageAll { unstage: false }).unwrap();
        let response = reply(
            &request,
            Ok(Reply::Done {
                message: "Index updated".into(),
                committed_draft: None,
            }),
        );
        assert!(c.accepts_response(Lane::Mutation, &response));
        assert_eq!(c.complete(Lane::Mutation, response.clone()).len(), 1);
        assert!(!c.accepts_response(Lane::Mutation, &response));
        assert!(c.complete(Lane::Mutation, response).is_empty());
    }
    #[cfg(windows)]
    #[test]
    fn native_picker_path_preserves_a_canonical_repository_draft() {
        let mut c = Controller {
            path: "C:/work/repo".into(),
            draft: "Unicode draft 日本語".into(),
            ..Default::default()
        };
        c.open_trusted(RepositoryLocation::test_path("C:\\work\\other"));
        c.open_trusted(RepositoryLocation::test_path("C:\\work\\repo\\"));
        assert_eq!(c.draft, "Unicode draft 日本語");
    }
    fn change(path: &str, index: char, worktree: char) -> crate::git::FileChange {
        crate::git::FileChange {
            path: path.into(),
            index,
            worktree,
            original_path: None,
            unsupported_path: false,
        }
    }
    fn refresh_review(
        old: Vec<crate::git::FileChange>,
        new: Vec<crate::git::FileChange>,
        staged: bool,
    ) -> (Controller, Vec<(Lane, Request)>) {
        let mut c = Controller {
            epoch: 1,
            ..Default::default()
        };
        c.snapshot = Some(RepoSnapshot {
            status: old,
            ..Default::default()
        });
        c.diff_context = Some(DiffSelection {
            path: "b".into(),
            staged,
            ..Default::default()
        });
        let (_, mutation) = c
            .mutate(Action::StageFile {
                reviewed: change("b", 'M', 'M'),
                unstage: staged,
            })
            .unwrap();
        let refresh = c.complete(
            Lane::Mutation,
            reply(
                &mutation,
                Ok(Reply::Done {
                    message: "ok".into(),
                    committed_draft: None,
                }),
            ),
        );
        let jobs = c.complete(
            Lane::Snapshot,
            reply(
                &refresh[0].1,
                Ok(Reply::Snapshot {
                    path: "repo".into(),
                    snapshot: RepoSnapshot {
                        status: new,
                        ..Default::default()
                    },
                    graph: Default::default(),
                }),
            ),
        );
        (c, jobs)
    }
    #[test]
    fn staging_and_unstaging_do_not_open_a_different_file() {
        let (c, jobs) = refresh_review(
            vec![
                change("a", ' ', 'M'),
                change("b", ' ', 'M'),
                change("c", ' ', 'M'),
            ],
            vec![
                change("a", ' ', 'M'),
                change("b", 'M', ' '),
                change("c", ' ', 'M'),
            ],
            false,
        );
        assert!(jobs.is_empty());
        let review = c.diff_context.unwrap();
        assert_eq!(review.path, "b");
        assert!(!review.staged);
        let (c, jobs) = refresh_review(
            vec![change("a", 'M', ' '), change("b", 'M', ' ')],
            vec![change("a", 'M', ' '), change("b", ' ', 'M')],
            true,
        );
        assert!(jobs.is_empty());
        let review = c.diff_context.unwrap();
        assert_eq!(review.path, "b");
        assert!(review.staged);
    }
    #[test]
    fn final_file_completes_review_without_an_empty_patch_request() {
        let (c, jobs) = refresh_review(
            vec![change("b", ' ', 'M')],
            vec![change("b", 'M', ' ')],
            false,
        );
        assert!(jobs.is_empty());
        assert!(c.patch.is_none());
        assert!(!c.diff_loading());
        assert_eq!(
            c.diff_context,
            Some(DiffSelection {
                path: "b".into(),
                ..Default::default()
            })
        );
        assert_eq!(c.notice, "ok");
        assert_eq!(c.completed_review().map(|s| s.path.as_str()), Some("b"));
        assert!(c.has_side("b", true));
        assert_eq!(c.next_file(false, "b"), None);
    }
    #[test]
    fn completed_file_offers_its_staged_side_and_the_next_unstaged_file() {
        let (c, jobs) = refresh_review(
            vec![
                change("a", ' ', 'M'),
                change("b", ' ', 'M'),
                change("c", ' ', 'M'),
                change("d", 'U', 'U'),
            ],
            vec![
                change("a", ' ', 'M'),
                change("b", 'M', ' '),
                change("c", ' ', 'M'),
                change("d", 'U', 'U'),
            ],
            false,
        );
        assert!(jobs.is_empty());
        assert_eq!(c.completed_review().map(|s| s.path.as_str()), Some("b"));
        assert!(c.has_side("b", true));
        assert_eq!(c.next_file(false, "b").as_deref(), Some("c"));
        assert_eq!(c.next_file(false, "c").as_deref(), Some("a"));
        // Conflicts are never offered as ordinary unstaged files.
        assert_eq!(c.next_file(false, "a").as_deref(), Some("c"));
    }
    #[test]
    fn conflicted_review_stays_open_and_marker_refusals_are_remembered() {
        let mut c = Controller {
            epoch: 1,
            snapshot: Some(RepoSnapshot {
                status: vec![change("d", 'U', 'U')],
                ..Default::default()
            }),
            diff_context: Some(DiffSelection {
                path: "d".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let (_, mutation) = c
            .mutate(Action::MarkResolved {
                path: "d".into(),
                allow_markers: false,
            })
            .unwrap();
        let refresh = c.complete(
            Lane::Mutation,
            reply(
                &mutation,
                Err(Failure {
                    message: "markers".into(),
                    kind: crate::git::FailureKind::ConflictMarkers,
                    refresh: true,
                }),
            ),
        );
        assert!(c.marker_refusals.contains("d"));
        let snapshot = |status| {
            Ok(Reply::Snapshot {
                path: "repo".into(),
                snapshot: RepoSnapshot {
                    status,
                    ..Default::default()
                },
                graph: Default::default(),
            })
        };
        let jobs = c.complete(
            Lane::Snapshot,
            reply(&refresh[0].1, snapshot(vec![change("d", 'U', 'U')])),
        );
        assert!(
            matches!(&jobs[..], [(Lane::Diff, Request { action: Action::Diff { path, .. }, .. })] if path == "d")
        );
        assert!(c.completed_review().is_none());
        assert!(c.marker_refusals.contains("d"));
        let (_, refresh) = c.refresh().unwrap().dispatch.unwrap();
        c.complete(
            Lane::Snapshot,
            reply(&refresh, snapshot(vec![change("d", 'M', ' ')])),
        );
        assert!(c.marker_refusals.is_empty());
    }
    #[test]
    fn hunk_writes_keep_the_patch_on_screen_until_it_reloads() {
        let mut c = Controller {
            epoch: 1,
            snapshot: Some(RepoSnapshot {
                status: vec![change("b", ' ', 'M')],
                ..Default::default()
            }),
            ..Default::default()
        };
        let (_, read) = c.diff("b".into(), false).unwrap();
        let shown = |token| Reply::Diff {
            id: None,
            path: "b".into(),
            staged: false,
            original_path: None,
            content: diff::prepare(vec![]),
            hunks: vec!["@@ -1 +1 @@".into()],
            hunk_blocked: None,
            token,
            preview: Default::default(),
        };
        c.complete(Lane::Diff, reply(&read, Ok(shown(read.sequence))));
        assert!(c.patch_ready());
        let (_, write) = c
            .mutate(Action::Hunk {
                token: read.sequence,
                index: 0,
                unstage: false,
            })
            .unwrap();
        // The old patch stays visible but its actions are unavailable.
        assert!(c.patch.is_some() && !c.patch_ready());
        let done = Ok(Reply::Done {
            message: "ok".into(),
            committed_draft: None,
        });
        let refresh = c.complete(Lane::Mutation, reply(&write, done));
        let jobs = c.complete(
            Lane::Snapshot,
            reply(
                &refresh[0].1,
                Ok(Reply::Snapshot {
                    path: "repo".into(),
                    snapshot: RepoSnapshot {
                        status: vec![change("b", 'M', 'M')],
                        ..Default::default()
                    },
                    graph: Default::default(),
                }),
            ),
        );
        assert!(c.patch.is_some() && !c.patch_ready());
        let (_, reload) = &jobs[0];
        c.complete(Lane::Diff, reply(reload, Ok(shown(reload.sequence))));
        assert!(c.patch_ready());
        assert_eq!(c.patch.as_ref().unwrap().token, reload.sequence);
    }
    #[test]
    fn remaining_hunks_keep_the_current_file() {
        let (_, jobs) = refresh_review(
            vec![change("b", ' ', 'M'), change("c", ' ', 'M')],
            vec![change("b", 'M', 'M'), change("c", ' ', 'M')],
            false,
        );
        assert!(matches!(&jobs[0].1.action,Action::Diff{path,..} if path=="b"));
    }
    #[test]
    fn refresh_does_not_reopen_an_older_diff_or_a_closed_diff() {
        for new_selection in [Some("new"), None] {
            let mut c = Controller {
                epoch: 1,
                snapshot: Some(RepoSnapshot {
                    status: vec![change("old", ' ', 'M'), change("new", ' ', 'M')],
                    ..Default::default()
                }),
                ..Default::default()
            };
            let (_, old) = c.diff("old".into(), false).unwrap();
            let (_, refresh) = c.refresh().unwrap().dispatch.unwrap();
            if let Some(path) = new_selection {
                assert!(c.diff(path.into(), false).is_none());
            } else {
                c.close_diff();
            }
            let snapshot = c.snapshot.clone().unwrap();
            let after_refresh = c.complete(
                Lane::Snapshot,
                reply(
                    &refresh,
                    Ok(Reply::Snapshot {
                        path: "repo".into(),
                        snapshot,
                        graph: Graph::default(),
                    }),
                ),
            );
            assert!(after_refresh.is_empty());
            let obsolete = Failure {
                message: "old error".into(),
                kind: Default::default(),
                refresh: false,
            };
            let next = c.complete(Lane::Diff, reply(&old, Err(obsolete)));
            assert!(!c.error);
            if let Some(path) = new_selection {
                assert_eq!(c.diff_context.as_ref().unwrap().path, path);
                assert!(
                    matches!(&next[..], [(Lane::Diff, Request { action: Action::Diff { path, .. }, .. })] if path == "new")
                );
            } else {
                assert!(c.diff_context.is_none());
                assert!(next.is_empty());
            }
        }
    }

    #[test]
    fn successful_refresh_clears_a_transient_failure() {
        let mut c = Controller {
            snapshot: Some(RepoSnapshot::default()),
            ..Default::default()
        };
        let (_, first) = c.refresh().unwrap().dispatch.unwrap();
        let failure = Failure {
            message: "transient read failure".into(),
            kind: Default::default(),
            refresh: false,
        };
        assert!(c
            .complete(Lane::Snapshot, reply(&first, Err(failure)))
            .is_empty());
        assert!(c.error);
        let (_, retry) = c.refresh().unwrap().dispatch.unwrap();
        assert!(c
            .complete(
                Lane::Snapshot,
                reply(
                    &retry,
                    Ok(Reply::Snapshot {
                        path: "repo".into(),
                        snapshot: RepoSnapshot::default(),
                        graph: Graph::default(),
                    })
                )
            )
            .is_empty());
        assert!(!c.error);
        assert!(c.notice.is_empty());
    }

    #[test]
    fn repository_switch_drops_pending_review_but_preserves_each_draft() {
        let mut c = Controller {
            path: "a".into(),
            draft: "draft a".into(),
            ..Default::default()
        };
        c.diff_context = Some(DiffSelection {
            path: "old".into(),
            ..Default::default()
        });
        c.open_trusted(RepositoryLocation::test_path("b"));
        assert!(c.diff_context.is_none());
        assert!(c.draft.is_empty());
        c.draft = "draft b".into();
        c.open_trusted(RepositoryLocation::test_path("c"));
        c.open_trusted(RepositoryLocation::test_path("a"));
        assert_eq!(c.draft, "draft a");
        c.open_trusted(RepositoryLocation::test_path("b"));
        assert_eq!(c.draft, "draft b");
    }
    fn reply(r: &Request, result: Result<Reply, Failure>) -> Response {
        Response {
            epoch: r.epoch,
            sequence: r.sequence,
            elapsed_ms: 1.,
            queue_ms: 0.,
            result,
        }
    }
    #[test]
    fn preview_tracks_one_reference_even_when_branches_share_a_tip() {
        use crate::git::{Commit, LocalBranch, WriteIdentity};
        let mut c = Controller::default();
        let mut snapshot = RepoSnapshot {
            head: "main".into(),
            head_id: "tip".into(),
            reviewed: Some(WriteIdentity {
                symbolic_head: "refs/heads/main".into(),
                ..Default::default()
            }),
            commits: vec![Commit {
                id: "tip".into(),
                ..Default::default()
            }],
            branches: ["main", "feature", "backup"]
                .map(|name| LocalBranch {
                    name: name.into(),
                    tip: "tip".into(),
                })
                .to_vec(),
            ..Default::default()
        };
        c.snapshot = Some(snapshot.clone());

        let (lane, job) = c
            .preview_ref(ReferenceKind::LocalBranch, "feature")
            .unwrap();
        assert_eq!(lane, Lane::Details);
        assert!(matches!(job.action, Action::Details { id } if id == "tip"));
        assert_eq!(
            c.snapshot.as_ref().unwrap().checked_out_branch(),
            Some("main")
        );
        assert!(c.is_previewing(ReferenceKind::LocalBranch, "feature"));
        assert!(!c.is_previewing(ReferenceKind::LocalBranch, "main"));
        assert!(!c.is_previewing(ReferenceKind::LocalBranch, "backup"));

        // A checkout elsewhere updates the green marker without losing the preview.
        snapshot.head = "backup".into();
        snapshot.reviewed.as_mut().unwrap().symbolic_head = "refs/heads/backup".into();
        let (_, refresh) = c.refresh().unwrap().dispatch.unwrap();
        c.complete(
            Lane::Snapshot,
            reply(
                &refresh,
                Ok(Reply::Snapshot {
                    path: "repo".into(),
                    snapshot,
                    graph: Default::default(),
                }),
            ),
        );
        assert_eq!(
            c.snapshot.as_ref().unwrap().checked_out_branch(),
            Some("backup")
        );
        assert!(c.is_previewing(ReferenceKind::LocalBranch, "feature"));

        // Selecting the same commit in the graph is not a branch preview.
        c.select(Some("tip".into()));
        assert!(!c.is_previewing(ReferenceKind::LocalBranch, "feature"));
    }

    #[test]
    fn rapid_selection_coalesces_and_rejects_old_replies() {
        let mut c = Controller {
            epoch: 1,
            ..Default::default()
        };
        let (_, a) = c.select(Some("a".into())).unwrap();
        assert!(c.select(Some("b".into())).is_none());
        assert!(c.select(Some("c".into())).is_none());
        let next = c.complete(
            Lane::Details,
            reply(
                &a,
                Ok(Reply::Details {
                    id: "a".into(),
                    files: vec![crate::git::FileChange {
                        path: "obsolete".into(),
                        ..Default::default()
                    }],
                    body: "Obsolete details".into(),
                    author_email: "old@example.invalid".into(),
                }),
            ),
        );
        assert!(c.files.is_empty());
        assert!(c.commit_body.is_empty());
        assert!(c.details_loading());
        assert_eq!(next.len(), 1);
        assert!(matches!(&next[0].1.action, Action::Details { id } if id == "c"));
    }
    #[test]
    fn repository_switch_opens_only_latest() {
        let mut c = Controller::default();
        let (_, a) = c.open_trusted(RepositoryLocation::test_path("a")).unwrap();
        assert!(c.open_trusted(RepositoryLocation::test_path("b")).is_none());
        assert!(c.open_trusted(RepositoryLocation::test_path("c")).is_none());
        let jobs = c.complete(
            Lane::Snapshot,
            reply(
                &a,
                Ok(Reply::Snapshot {
                    path: "a".into(),
                    snapshot: Default::default(),
                    graph: Default::default(),
                }),
            ),
        );
        assert!(c.snapshot.is_none());
        assert_eq!(jobs.len(), 1);
        assert!(
            matches!(&jobs[0].1.action, Action::Open { trust: RepositoryTrust::Confirmed(location), .. } if location.worktree == std::path::Path::new("c"))
        );
    }
    #[test]
    fn draft_recovery_is_tab_local_and_closing_drops_it() {
        let mut tab = Controller::default();
        tab.open_trusted(RepositoryLocation::test_path("repo-a"));
        tab.set_draft("private draft".into());
        tab.open_trusted(RepositoryLocation::test_path("repo-b"));
        assert!(tab.has_drafts());
        tab.open_trusted(RepositoryLocation::test_path("repo-a"));
        assert_eq!(tab.draft, "private draft");
        tab.set_draft(String::new());
        assert!(!tab.has_drafts());
        drop(tab);
        let mut reopened = Controller::default();
        reopened.open_trusted(RepositoryLocation::test_path("repo-a"));
        assert!(!reopened.has_drafts());
        assert!(reopened.draft.is_empty());
    }

    #[test]
    fn dismissed_operation_feedback_does_not_return_on_refresh() {
        let mut c = Controller {
            snapshot: Some(RepoSnapshot::default()),
            ..Default::default()
        };
        c.operation_notice = Some(("inspect the index".into(), true));
        c.error = true;
        c.notice = "inspect the index".into();
        c.dismiss_notice();
        let (_, request) = c.refresh().unwrap().dispatch.unwrap();
        c.complete(
            Lane::Snapshot,
            reply(
                &request,
                Ok(Reply::Snapshot {
                    path: "repo".into(),
                    snapshot: RepoSnapshot::default(),
                    graph: Graph::default(),
                }),
            ),
        );
        assert!(!c.error);
        assert!(c.notice.is_empty());
    }

    #[test]
    fn commit_completion_keeps_a_newer_revision_even_when_text_matches() {
        let mut c = Controller {
            epoch: 1,
            snapshot: Some(Default::default()),
            ..Default::default()
        };
        c.set_draft("submitted".into());
        let (_, r) = c
            .mutate(Action::Commit {
                message: c.draft.clone(),
                reviewed: None,
            })
            .unwrap();
        c.set_draft("new edit".into());
        c.set_draft("submitted".into());
        c.complete(
            Lane::Mutation,
            reply(
                &r,
                Ok(Reply::Done {
                    message: "verified".into(),
                    committed_draft: Some("submitted".into()),
                }),
            ),
        );
        assert_eq!(c.draft, "submitted");
    }
    #[test]
    fn a_write_retires_a_refresh_in_flight_and_replaces_it() {
        let mut c = Controller {
            epoch: 1,
            snapshot: Some(Default::default()),
            ..Default::default()
        };
        let refresh = c.refresh().unwrap().dispatch.unwrap().1;
        let (_, write) = c.mutate(Action::StageAll { unstage: false }).unwrap();
        // The worker cancels the read when the write is submitted.
        let cancelled = reply(
            &refresh,
            Err(Failure {
                message: "Git read was superseded.".into(),
                kind: Default::default(),
                refresh: false,
            }),
        );
        assert!(!c.accepts_response(Lane::Snapshot, &cancelled));
        assert!(c.complete(Lane::Snapshot, cancelled).is_empty());
        assert!(!c.error);
        // Even a failure that would not refresh by itself replaces the read.
        let jobs = c.complete(
            Lane::Mutation,
            reply(
                &write,
                Err(Failure {
                    message: "rejected".into(),
                    kind: crate::git::FailureKind::Rejected,
                    refresh: false,
                }),
            ),
        );
        assert!(matches!(
            jobs.as_slice(),
            [(
                Lane::Snapshot,
                Request {
                    action: Action::Refresh { .. },
                    ..
                }
            )]
        ));
        // The next write without a read in flight refreshes only on its own terms.
        c.complete(
            Lane::Snapshot,
            reply(
                &jobs[0].1,
                Ok(Reply::Snapshot {
                    path: "r".into(),
                    snapshot: Default::default(),
                    graph: Default::default(),
                }),
            ),
        );
        let (_, write) = c.mutate(Action::StageAll { unstage: false }).unwrap();
        let failure = Failure {
            message: "rejected".into(),
            kind: crate::git::FailureKind::Rejected,
            refresh: false,
        };
        assert!(c
            .complete(Lane::Mutation, reply(&write, Err(failure)))
            .is_empty());
    }
    #[test]
    fn index_writes_refresh_the_worktree_and_other_writes_refresh_history() {
        let done = || {
            Ok(Reply::Done {
                message: "ok".into(),
                committed_draft: None,
            })
        };
        for (action, worktree) in [
            (Action::StageAll { unstage: false }, true),
            (
                Action::Lines {
                    token: 1,
                    rows: vec![1],
                    unstage: false,
                },
                true,
            ),
            (
                Action::Commit {
                    message: "m".into(),
                    reviewed: None,
                },
                false,
            ),
        ] {
            let mut c = Controller {
                epoch: 1,
                snapshot: Some(Default::default()),
                ..Default::default()
            };
            let (_, write) = c.mutate(action).unwrap();
            let jobs = c.complete(Lane::Mutation, reply(&write, done()));
            let refresh = &jobs[0].1.action;
            assert_eq!(
                matches!(refresh, Action::RefreshWorktree { .. }),
                worktree,
                "{refresh:?}"
            );
        }
    }
    #[test]
    fn mutation_preserves_new_draft_and_refreshes_failure() {
        let mut c = Controller {
            epoch: 1,
            snapshot: Some(Default::default()),
            draft: "old".into(),
            ..Default::default()
        };
        let (_, a) = c
            .mutate(Action::Commit {
                message: "old".into(),
                reviewed: None,
            })
            .unwrap();
        assert!(c.mutate(Action::StageAll { unstage: false }).is_none());
        c.draft = "new".into();
        let jobs = c.complete(
            Lane::Mutation,
            reply(
                &a,
                Ok(Reply::Done {
                    message: "ok".into(),
                    committed_draft: Some("old".into()),
                }),
            ),
        );
        assert_eq!(c.draft, "new");
        assert!(!c.busy);
        assert_eq!(jobs.len(), 1);
        c.complete(
            Lane::Snapshot,
            reply(
                &jobs[0].1,
                Ok(Reply::Snapshot {
                    path: "r".into(),
                    snapshot: Default::default(),
                    graph: Default::default(),
                }),
            ),
        );
        let (_, b) = c
            .mutate(Action::Commit {
                message: "new".into(),
                reviewed: None,
            })
            .unwrap();
        assert_eq!(
            c.complete(
                Lane::Mutation,
                reply(
                    &b,
                    Err(Failure {
                        message: "conflict".into(),
                        kind: crate::git::FailureKind::Conflict,
                        refresh: true
                    })
                )
            )
            .len(),
            1
        );
        assert!(c.error);
    }

    #[test]
    fn activating_an_unloaded_parent_or_reference_pages_until_it_appears() {
        use crate::git::{Commit, LocalBranch};
        let page = |count: usize| RepoSnapshot {
            commits: (0..count)
                .map(|i| Commit {
                    id: format!("c{i}"),
                    ..Default::default()
                })
                .collect(),
            branches: vec![LocalBranch {
                name: "old".into(),
                tip: "c700".into(),
            }],
            ..Default::default()
        };
        let mut c = Controller {
            snapshot: Some(page(500)),
            ..Default::default()
        };
        assert!(matches!(c.seek_commit("c3"), Seek::Selected(Some(_))));
        let Seek::Loading(Some(plan)) = c.seek_ref(ReferenceKind::LocalBranch, "old") else {
            panic!("expected a history page request");
        };
        let (_, request) = plan.dispatch.unwrap();
        assert!(matches!(request.action, Action::LoadMore { limit: 1000 }));
        assert!(c.notice.contains("old"));
        let jobs = c.complete(
            Lane::Snapshot,
            reply(
                &request,
                Ok(Reply::Snapshot {
                    path: "repo".into(),
                    snapshot: page(1000),
                    graph: Default::default(),
                }),
            ),
        );
        assert_eq!(c.selected.as_deref(), Some("c700"));
        assert!(c.is_previewing(ReferenceKind::LocalBranch, "old"));
        assert!(c.revealed);
        // The earlier c3 details read is still active, so the new read waits.
        assert!(jobs.iter().all(|(lane, _)| *lane != Lane::Snapshot));

        let mut c = Controller {
            snapshot: Some(page(20)),
            ..Default::default()
        };
        assert!(matches!(c.seek_commit("missing"), Seek::NotFound));
        assert!(c.notice.contains("not reachable"));
        assert!(matches!(
            c.seek_ref(ReferenceKind::Tag, "refs/tags/gone"),
            Seek::NotFound
        ));
    }
}
