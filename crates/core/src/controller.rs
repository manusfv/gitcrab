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
    /// User-facing reason Discard cannot act on this patch's hunks or lines,
    /// when staging them still can.
    pub discard_blocked: Option<String>,
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
    /// The write in flight is a commit: it moves the branch but leaves the
    /// index and working tree, so index writes can wait behind it.
    running_commit: bool,
    /// The write's reply installed a current patch, so the refresh that
    /// follows it does not reload the diff or hold back its actions.
    keep_patch_on_refresh: bool,
    /// The write's reply already updated the working changes, so no
    /// snapshot refresh follows it.
    worktree_fresh: bool,
    /// Working changes as Git last reported them, kept while a whole-file
    /// write shows its predicted result, to put back if the write fails.
    predicted_from: Option<Vec<crate::git::FileChange>>,
    /// Paths a whole-file write in flight moves between the lists.
    pub pending_paths: Vec<String>,
    /// Index writes the user asked for while another write ran. They run in
    /// order, each as soon as the one before it finishes.
    queued: std::collections::VecDeque<Action>,
    /// The write in flight cannot change the patch on screen, which keeps
    /// its hunk actions and stays visible as it is.
    patch_untouched: bool,
    /// When the worker started reading the working changes a write returned.
    /// The view treats file watcher events from before then as covered.
    pub worktree_read_at: Option<std::time::Instant>,
    pub operation_review: Option<Result<ReviewedOperation, String>>,
    pub notice: String,
    pub error: bool,
    operation_notice: Option<(String, bool)>,
    /// Limit of the installed history snapshot.
    pub limit: usize,
    requested_limit: Option<usize>,
    /// Set when a history page fails, so scrolling does not retry it until
    /// the next successful snapshot.
    history_page_failed: bool,
    /// Worker execution time for the last accepted response, in milliseconds.
    pub worker_ms: f64,
    /// Time that request waited in the worker queue, in milliseconds.
    pub queue_ms: f64,
    reopen: Option<ReopenDiff>,
    selection_generation: u64,
    // Some(true) requires a full refresh because Git metadata changed.
    pending_watcher_refresh: Option<bool>,
    drafts: std::collections::HashMap<std::path::PathBuf, String>,
    /// Git's prepared message for the operation in progress, once it has
    /// been offered to the draft. Clearing the draft does not bring it back.
    offered_message: Option<String>,
    /// Unmerged paths whose resolution GitCrab refused because of conflict
    /// markers. A path is forgotten once a read finds no markers in its file.
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

