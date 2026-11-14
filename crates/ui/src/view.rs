mod branch_picker;
#[cfg(feature = "native-evidence")]
mod evidence;
mod history;
mod inspector;
mod operations;
use operations::{BranchPrompt, PendingRun};
mod repository;
mod review;
mod settings;
mod sidebar;
use super::components::{self, prelude::*};
use super::preferences::Preferences;
use super::theme::Role;
use super::theme::*;
use gitcrab_core::{
    commands::{Command, CommandContext},
    controller::{Controller, Lane, RefreshPlan, Seek},
    git::ReferenceKind,
    presentation::{
        compose_message, display_date, display_path, path_labels, split_message, ToolbarActions,
    },
    service::{
        Action, Failure, GitBackend, GitWorker, Operation, ReadKind, Request, Response,
        ReviewedOperation,
    },
};
use gpui::component::{
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem},
    resizable::{h_resizable, resizable_panel},
    tooltip::Tooltip,
    Selectable, Sizable, Theme, WindowExt,
};
use gpui::{prelude::*, *};
use std::ops::Range;

/// What startup restores, shown where recent repositories are listed.
const SESSION_NOTE: &str = "At startup GitCrab reopens the repositories that were open in your tabs. Empty tabs and commit drafts are not restored.";
/// Shown beside the commit composer while a draft exists.
pub(super) const DRAFT_NOTE: &str =
    "Draft kept in memory for this tab until you commit or close it. It is not saved to disk.";

fn command_glyph(command: Command) -> &'static str {
    match command {
        Command::Refresh => "refresh",
        Command::Undo => "undo",
        Command::Redo => "redo",
        Command::CreateBranch => "branch-plus",
        Command::Checkout => "branch",
        Command::Merge | Command::CherryPick => "merge",
        Command::Revert => "back",
        Command::Stash => "stash",
        Command::PopStash => "pop",
        Command::Fetch => "fetch",
        Command::Pull => "pull",
        Command::Push => "push",
    }
}

const GRAPH_LANE_PITCH: f32 = geometry::SIZE_GRAPH_LANE_PITCH;
const GRAPH_LANE_INSET: f32 = 35.;
// At the minimum width, the rightmost drawable lane is the first lane.
const GRAPH_RIGHT_INSET: f32 =
    gitcrab_core::history_columns::HistoryColumn::Graph.minimum_width() - GRAPH_LANE_INSET;

fn local_author_images() -> std::collections::HashMap<String, std::path::PathBuf> {
    let Some(directory) = std::env::var_os("GITCRAB_AVATAR_DIR") else {
        return Default::default();
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Default::default();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path.extension()?.to_str()?;
            if !entry.file_type().ok()?.is_file() || !matches!(extension, "png" | "jpg" | "jpeg") {
                return None;
            }
            Some((path.file_stem()?.to_str()?.to_owned(), path))
        })
        .collect()
}

fn graph_stroke(scale: f32) -> Pixels {
    px((2. * scale).round() / scale)
}

fn snap_graph_stroke_center(center: Pixels, scale: f32) -> Pixels {
    let half_stroke = graph_stroke(scale) / 2.;
    let edge = (f32::from(center - half_stroke) * scale).round() / scale;
    px(edge) + half_stroke
}

fn graph_lane_positions(
    bounds: Bounds<Pixels>,
    lane_count: usize,
    scale: f32,
) -> impl Fn(usize) -> Pixels {
    let span = (f32::from(bounds.size.width) - GRAPH_LANE_INSET - GRAPH_RIGHT_INSET).max(0.);
    let span = (span * scale).floor();
    let gaps = lane_count.saturating_sub(1).max(1) as f32;
    let min_pitch = (f32::from(graph_stroke(scale)) * scale).round();
    let max_pitch = (GRAPH_LANE_PITCH * scale).round();
    let pitch = (span / gaps).floor().clamp(min_pitch, max_pitch);
    // If the strokes no longer fit side by side, share the last full slot.
    let last_lane = (span / pitch) as usize;
    let first = snap_graph_stroke_center(bounds.origin.x + px(GRAPH_LANE_INSET), scale);
    move |lane| first + px(lane.min(last_lane) as f32 * pitch / scale)
}

fn paint_vertical_lane(w: &mut Window, x: Pixels, top: Pixels, bottom: Pixels, color: u32) {
    let stroke = graph_stroke(w.scale_factor());
    w.paint_quad(fill(
        Bounds::new(point(x - stroke / 2., top), size(stroke, bottom - top)),
        rgb(color),
    ));
}

actions!(
    gitcrab,
    [
        Open,
        Refresh,
        Back,
        Next,
        Previous,
        Copy,
        SelectAll,
        SplitBefore,
        SplitAfter,
        Quit,
        CommitStaged,
        Find,
        FindInHistory,
        SearchNext,
        SearchPrevious,
        StageSelectedFile,
        CopyPatch,
        NextHunk,
        PreviousHunk,
        UndoLast,
        RedoLast
    ]
);
/// A finished write, as the Activity list and status bar show it.
#[derive(Clone)]
struct OperationRecord {
    at: std::time::Instant,
    action: String,
    target: Option<String>,
    /// `None` for success, otherwise how the write failed.
    failure: Option<gitcrab_core::git::FailureKind>,
    details: String,
}

impl OperationRecord {
    fn status(&self) -> &'static str {
        use gitcrab_core::git::FailureKind;
        match self.failure {
            None => "Succeeded",
            Some(FailureKind::Rejected) => "Not completed",
            Some(FailureKind::GitFailure) => "Failed",
            Some(FailureKind::Conflict) => "Conflicts",
            Some(FailureKind::Inspect) => "Needs inspection",
            Some(FailureKind::ConflictMarkers) => "Markers remain",
        }
    }

    /// One copyable line with the time, action, target, status and details.
    fn summary(&self) -> String {
        let target = self
            .target
            .as_ref()
            .map(|target| format!(" · {target}"))
            .unwrap_or_default();
        format!(
            "{}{target} · {} · {}",
            self.action,
            self.status(),
            self.details
        )
    }
}

/// A command chosen while an index write ran. It runs once every write has
/// landed and the working changes are current, as if chosen then.
pub(super) enum AfterWrites {
    Commit,
    Fetch,
    Operation {
        op: Operation,
        target: String,
        then_switch: bool,
    },
}

/// A hunk or line write in flight, drawn on the patch it was started from.
pub(super) struct PendingChange {
    /// Token of the patch the write addresses.
    pub(super) token: u32,
    /// Progress word for the hunk header, such as `Staging…`.
    pub(super) label: &'static str,
    pub(super) target: PendingRows,
}

pub(super) enum PendingRows {
    /// Every line of this zero-based hunk.
    Hunk(usize),
    /// These indices into the patch lines, sorted.
    Lines(Vec<usize>),
}

impl PendingChange {
    /// Describes the rows a hunk or line write changes, or `None` for any
    /// other write.
    fn of(action: &Action) -> Option<Self> {
        let verb = |unstage: bool| {
            if unstage {
                "Unstaging…"
            } else {
                "Staging…"
            }
        };
        let (token, label, target) = match action {
            Action::Hunk {
                token,
                index,
                unstage,
            } => (*token, verb(*unstage), PendingRows::Hunk(*index)),
            Action::DiscardHunk { token, index } => {
                (*token, "Discarding…", PendingRows::Hunk(*index))
            }
            Action::Lines {
                token,
                rows,
                unstage,
            } => (*token, verb(*unstage), PendingRows::Lines(sorted(rows))),
            Action::DiscardLines { token, rows } => {
                (*token, "Discarding…", PendingRows::Lines(sorted(rows)))
            }
            _ => return None,
        };
        Some(Self {
            token,
            label,
            target,
        })
    }

    /// Whether the write changes this line of the patch.
    pub(super) fn covers(&self, content: &gitcrab_core::diff::Content, line: usize) -> bool {
        match &self.target {
            PendingRows::Hunk(index) => gitcrab_core::diff::hunk_of(content, line) == Some(*index),
            PendingRows::Lines(rows) => rows.binary_search(&line).is_ok(),
        }
    }

    /// Whether the write touches this zero-based hunk.
    pub(super) fn touches_hunk(&self, content: &gitcrab_core::diff::Content, hunk: usize) -> bool {
        match &self.target {
            PendingRows::Hunk(index) => *index == hunk,
            PendingRows::Lines(rows) => rows
                .iter()
                .any(|&line| gitcrab_core::diff::hunk_of(content, line) == Some(hunk)),
        }
    }
}

fn sorted(rows: &[usize]) -> Vec<usize> {
    let mut rows = rows.to_vec();
    rows.sort_unstable();
    rows
}

/// A write in flight, named for progress and its activity record.
struct RunningWrite {
    request: (u32, u32),
    action: &'static str,
    target: Option<String>,
    /// A Fetch, Pull or Push, which the status bar and dialog can cancel.
    network: bool,
    /// Cancel was requested and Git is being stopped.
    cancelling: bool,
    /// The toolbar button that spins while this write runs.
    command: Option<Command>,
    /// When its loading period began, including any review before it.
    started: std::time::Instant,
}

/// Save state of the Appearance panel, shown in its footer.
#[derive(Clone, Default, PartialEq)]
enum AppearanceStatus {
    #[default]
    Idle,
    /// A save is queued; the number identifies it, or 0 while debouncing.
    Saving(u64),
    Saved,
    Failed(String),
}

struct OpenFailure {
    path: String,
    details: String,
}

/// A repository tab with its own controller, Git worker and view state.
pub struct GitCrab {
    state: Controller,
    active: bool,
    service: Box<dyn GitWorker>,
    backend: std::sync::Arc<dyn GitBackend>,
    focus: FocusHandle,
    draft: Entity<InputState>,
    wip_summary: Entity<InputState>,
    description: Entity<TextareaState>,
    search: Entity<InputState>,
    reference_search: Entity<InputState>,
    target: Entity<InputState>,
    /// A one-click operation whose preflight review is in the worker.
    pending_run: Option<PendingRun>,
    /// The inline bar naming a new branch.
    branch_prompt: Option<BranchPrompt>,
    /// The branch to switch to once its creation succeeds.
    switch_after_branch: Option<String>,
    split: bool,
    compact: bool,
    show_whitespace: bool,
    /// Split was requested and fits the pane this frame.
    split_active: bool,
    /// Split rows wrapped to the pane, with the patch token they were built for.
    split_wrap: Option<(u32, gitcrab_core::diff::Wrap)>,
    /// Pane that Copy reads in Split.
    copy_side: gitcrab_core::diff::Side,
    /// The patch on screen, kept to follow the selection across reloads.
    shown_patch: Option<gitcrab_core::controller::Patch>,
    /// The displayed file changed outside GitCrab since it was opened.
    diff_updated: bool,
    /// A write finished, so the next patch change is the user's own.
    own_write: bool,
    /// The hunk or lines a write in flight changes, shown as pending on the
    /// patch it was started from until the result replaces that patch.
    pending_change: Option<PendingChange>,
    /// A commit or operation waiting for index writes to land.
    after_writes: Option<AfterWrites>,
    diff_search: Entity<InputState>,
    diff_find_open: bool,
    diff_matches: Vec<usize>,
    diff_match: Option<usize>,
    /// The inspector is hidden while reviewing a diff.
    inspector_collapsed: bool,
    running: Option<RunningWrite>,
    /// Start of the review whose write is being dispatched, handed to it.
    loading_since: Option<std::time::Instant>,
    error_heading: String,
    appearance_status: AppearanceStatus,
    history_scroll: UniformListScrollHandle,
    history_horizontal_scroll: ScrollHandle,
    history_column_drag:
        Option<components::rail::RailDrag<gitcrab_core::history_columns::HistoryColumn>>,
    history_column_drag_start: Option<(gitcrab_core::history_columns::HistoryColumn, Pixels)>,
    history_column_resize: Option<(gitcrab_core::history_columns::HistoryColumn, Pixels, f32)>,
    diff_scroll: UniformListScrollHandle,
    settings_scroll: ScrollHandle,
    activity_scroll: ScrollHandle,
    recent_scroll: ScrollHandle,
    failure_scroll: ScrollHandle,
    error_scroll: ScrollHandle,
    selection: Option<Range<usize>>,
    anchor: usize,
    _subscriptions: Vec<Subscription>,
    preferences: Preferences,
    settings_open: bool,
    last_repository: Option<String>,
    pub(super) awaiting_trust: bool,
    open_failure: Option<OpenFailure>,
    repository_location: Option<gitcrab_core::platform::RepositoryLocation>,
    activity: std::collections::VecDeque<OperationRecord>,
    unread_result: bool,
    constrained_inspector: bool,
    preference_save: Option<Task<()>>,
    saved_preferences: Preferences,
    layout_revision: u64,
    panels: [Entity<gpui::component::resizable::ResizableState>; 3],
    panel_subscriptions: Vec<Subscription>,
    actual_inspector_width: [f32; 3],
    watcher: Option<notify::RecommendedWatcher>,
    watch_epoch: u32,
    watch_pending: bool,
    watch_warning: Option<String>,
    external_dirty: bool,
    /// When the latest full or worktree refresh was sent to the worker. A watcher change
    /// seen before then is already covered by that refresh.
    full_refresh_sent: Option<std::time::Instant>,
    search_matches: Vec<String>,
    search_query: String,
    /// The References dialog is open over a narrow layout.
    references_open: bool,
    /// The last render used the single-pane layout.
    single_pane: bool,
    /// History columns as displayed this frame, adapted to the pane width.
    history_layout: gitcrab_core::history_columns::HistoryColumns,
    /// The open repository's own history column layout, as the user edits it.
    history_columns: gitcrab_core::history_columns::HistoryColumns,
    /// Preference key of the repository `history_columns` belongs to.
    history_columns_repository: Option<String>,
    /// Estimated width of the history pane, in logical pixels.
    history_available: f32,
    author_images: std::collections::HashMap<String, std::path::PathBuf>,
    #[cfg(feature = "native-evidence")]
    evidence_phase: u8,
    #[cfg(feature = "native-evidence")]
    feedback_start: Option<(std::time::Instant, &'static str)>,
    #[cfg(feature = "native-evidence")]
    watch_start: Option<std::time::Instant>,
    #[cfg(feature = "native-evidence")]
    benchmark: (Option<std::time::Instant>, Vec<f64>),
    /// Scripted stage and unstage writes for the `stagefiles` capture view.
    #[cfg(feature = "native-evidence")]
    stage_bench: evidence::StageBench,
}
impl GitCrab {
    /// Creates a tab and starts discovering the supplied repository, if any.
    pub fn new(
        path: Option<String>,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let draft = cx.new(|cx| InputState::new(window, cx).placeholder("Commit summary"));
        let wip_summary = cx.new(|cx| InputState::new(window, cx).placeholder("// WIP"));
        let description = cx.new(|cx| TextareaState::new(window, cx).placeholder("Description"));
        let reference_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter references…"));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search loaded history…"));
        let target = cx
            .new(|cx| InputState::new(window, cx).placeholder("Branch, revision or stash message"));
        let diff_search = cx.new(|cx| InputState::new(window, cx).placeholder("Find in diff…"));
        let subscriptions = vec![
            cx.subscribe(&reference_search, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe_in(
                &target,
                window,
                |v, _, event: &InputEvent, w, cx| match event {
                    InputEvent::Change => v.operation_target_changed(cx),
                    InputEvent::PressEnter {
                        secondary: false, ..
                    } => v.submit_branch_prompt(w, cx),
                    _ => {}
                },
            ),
            cx.observe_in(&draft, window, |v, input, w, cx| {
                if !input.read(cx).focus_handle(cx).is_focused(w) {
                    return;
                }
                let summary = input.read(cx).value().to_string();
                v.state.set_draft(compose_message(
                    &summary,
                    v.description.read(cx).value().as_ref(),
                ));
                if v.wip_summary.read(cx).value().as_ref() != summary {
                    v.wip_summary
                        .update(cx, |s, cx| s.set_value(summary, w, cx));
                }
                cx.notify();
            }),
            cx.observe_in(&wip_summary, window, |v, input, w, cx| {
                if !input.read(cx).focus_handle(cx).is_focused(w) {
                    return;
                }
                let summary = input.read(cx).value().to_string();
                v.state.set_draft(compose_message(
                    &summary,
                    v.description.read(cx).value().as_ref(),
                ));
                if v.draft.read(cx).value().as_ref() != summary {
                    v.draft.update(cx, |s, cx| s.set_value(summary, w, cx));
                }
                cx.notify();
            }),
            cx.observe_in(&description, window, |v, input, w, cx| {
                if !input.read(cx).focus_handle(cx).is_focused(w) {
                    return;
                }
                v.state.set_draft(compose_message(
                    v.draft.read(cx).value().as_ref(),
                    input.read(cx).value().as_ref(),
                ));
                cx.notify();
            }),
            cx.subscribe_in(&search, window, |v, _, event: &InputEvent, w, cx| {
                if let InputEvent::PressEnter {
                    secondary: false, ..
                } = event
                {
                    v.search_step(1, w, cx);
                } else if matches!(event, InputEvent::Change) {
                    v.update_search(cx);
                }
                cx.notify();
            }),
            cx.subscribe_in(&diff_search, window, |v, _, event: &InputEvent, w, cx| {
                match event {
                    InputEvent::PressEnter {
                        secondary: false, ..
                    } => v.diff_find_step(1, w, cx),
                    InputEvent::Change => v.update_diff_find(true, cx),
                    _ => {}
                }
                cx.notify();
            }),
            cx.observe_window_activation(window, |v, w, cx| {
                if v.active
                    && w.is_window_active()
                    && !v.state.busy
                    && (v.external_dirty || v.watcher.is_none())
                {
                    v.external_dirty = false;
                    v.refresh(&Refresh, w, cx);
                }
            }),
        ];
        let backend = crate::backend::get(cx);
        let preferences = crate::preferences::current(cx);
        gpui::component::Theme::global_mut(cx).font_size = px(preferences.font_size);
        let mut v = Self {
            state: Controller::default(),
            active,
            service: backend.worker(),
            backend,
            focus: cx.focus_handle(),
            draft,
            wip_summary,
            description,
            search,
            reference_search,
            target,
            pending_run: None,
            branch_prompt: None,
            switch_after_branch: None,
            split: preferences.split,
            compact: preferences.compact,
            show_whitespace: false,
            split_active: false,
            split_wrap: None,
            copy_side: Default::default(),
            shown_patch: None,
            diff_updated: false,
            own_write: false,
            pending_change: None,
            after_writes: None,
            diff_search,
            diff_find_open: false,
            diff_matches: vec![],
            diff_match: None,
            inspector_collapsed: false,
            running: None,
            loading_since: None,
            error_heading: String::new(),
            appearance_status: AppearanceStatus::Idle,
            history_scroll: UniformListScrollHandle::new(),
            history_horizontal_scroll: ScrollHandle::new(),
            history_column_drag: None,
            history_column_drag_start: None,
            history_column_resize: None,
            diff_scroll: UniformListScrollHandle::new(),
            settings_scroll: ScrollHandle::new(),
            activity_scroll: ScrollHandle::new(),
            recent_scroll: ScrollHandle::new(),
            failure_scroll: ScrollHandle::new(),
            error_scroll: ScrollHandle::new(),
            selection: None,
            anchor: 0,
            _subscriptions: subscriptions,
            saved_preferences: preferences.clone(),
            preferences,
            settings_open: false,
            last_repository: crate::preferences::last_repository(cx),
            awaiting_trust: false,
            open_failure: None,
            repository_location: None,
            activity: Default::default(),
            unread_result: false,
            constrained_inspector: false,
            preference_save: None,
            layout_revision: 0,
            panels: [
                cx.new(|_| Default::default()),
                cx.new(|_| Default::default()),
                cx.new(|_| Default::default()),
            ],
            panel_subscriptions: vec![],
            actual_inspector_width: [0.; 3],
            watcher: None,
            watch_epoch: 0,
            watch_pending: false,
            watch_warning: None,
            external_dirty: false,
            full_refresh_sent: None,
            search_matches: vec![],
            search_query: String::new(),
            references_open: false,
            single_pane: false,
            history_layout: Default::default(),
            history_columns: Default::default(),
            history_columns_repository: None,
            history_available: 0.,
            author_images: local_author_images(),
            #[cfg(feature = "native-evidence")]
            evidence_phase: 0,
            #[cfg(feature = "native-evidence")]
            feedback_start: None,
            #[cfg(feature = "native-evidence")]
            watch_start: None,
            #[cfg(feature = "native-evidence")]
            benchmark: (None, vec![]),
            #[cfg(feature = "native-evidence")]
            stage_bench: Default::default(),
        };
        v.reset_panels(cx);
        if active {
            v.focus.focus(window, cx);
        }
        if let Some(path) = path {
            let view = cx.weak_entity();
            window.defer(cx, move |window, cx| {
                let _ = view.update(cx, |v, cx| v.open_path(path, window, cx));
            });
        }
        v
    }
    fn dispatch(
        &mut self,
        job: Option<(Lane, Request)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((lane, request)) = job else {
            cx.notify();
            return;
        };
        let epoch = request.epoch;
        let sequence = request.sequence;
        // A worktree refresh also re-reads references, HEAD and history
        // metadata and reloads history when they changed, so it covers
        // metadata changes too.
        if matches!(
            request.action,
            Action::Refresh { .. } | Action::RefreshWorktree { .. } | Action::Open { .. }
        ) {
            self.full_refresh_sent = Some(std::time::Instant::now());
        }
        match self.service.submit(request) {
            Ok(rx) => {
                cx.spawn_in(window, async move |view, cx| {
                    let response = rx.await.unwrap_or(Response {
                        epoch,
                        sequence,
                        elapsed_ms: 0.,
                        queue_ms: 0.,
                        result: Err(Failure {
                            message: "Git worker closed".into(),
                            kind: gitcrab_core::git::FailureKind::Inspect,
                            refresh: false,
                        }),
                    });
                    let _ =
                        view.update_in(cx, |v, window, cx| v.complete(lane, response, window, cx));
                })
                .detach();
            }
            Err(message) => self.complete(
                lane,
                Response {
                    epoch,
                    sequence,
                    elapsed_ms: 0.,
                    queue_ms: 0.,
                    result: Err(Failure {
                        message,
                        kind: gitcrab_core::git::FailureKind::Rejected,
                        refresh: false,
                    }),
                },
                window,
                cx,
            ),
        }
        cx.notify();
    }
    fn complete(
        &mut self,
        lane: Lane,
        response: Response,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.accepts_response(lane, &response) {
            for job in self.state.complete(lane, response) {
                self.dispatch(Some(job), window, cx);
            }
            cx.notify();
            return;
        }
        // gpui-component does not emit Change for provisional IME text. Capture
        // the focused native buffer before reconciling an asynchronous result.
        if [&self.wip_summary, &self.draft]
            .into_iter()
            .any(|input| input.read(cx).focus_handle(cx).is_focused(window))
            || self
                .description
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        {
            let summary = if self
                .wip_summary
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
            {
                self.wip_summary.read(cx).value()
            } else {
                self.draft.read(cx).value()
            };
            self.state.set_draft(compose_message(
                summary.as_ref(),
                self.description.read(cx).value().as_ref(),
            ));
        }
        let request = (response.epoch, response.sequence);
        let mut mutation_ok = None;
        let failure_kind = response.result.as_ref().err().map(|failure| failure.kind);
        let running = self
            .running
            .take_if(|running| running.request == request)
            .map(|running| (running.action, running.target));
        if lane == Lane::Mutation {
            let (summary, failure) = match &response.result {
                Ok(
                    gitcrab_core::service::Reply::Done { message, .. }
                    | gitcrab_core::service::Reply::IndexDone { message, .. },
                ) => (message.clone(), None),
                Err(failure) => (failure.message.clone(), Some(failure.kind)),
                _ => (
                    "Operation result requires inspection".into(),
                    Some(gitcrab_core::git::FailureKind::Inspect),
                ),
            };
            let (action, target) = running.clone().unwrap_or(("Git operation", None));
            if let Some(kind) = failure {
                self.error_heading = gitcrab_core::presentation::failure_heading(action, kind);
                // The patch on screen is unchanged, so show it as it was.
                self.pending_change = None;
                // A commit chosen during the failed write would no longer
                // commit what was on screen.
                self.after_writes = None;
            }
            self.activity.push_front(OperationRecord {
                at: std::time::Instant::now(),
                action: action.to_owned(),
                target,
                failure,
                details: summary.clone(),
            });
            self.activity.truncate(50);
            self.unread_result = !self.active;
            self.own_write = true;
            mutation_ok = Some(failure.is_none());
        } else if failure_kind.is_some() {
            self.error_heading = match lane {
                Lane::Snapshot if self.state.snapshot.is_none() => "Could not open the repository",
                Lane::Snapshot => "Could not refresh the repository",
                Lane::Details => "Could not load commit details",
                Lane::Diff => "Could not load changes",
                _ => "Could not review the operation",
            }
            .into();
        }
        let opening = lane == Lane::Snapshot && self.state.snapshot.is_none();
        let jobs = self.state.complete(lane, response);
        if opening && self.state.snapshot.is_some() {
            self.load_history_columns(cx);
        }
        // A write that returned the working changes read them like a
        // refresh would, so earlier watcher events are already reflected.
        if let Some(read) = self.state.worktree_read_at.take() {
            self.full_refresh_sent = self.full_refresh_sent.max(Some(read));
        }
        if matches!(lane, Lane::Diff | Lane::Snapshot | Lane::Mutation) {
            self.follow_patch(cx);
        }
        if lane == Lane::OperationReview {
            if let Some(result) = self.state.operation_review.take() {
                self.operation_resolved(request, result, window, cx);
            }
        }
        if opening && !self.state.loading() {
            if self.state.snapshot.is_some() {
                // The workspace persists the full tab list when tabs change.
                self.last_repository = Some(self.state.path.clone());
            } else if self.state.error {
                let path = std::mem::take(&mut self.state.path);
                match crate::preferences::forget_last_repository(&path, cx) {
                    Ok(result) => {
                        if self.last_repository.as_deref() == Some(path.as_str()) {
                            self.last_repository = None;
                        }
                        self.observe_persistence(result, "last repository", cx);
                    }
                    Err(error) => self.persistence_error("last repository", error),
                }
                let details = if std::path::Path::new(&path).is_dir() {
                    std::mem::take(&mut self.state.notice)
                } else {
                    self.state.notice.clear();
                    "The folder is unavailable. Browse to its current location or choose another repository.".into()
                };
                self.state.error = false;
                self.open_failure = Some(OpenFailure { path, details });
            }
        }
        #[cfg(feature = "native-evidence")]
        if lane == Lane::Snapshot && !self.state.loading() {
            if let Some(start) = self.watch_start.take() {
                super::trace::record("external-event-to-state", start, cx);
            }
        }
        let (summary, body) = split_message(&self.state.draft);
        let summary = summary.to_string();
        let body = body.to_string();
        for (input, value) in [(&self.wip_summary, summary.clone()), (&self.draft, summary)] {
            input.update(cx, |s, cx| {
                if s.marked_text_range(window, cx).is_none() && s.value().as_ref() != value {
                    s.set_value(value, window, cx);
                }
            });
        }
        self.description.update(cx, |s, cx| {
            if s.marked_text_range(window, cx).is_none() && s.value().as_ref() != body {
                s.set_value(body, window, cx);
            }
        });
        if lane == Lane::Snapshot && self.state.snapshot.is_some() {
            self.update_search(cx);
            self.install_watch(window, cx);
            if opening {
                match crate::preferences::add_recent(&self.state.path, cx) {
                    Ok(Some(result)) => self.observe_persistence(result, "recent repositories", cx),
                    Err(error) => self.persistence_error("recent repositories", error),
                    _ => {}
                }
            }
        }
        for job in jobs {
            // A queued write starting now gets its progress label and
            // pending rows like one started by a click.
            if job.0 == Lane::Mutation {
                self.dispatch_mutation(Some(job), window, cx);
            } else {
                self.dispatch(Some(job), window, cx);
            }
        }
        if std::mem::take(&mut self.state.revealed) {
            // A commit or reference activated outside the loaded page has arrived.
            let id = self.state.selected.clone();
            self.reveal_selection(id.as_deref(), window, cx);
        }
        if matches!(lane, Lane::Snapshot | Lane::Mutation) {
            let plan = self.state.take_watcher_refresh();
            self.apply_refresh(plan, window, cx);
        }
        if let Some(ok) = mutation_ok {
            self.after_mutation(ok, window, cx);
        }
        // Writes have landed and the working changes are current: run the
        // command chosen meanwhile.
        if !self.state.busy && !self.state.loading() {
            match self.after_writes.take() {
                Some(AfterWrites::Commit) => self.commit_staged(&CommitStaged, window, cx),
                Some(AfterWrites::Fetch) => self.mutate(Action::Fetch, window, cx),
                Some(AfterWrites::Operation {
                    op,
                    target,
                    then_switch,
                }) => {
                    self.start_operation(op, target, then_switch, window, cx);
                }
                None => {}
            }
        }
        #[cfg(feature = "native-evidence")]
        self.advance_evidence(lane, window, cx);
        cx.notify();
    }
    fn update_search(&mut self, cx: &Context<Self>) {
        self.search_query = gitcrab_core::git::normalize_search(&self.search.read(cx).value());
        let query = &self.search_query;
        self.search_matches = self
            .state
            .snapshot
            .as_ref()
            .map(|s| {
                s.commits
                    .iter()
                    .filter(|c| !query.is_empty() && c.matches_search(query))
                    .map(|c| c.id.clone())
                    .collect()
            })
            .unwrap_or_default();
    }
    fn persist_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences.compact = self.compact;
        self.preferences.split = self.split;
        if self.settings_open {
            self.appearance_status = AppearanceStatus::Saving(0);
        }
        let timer = cx
            .background_executor()
            .timer(std::time::Duration::from_millis(200));
        self.preference_save = Some(cx.spawn_in(window, async move |view, cx| {
            timer.await;
            let _ = view.update_in(cx, |v, _, cx| {
                v.preference_save = None;
                if let Err(error) = v.save_preferences(false, cx) {
                    v.persistence_error("appearance", &error);
                    cx.notify();
                }
            });
        }));
    }

    /// Queues the tab's appearance and layout. The Appearance footer reports
    /// Saved only after the settings writer acknowledges this exact save.
    fn save_preferences(&mut self, announce: bool, cx: &mut Context<Self>) -> Result<(), String> {
        static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        self.store_history_columns();
        self.preferences.compact = self.compact;
        self.preferences.split = self.split;
        let result =
            crate::preferences::save_changes(&self.preferences, &self.saved_preferences, cx)?;
        let save = SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.appearance_status = AppearanceStatus::Saving(save);
        cx.spawn(async move |view, cx| {
            let outcome = result
                .await
                .unwrap_or_else(|_| Err("Settings writer closed.".into()));
            let _ = view.update(cx, |v, cx| {
                match outcome {
                    Ok(()) if v.appearance_status == AppearanceStatus::Saving(save) => {
                        v.appearance_status = AppearanceStatus::Saved;
                        if announce && !v.state.busy && !v.state.error {
                            v.state.notice = "Appearance saved".into();
                        }
                    }
                    Ok(()) => {}
                    Err(error) => v.persistence_error("appearance", error),
                }
                cx.notify();
            });
        })
        .detach();
        self.saved_preferences = self.preferences.clone();
        Ok(())
    }

    fn persistence_error(&mut self, setting: &str, error: impl std::fmt::Display) {
        self.state.error = true;
        self.error_heading = format!("Could not save {setting}");
        self.state.notice = format!("Could not save {setting}: {error}");
        if setting == "appearance" {
            self.appearance_status = AppearanceStatus::Failed(error.to_string());
        }
    }

    fn observe_persistence(
        &mut self,
        result: super::preferences::PersistenceResult,
        setting: &'static str,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |view, cx| {
            if let Err(error) = result
                .await
                .unwrap_or_else(|_| Err("Settings writer closed.".into()))
            {
                let _ = view.update(cx, |v, cx| {
                    v.persistence_error(setting, error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Queues any unsaved tab preferences immediately.
    /// Use [`crate::preferences::flush`] to wait for the settings writer.
    pub fn flush_preferences(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        self.store_history_columns();
        if self.preference_save.take().is_some() || self.preferences != self.saved_preferences {
            self.save_preferences(false, cx)?;
        }
        Ok(())
    }

    /// Records the column layout under its repository when the user changed it,
    /// so repositories that keep the shared layout gain no entry.
    fn store_history_columns(&mut self) {
        let Some(key) = self.history_columns_repository.clone() else {
            return;
        };
        if self.preferences.columns_for(&key) != self.history_columns {
            self.preferences
                .repository_columns
                .insert(key, self.history_columns.clone());
        }
    }

    /// Switches the history columns to the layout saved for the repository just opened.
    /// Edits made for the previous repository are kept under that repository.
    fn load_history_columns(&mut self, cx: &mut Context<Self>) {
        self.store_history_columns();
        let key = Preferences::repository_key(&self.state.path);
        let unsaved = self.preferences.repository_columns.get(&key)
            != self.saved_preferences.repository_columns.get(&key);
        self.history_columns = if unsaved {
            self.preferences.columns_for(&key)
        } else {
            // Another tab may have saved a newer layout for this repository.
            crate::preferences::current(cx).columns_for(&key)
        };
        self.history_columns_repository = Some(key);
    }

    fn reset_panels(&mut self, cx: &mut Context<Self>) {
        self.panels = [
            cx.new(|_| Default::default()),
            cx.new(|_| Default::default()),
            cx.new(|_| Default::default()),
        ];
        self.actual_inspector_width = [self.preferences.inspector_width; 3];
        self.panel_subscriptions = self
            .panels
            .iter()
            .enumerate()
            .map(|(index, panel)| {
                cx.observe(panel, move |v, panel, cx| {
                    if let Some(width) = panel.read(cx).sizes().get(2).map(|w| f32::from(*w)) {
                        if width > 0. && (width - v.actual_inspector_width[index]).abs() > 0.5 {
                            v.actual_inspector_width[index] = width;
                            cx.notify();
                        }
                    }
                })
            })
            .collect();
    }
    fn inspector_editor_width(&self) -> f32 {
        (self.actual_inspector_width[usize::from(self.state.diff_context.is_some())]
            - (2. * INSPECTOR_INSET * self.preferences.font_size / 13. + SEAM))
            .max(100.)
    }
    fn search_step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open || window.has_active_dialog(cx) {
            return;
        }
        if self.search_matches.is_empty() {
            return;
        }
        let keep_focus = self.search.read(cx).focus_handle(cx).is_focused(window);
        let current = self
            .state
            .selected
            .as_ref()
            .and_then(|id| self.search_matches.iter().position(|m| m == id));
        let index = current
            .map(|i| (i as isize + delta).rem_euclid(self.search_matches.len() as isize) as usize)
            .unwrap_or(0);
        self.select(Some(self.search_matches[index].clone()), window, cx);
        if keep_focus {
            self.search.update(cx, |s, cx| s.focus(window, cx));
        }
    }
    pub fn repository_path(&self) -> &str {
        &self.state.path
    }

    /// Returns the canonical worktree path once a snapshot has loaded.
    pub fn repository_identity(&self) -> Option<&std::path::Path> {
        self.state
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.worktree_id.as_path())
    }

    /// Reports whether a repository is loaded or a snapshot request is active.
    pub fn has_repository(&self) -> bool {
        self.state.snapshot.is_some() || self.state.loading()
    }

    pub fn settings_open(&self) -> bool {
        self.settings_open
    }

    /// Reports whether a write finished while this tab was inactive.
    pub fn unread_result(&self) -> bool {
        self.unread_result
    }

    pub fn mutating(&self) -> bool {
        self.state.busy
    }

    pub fn has_drafts(&self) -> bool {
        self.state.has_drafts()
    }

    /// Returns the repository epoch and draft revision for rechecking a deferred close.
    pub fn close_identity(&self) -> (u32, u64) {
        (self.state.epoch, self.state.draft_revision)
    }

    /// Focuses the tab, syncs shared appearance and refreshes deferred external changes.
    pub fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active = true;
        self.unread_result = false;
        let app = crate::preferences::current(cx);
        self.preferences.theme = app.theme;
        self.preferences.font_size = app.font_size;
        self.focus.focus(window, cx);
        if self.external_dirty && !self.state.busy {
            self.external_dirty = false;
            self.refresh(&Refresh, window, cx);
        }
    }

    pub fn deactivate(&mut self) {
        self.active = false;
    }

    fn refresh(&mut self, _: &Refresh, window: &mut Window, cx: &mut Context<Self>) {
        let plan = self.state.refresh();
        self.apply_refresh(plan, window, cx);
    }
    fn apply_refresh(
        &mut self,
        plan: Option<RefreshPlan>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(plan) = plan {
            for kind in plan.cancel {
                self.service.cancel_read(kind);
            }
            self.dispatch(plan.dispatch, window, cx);
        }
    }
    fn select(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.service.cancel_read(ReadKind::Details);
        self.service.cancel_read(ReadKind::Diff);
        self.reveal_selection(id.as_deref(), window, cx);
        let job = self.state.select(id);
        self.dispatch(job, window, cx);
    }

    fn preview_ref(
        &mut self,
        kind: ReferenceKind,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.state.seek_ref(kind, name) {
            Seek::Selected(job) => {
                self.service.cancel_read(ReadKind::Details);
                self.service.cancel_read(ReadKind::Diff);
                let id = self.state.selected.clone();
                self.reveal_selection(id.as_deref(), window, cx);
                self.dispatch(job, window, cx);
            }
            Seek::Loading(plan) => self.apply_refresh(plan, window, cx),
            Seek::NotFound => cx.notify(),
        }
        self.close_references(window, cx);
    }

    fn reveal_selection(&mut self, id: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(feature = "native-evidence")]
        {
            self.feedback_start = Some((std::time::Instant::now(), "selection-to-next-frame"));
        }
        self.selection = None;
        self.focus.focus(window, cx);
        if let Some(snapshot) = &self.state.snapshot {
            if let Some(index) = id.and_then(|id| snapshot.commits.iter().position(|c| c.id == id))
            {
                self.history_scroll
                    .scroll_to_item(index + 1, ScrollStrategy::Center);
            } else if id.is_none() {
                self.history_scroll.scroll_to_item(0, ScrollStrategy::Top);
            } else {
                self.state.notice =
                    "This commit is outside loaded history. Load more commits to reveal it.".into();
            }
        }
    }
    fn file(&mut self, path: String, staged: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.service.cancel_read(ReadKind::Diff);
        self.constrained_inspector = false;
        self.selection = None;
        self.shown_patch = None;
        self.diff_updated = false;
        self.diff_scroll = UniformListScrollHandle::new();
        let j = self.state.diff(path, staged);
        self.dispatch(j, window, cx);
    }
    fn mutate(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        let job = self.state.mutate(action);
        self.dispatch_mutation(job, window, cx);
    }
    fn dispatch_mutation(
        &mut self,
        job: Option<(Lane, Request)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        #[cfg(feature = "native-evidence")]
        {
            self.feedback_start =
                Some((std::time::Instant::now(), "mutation-feedback-to-next-frame"));
        }
        let started = self
            .loading_since
            .take()
            .unwrap_or_else(std::time::Instant::now);
        // Without a job the write was queued or refused; what is on screen
        // stays as it is until a write actually starts.
        if job.is_some() {
            if self.state.write_touches_patch() {
                self.selection = None;
            }
            self.pending_change = job
                .as_ref()
                .and_then(|(_, request)| PendingChange::of(&request.action));
        }
        if let Some((_, request)) = &job {
            let target = match &request.action {
                Action::Hunk { .. }
                | Action::DiscardHunk { .. }
                | Action::Lines { .. }
                | Action::DiscardLines { .. } => self
                    .state
                    .patch
                    .as_ref()
                    .map(|patch| gitcrab_core::git::display_path(&patch.path)),
                action => action.target(),
            };
            self.running = Some(RunningWrite {
                request: (request.epoch, request.sequence),
                action: request.action.label(),
                target,
                network: request.action.is_network(),
                cancelling: false,
                command: operations::loading_command(&request.action),
                started,
            });
            self.reveal_loader_later(cx);
        }
        self.dispatch(job, window, cx);
    }

    /// The pending hunk or line write, when the patch it addresses is on screen.
    pub(super) fn pending_on_screen(&self) -> Option<&PendingChange> {
        let token = self.state.patch.as_ref()?.token;
        self.pending_change
            .as_ref()
            .filter(|pending| pending.token == token)
    }

    /// Progress text for the write in flight, such as `Stage hunk · a.txt…`.
    fn running_label(&self) -> String {
        match &self.running {
            Some(RunningWrite {
                action,
                cancelling: true,
                ..
            }) => format!("Cancelling {action}…"),
            Some(RunningWrite {
                action,
                target: Some(target),
                ..
            }) => format!("{action} · {target}…"),
            Some(running) => format!("{}…", running.action),
            None => "Running Git operation…".into(),
        }
    }

    /// Keeps the diff selection on the same text when a reload replaces the
    /// patch, and notes changes that did not come from this tab's own write.
    fn follow_patch(&mut self, cx: &mut Context<Self>) {
        let Some(patch) = self.state.patch.as_ref() else {
            return;
        };
        let shown = self.shown_patch.take();
        if shown
            .as_ref()
            .is_some_and(|shown| shown.token == patch.token)
        {
            self.shown_patch = shown;
            return;
        }
        match shown {
            Some(shown)
                if shown.id == patch.id
                    && shown.path == patch.path
                    && shown.staged == patch.staged =>
            {
                let changed = shown.content.lines.len() != patch.content.lines.len()
                    || shown
                        .content
                        .lines
                        .iter()
                        .zip(&patch.content.lines)
                        .any(|(old, new)| old.text != new.text);
                if changed {
                    if !self.own_write {
                        self.diff_updated = true;
                    }
                    if let Some(range) = self.selection.take() {
                        let old = gitcrab_core::diff::row_keys(&shown.content, self.split_active);
                        let new = gitcrab_core::diff::row_keys(&patch.content, self.split_active);
                        self.selection = gitcrab_core::diff::reanchor(&old, &new, range.clone());
                        self.anchor = match &self.selection {
                            Some(moved) => (self.anchor + moved.start).saturating_sub(range.start),
                            None => 0,
                        };
                    }
                }
            }
            _ => {
                self.selection = None;
                self.anchor = 0;
                self.diff_updated = false;
            }
        }
        self.own_write = false;
        self.shown_patch = Some(patch.clone());
        self.update_diff_find(false, cx);
    }

    /// Recomputes Find in diff matches for the displayed layout. With
    /// `reveal`, the query changed and the first match scrolls into view.
    fn update_diff_find(&mut self, reveal: bool, cx: &mut Context<Self>) {
        let query = self.diff_search.read(cx).value().to_string();
        self.diff_matches = match (&self.state.patch, self.diff_find_open) {
            (Some(patch), true) => gitcrab_core::diff::find_rows(
                &patch.content.lines,
                self.split_active.then_some(patch.content.split.as_slice()),
                &query,
            ),
            _ => vec![],
        };
        self.diff_match = match self.diff_match {
            _ if self.diff_matches.is_empty() => None,
            Some(index) if !reveal => Some(index.min(self.diff_matches.len() - 1)),
            _ => Some(0),
        };
        if reveal {
            if let Some(row) = self.diff_match.map(|index| self.diff_matches[index]) {
                self.scroll_diff_to(row, ScrollStrategy::Center);
            }
        }
    }

    /// Moves to the next or previous Find in diff match, wrapping at the ends.
    fn diff_find_step(&mut self, delta: isize, _: &mut Window, cx: &mut Context<Self>) {
        if self.diff_matches.is_empty() {
            return;
        }
        let count = self.diff_matches.len() as isize;
        let index = match self.diff_match {
            Some(index) => (index as isize + delta).rem_euclid(count) as usize,
            None => 0,
        };
        self.diff_match = Some(index);
        self.scroll_diff_to(self.diff_matches[index], ScrollStrategy::Center);
        cx.notify();
    }

    fn open_diff_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.diff_find_open = true;
        self.diff_search.update(cx, |s, cx| s.focus(window, cx));
        self.update_diff_find(true, cx);
        cx.notify();
    }

    fn close_diff_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.diff_find_open = false;
        self.diff_matches.clear();
        self.diff_match = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Closes the diff from its visible back controls, including any Find bar.
    fn leave_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.diff_find_open = false;
        self.diff_matches.clear();
        self.diff_match = None;
        self.back(&Back, window, cx);
    }
    /// Shows reference navigation over a narrow layout. Choosing a reference closes it.
    fn show_references(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open || window.has_active_dialog(cx) {
            return;
        }
        self.references_open = true;
        let weak = cx.weak_entity();
        let closed = weak.clone();
        window.open_alert_dialog(cx, move |d, w, cx| {
            let content = weak.update(cx, |v, cx| v.sidebar(w, cx)).ok();
            let closed = closed.clone();
            d.title("References")
                .footer(components::dialog_footer([components::dialog_ok(
                    "OK",
                    components::ButtonKind::Primary,
                    cx,
                )]))
                .p_4()
                .child(div().h(px(400.)).children(content))
                .on_close(move |_, _, cx| {
                    let _ = closed.update(cx, |v, cx| {
                        v.references_open = false;
                        cx.notify();
                    });
                })
        });
    }

    fn close_references(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.references_open) {
            window.close_dialog(cx);
            cx.notify();
        }
    }

    /// Captures the state every command surface uses to decide availability.
    fn command_context(&self, window: &mut Window, cx: &mut App) -> CommandContext {
        let snapshot = self.state.snapshot.as_ref();
        CommandContext {
            repository: snapshot.is_some(),
            // Index writes do not block commands: a command chosen during
            // one waits for it (`AfterWrites`).
            busy: self.state.blocking_write() || self.pending_run.is_some(),
            modal: self.settings_open || (window.has_active_dialog(cx) && !self.references_open),
            reviewable: snapshot.is_some_and(|s| s.reviewed.is_some()),
            unborn: snapshot.is_some_and(|s| s.head_id.is_empty()),
            stash_count: snapshot.map_or(0, |s| s.stash_count),
            tracked_changes: snapshot
                .is_some_and(|s| s.status.iter().any(|f| !matches!(f.index, '?' | '!'))),
            undo: snapshot.map_or(Default::default(), |s| {
                s.journal.state(gitcrab_core::journal::Direction::Undo)
            }),
            redo: snapshot.map_or(Default::default(), |s| {
                s.journal.state(gitcrab_core::journal::Direction::Redo)
            }),
            detached: snapshot.is_some_and(|s| s.is_detached()),
            remotes: snapshot.map_or(0, |s| s.remote.remotes.len()),
            upstream: snapshot.is_some_and(|s| s.remote.network_upstream().is_some()),
            push_remote: snapshot.is_some_and(|s| {
                s.remote.network_upstream().is_some()
                    || (s.remote.upstream.is_none() && s.remote.default_push_remote().is_some())
            }),
            ahead: snapshot
                .and_then(|s| s.remote.upstream.as_ref())
                .map_or(0, |u| u.ahead),
            behind: snapshot
                .and_then(|s| s.remote.upstream.as_ref())
                .map_or(0, |u| u.behind),
        }
    }

    /// Tooltip for a command, naming the journal step Undo or Redo acts on.
    fn command_tooltip(&self, command: Command, context: &CommandContext) -> String {
        let direction = match command {
            Command::Undo => gitcrab_core::journal::Direction::Undo,
            Command::Redo => gitcrab_core::journal::Direction::Redo,
            Command::Fetch | Command::Pull | Command::Push => {
                let upstream = self
                    .state
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.remote.network_upstream())
                    .map(|u| u.name.as_str());
                return command.describe_with_upstream(context, upstream);
            }
            _ => return command.describe(context),
        };
        let summary = self
            .state
            .snapshot
            .as_ref()
            .and_then(|s| s.journal.summary(direction));
        command.describe_with_summary(context, summary)
    }

    /// Runs Undo or Redo from its shortcut. A focused text field keeps its
    /// own editing undo, so the shortcut does nothing to the repository there.
    fn journal_shortcut(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        if window
            .context_stack()
            .iter()
            .any(|context| context.contains("Input"))
        {
            return;
        }
        self.run_command(command, String::new(), window, cx);
    }

    /// Runs a command from any surface, or explains in the status bar why it cannot run.
    fn run_command(
        &mut self,
        command: Command,
        target: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let context = self.command_context(window, cx);
        // A target naming a loaded commit carries that commit's parent count.
        let parents = self.state.snapshot.as_ref().map_or(0, |s| {
            s.commits
                .iter()
                .find(|c| c.id == target)
                .map_or(0, |c| c.parents.len())
        });
        if let Some(reason) = command.availability_for_commit(&context, parents).reason() {
            self.state.notice = format!(
                "{} unavailable. {reason}",
                command.menu_label().trim_end_matches('…')
            );
            cx.notify();
            return;
        }
        self.close_references(window, cx);
        let op = match command {
            Command::Refresh => {
                self.refresh(&Refresh, window, cx);
                return;
            }
            Command::CreateBranch => {
                self.prompt_branch(None, window, cx);
                return;
            }
            Command::Checkout => Operation::Checkout,
            Command::Merge => Operation::Merge,
            Command::CherryPick => Operation::CherryPick,
            Command::Revert => Operation::Revert,
            Command::Stash => Operation::Stash,
            Command::PopStash => Operation::PopStash,
            Command::Undo => Operation::Undo,
            Command::Redo => Operation::Redo,
            Command::Fetch => {
                if self.state.busy && !self.state.blocking_write() {
                    self.after_writes = Some(AfterWrites::Fetch);
                } else {
                    self.mutate(Action::Fetch, window, cx);
                }
                return;
            }
            Command::Pull => Operation::Pull {
                mode: gitcrab_core::git::PullMode::default(),
            },
            Command::Push => Operation::Push,
        };
        if op.takes_revision() && target.trim().is_empty() {
            self.state.notice = format!(
                "{} needs a branch or commit. Choose one in the history or the sidebar.",
                op.label()
            );
            cx.notify();
            return;
        }
        self.run_operation(op, target, window, cx);
    }

    /// Pulls with a mode chosen from the Pull menu, after the same checks as
    /// the toolbar button.
    fn run_pull(
        &mut self,
        mode: gitcrab_core::git::PullMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let context = self.command_context(window, cx);
        if let Some(reason) = Command::Pull.availability(&context).reason() {
            self.state.notice = format!("Pull unavailable. {reason}");
            cx.notify();
            return;
        }
        self.close_references(window, cx);
        self.run_operation(Operation::Pull { mode }, String::new(), window, cx);
    }

    /// Stops the running Fetch, Pull or Push and its helper processes.
    fn cancel_network(&mut self, cx: &mut Context<Self>) {
        if let Some(running) = self.running.as_mut().filter(|running| running.network) {
            running.cancelling = true;
            self.service.cancel_network();
            cx.notify();
        }
    }

    /// The caret beside Pull: Fetch, then each pull mode with the toolbar
    /// default checked. Disabled entries keep their reason in the tooltip.
    fn pull_menu(
        &self,
        commands: &CommandContext,
        ui_scale: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gitcrab_core::git::PullMode;
        let fetch = Command::Fetch.availability(commands).enabled();
        let pull = Command::Pull.availability(commands).enabled();
        let weak = cx.weak_entity();
        Button::new("toolbar-pull-menu")
            .ghost()
            .tab_stop(true)
            .cursor_pointer()
            .h(scaled(42.))
            .w(px(12. * ui_scale.max(0.5)))
            .px_0()
            .child(icon("down").size(px(10.)))
            .tooltip(format!(
                "Fetch and pull options · {}",
                self.command_tooltip(Command::Fetch, commands)
            ))
            .dropdown_menu(move |menu, _, _| {
                let fetch_view = weak.clone();
                let mut menu = menu.item(components::menu_item("Fetch", fetch).on_click(
                    move |_, w, cx| {
                        let _ = fetch_view.update(cx, |v, cx| {
                            v.run_command(Command::Fetch, String::new(), w, cx)
                        });
                    },
                ));
                menu = menu.separator();
                for mode in PullMode::ALL {
                    let view = weak.clone();
                    menu = menu.item(
                        components::menu_item(mode.label(), pull)
                            .checked(mode == PullMode::default())
                            .on_click(move |_, w, cx| {
                                let _ = view.update(cx, |v, cx| v.run_pull(mode, w, cx));
                            }),
                    );
                }
                menu
            })
    }

    /// When this worktree last fetched, for the status bar.
    fn last_fetch_label(&self) -> Option<String> {
        let fetched = self.state.snapshot.as_ref()?.remote.last_fetch?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        let ago = now.saturating_sub(fetched);
        Some(match ago {
            0..60 => "Fetched just now".into(),
            60..3600 => format!("Fetched {} min ago", ago / 60),
            3600..86400 => format!("Fetched {} h ago", ago / 3600),
            _ => format!("Fetched {} d ago", ago / 86400),
        })
    }

    /// Selects a commit, loading more history first when it is outside the loaded page.
    fn seek_commit(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        match self.state.seek_commit(&id) {
            Seek::Selected(job) => {
                self.service.cancel_read(ReadKind::Details);
                self.service.cancel_read(ReadKind::Diff);
                self.reveal_selection(Some(&id), window, cx);
                self.dispatch(job, window, cx);
            }
            Seek::Loading(plan) => self.apply_refresh(plan, window, cx),
            Seek::NotFound => cx.notify(),
        }
    }

    /// Selects a commit the user activated, showing its details in a single-pane layout.
    fn activate_commit(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.single_pane {
            self.constrained_inspector = true;
        }
        self.select(Some(id), window, cx);
    }
    /// Opens the tab's Activity list. It reads the live records each frame,
    /// so writes that finish while it is open appear immediately.
    fn show_activity(&self, window: &mut Window, cx: &mut Context<Self>) {
        let repo = display_path(&self.state.path);
        let weak = cx.weak_entity();
        let scroll = self.activity_scroll.clone();
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let records = weak
                .read_with(cx, |v, _| v.activity.clone())
                .unwrap_or_default();
            let all = records
                .iter()
                .map(OperationRecord::summary)
                .collect::<Vec<_>>()
                .join("\n");
            dialog
                .title("Activity · this tab")
                .footer(components::dialog_footer([components::dialog_ok(
                    "OK",
                    components::ButtonKind::Primary,
                    cx,
                )]))
                .p_4()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_role(Role::TextSecondary)
                                .child(repo.clone()),
                        )
                        .child(
                            components::sized(
                                Button::new("activity-copy-all").label("Copy all").ghost(),
                                ControlSize::Compact,
                            )
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(all.clone()))
                            }),
                        ),
                )
                .child(scroll_area(
                    "activity-records",
                    &scroll,
                    div().id("activity-records").max_h(px(360.)).children(
                        records.iter().enumerate().map(|(i, r)| {
                            let copy = r.summary();
                            div()
                                .py_2()
                                .flex()
                                .gap_2()
                                .border_b_1()
                                .border_role(Role::BorderDivider)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .flex()
                                                .flex_wrap()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child(r.action.clone()),
                                                )
                                                .when_some(r.target.clone(), |row, target| {
                                                    row.child(target)
                                                })
                                                .child(
                                                    div()
                                                        .text_role(if r.failure.is_some() {
                                                            Role::StatusErrorFg
                                                        } else {
                                                            Role::StatusSuccessFg
                                                        })
                                                        .child(r.status()),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .text_size(scaled(12.))
                                                .text_role(Role::TextMuted)
                                                .child(gitcrab_core::presentation::elapsed_label(
                                                    r.at.elapsed().as_secs(),
                                                )),
                                        )
                                        .child(
                                            div()
                                                .text_size(scaled(12.))
                                                .text_role(Role::TextSecondary)
                                                .child(r.details.clone()),
                                        ),
                                )
                                .child(
                                    components::sized(
                                        Button::new(("activity-copy", i))
                                            .icon(icon("copy"))
                                            .ghost(),
                                        ControlSize::Compact,
                                    )
                                    .tooltip("Copy this record")
                                    .on_click(
                                        move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                copy.clone(),
                                            ))
                                        },
                                    ),
                                )
                        }),
                    ),
                ))
        });
    }
    fn can_commit(&self) -> bool {
        !self.settings_open
            && !self.state.blocking_write()
            && self
                .state
                .draft
                .lines()
                .next()
                .is_some_and(|s| !s.trim().is_empty())
            && self
                .state
                .snapshot
                .as_ref()
                .is_some_and(|s| s.commit_blocker().is_none())
    }
    fn commit_staged(&mut self, _: &CommitStaged, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open || window.has_active_dialog(cx) {
            return;
        }
        let composing = [&self.draft, &self.wip_summary]
            .into_iter()
            .any(|input| input.update(cx, |s, cx| s.marked_text_range(window, cx).is_some()))
            || self
                .description
                .update(cx, |s, cx| s.marked_text_range(window, cx).is_some());
        if self.can_commit() && !composing && self.state.busy {
            // Staging is still being written: commit once it lands, against
            // the index it leaves.
            self.after_writes = Some(AfterWrites::Commit);
            cx.notify();
        } else if self.can_commit() && !composing {
            self.mutate(
                Action::Commit {
                    message: self.state.draft.clone(),
                    reviewed: self
                        .state
                        .snapshot
                        .as_ref()
                        .and_then(|s| s.reviewed.clone()),
                },
                window,
                cx,
            );
        }
    }
    fn back(&mut self, _: &Back, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            self.finish_settings(window, cx);
            window.close_dialog(cx);
            return;
        }
        if self.branch_prompt_open() {
            self.close_branch_prompt(window, cx);
            return;
        }
        if window.has_active_dialog(cx) {
            window.close_all_dialogs(cx);
            self.references_open = false;
            cx.notify();
            return;
        }
        // Escape closes Find in diff before leaving the diff.
        if self.diff_find_open {
            self.close_diff_find(window, cx);
            return;
        }
        // Escape in the search field clears the query. Navigating back keeps it.
        if self.search.read(cx).focus_handle(cx).is_focused(window)
            && !self.search.read(cx).value().is_empty()
        {
            self.search.update(cx, |s, cx| s.set_value("", window, cx));
            self.update_search(cx);
            cx.notify();
            return;
        }
        if self.single_pane && self.constrained_inspector {
            self.constrained_inspector = false;
            self.focus.focus(window, cx);
            cx.notify();
            return;
        }
        self.state.close_diff();
        self.focus.focus(window, cx);
        cx.notify();
    }
    fn move_selection(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open || window.has_focused_input(cx) || self.state.diff_context.is_some() {
            return;
        }
        if let Some(s) = &self.state.snapshot {
            let current = self
                .state
                .selected
                .as_ref()
                .and_then(|id| s.commits.iter().position(|c| &c.id == id));
            let next = (current.map(|i| i as isize).unwrap_or(-1) + delta)
                .clamp(-1, s.commits.len() as isize - 1);
            let id = (next >= 0).then(|| s.commits[next as usize].id.clone());
            self.history_scroll
                .scroll_to_item((next + 1) as usize, ScrollStrategy::Top);
            self.select(id, window, cx);
        }
    }
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            return;
        }
        if self.state.patch.is_some() {
            self.copy_diff(false, cx);
        } else if self.state.diff_context.is_some() {
            self.state.notice = "Nothing to copy. The clipboard is unchanged.".into();
            cx.notify();
        } else if let Some(id) = &self.state.selected {
            cx.write_to_clipboard(ClipboardItem::new_string(id.clone()));
        }
    }

    /// Copies selected diff rows as source text from the focused side, or as
    /// a patch with markers. Leaves the clipboard alone when nothing applies.
    fn copy_diff(&mut self, patch_text: bool, cx: &mut Context<Self>) {
        let Some(patch) = &self.state.patch else {
            return;
        };
        let Some(range) = self.selection.clone() else {
            self.state.notice =
                "Select diff rows to copy. Nothing was copied; the clipboard is unchanged.".into();
            cx.notify();
            return;
        };
        let split = self.split_active.then_some(patch.content.split.as_slice());
        let text = if patch_text {
            gitcrab_core::diff::copy_patch(&patch.content.lines, split, range.clone())
        } else {
            gitcrab_core::diff::copy_source(
                &patch.content.lines,
                split.map(|rows| (rows, self.copy_side)),
                range.clone(),
            )
        };
        let rows = range.len();
        let side = match (split.is_some(), self.copy_side) {
            (true, gitcrab_core::diff::Side::Before) => " from Before",
            (true, gitcrab_core::diff::Side::After) => " from After",
            _ => "",
        };
        self.state.notice = if text.is_empty() {
            format!("The selected rows have no source text{side}. The clipboard is unchanged.")
        } else {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            format!(
                "Copied {rows} {} {}{side}",
                if rows == 1 { "row" } else { "rows" },
                if patch_text {
                    "as a patch"
                } else {
                    "of source"
                }
            )
        };
        cx.notify();
    }

    fn stage_selected_file(
        &mut self,
        _: &StageSelectedFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_open || window.has_active_dialog(cx) {
            return;
        }
        let Some(patch) = self.state.patch.as_ref().filter(|patch| patch.id.is_none()) else {
            return;
        };
        let Some(reviewed) = self
            .state
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .status
                    .iter()
                    .find(|file| file.path == patch.path && file.on_side(patch.staged))
            })
            .cloned()
        else {
            return;
        };
        self.mutate(
            Action::StageFile {
                reviewed,
                unstage: patch.staged,
            },
            window,
            cx,
        );
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            return;
        }
        if let Some(p) = &self.state.patch {
            self.selection = Some(
                0..if self.split_active {
                    p.content.split.len()
                } else {
                    p.content.lines.len()
                },
            );
            cx.notify();
        }
    }
    fn button(&self, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
        components::basic(
            Button::new(id).label(label),
            components::ButtonKind::Ghost,
            ControlSize::Compact,
        )
        .tab_stop(!self.settings_open)
    }
    fn action_button(&self, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
        components::basic(
            Button::new(id).label(label),
            components::ButtonKind::Secondary,
            ControlSize::Compact,
        )
        .tab_stop(!self.settings_open)
    }
    /// A button of a staging role (stage, unstage, discard) at its standard size.
    fn semantic_button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        kind: components::ButtonKind,
        size: ControlSize,
        cx: &App,
    ) -> Button {
        components::style(Button::new(id).label(label), kind, size, cx)
            .tab_stop(!self.settings_open)
    }
    fn primary_button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        disabled: bool,
    ) -> Button {
        // An unavailable primary action drops the accent fill so it reads as disabled.
        components::basic(
            Button::new(id).label(label),
            components::ButtonKind::Primary,
            ControlSize::Regular,
        )
        .tab_stop(!self.settings_open && !disabled)
        .disabled_when(disabled)
    }

    fn segment_button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        selected: bool,
    ) -> Button {
        self.button(id, label)
            .selected(selected)
            .when(selected, |button| {
                button
                    .bg_role(Role::InteractionSelected)
                    .text_role(Role::TextDefault)
            })
    }

    fn icon_button(
        &self,
        id: impl Into<ElementId>,
        glyph: &str,
        name: impl Into<SharedString>,
    ) -> Button {
        self.button(id, "")
            .icon(icon(glyph).size(scaled(GLYPH_SIZE)))
            .size(scaled(CONTROL_HEIGHT))
            .flex_none()
            .tooltip(name)
    }
    fn editor(
        &self,
        input: &Entity<InputState>,
        height: f32,
        width: f32,
        placeholder: &str,
        cx: &App,
    ) -> AnyElement {
        if self.settings_open {
            Self::editor_preview(input.read(cx).value().as_ref(), height, width, placeholder)
        } else {
            Input::new(input)
                .w(px(width))
                .h(px(height))
                .into_any_element()
        }
    }
    /// A static stand-in for an editor while the settings dialog is open.
    fn editor_preview(value: &str, height: f32, width: f32, placeholder: &str) -> AnyElement {
        div()
            .w(px(width))
            .h(px(height))
            .px_2()
            .rounded(px(6.))
            .bg_role(Role::SurfaceChrome)
            .overflow_hidden()
            .text_role(Role::TextMuted)
            .child(if value.is_empty() {
                placeholder.to_string()
            } else {
                value.to_string()
            })
            .into_any_element()
    }
    fn highlight_sidebar_row(button: Button, highlighted: bool) -> Button {
        button.selected(highlighted).when(highlighted, |button| {
            button
                .bg_role(Role::ActionAccentTintHover)
                .text_role(Role::TextDefault)
        })
    }

    fn new_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let loading = self.state.loading();
        let recent = crate::preferences::current(cx).recent;
        let notice = self
            .open_failure
            .as_ref()
            .map(|failure| (failure.path.as_str(), failure.details.as_str()))
            .or_else(|| self.state.error.then_some(("", self.state.notice.as_str())));
        panel()
            .relative()
            .track_focus(&self.focus)
            .bg_role(Role::SurfaceCanvas)
            .text_role(Role::TextDefault)
            .text_size(px(self.preferences.font_size))
            .items_center()
            .justify_center()
            .gap_3()
            .p_4()
            .child(super::assets::logo(!Theme::global(cx).is_dark()).size(px(52.)))
            .child(
                div()
                    .text_size(scaled(22.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if loading {
                        "Opening repository…"
                    } else {
                        "Open a repository"
                    }),
            )
            .when(!loading, |el| {
                el.child(
                    div().text_role(Role::TextSecondary).child(
                        "Choose a local Git repository to browse history and review changes.",
                    ),
                )
                .child(
                    Button::new("browse-repository")
                        .primary()
                        .tab_stop(true)
                        .icon(icon("folder"))
                        .label("Browse…")
                        .set_disabled(self.settings_open)
                        .on_click(|_, w, cx| w.dispatch_action(Box::new(Open), cx)),
                )
                .when(!recent.is_empty(), |el| {
                    el.child(
                        div()
                            .w_full()
                            .max_w(px(360.))
                            .mt_3()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_role(Role::TextMuted)
                                    .child("Recent repositories"),
                            )
                            .child(scroll_area(
                                "new-tab-recent-list",
                                &self.recent_scroll,
                                div()
                                    .id("new-tab-recent-list")
                                    .max_h(scaled(180.))
                                    .children(
                                        path_labels(&recent)
                                            .into_iter()
                                            .zip(recent.clone())
                                            .enumerate()
                                            .map(|(index, (label, path))| {
                                                self.button(("new-tab-recent", index), "")
                                                    .icon(icon("history"))
                                                    .w_full()
                                                    .h(scaled(40.))
                                                    .justify_start()
                                                    .overflow_hidden()
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .flex()
                                                            .flex_col()
                                                            // Not `truncate()`: gpui keeps the first
                                                            // measure of no-wrap text, taken here before
                                                            // the width is known, so it would clip without
                                                            // an ellipsis. A one-line clamp re-measures.
                                                            .child(
                                                                div()
                                                                    .w_full()
                                                                    .overflow_hidden()
                                                                    .text_ellipsis()
                                                                    .line_clamp(1)
                                                                    .child(label.suffix),
                                                            )
                                                            .child(
                                                                div()
                                                                    .w_full()
                                                                    .overflow_hidden()
                                                                    .text_ellipsis()
                                                                    .line_clamp(1)
                                                                    .text_size(scaled(11.))
                                                                    .text_role(Role::TextMuted)
                                                                    .child(label.location),
                                                            ),
                                                    )
                                                    .tooltip(display_path(&path))
                                                    .tab_stop(true)
                                                    .set_disabled(self.settings_open)
                                                    .on_click(move |_, w, cx| {
                                                        w.dispatch_action(
                                                            Box::new(super::workspace::OpenRecent(
                                                                path.clone(),
                                                            )),
                                                            cx,
                                                        );
                                                    })
                                            }),
                                    ),
                            ))
                            .child(
                                div()
                                    .pt_1()
                                    .text_size(scaled(11.))
                                    .text_role(Role::TextMuted)
                                    .child(SESSION_NOTE),
                            ),
                    )
                })
            })
            .child(
                div().absolute().left(px(16.)).bottom(px(16.)).child(
                    self.button("new-tab-appearance", "Appearance")
                        .icon(icon("settings"))
                        .on_click(cx.listener(|v, _, w, cx| {
                            v.open_settings(w, cx);
                        })),
                ),
            )
            .when_some(notice, |el, (path, details)| {
                let name = std::path::Path::new(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Repository".into());
                let copy = if path.is_empty() {
                    details.to_owned()
                } else {
                    format!("{path}\n{details}")
                };
                el.child(
                    div()
                        .absolute()
                        .top(px(16.))
                        .right(px(16.))
                        .w(px(380.))
                        .p_3()
                        .rounded(px(8.))
                        .bg_role(Role::SurfacePanel)
                        .border_1()
                        .border_role(Role::BorderDivider)
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .size(px(6.))
                                        .rounded_full()
                                        .bg_role(Role::StatusWarningFg),
                                )
                                .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(
                                    if !path.is_empty() {
                                        "Couldn’t open repository".to_owned()
                                    } else if self.error_heading.is_empty() {
                                        "Something went wrong".to_owned()
                                    } else {
                                        self.error_heading.clone()
                                    },
                                ))
                                .child(
                                    self.icon_button(
                                        "copy-open-error",
                                        "copy",
                                        "Copy error details",
                                    )
                                    .tab_stop(true)
                                    .on_click(
                                        move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                copy.clone(),
                                            ));
                                        },
                                    ),
                                )
                                .child(
                                    self.button("dismiss-open-error", "×")
                                        .tab_stop(true)
                                        .tooltip("Dismiss error")
                                        .on_click(cx.listener(|v, _, _, cx| {
                                            v.open_failure = None;
                                            v.state.error = false;
                                            v.state.notice.clear();
                                            cx.notify();
                                        })),
                                ),
                        )
                        .when(!path.is_empty(), |toast| {
                            toast.child(
                                div()
                                    .overflow_hidden()
                                    .truncate()
                                    .text_size(scaled(12.))
                                    .child(name),
                            )
                        })
                        .child(
                            scroll_area(
                                "open-failure-details",
                                &self.failure_scroll,
                                div()
                                    .id("open-failure-details")
                                    .max_h(px(80.))
                                    .text_size(scaled(12.))
                                    .text_role(Role::TextSecondary)
                                    .child(details.to_owned()),
                            )
                            .mt_1(),
                        ),
                )
            })
            .into_any_element()
    }
}
impl Render for GitCrab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(feature = "native-evidence")]
        self.render_evidence(window, cx);
        if self.state.snapshot.is_none() {
            return div()
                .relative()
                .size_full()
                .child(self.new_tab(cx))
                .into_any_element();
        }
        let title = self
            .state
            .path
            .rsplit(['/', '\\'])
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("Repository")
            .to_string();
        let in_diff = self.state.diff_context.is_some();
        if !in_diff && self.diff_find_open {
            self.diff_find_open = false;
            self.diff_matches.clear();
            self.diff_match = None;
        }
        let ui_scale = self.preferences.font_size / 13.;
        let single_pane = f32::from(window.viewport_size().width) < 648. * ui_scale + 44.;
        // Below this width the sidebar folds into an icon rail. 1000 keeps the
        // full sidebar at typical half-screen windows while
        // still leaving the history about 400 px next to the inspector.
        let rail = in_diff || f32::from(window.viewport_size().width) < 1000. * ui_scale;
        let viewport_width = f32::from(window.viewport_size().width);
        self.single_pane = single_pane;
        // The inspector can be hidden while reviewing a diff to give it room.
        let collapsed = in_diff && self.inspector_collapsed && !single_pane;
        self.history_available = if single_pane {
            viewport_width
        } else if collapsed {
            viewport_width - 44. - SEAM
        } else {
            let inspector = match self.actual_inspector_width[usize::from(rail)] {
                width if width > 0. => width,
                _ => self.preferences.inspector_width,
            };
            viewport_width
                - if rail {
                    44.
                } else {
                    self.preferences.nav_width
                }
                - inspector
                - 2. * SEAM
        };
        // Split shows both sides only when each can show its lines; otherwise
        // the diff falls back to Unified and says so.
        let code_scale = self.preferences.code_size / 12.;
        let split_active = self.split
            && self.state.patch.as_ref().is_some_and(|patch| {
                patch.preview.has_layout()
                    && gitcrab_core::presentation::split_fits(
                        self.history_available,
                        patch.content.columns,
                        self.preferences.code_size * 0.65,
                        54. * code_scale + 32.,
                    )
            });
        if split_active != self.split_active {
            self.split_active = split_active;
            self.selection = None;
            self.update_diff_find(false, cx);
        }
        self.update_split_wrap(window);
        let sidebar = if rail {
            div()
                .flex()
                .flex_col()
                .items_center()
                .size_full()
                .bg_role(Role::SurfaceChrome)
                .gap_2()
                .when(in_diff, |rail| {
                    rail.child(
                        self.icon_button(
                            "rail-back",
                            "back",
                            if self.state.selected.is_some() {
                                "Close the diff and return to history · Escape"
                            } else {
                                "Close the diff and return to working changes in history · Escape"
                            },
                        )
                        .on_click(cx.listener(|v, _, w, cx| v.leave_diff(w, cx))),
                    )
                })
                .child(
                    self.icon_button("rail-references", "branch", "References…")
                        .on_click(cx.listener(|v, _, w, cx| v.show_references(w, cx))),
                )
                .child(
                    self.icon_button("rail-working", "changes", "Working changes")
                        .map(|button| {
                            Self::highlight_sidebar_row(button, self.state.selected.is_none())
                        })
                        .on_click(cx.listener(|v, _, w, cx| v.select(None, w, cx))),
                )
                .into_any_element()
        } else {
            self.sidebar(window, cx)
        };
        let inspector = self.inspector(window, cx);
        let center = if self.state.patch.is_some() {
            self.diff(cx)
        } else if in_diff {
            self.review_placeholder(cx)
        } else {
            self.history(window, cx)
        };
        let recent_paths = crate::preferences::current(cx)
            .recent
            .iter()
            .filter(|path| !gitcrab_core::presentation::same_path(path, &self.state.path))
            .cloned()
            .collect::<Vec<_>>();
        let recent = recent_paths
            .iter()
            .cloned()
            .zip(path_labels(&recent_paths))
            .collect::<Vec<_>>();
        let commands = self.command_context(window, cx);
        let searching = !self.search.read(cx).value().is_empty();
        let loaded = self.state.snapshot.as_ref().map_or(0, |s| s.commits.len());
        let more_history = self.state.more_history();
        let search_status = searching.then(|| {
            gitcrab_core::presentation::search_status(
                &self.search_matches,
                self.state.selected.as_deref(),
                loaded,
                more_history,
            )
        });
        let load_more_offered = searching && self.search_matches.is_empty() && more_history;
        // Room taken by controls beside the selectors and search: the
        // Appearance button, match navigation while searching and, in the
        // single-pane layout, References. Single-pane widths never centre the
        // actions, so References can be counted on this side too.
        let icon_slot = CONTROL_HEIGHT * ui_scale + 4.;
        let toolbar_extra = icon_slot
            + search_status.as_ref().map_or(0., |status| {
                status.chars().count() as f32 * 6.5 * ui_scale + 4. + 2. * icon_slot
            })
            + if load_more_offered {
                150. * ui_scale
            } else {
                0.
            }
            + if single_pane { icon_slot } else { 0. };
        let layout = gitcrab_core::presentation::toolbar_layout(
            f32::from(window.viewport_size().width),
            ui_scale,
            Command::PRIMARY.len(),
            toolbar_extra,
        );
        let selector_width = px(layout.selector_width);
        let selector_height = type_size(cx, 42.);
        let labelled = layout.labels();
        // Undo, Redo, Pull, Push, Branch, Stash, Pop. Each stays visible and
        // is disabled with its reason when it cannot run.
        let loader = self.loader();
        let action_buttons = Command::PRIMARY
            .into_iter()
            .enumerate()
            .map(|(i, command)| {
                let button = components::toolbar_command(
                    ("toolbar", i),
                    command.label(),
                    command_glyph(command),
                    !command.availability(&commands).enabled(),
                    loader == Some(Some(command)),
                    labelled,
                    false,
                )
                .tooltip(self.command_tooltip(command, &commands))
                .on_click(
                    cx.listener(move |v, _, w, cx| v.run_command(command, String::new(), w, cx)),
                );
                if command != Command::Pull {
                    return button.into_any_element();
                }
                // Pull is a split button: the main part pulls in the default
                // mode, the caret offers Fetch and every pull mode.
                div()
                    .flex()
                    .items_center()
                    .child(button)
                    .child(self.pull_menu(&commands, ui_scale, cx))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let actions = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .children(action_buttons);
        let (inline_actions, placed_actions) = match layout.actions {
            ToolbarActions::Inline | ToolbarActions::Compact => (Some(actions), None),
            ToolbarActions::Centered { left, width } => (
                None,
                Some(
                    // Centred on the whole toolbar. Only the buttons take
                    // pointer input, so the box does not block the controls
                    // beside it.
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(left))
                        .w(px(width))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(actions),
                ),
            ),
            ToolbarActions::Wrapped => (
                None,
                Some(div().w_full().flex().justify_center().child(actions)),
            ),
        };

        let menu_focus = self.focus.clone();
        let mut toolbar = div()
            .relative()
            .min_h(scaled(TOOLBAR_HEIGHT))
            .bg_role(Role::SurfaceChrome)
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_3()
            .py_1()
            .border_b_1()
            .border_role(Role::BorderDivider)
            // The single-pane layout has no sidebar or rail, so References
            // opens from here.
            .when(single_pane, |bar| {
                bar.child(
                    self.icon_button("toolbar-references", "branch", "References…")
                        .on_click(cx.listener(|v, _, w, cx| v.show_references(w, cx))),
                )
            })
            .child({
                // Index 0 is "Browse", the rest follow `recent`.
                let choices = recent
                    .iter()
                    .map(|(path, _)| path.clone())
                    .collect::<Vec<_>>();
                let action_focus = menu_focus.clone();
                let entries = std::iter::once(
                    PickerItem::new("Browse for a repository…")
                        .icon("folder")
                        .note("Ctrl/Cmd+O")
                        .into(),
                )
                .chain((!recent.is_empty()).then(|| {
                    PickerEntry::Heading(
                        "Recent · opens in a new tab or switches to its tab".into(),
                    )
                }))
                .chain(recent.iter().map(|(path, label)| {
                    PickerItem::new(label.suffix.clone())
                        .icon("history")
                        .detail(label.location.clone())
                        .tooltip(display_path(path))
                        .into()
                }));
                Picker::new(
                    "repository-picker",
                    components::selector(
                        "repository-selector",
                        Some("Open repository"),
                        title.clone(),
                        "folder",
                        selector_width,
                        selector_height,
                    )
                    .tooltip(format!(
                        "{} · Open another repository in a tab · Ctrl/Cmd+O",
                        display_path(&self.state.path)
                    ))
                    .set_disabled(self.state.blocking_write()),
                    selector_width.max(px(320.)),
                    &menu_focus,
                    // Dispatch from this view's own focus node, as the old menu's
                    // action context did, so it does not depend on where focus
                    // happens to be once the list has closed.
                    move |index, window, cx| match index.checked_sub(1) {
                        None => action_focus.dispatch_action(&Open, window, cx),
                        Some(recent) => {
                            if let Some(path) = choices.get(recent) {
                                action_focus.dispatch_action(
                                    &super::workspace::OpenRecent(path.clone()),
                                    window,
                                    cx,
                                );
                            }
                        }
                    },
                )
                .entries(entries)
                .build(window, cx)
            })
            .child(separator())
            .child(self.branch_picker(selector_width, selector_height, &commands, window, cx))
            .when_some(inline_actions, |bar, actions| {
                bar.child(separator()).child(actions)
            });
        toolbar = toolbar
            .child(div().flex_1())
            .child(
                div()
                    .flex_none()
                    .w(px(layout.search_width))
                    .child(self.editor(
                        &self.search,
                        32.,
                        layout.search_width,
                        "Search history…",
                        cx,
                    )),
            )
            .when_some(search_status, |bar, status| {
                bar.child(
                    div()
                        .text_size(scaled(12.))
                        .text_role(Role::TextMuted)
                        .child(status),
                )
                .child(
                    self.icon_button(
                        "search-prev",
                        "up",
                        "Previous match · Shift+Enter or Shift+F3",
                    )
                    .set_disabled(self.search_matches.is_empty())
                    .on_click(cx.listener(|v, _, w, cx| v.search_step(-1, w, cx))),
                )
                .child(
                    self.icon_button("search-next", "down", "Next match · Enter or F3")
                        .set_disabled(self.search_matches.is_empty())
                        .on_click(cx.listener(|v, _, w, cx| v.search_step(1, w, cx))),
                )
                .when(load_more_offered, |bar| {
                    bar.child(
                        self.action_button("search-load-more", "Load more history")
                            .set_disabled(self.state.loading())
                            .tooltip("Load 500 more commits and search them")
                            .on_click(cx.listener(|v, _, w, cx| {
                                let plan = v.state.load_more();
                                v.apply_refresh(plan, w, cx);
                            })),
                    )
                })
            })
            .child(
                self.icon_button("toolbar-appearance", "settings", "Appearance…")
                    .set_disabled(self.settings_open)
                    .on_click(cx.listener(|v, _, w, cx| v.open_settings(w, cx))),
            )
            .when_some(placed_actions, |bar, actions| bar.child(actions));
        let main_content = if single_pane {
            div()
                .flex_1()
                .min_h_0()
                .child(if self.constrained_inspector {
                    inspector
                } else {
                    center
                })
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .bg_role(Role::BorderDivider)
                .child(
                    h_resizable(SharedString::from(format!(
                        "workspace-{rail}-{collapsed}-{}",
                        self.layout_revision
                    )))
                    .with_state(&self.panels[if collapsed { 2 } else { usize::from(rail) }])
                    .on_resize(cx.listener(
                        move |v,
                              state: &Entity<gpui::component::resizable::ResizableState>,
                              w,
                              cx| {
                            let sizes = state.read(cx).sizes();
                            if sizes.len() == 3 {
                                if !rail {
                                    v.preferences.nav_width = f32::from(sizes[0]).clamp(150., 330.);
                                }
                                v.preferences.inspector_width =
                                    f32::from(sizes[2]).clamp(260., 560.);
                            }
                            v.persist_preferences(w, cx);
                            cx.notify();
                        },
                    ))
                    .child(
                        resizable_panel()
                            .size(px(if rail {
                                44.
                            } else {
                                self.preferences.nav_width
                            }))
                            .size_range(if rail {
                                px(44.)..px(45.)
                            } else {
                                px(150.)..px(330.)
                            })
                            .child(sidebar),
                    )
                    .child(resizable_panel().child(div().size_full().pl(px(SEAM)).child(center)))
                    .when(!collapsed, |panels| {
                        panels.child(
                            resizable_panel()
                                .size(px(self.preferences.inspector_width))
                                .size_range(px(260.)..px(560.))
                                .child(div().size_full().pl(px(SEAM)).child(inspector)),
                        )
                    }),
                )
                .into_any_element()
        };
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg_role(Role::SurfaceCanvas)
            .text_role(Role::TextDefault)
            .text_size(px(self.preferences.font_size))
            .track_focus(&self.focus)
            .key_context("GitCrab")
            .on_action(cx.listener(|v, _: &Refresh, w, cx| {
                v.run_command(Command::Refresh, String::new(), w, cx)
            }))
            .on_action(cx.listener(Self::back))
            .on_action(cx.listener(|v, _: &Next, w, cx| v.move_selection(1, w, cx)))
            .on_action(cx.listener(|v, _: &Previous, w, cx| v.move_selection(-1, w, cx)))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(|v, _: &CopyPatch, _, cx| {
                if !v.settings_open {
                    v.copy_diff(true, cx);
                }
            }))
            .on_action(cx.listener(Self::stage_selected_file))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(|v, _: &SplitBefore, w, cx| v.keyboard_pan_diff(false, w, cx)))
            .on_action(cx.listener(|v, _: &SplitAfter, w, cx| v.keyboard_pan_diff(true, w, cx)))
            .on_action(cx.listener(Self::commit_staged))
            .on_action(
                cx.listener(|v, _: &UndoLast, w, cx| v.journal_shortcut(Command::Undo, w, cx)),
            )
            .on_action(
                cx.listener(|v, _: &RedoLast, w, cx| v.journal_shortcut(Command::Redo, w, cx)),
            )
            .on_key_down(cx.listener(|v, event: &KeyDownEvent, w, cx| {
                if event.keystroke.key == "enter"
                    && event.keystroke.modifiers.shift
                    && v.search.read(cx).focus_handle(cx).is_focused(w)
                {
                    v.search_step(-1, w, cx);
                    cx.stop_propagation();
                } else if event.keystroke.key == "enter"
                    && event.keystroke.modifiers.shift
                    && v.diff_search.read(cx).focus_handle(cx).is_focused(w)
                {
                    v.diff_find_step(-1, w, cx);
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|v, _: &NextHunk, w, cx| v.step_hunk(true, w, cx)))
            .on_action(cx.listener(|v, _: &PreviousHunk, w, cx| v.step_hunk(false, w, cx)))
            .on_action(cx.listener(|v, _: &Find, w, cx| {
                if w.has_active_dialog(cx) || v.settings_open {
                    return;
                }
                if v.state.patch.is_some() {
                    // Find searches the displayed diff. Ctrl/Cmd+Shift+F still searches history.
                    v.open_diff_find(w, cx);
                } else if v.state.diff_context.is_some() {
                    v.state.notice =
                        "Find in diff is available once the changes have loaded.".into();
                    cx.notify();
                } else {
                    v.search.update(cx, |s, cx| s.focus(w, cx));
                }
            }))
            .on_action(cx.listener(|v, _: &FindInHistory, w, cx| {
                if !w.has_active_dialog(cx) && !v.settings_open {
                    v.search.update(cx, |s, cx| s.focus(w, cx));
                }
            }))
            .on_action(cx.listener(|v, _: &SearchNext, w, cx| {
                if v.diff_find_open {
                    v.diff_find_step(1, w, cx)
                } else {
                    v.search_step(1, w, cx)
                }
            }))
            .on_action(cx.listener(|v, _: &SearchPrevious, w, cx| {
                if v.diff_find_open {
                    v.diff_find_step(-1, w, cx)
                } else {
                    v.search_step(-1, w, cx)
                }
            }))
            .child(self.column_drag_events(cx))
            .child(toolbar)
            .when(single_pane, |el| {
                let center_label = if in_diff { "Diff" } else { "History" };
                let side_label = if self.state.selected.is_some() {
                    "Commit details"
                } else {
                    "Working changes"
                };
                el.child(
                    section_bar().h(scaled(36.)).child(
                        segmented_group()
                            .child(
                                self.segment_button(
                                    "compact-history",
                                    center_label,
                                    !self.constrained_inspector,
                                )
                                .on_click(cx.listener(
                                    |v, _, _, cx| {
                                        v.constrained_inspector = false;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                self.segment_button(
                                    "compact-inspector",
                                    side_label,
                                    self.constrained_inspector,
                                )
                                .on_click(cx.listener(
                                    |v, _, _, cx| {
                                        v.constrained_inspector = true;
                                        cx.notify();
                                    },
                                )),
                            ),
                    ),
                )
            })
            .when(self.state.error, |el| {
                let details = self.state.notice.clone();
                el.child(
                    div()
                        .id("error-notice")
                        .p_3()
                        .mx_2()
                        .mt_2()
                        .rounded(px(6.))
                        .bg_role(Role::SurfacePanel)
                        .border_1()
                        .border_role(Role::BorderDivider)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .size(px(6.))
                                        .rounded_full()
                                        .bg_role(Role::StatusErrorFg),
                                )
                                .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(
                                    if self.error_heading.is_empty() {
                                        "Something went wrong".to_owned()
                                    } else {
                                        self.error_heading.clone()
                                    },
                                ))
                                .child(
                                    self.icon_button("copy-error", "copy", "Copy error details")
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                details.clone(),
                                            ));
                                        }),
                                )
                                .child(self.button("dismiss-error", "Dismiss").on_click(
                                    cx.listener(|v, _, _, cx| {
                                        v.state.dismiss_notice();
                                        cx.notify();
                                    }),
                                )),
                        )
                        .child(
                            scroll_area(
                                "error-details",
                                &self.error_scroll,
                                div()
                                    .id("error-details")
                                    .max_h(px(240.))
                                    .text_size(scaled(12.))
                                    .text_role(Role::TextSecondary)
                                    .child(self.state.notice.clone()),
                            )
                            .mt_1(),
                        ),
                )
            })
            .children(self.render_branch_prompt(cx))
            .child(main_content)
            .child(
                div()
                    .h(scaled(28.))
                    .bg_role(Role::SurfaceChrome)
                    .border_t_1()
                    .border_role(Role::BorderDivider)
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .gap_2()
                    .text_size(scaled(12.))
                    .text_role(Role::TextMuted)
                    .when(matches!(self.loader(), Some(Some(_))), |bar| {
                        bar.child(div().flex_none().child(components::spinner(px(12.))))
                    })
                    .child(div().flex_1().truncate().child(if self.state.busy {
                        self.running_label()
                    } else if let Some(pending) = self.pending_label() {
                        pending
                    } else if self.state.loading() {
                        "Loading repository…".into()
                    } else if self.state.error {
                        String::new()
                    } else {
                        if self.state.notice.is_empty() {
                            self.activity
                                .front()
                                .map(OperationRecord::summary)
                                .unwrap_or_default()
                        } else {
                            self.state.notice.clone()
                        }
                    }))
                    .when(
                        self.running
                            .as_ref()
                            .is_some_and(|running| running.network && !running.cancelling),
                        |bar| {
                            bar.child(
                                self.button("cancel-network", "Cancel")
                                    .tooltip("Stop Git while it contacts the remote. A pull that has started updating the branch finishes first.")
                                    .on_click(cx.listener(|v, _, _, cx| v.cancel_network(cx))),
                            )
                        },
                    )
                    .when_some(self.last_fetch_label(), |bar, fetched| {
                        let tooltip = self
                            .state
                            .snapshot
                            .as_ref()
                            .map(|s| s.upstream_context.clone())
                            .unwrap_or_default();
                        bar.child(
                            div()
                                .id("last-fetch")
                                .flex_none()
                                .child(fetched)
                                .tooltip(move |w, cx| Tooltip::new(tooltip.clone()).build(w, cx)),
                        )
                    })
                    .child(concat!("v", env!("CARGO_PKG_VERSION")))
                    .when(!self.activity.is_empty(), |bar| {
                        bar.child(
                            self.button(
                                "activity-results",
                                format!("Activity ({})", self.activity.len()),
                            )
                            .on_click(cx.listener(|v, _, w, cx| v.show_activity(w, cx))),
                        )
                    })
                    .when_some(self.watch_warning.clone(), |bar, warning| {
                        bar.child(
                            self.button("watch-warning", "Live refresh unavailable")
                                .text_role(Role::StatusWarningFg)
                                .tooltip(warning),
                        )
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod graph_geometry_tests {
    use super::*;
    use gitcrab_core::history_columns::HistoryColumn;

    #[::core::prelude::v1::test]
    fn a_minimized_graph_uses_one_lane_and_separates_lanes_as_it_widens() {
        let origin = point(px(211.4), px(0.));
        let minimum = HistoryColumn::Graph.minimum_width();
        let narrow = Bounds::new(origin, size(px(minimum), px(30.)));
        let slightly_wider = Bounds::new(origin, size(px(minimum + 8.), px(30.)));
        for scale in [1., 1.25, 2.] {
            let collapsed = graph_lane_positions(narrow, 50, scale);
            let expanded = graph_lane_positions(slightly_wider, 50, scale);
            let first = collapsed(0);
            for lane in [1, 3, 49] {
                assert_eq!(collapsed(lane), first);
            }
            assert_eq!(expanded(0), first);
            assert!(expanded(1) > first);
            assert!(expanded(3) > expanded(2));
            assert_eq!(expanded(10), expanded(49));
            assert!(first + px(9.) < narrow.right());
        }
    }

    #[::core::prelude::v1::test]
    fn five_graph_lanes_have_equal_pixel_aligned_gaps_at_default_and_wide_widths() {
        let origin = point(px(211.4), px(0.));
        for scale in [1., 1.25, 1.5, 2.] {
            for width in [100., 180.] {
                let bounds = Bounds::new(origin, size(px(width), px(30.)));
                let x = graph_lane_positions(bounds, 5, scale);
                let gap = f32::from(x(1) - x(0)) * scale;
                assert!(gap > 0.);
                for lane in 1..5 {
                    let actual_gap = f32::from(x(lane) - x(lane - 1)) * scale;
                    assert!((actual_gap - gap).abs() < 0.001);
                    let edge = f32::from(x(lane) - graph_stroke(scale) / 2.) * scale;
                    assert!((edge - edge.round()).abs() < 0.001);
                }
                assert!(x(4) + px(9.) < bounds.right());
            }
        }
    }
}