/// Commits read by the first history page and added by each later one.
pub const HISTORY_PAGE: usize = 500;
/// Most commits a history snapshot ever holds.
pub const HISTORY_MAX: usize = 100_000;
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
            running_commit: false,
            keep_patch_on_refresh: false,
            worktree_fresh: false,
            worktree_read_at: None,
            predicted_from: None,
            pending_paths: vec![],
            queued: Default::default(),
            patch_untouched: false,
            operation_review: None,
            notice: "Open a local repository to begin".into(),
            error: false,
            operation_notice: None,
            limit: HISTORY_PAGE,
            requested_limit: None,
            history_page_failed: false,
            worker_ms: 0.,
            queue_ms: 0.,
            reopen: None,
            selection_generation: 0,
            pending_watcher_refresh: None,
            drafts: Default::default(),
            offered_message: None,
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

    /// Replaces `prefix` at the start of the last successful operation's
    /// feedback with `replacement`, as when the view ran two writes for one
    /// command. Other feedback is left as it is.
    pub fn reword_operation_notice(&mut self, prefix: &str, replacement: &str) {
        let Some((message, false)) = &mut self.operation_notice else {
            return;
        };
        let Some(rest) = message.strip_prefix(prefix) else {
            return;
        };
        let reworded = format!("{replacement}{rest}");
        if self.notice == *message {
            self.notice = reworded.clone();
        }
        *message = reworded;
    }

    /// Updates the commit message and advances its revision when the text changes.
    pub fn set_draft(&mut self, value: String) {
        if self.draft != value {
            self.draft = value;
            self.draft_revision += 1;
        }
    }

    /// Puts Git's prepared message for a merge, cherry-pick or revert in
    /// progress into an empty draft, once per message, so continuing commits
    /// with it rather than with a stale draft.
    fn offer_operation_message(&mut self, message: Option<&String>) {
        if self.offered_message.as_ref() == message {
            return;
        }
        self.offered_message = message.cloned();
        if let Some(message) = message {
            if self.draft.trim().is_empty() {
                self.set_draft(message.clone());
            }
        }
    }

    /// Replaces the draft with Git's prepared message for the operation in
    /// progress, if there is one.
    pub fn use_operation_message(&mut self) {
        if let Some(message) = self
            .snapshot
            .as_ref()
            .and_then(|s| s.operation_message.clone())
        {
            self.set_draft(message);
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
        self.offered_message = None;
        // Writes waiting for a refresh belong to the repository being left.
        self.queued.clear();
        self.predicted_from = None;
        self.pending_paths.clear();
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
        self.limit = HISTORY_PAGE;
        self.requested_limit = None;
        self.history_page_failed = false;
        self.notice = "Opening repository…".into();
        self.error = false;
        self.operation_notice = None;
        self.read(
            Lane::Snapshot,
            Action::Open {
                limit: HISTORY_PAGE,
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
                "This reference points outside loaded history. Scroll down to load older commits."
                    .into();
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
            .is_some_and(|s| s.commits.len() >= self.limit && self.limit < HISTORY_MAX)
    }

    /// Reports whether the view should request the next history page once
    /// the user scrolls near the end of loaded history.
    pub fn wants_next_page(&self) -> bool {
        self.more_history()
            && !self.loading()
            && self.requested_limit.is_none()
            && !self.history_page_failed
    }

    /// Reports whether older history is being read, for a status indicator.
    pub fn loading_history_page(&self) -> bool {
        self.requested_limit.is_some() && self.loading()
    }

    fn start_reveal(&mut self, reveal: Reveal) -> Seek {
        let target = match &reveal {
            Reveal::Commit(id) => format!("commit {}", crate::presentation::short_id(id)),
            Reveal::Reference(_, name) => name.clone(),
        };
        if !self.more_history() || self.limit >= REVEAL_LIMIT {
            let loaded = self.snapshot.as_ref().map_or(0, |s| s.commits.len());
            self.notice = if self.more_history() {
                format!("{target} is not in the first {loaded} commits. Scroll to the end of history to load older commits.")
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

    /// The reference the sidebar selected, named as [`Self::preview_ref`] took it.
    pub fn previewed_ref(&self) -> Option<(ReferenceKind, &str)> {
        self.previewed_ref
            .as_ref()
            .map(|(kind, name)| (*kind, name.as_str()))
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
    /// Requests [`HISTORY_PAGE`] more commits, up to [`HISTORY_MAX`].
    ///
    /// Requires a loaded snapshot. The installed limit advances after a
    /// successful snapshot response.
    pub fn load_more(&mut self) -> Option<RefreshPlan> {
        self.snapshot.as_ref()?;
        let limit = (self.requested_limit.unwrap_or(self.limit) + HISTORY_PAGE).min(HISTORY_MAX);
        self.requested_limit = Some(limit);
        self.refresh_with(Action::LoadMore { limit })
    }
    fn refresh_with(&mut self, action: Action) -> Option<RefreshPlan> {
        self.snapshot.as_ref()?;
        self.pending_watcher_refresh = None;
        if !std::mem::take(&mut self.keep_patch_on_refresh) {
            self.remember_patch();
            self.invalidate(Lane::Diff);
        }
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
    /// Reports whether a selected diff is waiting on a read, a write, or a
    /// refresh that will reload it.
    pub fn diff_loading(&self) -> bool {
        self.diff_context.is_some()
            && (self.slots[Lane::Diff as usize].active.is_some()
                || (self.slots[Lane::Snapshot as usize].active.is_some() && self.reopen.is_some())
                || self.write_touches_patch())
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
        self.snapshot.as_ref()?;
        if self.busy {
            // Index writes wait their turn instead of being dropped, and
            // their files move at once like the running one's.
            if self.queueable(&action) {
                self.predict(&action);
                self.queued.push_back(action);
            }
            return None;
        }
        // Writes waiting for a refresh keep their order: a whole-file write
        // clicked meanwhile joins them instead of running first.
        if !self.queued.is_empty()
            && matches!(
                action,
                Action::StageFile { .. } | Action::StageAll { .. } | Action::MarkResolved { .. }
            )
        {
            self.predict(&action);
            self.queued.push_back(action);
            return None;
        }
        self.start_write(action)
    }

    /// Starts `action` now. Callers have checked that no write is running.
    fn start_write(&mut self, action: Action) -> Option<(Lane, Request)> {
        self.submitted_revision = matches!(
            action,
            Action::Commit { .. }
                | Action::RunOperation {
                    op: crate::service::Operation::Continue { message: Some(_) },
                    ..
                }
        )
        .then_some(self.draft_revision);
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
        self.running_commit = matches!(action, Action::Commit { .. });
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
        // Every accepted write cancels the worker's reads in flight.
        self.invalidate(Lane::Details);
        // A write to another file leaves the patch on screen current, and
        // the worker keeps its hunks, unless a diff read was in flight.
        self.patch_untouched = self.patch.is_some()
            && self.reopen.is_none()
            && self.slots[Lane::Diff as usize].active.is_none()
            && !self.touches_patch(&action);
        if !self.patch_untouched {
            self.invalidate(Lane::Diff);
            self.remember_patch();
        }
        self.predict(&action);
        self.request(Lane::Mutation, action)
    }

    /// Whether `action` can change the comparison on screen. The worker
    /// applies the same rule to its cached hunks (`Session::keeps_patch`).
    fn touches_patch(&self, action: &Action) -> bool {
        let Some(patch) = &self.patch else {
            return false;
        };
        let names = |path: &str| path == patch.path || patch.original_path.as_deref() == Some(path);
        match action {
            Action::StageFile { reviewed, .. } => {
                names(&reviewed.path) || reviewed.original_path.as_deref().is_some_and(names)
            }
            Action::MarkResolved { path, .. } => names(path),
            // A commit empties staged comparisons, but leaves the index and
            // working tree, so unstaged ones and commit comparisons stay.
            Action::Commit { .. } => patch.id.is_none() && patch.staged,
            _ => true,
        }
    }

    /// Whether `action` may wait for the write in flight: an index write,
    /// behind another index write, that either names its file or addresses
    /// a patch no waiting write changes. Commits and operations still block.
    fn queueable(&self, action: &Action) -> bool {
        if !self.index_only_write && !self.running_commit {
            return false;
        }
        match action {
            Action::StageFile { .. } | Action::StageAll { .. } | Action::MarkResolved { .. } => {
                true
            }
            Action::Hunk { .. }
            | Action::Lines { .. }
            | Action::DiscardHunk { .. }
            | Action::DiscardLines { .. } => {
                self.patch_untouched && self.queued.iter().all(|queued| !self.touches_patch(queued))
            }
            _ => false,
        }
    }

    /// A write is running that the rest of the window must wait for. Index
    /// writes and commits do not count: clicks during them queue or wait on
    /// their own. Operations that move HEAD or rewrite files still do.
    pub fn blocking_write(&self) -> bool {
        self.busy && !self.index_only_write && !self.running_commit
    }

    /// The write in flight changes the patch on screen.
    pub fn write_touches_patch(&self) -> bool {
        self.busy && !self.patch_untouched
    }

    /// Writes waiting for the one in flight.
    pub fn queued_writes(&self) -> usize {
        self.queued.len()
    }

    /// Takes the next waiting write, brought up to date with the status:
    /// a whole-file write reviews the file's entry as it now is, and is
    /// dropped when the file has nothing left on that side.
    fn next_queued(&mut self) -> Option<Action> {
        while let Some(action) = self.queued.pop_front() {
            let Action::StageFile { reviewed, unstage } = action else {
                return Some(action);
            };
            // Resolve against what Git reported, not this write's own prediction.
            let status = self
                .predicted_from
                .as_ref()
                .or(self.snapshot.as_ref().map(|s| &s.status))?;
            if let Some(current) = status
                .iter()
                .find(|f| f.path == reviewed.path && f.on_side(unstage))
            {
                return Some(Action::StageFile {
                    reviewed: current.clone(),
                    unstage,
                });
            }
        }
        None
    }

    /// Moves the files a whole-file write stages or unstages to their
    /// predicted place at once. The status Git reports afterwards replaces
    /// the prediction; a failure puts the reported status back.
    fn predict(&mut self, action: &Action) {
        let (only, unstage) = match action {
            Action::StageFile { reviewed, unstage } => (Some(reviewed.path.as_str()), *unstage),
            Action::StageAll { unstage } => (None, *unstage),
            Action::Commit { .. } => return self.predict_commit(),
            _ => return,
        };
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        let mut moved = vec![];
        let predicted = snapshot
            .status
            .iter()
            .filter_map(|file| {
                if only.is_some_and(|path| path != file.path) {
                    return Some(file.clone());
                }
                match file.predicted(unstage) {
                    Some(next) => {
                        if next.as_ref() != Some(file) {
                            moved.push(file.path.clone());
                        }
                        next
                    }
                    None => Some(file.clone()),
                }
            })
            .collect();
        if !moved.is_empty() {
            // Predictions stack; the status Git last reported is kept once.
            let reported = std::mem::replace(&mut snapshot.status, predicted);
            self.predicted_from.get_or_insert(reported);
            for path in moved {
                if !self.pending_paths.contains(&path) {
                    self.pending_paths.push(path);
                }
            }
        }
    }

    /// Empties the Staged list at once for a commit: its files leave it, and
    /// any working-tree changes they also have stay in Unstaged.
    fn predict_commit(&mut self) {
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        if snapshot.status.iter().any(|f| f.after_commit().is_none()) {
            return;
        }
        let predicted: Vec<_> = snapshot
            .status
            .iter()
            .filter_map(|file| file.after_commit().flatten())
            .collect();
        if predicted != snapshot.status {
            let reported = std::mem::replace(&mut snapshot.status, predicted);
            self.predicted_from.get_or_insert(reported);
        }
    }

    /// Shows the waiting writes' predictions again on a newly reported status.
    fn repredict_queued(&mut self) {
        let queued = std::mem::take(&mut self.queued);
        for action in &queued {
            self.predict(action);
        }
        self.queued = queued;
    }

    /// Ends a prediction: puts the reported status back if `restore`.
    fn settle_prediction(&mut self, restore: bool) {
        self.pending_paths.clear();
        if let Some(reported) = self.predicted_from.take() {
            if let (true, Some(snapshot)) = (restore, self.snapshot.as_mut()) {
                snapshot.status = reported;
            }
        }
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
        let failed = response.result.is_err();
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
                    self.history_page_failed = false;
                    if self.previewed_ref.as_ref().is_some_and(|(kind, name)| {
                        snapshot.loaded_ref_tip(*kind, name) != self.selected.as_deref()
                    }) {
                        self.previewed_ref = None;
                    }
                    self.marker_refusals
                        .retain(|path| snapshot.conflict_markers.contains(path));
                    self.path = path;
                    // Git's status replaces any predicted file move.
                    self.settle_prediction(false);
                    self.offer_operation_message(snapshot.operation_message.as_ref());
                    self.snapshot = Some(snapshot);
                    // Writes queued behind a commit start once its refresh lands.
                    if !self.busy {
                        if let Some(next) = self.next_queued() {
                            jobs.extend(self.start_write(next));
                            self.repredict_queued();
                        }
                    }
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
                        let can_reopen = self.can_reopen(&reopen.selection);
                        let DiffSelection { id, path, staged } = reopen.selection;
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
                    discard_blocked,
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
                        discard_blocked,
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
                Ok(Reply::IndexDone {
                    message,
                    worktree,
                    diff,
                }) => {
                    self.operation_notice = Some((message.clone(), false));
                    self.notice = message;
                    if let Some(state) = worktree {
                        self.apply_worktree(state);
                    }
                    if let Some(Reply::Diff {
                        id,
                        path,
                        staged,
                        original_path,
                        content,
                        hunks,
                        hunk_blocked,
                        discard_blocked,
                        token,
                        preview,
                    }) = diff.map(|diff| *diff)
                    {
                        // Install it only if the user still reviews the same
                        // comparison the write started from.
                        let reviewed = DiffSelection {
                            id: id.clone(),
                            path: path.clone(),
                            staged,
                        };
                        if self.selected == id
                            && self.reopen.as_ref().is_some_and(|reopen| {
                                reopen.generation == self.selection_generation
                                    && reopen.selection == reviewed
                            })
                        {
                            self.patch = Some(Patch {
                                id,
                                path,
                                staged,
                                original_path,
                                content,
                                hunks,
                                hunk_blocked,
                                discard_blocked,
                                token,
                                preview,
                            });
                            self.reopen = None;
                            self.keep_patch_on_refresh = true;
                        }
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
                        self.history_page_failed |= requested_limit.is_some();
                        // Writes waiting for this refresh cannot be reviewed.
                        if !self.busy && !self.queued.is_empty() {
                            self.queued.clear();
                            self.settle_prediction(true);
                        }
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
            if !current || failed {
                self.settle_prediction(true);
            } else if self.worktree_fresh {
                // The reply's status already replaced the prediction.
                self.settle_prediction(false);
            }
            let replaced = std::mem::take(&mut self.refresh_after_write);
            let index_only = std::mem::take(&mut self.index_only_write);
            self.running_commit = false;
            // The patch on screen did not depend on this write, so the
            // refresh after it need not reload it either.
            if self.patch_untouched && current && !failed {
                self.keep_patch_on_refresh = true;
            }
            // The reply carried the new working changes; a refresh would
            // only read them again. A retired refresh still runs in full.
            let status_fresh = std::mem::take(&mut self.worktree_fresh);
            let fresh = status_fresh && index_only && !replaced && self.requested_limit.is_none();
            // A failed write leaves the writes waiting behind it unasked for.
            // After a commit they wait for its refresh, so each reviews its
            // file as the commit left it. A whole-file write waits for the
            // refresh too when this write did not return the working changes,
            // as after a hunk write: the status loaded before it no longer
            // matches the file, and the write would be refused.
            let next = if !current || failed {
                self.queued.clear();
                None
            } else if index_only
                && (status_fresh || !matches!(self.queued.front(), Some(Action::StageFile { .. })))
            {
                self.next_queued()
            } else {
                None
            };
            if let Some(next) = next {
                // The next write starts at once. The last one in line
                // refreshes as needed, so this one does not.
                self.refresh_after_write |= replaced;
                jobs.extend(self.start_write(next));
                self.repredict_queued();
            } else if current && (refresh || replaced) && !fresh {
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
                // address a current worker patch again. With nothing left on
                // its side, the view offers the next step instead.
                if fresh && !self.can_reopen(&reopen.selection) {
                    self.patch = None;
                } else {
                    let DiffSelection { path, staged, .. } = reopen.selection;
                    jobs.extend(self.diff(path, staged));
                }
            }
            if !self.busy {
                self.patch_untouched = false;
                // The write cancelled a commit's details read in flight;
                // without a refresh to follow, read them again here.
                if let Some(id) = self.selected.clone().filter(|id| {
                    fresh && self.details_loaded.as_ref() != Some(id) && !self.details_loading()
                }) {
                    jobs.extend(self.read(Lane::Details, Action::Details { id }));
                }
            }
            self.keep_patch_on_refresh = false;
        }
        let slot = &mut self.slots[lane as usize];
        if let Some(request) = slot.pending.take().filter(|r| r.epoch == self.epoch) {
            slot.active = Some((request.epoch, request.sequence));
            jobs.push((lane, request));
        }
        jobs
    }
    /// Installs the working changes an index write returned, if HEAD is
    /// still where the loaded snapshot has it. Otherwise the usual refresh
    /// follows, since history or references may have moved too.
    fn apply_worktree(&mut self, state: crate::service::WorktreeState) {
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        let same_head = snapshot.reviewed.as_ref().is_some_and(|reviewed| {
            reviewed.head == state.reviewed.head
                && reviewed.symbolic_head == state.reviewed.symbolic_head
        });
        if !same_head {
            return;
        }
        snapshot.status = state.status;
        snapshot.reviewed = Some(state.reviewed);
        snapshot.journal = state.journal;
        snapshot.conflict_markers = state.conflict_markers;
        let markers = &snapshot.conflict_markers;
        self.marker_refusals.retain(|path| markers.contains(path));
        self.worktree_fresh = true;
        self.worktree_read_at = state.read_started;
    }

    /// Whether a working-changes comparison still has something to show,
    /// so reloading it is worthwhile. Commit comparisons always do.
    fn can_reopen(&self, selection: &DiffSelection) -> bool {
        selection.id.is_some()
            || self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.status.iter().any(|file| {
                    file.path == selection.path
                        && !file.unsupported_path
                        && (file.on_side(selection.staged) || file.is_unmerged())
                })
            })
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
    fn scroll_paging_waits_for_the_page_and_stops_after_a_failure() {
        let full_page = |count: usize| RepoSnapshot {
            commits: vec![Default::default(); count],
            ..Default::default()
        };
        let mut controller = Controller {
            snapshot: Some(full_page(HISTORY_PAGE)),
            ..Default::default()
        };
        assert!(controller.wants_next_page());
        let (_, page) = controller.load_more().unwrap().dispatch.unwrap();
        assert!(!controller.wants_next_page());
        assert!(controller.loading_history_page());
        let failure = Failure {
            message: "could not load page".into(),
            kind: Default::default(),
            refresh: false,
        };
        controller.complete(Lane::Snapshot, reply(&page, Err(failure)));
        assert!(!controller.wants_next_page());
        assert!(!controller.loading_history_page());

        let (_, refresh) = controller.refresh().unwrap().dispatch.unwrap();
        controller.complete(
            Lane::Snapshot,
            reply(
                &refresh,
                Ok(Reply::Snapshot {
                    path: "repo".into(),
                    snapshot: full_page(HISTORY_PAGE),
                    graph: Default::default(),
                }),
            ),
        );
        assert!(controller.wants_next_page());
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
        let snapshot = |status, markers: &[&str]| {
            Ok(Reply::Snapshot {
                path: "repo".into(),
                snapshot: RepoSnapshot {
                    status,
                    conflict_markers: markers.iter().map(|path| (*path).to_owned()).collect(),
                    ..Default::default()
                },
                graph: Default::default(),
            })
        };
        let jobs = c.complete(
            Lane::Snapshot,
            reply(&refresh[0].1, snapshot(vec![change("d", 'U', 'U')], &["d"])),
        );
        assert!(
            matches!(&jobs[..], [(Lane::Diff, Request { action: Action::Diff { path, .. }, .. })] if path == "d")
        );
        assert!(c.completed_review().is_none());
        assert!(c.marker_refusals.contains("d"));
        let (_, refresh) = c.refresh().unwrap().dispatch.unwrap();
        c.complete(
            Lane::Snapshot,
            reply(&refresh, snapshot(vec![change("d", 'M', ' ')], &[])),
        );
        assert!(c.marker_refusals.is_empty());
    }

    #[test]
    fn marker_refusals_clear_once_the_file_no_longer_has_markers() {
        let mut c = Controller {
            epoch: 1,
            snapshot: Some(RepoSnapshot {
                status: vec![change("d", 'U', 'U')],
                conflict_markers: ["d".to_owned()].into(),
                ..Default::default()
            }),
            marker_refusals: ["d".to_owned()].into(),
            ..Default::default()
        };
        let (_, refresh) = c.refresh().unwrap().dispatch.unwrap();
        c.complete(
            Lane::Snapshot,
            reply(
                &refresh,
                Ok(Reply::Snapshot {
                    path: "repo".into(),
                    snapshot: RepoSnapshot {
                        status: vec![change("d", 'U', 'U')],
                        reviewed: Some(Default::default()),
                        ..Default::default()
                    },
                    graph: Default::default(),
                }),
            ),
        );
        assert!(c.marker_refusals.is_empty());

        c.marker_refusals.insert("d".into());
        c.apply_worktree(crate::service::WorktreeState {
            status: vec![change("d", 'U', 'U')],
            reviewed: Default::default(),
            conflict_markers: ["d".to_owned()].into(),
            journal: Default::default(),
            read_started: None,
        });
        assert!(c.marker_refusals.contains("d"));
        c.apply_worktree(crate::service::WorktreeState {
            status: vec![change("d", 'U', 'U')],
            reviewed: Default::default(),
            conflict_markers: Default::default(),
            journal: Default::default(),
            read_started: None,
        });
        assert!(c.marker_refusals.is_empty());
    }

    #[test]
    fn reworded_operation_feedback_keeps_the_rest_of_the_message() {
        let done = "Checked out topic. Undo is available.";
        let mut c = Controller {
            notice: done.into(),
            operation_notice: Some((done.into(), false)),
            ..Default::default()
        };
        c.reword_operation_notice(
            "Checked out topic",
            "Created branch topic and switched to it",
        );
        assert_eq!(
            c.notice,
            "Created branch topic and switched to it. Undo is available."
        );
        assert_eq!(c.operation_notice.as_ref().unwrap().0, c.notice);
        c.operation_notice = Some(("Checked out topic failed".into(), true));
        c.reword_operation_notice("Checked out topic", "Created");
        assert_eq!(c.operation_notice.unwrap().0, "Checked out topic failed");
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
            discard_blocked: None,
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
    fn a_hunk_write_with_its_diff_is_usable_before_the_refresh_ends() {
        let mut c = Controller {
            epoch: 1,
            snapshot: Some(RepoSnapshot {
                status: vec![change("b", ' ', 'M')],
                ..Default::default()
            }),
            ..Default::default()
        };
        let shown = |token, hunks: usize| Reply::Diff {
            id: None,
            path: "b".into(),
            staged: false,
            original_path: None,
            content: diff::prepare(vec![]),
            hunks: vec!["@@ -1 +1 @@".into(); hunks],
            hunk_blocked: None,
            discard_blocked: None,
            token,
            preview: Default::default(),
        };
        let (_, read) = c.diff("b".into(), false).unwrap();
        c.complete(Lane::Diff, reply(&read, Ok(shown(read.sequence, 2))));
        let (_, write) = c
            .mutate(Action::Hunk {
                token: read.sequence,
                index: 0,
                unstage: false,
            })
            .unwrap();
        // Without the working changes, the usual refresh follows.
        let done = Ok(Reply::IndexDone {
            message: "Staged hunk in b".into(),
            worktree: None,
            diff: Some(Box::new(shown(write.sequence, 1))),
        });
        let refresh = c.complete(Lane::Mutation, reply(&write, done));
        // The refresh still runs, but the new patch is usable at once.
        assert!(matches!(refresh[..], [(Lane::Snapshot, _)]));
        assert!(c.patch_ready());
        assert_eq!(c.patch.as_ref().unwrap().token, write.sequence);
        assert_eq!(c.patch.as_ref().unwrap().hunks.len(), 1);
        // When the refresh lands, nothing reloads the diff.
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
        assert!(jobs.is_empty());
        assert!(c.patch_ready());
        assert_eq!(c.patch.as_ref().unwrap().token, write.sequence);
        // A later refresh, such as from the file watcher, reloads it again.
        let watcher = c.watcher_refresh(false).unwrap().dispatch.unwrap();
        assert!(!c.patch_ready());
        let jobs = c.complete(
            Lane::Snapshot,
            reply(
                &watcher.1,
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
        assert!(matches!(&jobs[0].1.action, Action::Diff { path, .. } if path == "b"));
    }
    #[test]
    fn a_diff_from_a_write_is_dropped_after_switching_files() {
        let mut c = Controller {
            epoch: 1,
            snapshot: Some(RepoSnapshot {
                status: vec![change("b", ' ', 'M'), change("c", ' ', 'M')],
                ..Default::default()
            }),
            ..Default::default()
        };
        let (_, read) = c.diff("b".into(), false).unwrap();
        c.complete(
            Lane::Diff,
            reply(
                &read,
                Ok(Reply::Diff {
                    id: None,
                    path: "b".into(),
                    staged: false,
                    original_path: None,
                    content: diff::prepare(vec![]),
                    hunks: vec!["@@ -1 +1 @@".into(); 2],
                    hunk_blocked: None,
                    discard_blocked: None,
                    token: read.sequence,
                    preview: Default::default(),
                }),
            ),
        );
        let (_, write) = c
            .mutate(Action::Hunk {
                token: read.sequence,
                index: 0,
                unstage: false,
            })
            .unwrap();
        // The user opens another file while the write runs.
        let _ = c.diff("c".into(), false);
        let stale = Reply::Diff {
            id: None,
            path: "b".into(),
            staged: false,
            original_path: None,
            content: diff::prepare(vec![]),
            hunks: vec!["@@ -1 +1 @@".into()],
            hunk_blocked: None,
            discard_blocked: None,
            token: write.sequence,
            preview: Default::default(),
        };
        c.complete(
            Lane::Mutation,
            reply(
                &write,
                Ok(Reply::IndexDone {
                    message: "ok".into(),
                    worktree: None,
                    diff: Some(Box::new(stale)),
                }),
            ),
        );
        assert!(c.patch.as_ref().is_none_or(|patch| patch.path != "b"));
    }
    #[test]
    fn an_index_write_with_its_working_changes_needs_no_refresh() {
        let identity = |head: &str, index: &[u8]| crate::git::WriteIdentity {
            head: head.into(),
            symbolic_head: "refs/heads/main".into(),
            index: index.to_vec(),
        };
        let state = |head: &str, file: crate::git::FileChange| crate::service::WorktreeState {
            status: vec![file],
            reviewed: identity(head, b"new"),
            conflict_markers: Default::default(),
            journal: Default::default(),
            read_started: Some(std::time::Instant::now()),
        };
        let mut c = Controller {
            epoch: 1,
            snapshot: Some(RepoSnapshot {
                status: vec![change("b", ' ', 'M')],
                reviewed: Some(identity("h", b"old")),
                ..Default::default()
            }),
            ..Default::default()
        };
        // Review the file's unstaged diff, then stage the whole file.
        let (_, read) = c.diff("b".into(), false).unwrap();
        c.complete(
            Lane::Diff,
            reply(
                &read,
                Ok(Reply::Diff {
                    id: None,
                    path: "b".into(),
                    staged: false,
                    original_path: None,
                    content: diff::prepare(vec![]),
                    hunks: vec!["@@ -1 +1 @@".into()],
                    hunk_blocked: None,
                    discard_blocked: None,
                    token: read.sequence,
                    preview: Default::default(),
                }),
            ),
        );
        let (_, write) = c
            .mutate(Action::StageFile {
                reviewed: change("b", ' ', 'M'),
                unstage: false,
            })
            .unwrap();
        let jobs = c.complete(
            Lane::Mutation,
            reply(
                &write,
                Ok(Reply::IndexDone {
                    message: "Staged b".into(),
                    worktree: Some(state("h", change("b", 'M', ' '))),
                    diff: None,
                }),
            ),
        );
        // No refresh and no diff reload: the side is empty, so the review is done.
        assert!(jobs.is_empty() && !c.loading() && !c.busy);
        let snapshot = c.snapshot.as_ref().unwrap();
        assert!(snapshot.status[0].on_side(true) && !snapshot.status[0].on_side(false));
        assert_eq!(snapshot.reviewed.as_ref().unwrap().index, b"new");
        assert!(c.completed_review().is_some());
        assert!(c.worktree_read_at.is_some());

        // If HEAD moved meanwhile, history may have too: refresh as before.
        let (_, write) = c
            .mutate(Action::StageFile {
                reviewed: change("b", 'M', ' '),
                unstage: true,
            })
            .unwrap();
        let jobs = c.complete(
            Lane::Mutation,
            reply(
                &write,
                Ok(Reply::IndexDone {
                    message: "Unstaged b".into(),
                    worktree: Some(state("moved", change("b", ' ', 'M'))),
                    diff: None,
                }),
            ),
        );
        assert!(matches!(jobs[..], [(Lane::Snapshot, _)]));
        // The predicted move stays on screen until that refresh lands.
        assert_eq!(
            c.snapshot.as_ref().unwrap().status[0],
            change("b", ' ', 'M')
        );
        assert_eq!(c.pending_paths, ["b"]);
        c.complete(
            Lane::Snapshot,
            reply(
                &jobs[0].1,
                Ok(Reply::Snapshot {
                    path: "repo".into(),
                    snapshot: RepoSnapshot {
                        status: vec![change("b", ' ', 'M')],
                        reviewed: Some(identity("moved", b"new")),
                        ..Default::default()
                    },
                    graph: Default::default(),
                }),
            ),
        );
        assert!(c.pending_paths.is_empty());
    }
    fn fresh_state(files: Vec<crate::git::FileChange>) -> Result<Reply, crate::service::Failure> {
        Ok(Reply::IndexDone {
            message: "ok".into(),
            worktree: Some(crate::service::WorktreeState {
                status: files,
                reviewed: crate::git::WriteIdentity {
                    head: "h".into(),
                    ..Default::default()
                },
                conflict_markers: Default::default(),
                journal: Default::default(),
                read_started: None,
            }),
            diff: None,
        })
    }
    fn reviewed_snapshot(status: Vec<crate::git::FileChange>) -> Option<RepoSnapshot> {
        Some(RepoSnapshot {
            status,
            reviewed: Some(crate::git::WriteIdentity {
                head: "h".into(),
                ..Default::default()
            }),
            ..Default::default()
        })
    }
    #[test]
    fn index_writes_wait_their_turn_and_move_files_at_once() {
        let mut c = Controller {
            epoch: 1,
            snapshot: reviewed_snapshot(vec![change("a", ' ', 'M'), change("b", ' ', 'M')]),
            ..Default::default()
        };
        let stage = |path: &str| Action::StageFile {
            reviewed: change(path, ' ', 'M'),
            unstage: false,
        };
        let (_, first) = c.mutate(stage("a")).unwrap();
        // A second click while the first write runs waits instead of being
        // dropped, and its file moves at once too. Nothing else is blocked.
        assert!(c.mutate(stage("b")).is_none());
        assert_eq!(c.queued_writes(), 1);
        assert!(!c.blocking_write());
        let status = &c.snapshot.as_ref().unwrap().status;
        assert_eq!(status[..], [change("a", 'M', ' '), change("b", 'M', ' ')]);
        // The first write lands: the second starts at once, with b's entry
        // as Git reported it, and b still shows as moving.
        let jobs = c.complete(
            Lane::Mutation,
            reply(
                &first,
                fresh_state(vec![change("a", 'M', ' '), change("b", ' ', 'M')]),
            ),
        );
        let [(Lane::Mutation, second)] = &jobs[..] else {
            panic!("expected the queued write to start, got {jobs:?}");
        };
        assert!(matches!(
            &second.action,
            Action::StageFile { reviewed, unstage: false } if *reviewed == change("b", ' ', 'M')
        ));
        assert_eq!(
            c.snapshot.as_ref().unwrap().status[1],
            change("b", 'M', ' ')
        );
        assert_eq!(c.pending_paths, ["b"]);
        let jobs = c.complete(
            Lane::Mutation,
            reply(
                second,
                fresh_state(vec![change("a", 'M', ' '), change("b", 'M', ' ')]),
            ),
        );
        assert!(jobs.is_empty() && !c.busy && c.pending_paths.is_empty());
    }
    /// Opens b's unstaged diff with two hunks, stages the first hunk and
    /// queues "stage file b" behind it. Returns the hunk write's request.
    fn hunk_write_with_file_stage_queued(c: &mut Controller) -> Request {
        let (_, read) = c.diff("b".into(), false).unwrap();
        let shown = |token, hunks: usize| Reply::Diff {
            id: None,
            path: "b".into(),
            staged: false,
            original_path: None,
            content: diff::prepare(vec![]),
            hunks: vec!["@@ -1 +1 @@".into(); hunks],
            hunk_blocked: None,
            discard_blocked: None,
            token,
            preview: Default::default(),
        };
        c.complete(Lane::Diff, reply(&read, Ok(shown(read.sequence, 2))));
        let (_, write) = c
            .mutate(Action::Hunk {
                token: read.sequence,
                index: 0,
                unstage: false,
            })
            .unwrap();
        assert!(c
            .mutate(Action::StageFile {
                reviewed: change("b", ' ', 'M'),
                unstage: false,
            })
            .is_none());
        assert_eq!(c.queued_writes(), 1);
        write
    }
    #[test]
    fn a_file_stage_queued_behind_a_hunk_write_waits_for_the_new_status() {
        let mut c = Controller {
            epoch: 1,
            snapshot: reviewed_snapshot(vec![change("b", ' ', 'M'), change("c", ' ', 'M')]),
            ..Default::default()
        };
        let write = hunk_write_with_file_stage_queued(&mut c);
        // The hunk write returns only b's diff, so b's status as loaded
        // (" M") is out of date: the stage waits for the refresh.
        let jobs = c.complete(
            Lane::Mutation,
            reply(
                &write,
                Ok(Reply::IndexDone {
                    message: "Staged hunk in b".into(),
                    worktree: None,
                    diff: None,
                }),
            ),
        );
        let [(Lane::Snapshot, refresh)] = &jobs[..] else {
            panic!("expected only the refresh, got {jobs:?}");
        };
        assert!(!c.busy);
        // A file clicked meanwhile keeps its place behind the waiting one.
        assert!(c
            .mutate(Action::StageFile {
                reviewed: change("c", ' ', 'M'),
                unstage: false,
            })
            .is_none());
        assert_eq!(c.queued_writes(), 2);
        let jobs = c.complete(
            Lane::Snapshot,
            reply(
                refresh,
                Ok(Reply::Snapshot {
                    path: "repo".into(),
                    snapshot: reviewed_snapshot(vec![change("b", 'M', 'M'), change("c", ' ', 'M')])
                        .unwrap(),
                    graph: Default::default(),
                }),
            ),
        );
        // b now stages with the entry Git reports after the hunk write.
        let [(Lane::Mutation, next)] = &jobs[..] else {
            panic!("expected the queued write to start, got {jobs:?}");
        };
        assert!(matches!(
            &next.action,
            Action::StageFile { reviewed, unstage: false } if *reviewed == change("b", 'M', 'M')
        ));
        assert_eq!(c.queued_writes(), 1);
    }
    #[test]
    fn opening_another_repository_drops_the_writes_waiting_for_a_refresh() {
        let mut c = Controller {
            epoch: 1,
            snapshot: reviewed_snapshot(vec![change("b", ' ', 'M')]),
            ..Default::default()
        };
        let write = hunk_write_with_file_stage_queued(&mut c);
        let jobs = c.complete(
            Lane::Mutation,
            reply(
                &write,
                Ok(Reply::IndexDone {
                    message: "Staged hunk in b".into(),
                    worktree: None,
                    diff: None,
                }),
            ),
        );
        let [(Lane::Snapshot, refresh)] = &jobs[..] else {
            panic!("expected the refresh, got {jobs:?}");
        };
        assert_eq!(c.queued_writes(), 1);
        // The open waits for the refresh in flight, but the queue is gone.
        assert!(c
            .open_trusted(crate::platform::RepositoryLocation {
                worktree: "other".into(),
                git_dir: "other/.git".into(),
                common_dir: "other/.git".into(),
            })
            .is_none());
        assert_eq!(c.queued_writes(), 0);
        let jobs = c.complete(
            Lane::Snapshot,
            reply(
                refresh,
                Ok(Reply::Snapshot {
                    path: "repo".into(),
                    snapshot: reviewed_snapshot(vec![change("b", 'M', 'M')]).unwrap(),
                    graph: Default::default(),
                }),
            ),
        );
        let [(Lane::Snapshot, open)] = &jobs[..] else {
            panic!("expected the held open to start, got {jobs:?}");
        };
        assert!(matches!(open.action, Action::Open { .. }));
        // The other repository's b is not staged when it loads.
        let jobs = c.complete(
            Lane::Snapshot,
            reply(
                open,
                Ok(Reply::Snapshot {
                    path: "other".into(),
                    snapshot: reviewed_snapshot(vec![change("b", ' ', 'M')]).unwrap(),
                    graph: Default::default(),
                }),
            ),
        );
        assert!(jobs.iter().all(|(lane, _)| *lane != Lane::Mutation));
    }
    #[test]
    fn a_failed_index_write_drops_the_writes_waiting_behind_it() {
        let original = vec![change("a", ' ', 'M'), change("b", ' ', 'M')];
        let mut c = Controller {
            epoch: 1,
            snapshot: reviewed_snapshot(original.clone()),
            ..Default::default()
        };
        let (_, first) = c
            .mutate(Action::StageFile {
                reviewed: change("a", ' ', 'M'),
                unstage: false,
            })
            .unwrap();
        assert!(c.mutate(Action::StageAll { unstage: false }).is_none());
        let failure = Err(crate::service::Failure {
            message: "index.lock exists".into(),
            kind: Default::default(),
            refresh: true,
        });
        let jobs = c.complete(Lane::Mutation, reply(&first, failure));
        // Only the refresh after the failure follows; the status comes back.
        assert!(matches!(jobs[..], [(Lane::Snapshot, _)]));
        assert_eq!(c.queued_writes(), 0);
        assert_eq!(c.snapshot.as_ref().unwrap().status, original);
    }
    #[test]
    fn writing_another_file_keeps_the_open_diff_usable() {
        let mut c = Controller {
            epoch: 1,
            snapshot: reviewed_snapshot(vec![change("a", ' ', 'M'), change("b", ' ', 'M')]),
            ..Default::default()
        };
        let (_, read) = c.diff("b".into(), false).unwrap();
        c.complete(
            Lane::Diff,
            reply(
                &read,
                Ok(Reply::Diff {
                    id: None,
                    path: "b".into(),
                    staged: false,
                    original_path: None,
                    content: diff::prepare(vec![]),
                    hunks: vec!["@@ -1 +1 @@".into()],
                    hunk_blocked: None,
                    discard_blocked: None,
                    token: read.sequence,
                    preview: Default::default(),
                }),
            ),
        );
        let (_, write) = c
            .mutate(Action::StageFile {
                reviewed: change("a", ' ', 'M'),
                unstage: false,
            })
            .unwrap();
        // b's diff cannot change, so it stays usable while a is written.
        assert!(c.patch_ready() && !c.write_touches_patch());
        // A hunk click on it waits for that write instead of being dropped.
        assert!(c
            .mutate(Action::Hunk {
                token: read.sequence,
                index: 0,
                unstage: false,
            })
            .is_none());
        assert_eq!(c.queued_writes(), 1);
        let jobs = c.complete(
            Lane::Mutation,
            reply(
                &write,
                fresh_state(vec![change("a", 'M', ' '), change("b", ' ', 'M')]),
            ),
        );
        // The hunk write starts with the same token; nothing reloads the diff.
        assert!(matches!(
            &jobs[..],
            [(Lane::Mutation, Request { action: Action::Hunk { token, .. }, .. })] if *token == read.sequence
        ));
        assert_eq!(c.patch.as_ref().unwrap().token, read.sequence);
    }
    #[test]
    fn a_commit_empties_staged_at_once_without_locking_the_window() {
        let before = vec![
            change("a", 'M', ' '),
            change("b", 'M', 'M'),
            change("c", ' ', 'M'),
        ];
        let mut c = Controller {
            epoch: 1,
            snapshot: reviewed_snapshot(before.clone()),
            ..Default::default()
        };
        let commit = || Action::Commit {
            message: "m".into(),
            reviewed: None,
        };
        let (_, write) = c.mutate(commit()).unwrap();
        // Staged files leave the list at once; working-tree edits stay.
        let status = &c.snapshot.as_ref().unwrap().status;
        assert_eq!(status[..], [change("b", ' ', 'M'), change("c", ' ', 'M')]);
        assert!(c.busy && !c.blocking_write());
        let failure = Err(crate::service::Failure {
            message: "hook failed".into(),
            kind: Default::default(),
            refresh: true,
        });
        c.complete(Lane::Mutation, reply(&write, failure));
        // The commit failed: the staged files come back.
        assert_eq!(c.snapshot.as_ref().unwrap().status, before);
    }
    #[test]
    fn whole_file_writes_move_files_at_once_and_back_on_failure() {
        let mut c = Controller {
            epoch: 1,
            snapshot: Some(RepoSnapshot {
                status: vec![change("a", ' ', 'M'), change("b", '?', '?')],
                ..Default::default()
            }),
            ..Default::default()
        };
        let (_, write) = c.mutate(Action::StageAll { unstage: false }).unwrap();
        // Both files show as staged before Git answers.
        let status = &c.snapshot.as_ref().unwrap().status;
        assert_eq!(status[..], [change("a", 'M', ' '), change("b", 'A', ' ')]);
        assert_eq!(c.pending_paths, ["a", "b"]);
        let failure = Err(crate::service::Failure {
            message: "index.lock exists".into(),
            kind: Default::default(),
            refresh: true,
        });
        c.complete(Lane::Mutation, reply(&write, failure));
        // The write failed: the status Git last reported comes back.
        let status = &c.snapshot.as_ref().unwrap().status;
        assert_eq!(status[..], [change("a", ' ', 'M'), change("b", '?', '?')]);
        assert!(c.pending_paths.is_empty());
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
    fn gits_prepared_message_fills_only_an_empty_draft_once() {
        fn install(c: &mut Controller, message: Option<&str>) {
            let request = match c.snapshot {
                None => c.open_trusted(RepositoryLocation::test_path("repo")),
                Some(_) => c.refresh().unwrap().dispatch,
            }
            .unwrap()
            .1;
            c.complete(
                Lane::Snapshot,
                reply(
                    &request,
                    Ok(Reply::Snapshot {
                        path: "repo".into(),
                        snapshot: RepoSnapshot {
                            operations: vec!["merge".into()],
                            operation_message: message.map(str::to_owned),
                            ..Default::default()
                        },
                        graph: Default::default(),
                    }),
                ),
            );
        }
        let merge = "Merge branch 'topic'";
        let mut c = Controller::default();
        install(&mut c, Some(merge));
        assert_eq!(c.draft, merge);
        c.set_draft(String::new());
        install(&mut c, Some(merge));
        assert!(c.draft.is_empty());

        c.set_draft("Stale draft".into());
        install(&mut c, None);
        install(&mut c, Some("Revert \"Fix\""));
        assert_eq!(c.draft, "Stale draft");
        c.use_operation_message();
        assert_eq!(c.draft, "Revert \"Fix\"");
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
        // Stage all chosen during the commit waits for it and its refresh.
        assert!(c.mutate(Action::StageAll { unstage: false }).is_none());
        assert_eq!(c.queued_writes(), 1);
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
        let jobs = c.complete(
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
        let [(Lane::Mutation, stage_all)] = &jobs[..] else {
            panic!("the waiting Stage all should start after the refresh: {jobs:?}");
        };
        assert!(matches!(
            stage_all.action,
            Action::StageAll { unstage: false }
        ));
        let done = Ok(Reply::Done {
            message: "ok".into(),
            committed_draft: None,
        });
        let jobs = c.complete(Lane::Mutation, reply(stage_all, done));
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
