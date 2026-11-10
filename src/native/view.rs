mod history;
mod inspector;
mod review;
mod settings;
use super::components::{self, ControlSize};
use super::preferences::Preferences;
use super::theme::*;
use gitcrab_core::{
    controller::{Controller, Lane},
    git::ReferenceKind,
    service::{Action, Failure, Operation, Request, Response, Service},
};
use gpui::{prelude::*, *};
use gpui_component::{
    button::{Button, ButtonVariant, ButtonVariants},
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
    resizable::{h_resizable, resizable_panel},
    tooltip::Tooltip,
    Disableable, Selectable, Sizable, Theme, WindowExt,
};
use std::ops::Range;

fn compose_message(summary: &str, body: &str) -> String {
    if body.is_empty() {
        summary.to_string()
    } else {
        format!("{summary}\n\n{body}")
    }
}
fn split_message(message: &str) -> (&str, &str) {
    match message.split_once('\n') {
        Some((summary, body)) => (summary, body.strip_prefix('\n').unwrap_or(body)),
        None => (message, ""),
    }
}
fn display_date(date: &str) -> String {
    match (
        date.get(5..7),
        date.get(8..10),
        date.get(..4),
        date.get(11..16),
    ) {
        (Some(month), Some(day), Some(year), Some(time))
            if date.is_ascii() && date.get(4..5) == Some("-") && date.get(7..8) == Some("-") =>
        {
            format!("{day}/{month}/{year} @ {time}")
        }
        _ => date.to_string(),
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
        SearchNext,
        SearchPrevious
    ]
);
#[derive(Default)]
struct OperationFeedback {
    error: Option<String>,
    busy: bool,
}
#[derive(Clone)]
struct OperationRecord {
    id: String,
    summary: String,
    failed: bool,
}

struct OpenFailure {
    path: String,
    details: String,
}

pub struct GitCrab {
    state: Controller,
    active: bool,
    service: Service,
    focus: FocusHandle,
    draft: Entity<InputState>,
    wip_summary: Entity<InputState>,
    description: Entity<InputState>,
    search: Entity<InputState>,
    reference_search: Entity<InputState>,
    target: Entity<InputState>,
    operation: Option<Operation>,
    operation_feedback: Entity<OperationFeedback>,
    split: bool,
    compact: bool,
    show_whitespace: bool,
    active_hunk: usize,
    history_scroll: UniformListScrollHandle,
    history_horizontal_scroll: ScrollHandle,
    history_column_drag:
        Option<components::rail::RailDrag<gitcrab_core::history_columns::HistoryColumn>>,
    history_column_drag_start: Option<(gitcrab_core::history_columns::HistoryColumn, Pixels)>,
    history_column_resize: Option<(gitcrab_core::history_columns::HistoryColumn, Pixels, f32)>,
    diff_scroll: UniformListScrollHandle,
    selection: Option<Range<usize>>,
    anchor: usize,
    _subscriptions: Vec<Subscription>,
    preferences: Preferences,
    settings_open: bool,
    last_repository: Option<String>,
    open_failure: Option<OpenFailure>,
    activity: std::collections::VecDeque<OperationRecord>,
    unread_result: bool,
    constrained_inspector: bool,
    preference_save: Option<Task<()>>,
    saved_preferences: Preferences,
    layout_revision: u64,
    panels: [Entity<gpui_component::resizable::ResizableState>; 2],
    panel_subscriptions: Vec<Subscription>,
    actual_inspector_width: [f32; 2],
    watcher: Option<notify::RecommendedWatcher>,
    watch_epoch: u32,
    watch_warning: Option<String>,
    external_dirty: bool,
    search_matches: Vec<String>,
    author_images: std::collections::HashMap<String, std::path::PathBuf>,
    #[cfg(feature = "native-evidence")]
    evidence_phase: u8,
    #[cfg(feature = "native-evidence")]
    feedback_start: Option<(std::time::Instant, &'static str)>,
    #[cfg(feature = "native-evidence")]
    watch_start: Option<std::time::Instant>,
    #[cfg(feature = "native-evidence")]
    benchmark: (Option<std::time::Instant>, Vec<f64>),
}
impl GitCrab {
    pub fn new(
        path: Option<String>,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let draft = cx.new(|cx| InputState::new(window, cx).placeholder("Commit summary"));
        let wip_summary = cx.new(|cx| InputState::new(window, cx).placeholder("// WIP"));
        let description = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("Description (optional)")
        });
        let reference_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter references…"));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search loaded history…"));
        let target = cx
            .new(|cx| InputState::new(window, cx).placeholder("Branch, revision or stash message"));
        let operation_feedback = cx.new(|_| OperationFeedback::default());
        let subscriptions = vec![
            cx.subscribe(&reference_search, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&target, |v, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    v.operation_feedback.update(cx, |state, cx| {
                        state.error = None;
                        cx.notify();
                    });
                    cx.notify();
                }
            }),
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
                if let InputEvent::PressEnter { secondary: false } = event {
                    v.search_step(1, w, cx);
                } else if matches!(event, InputEvent::Change) {
                    v.update_search(cx);
                }
                cx.notify();
            }),
            cx.observe_window_activation(window, |v, w, cx| {
                if v.active && w.is_window_active() && !v.state.busy {
                    v.external_dirty = false;
                    v.refresh(&Refresh, w, cx);
                }
            }),
        ];
        let preferences = Preferences::current(cx);
        gpui_component::Theme::global_mut(cx).font_size = px(preferences.font_size);
        let mut v = Self {
            state: Controller::default(),
            active,
            service: Service::new(),
            focus: cx.focus_handle(),
            draft,
            wip_summary,
            description,
            search,
            reference_search,
            target,
            operation: None,
            operation_feedback,
            split: preferences.split,
            compact: preferences.compact,
            show_whitespace: false,
            active_hunk: 0,
            history_scroll: UniformListScrollHandle::new(),
            history_horizontal_scroll: ScrollHandle::new(),
            history_column_drag: None,
            history_column_drag_start: None,
            history_column_resize: None,
            diff_scroll: UniformListScrollHandle::new(),
            selection: None,
            anchor: 0,
            _subscriptions: subscriptions,
            saved_preferences: preferences.clone(),
            preferences,
            settings_open: false,
            last_repository: Preferences::last_repository(),
            open_failure: None,
            activity: Default::default(),
            unread_result: false,
            constrained_inspector: false,
            preference_save: None,
            layout_revision: 0,
            panels: [
                cx.new(|_| Default::default()),
                cx.new(|_| Default::default()),
            ],
            panel_subscriptions: vec![],
            actual_inspector_width: [0.; 2],
            watcher: None,
            watch_epoch: 0,
            watch_warning: None,
            external_dirty: false,
            search_matches: vec![],
            author_images: local_author_images(),
            #[cfg(feature = "native-evidence")]
            evidence_phase: 0,
            #[cfg(feature = "native-evidence")]
            feedback_start: None,
            #[cfg(feature = "native-evidence")]
            watch_start: None,
            #[cfg(feature = "native-evidence")]
            benchmark: (None, vec![]),
        };
        v.reset_panels(cx);
        if active {
            v.focus.focus(window);
        }
        if let Some(path) = path {
            let job = v.state.open(path);
            #[cfg(feature = "native-evidence")]
            let mut job = job;
            #[cfg(feature = "native-evidence")]
            if let Ok(limit) = std::env::var("GITCRAB_CAPTURE_LIMIT")
                .unwrap_or_default()
                .parse::<usize>()
            {
                v.state.limit = limit.clamp(1, 100_000);
                if let Some((
                    _,
                    Request {
                        action: Action::Open { limit, .. },
                        ..
                    },
                )) = &mut job
                {
                    *limit = v.state.limit;
                }
            }
            v.dispatch(job, window, cx);
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
        match self.service.submit(request) {
            Ok(rx) => {
                // Tokio oneshot is a runtime-independent Future. Only the dedicated std worker does Git I/O.
                cx.spawn_in(window, async move |view, cx| {
                    let response = rx.await.unwrap_or(Response {
                        epoch,
                        sequence,
                        elapsed_ms: 0.,
                        result: Err(Failure {
                            message: "Git worker closed".into(),
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
                    result: Err(Failure {
                        message,
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
        // gpui-component does not emit Change for provisional IME text. Capture
        // the focused native buffer before reconciling an asynchronous result.
        if [&self.wip_summary, &self.draft, &self.description]
            .into_iter()
            .any(|input| input.read(cx).focus_handle(cx).is_focused(window))
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
        if lane == Lane::Mutation && response.epoch == self.state.epoch {
            let (summary, failed) = match &response.result {
                Ok(gitcrab_core::service::Reply::Done { message, .. }) => (message.clone(), false),
                Err(failure) => (failure.message.clone(), true),
                _ => ("Operation result requires inspection".into(), true),
            };
            self.activity.push_front(OperationRecord {
                id: format!(
                    "{} · {}:{}",
                    self.state.path, response.epoch, response.sequence
                ),
                summary,
                failed,
            });
            self.activity.truncate(50);
            self.unread_result = !self.active;
        }
        if lane == Lane::Mutation && self.operation.is_some() {
            match &response.result {
                Ok(_) => {
                    self.operation = None;
                    self.set_operation_feedback(None, false, cx);
                    window.close_dialog(cx);
                }
                Err(failure) => {
                    self.set_operation_feedback(Some(failure.message.clone()), false, cx);
                    if self.active {
                        self.target.update(cx, |s, cx| s.focus(window, cx));
                    }
                }
            }
        }
        let opening = lane == Lane::Snapshot && self.state.snapshot.is_none();
        let jobs = self.state.complete(lane, response);
        if opening && !self.state.loading() {
            if self.state.snapshot.is_some() {
                if self.last_repository.as_deref() != Some(self.state.path.as_str()) {
                    match Preferences::remember_repository(&self.state.path) {
                        Ok(()) => self.last_repository = Some(self.state.path.clone()),
                        Err(error) => eprintln!("Could not save the last repository: {error}"),
                    }
                }
            } else if self.state.error {
                let path = std::mem::take(&mut self.state.path);
                match Preferences::forget_last_repository(&path) {
                    Ok(()) if self.last_repository.as_deref() == Some(path.as_str()) => {
                        self.last_repository = None;
                    }
                    Err(error) => eprintln!("Could not clear the last repository: {error}"),
                    _ => {}
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
        for (input, value) in [
            (&self.wip_summary, summary.clone()),
            (&self.draft, summary),
            (&self.description, body),
        ] {
            input.update(cx, |s, cx| {
                if s.marked_text_range(window, cx).is_none() && s.value().as_ref() != value {
                    s.set_value(value, window, cx);
                }
            });
        }
        if lane == Lane::Snapshot && self.state.snapshot.is_some() {
            self.update_search(cx);
            self.install_watch(window, cx);
            if opening {
                if let Err(error) = Preferences::add_recent(&self.state.path, cx) {
                    eprintln!("Could not save recent repositories: {error}");
                }
            }
        }
        for job in jobs {
            self.dispatch(Some(job), window, cx);
        }
        #[cfg(feature = "native-evidence")]
        self.advance_evidence(lane, window, cx);
        cx.notify();
    }
    fn install_watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.watch_epoch == self.state.epoch {
            return;
        }
        self.watch_epoch = self.state.epoch;
        self.watcher = None;
        self.watch_warning = None;
        let epoch = self.state.epoch;
        let root = self.state.path.clone();
        let task = cx
            .background_executor()
            .spawn(async move { super::watch::start(root) });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let receiver = view.update_in(cx, |v, _, cx| {
                if v.state.epoch != epoch { return None; }
                match result {
                    Ok((watcher, receiver)) => { v.watcher = Some(watcher); Some(receiver) }
                    Err(e) => { v.watch_warning = Some(format!("Live refresh unavailable: {e}. Use F5; focus refresh remains available.")); cx.notify(); None }
                }
            }).ok().flatten();
            let Some(receiver) = receiver else { return; };
            while let Ok((mut error,mut event_time)) = receiver.recv().await {
                cx.background_executor().timer(std::time::Duration::from_millis(150)).await;
                while let Ok((next,time)) = receiver.try_recv() { event_time=time;if next.is_some() {error=next;} }
                let keep = view.update_in(cx, |v, w, cx| {
                    if v.state.epoch != epoch { return false; }
                    if let Some(error) = error { v.watch_warning = Some(format!("Live refresh interrupted: {error}. Use F5.")); }
                    if v.active && !v.state.busy && w.is_window_active() {
                        #[cfg(feature="native-evidence")]
                        {v.watch_start=Some(event_time);}
                        #[cfg(not(feature="native-evidence"))]
                        let _=event_time;
                        v.refresh(&Refresh, w, cx);
                    }
                    else { v.external_dirty = true; }
                    true
                }).unwrap_or(false);
                if !keep { break; }
            }
        }).detach();
    }
    fn update_search(&mut self, cx: &Context<Self>) {
        let query = self.search.read(cx).value().trim().to_lowercase();
        self.search_matches = self
            .state
            .snapshot
            .as_ref()
            .map(|s| {
                s.commits
                    .iter()
                    .filter(|c| {
                        !query.is_empty()
                            && format!("{} {} {} {}", c.subject, c.author, c.id, c.refs.join(" "))
                                .to_lowercase()
                                .contains(&query)
                    })
                    .map(|c| c.id.clone())
                    .collect()
            })
            .unwrap_or_default();
    }
    fn persist_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences.compact = self.compact;
        self.preferences.split = self.split;
        let timer = cx
            .background_executor()
            .timer(std::time::Duration::from_millis(200));
        self.preference_save = Some(cx.spawn_in(window, async move |view, cx| {
            timer.await;
            let _ = view.update_in(cx, |v, _, cx| {
                if let Err(error) = v.save_preferences(cx) {
                    v.state.error = true;
                    v.state.notice = format!("Could not save appearance: {error}");
                    cx.notify();
                }
            });
        }));
    }

    fn save_preferences(&mut self, cx: &mut App) -> Result<(), String> {
        self.preferences.compact = self.compact;
        self.preferences.split = self.split;
        self.preferences.save_changes(&self.saved_preferences, cx)?;
        self.saved_preferences = self.preferences.clone();
        Ok(())
    }

    fn reset_panels(&mut self, cx: &mut Context<Self>) {
        self.panels = [
            cx.new(|_| Default::default()),
            cx.new(|_| Default::default()),
        ];
        self.actual_inspector_width = [self.preferences.inspector_width; 2];
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
            - (2. * INSPECTOR_INSET + SEAM))
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
    fn review_placeholder(&self, cx: &mut Context<Self>) -> AnyElement {
        let loading = self.state.diff_loading();
        let staged = self.state.diff_context.as_ref().is_some_and(|c| c.2);
        panel()
            .items_center()
            .justify_center()
            .gap_3()
            .p_4()
            .child(
                icon(if loading { "refresh" } else { "check" })
                    .text_color(rgb(semantic(Role::ActionPrimary))),
            )
            .child(div().text_size(type_size(cx, 16.)).child(if loading {
                "Loading changes…"
            } else if self.state.error {
                "Changes could not be loaded"
            } else if staged {
                "No staged changes to review"
            } else {
                "Working changes reviewed"
            }))
            .child(
                div()
                    .text_color(rgb(semantic(Role::TextMuted)))
                    .child(if loading {
                        "Your selection is being updated."
                    } else {
                        "Choose another file, review staged changes, or create your commit."
                    }),
            )
            .child(
                self.action_button("review-history", "Return to history")
                    .on_click(cx.listener(|v, _, w, cx| v.back(&Back, w, cx))),
            )
            .into_any_element()
    }
    #[cfg(feature = "native-evidence")]
    fn advance_evidence(&mut self, lane: Lane, window: &mut Window, cx: &mut Context<Self>) {
        if std::env::var_os("GITCRAB_NATIVE_READY").is_none() {
            return;
        }
        let mode = std::env::var("GITCRAB_CAPTURE_VIEW").unwrap_or_else(|_| "history".into());
        if self.state.error {
            self.evidence_phase = 9;
            return;
        }
        if self.evidence_phase == 0 && lane == Lane::Snapshot && self.state.snapshot.is_some() {
            self.evidence_phase = 1;
            if mode == "details" || mode == "commitdiff" {
                let id = self.state.snapshot.as_ref().unwrap().head_id.clone();
                self.select(Some(id), window, cx);
            } else if mode == "diff" || mode == "split" {
                self.split = mode == "split";
                if let Some(path) = self
                    .state
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .status
                    .first()
                    .map(|f| f.path.clone())
                {
                    self.file(path, false, window, cx);
                }
            } else {
                if mode == "dialog" {
                    self.review(Operation::CreateBranch, "feature/review".into(), window, cx);
                }
                self.evidence_phase = 3;
            }
        } else if self.evidence_phase == 1 && lane == Lane::Details {
            if mode == "commitdiff" {
                if let Some(path) = self.state.files.first().map(|file| file.path.clone()) {
                    self.file(path, false, window, cx);
                    self.evidence_phase = 2;
                }
            } else {
                self.evidence_phase = 3;
            }
        } else if matches!(self.evidence_phase, 1 | 2) && lane == Lane::Diff {
            self.evidence_phase = 3;
        }
    }
    pub fn repository_path(&self) -> &str {
        &self.state.path
    }

    pub fn has_repository(&self) -> bool {
        self.state.snapshot.is_some() || self.state.loading()
    }

    pub fn settings_open(&self) -> bool {
        self.settings_open
    }

    pub fn unread_result(&self) -> bool {
        self.unread_result
    }

    pub fn mutating(&self) -> bool {
        self.state.busy
    }

    pub fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active = true;
        self.unread_result = false;
        self.preferences.theme = Preferences::current(cx).theme;
        self.focus.focus(window);
        gpui_component::Theme::global_mut(cx).font_size = px(self.preferences.font_size);
        if self.external_dirty && !self.state.busy {
            self.external_dirty = false;
            self.refresh(&Refresh, window, cx);
        }
    }

    pub fn deactivate(&mut self) {
        self.active = false;
    }

    pub fn open_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_failure = None;
        let job = self.state.open(path);
        self.dispatch(job, window, cx);
        cx.notify();
    }
    fn refresh(&mut self, _: &Refresh, window: &mut Window, cx: &mut Context<Self>) {
        let j = self.state.refresh();
        self.dispatch(j, window, cx);
    }
    fn select(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_selection(id.as_deref(), window);
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
        let job = self.state.preview_ref(kind, name);
        if self.state.is_previewing(kind, name) {
            let id = self.state.selected.clone();
            self.reveal_selection(id.as_deref(), window);
        }
        self.dispatch(job, window, cx);
    }

    fn reveal_selection(&mut self, id: Option<&str>, window: &mut Window) {
        #[cfg(feature = "native-evidence")]
        {
            self.feedback_start = Some((std::time::Instant::now(), "selection-to-next-frame"));
        }
        self.selection = None;
        self.focus.focus(window);
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
        self.constrained_inspector = false;
        self.selection = None;
        self.active_hunk = 0;
        self.diff_scroll = UniformListScrollHandle::new();
        let j = self.state.diff(path, staged);
        self.dispatch(j, window, cx);
    }
    fn mutate(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(feature = "native-evidence")]
        {
            self.feedback_start =
                Some((std::time::Instant::now(), "mutation-feedback-to-next-frame"));
        }
        let j = self.state.mutate(action);
        self.selection = None;
        self.dispatch(j, window, cx);
    }
    fn show_references(&self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |d, _, cx| {
            let content = weak.update(cx, |v, cx| v.sidebar(cx)).ok();
            d.title("References")
                .alert()
                .p_4()
                .child(div().h(px(400.)).children(content))
        });
    }
    fn show_activity(&self, window: &mut Window, cx: &mut Context<Self>) {
        let records = self.activity.clone();
        let repo = self.state.path.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            dialog
                .title("Activity · this worktree")
                .alert()
                .p_4()
                .child(repo.clone())
                .child(
                    div()
                        .id("activity-records")
                        .max_h(px(320.))
                        .overflow_y_scroll()
                        .children(records.iter().map(|r| {
                            div()
                                .py_2()
                                .border_b_1()
                                .border_color(rgb(semantic(Role::BorderDivider)))
                                .child(
                                    div()
                                        .text_color(rgb(semantic(if r.failed {
                                            Role::StatusErrorFg
                                        } else {
                                            Role::StatusSuccessFg
                                        })))
                                        .child(r.summary.clone()),
                                )
                                .child(div().text_size(type_size(cx, 11.)).child(r.id.clone()))
                        })),
                )
        });
    }
    fn can_commit(&self) -> bool {
        !self.settings_open
            && !self.state.busy
            && self
                .state
                .draft
                .lines()
                .next()
                .is_some_and(|s| !s.trim().is_empty())
            && self.state.snapshot.as_ref().is_some_and(|s| {
                s.status.iter().any(|f| f.index != ' ' && f.index != '?') && !s.commit_blocked()
            })
    }
    fn commit_staged(&mut self, _: &CommitStaged, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open || window.has_active_dialog(cx) {
            return;
        }
        let composing = [&self.draft, &self.wip_summary, &self.description]
            .into_iter()
            .any(|input| input.update(cx, |s, cx| s.marked_text_range(window, cx).is_some()));
        if self.can_commit() && !composing {
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
    fn review(
        &mut self,
        op: Operation,
        target: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.busy || self.state.snapshot.is_none() {
            return;
        }
        self.set_operation_feedback(None, false, cx);
        self.operation = Some(op.clone());
        self.target.update(cx, |s, cx| {
            s.set_placeholder(op.placeholder(), window, cx);
            s.set_value(target, window, cx);
        });
        let target = self.target.clone();
        let feedback = self.operation_feedback.clone();
        let view = cx.weak_entity();
        let cancel = view.clone();
        let epoch = self.state.epoch;
        let repo = self.state.path.clone();
        let head = self.state.snapshot.as_ref().unwrap().head_label();
        let label = match op {
            Operation::CreateBranch | Operation::CreateBranchAt { .. } => "Create branch",
            Operation::Checkout => "Checkout",
            Operation::Merge => "Merge",
            Operation::CherryPick => "Cherry-pick",
            Operation::Revert => "Revert",
            Operation::Stash => "Stash",
            Operation::PopStash => "Pop stash",
        };
        window.open_dialog(cx, move |dialog, _, cx| {
            let view = view.clone();
            let cancel = cancel.clone();
            let target = target.clone();
            let op = op.clone();
            let issue = feedback
                .read(cx)
                .error
                .clone()
                .or_else(|| op.validate_target(target.read(cx).value().as_ref()).err());
            let invalid = issue.is_some();
            let busy = feedback.read(cx).busy;
            let submit = view.clone();
            let submit_op = op.clone();
            dialog
                .title(label)
                .confirm()
                .footer(move |_, cancel, w, cx| {
                    vec![
                        cancel(w, cx),
                        Button::new("operation-submit")
                            .label(label)
                            .primary()
                            .small()
                            .h(px(CONTROL_HEIGHT))
                            .rounded(px(CONTROL_RADIUS))
                            .text_size(rems(12. / 13.))
                            .disabled(busy || invalid)
                            .on_click({
                                let submit = submit.clone();
                                let op = submit_op.clone();
                                move |_, w, cx| {
                                    let _ = submit.update(cx, |v, cx| {
                                        v.confirm_operation(op.clone(), epoch, w, cx);
                                    });
                                }
                            })
                            .into_any_element(),
                    ]
                })
                .button_props(gpui_component::dialog::DialogButtonProps::default().ok_text(label))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .p_3()
                                .rounded(px(6.))
                                .bg(rgb(semantic(Role::SurfaceCanvas)))
                                .border_1()
                                .border_color(rgb(semantic(Role::BorderDivider)))
                                .child(
                                    div()
                                        .text_size(type_size(cx, 12.))
                                        .text_color(rgb(semantic(Role::TextSecondary)))
                                        .child("REPOSITORY"),
                                )
                                .child(
                                    div()
                                        .mt_1()
                                        .text_size(type_size(cx, 12.))
                                        .child(repo.clone()),
                                )
                                .child(div().mt_2().child(badge(
                                    format!("Current branch · {head}"),
                                    semantic(Role::ActionPrimary),
                                ))),
                        )
                        .child(div().font_weight(FontWeight::MEDIUM).child(match op {
                            Operation::CreateBranch | Operation::CreateBranchAt { .. } => {
                                "New branch name"
                            }
                            Operation::Stash => "Stash message (optional)",
                            Operation::PopStash => "Latest stash",
                            _ => "Target branch or commit",
                        }))
                        .when(!matches!(op, Operation::PopStash), |e| {
                            e.child(Input::new(&target))
                        })
                        .when_some(
                            match &op {
                                Operation::CreateBranchAt { start } => Some(start.clone()),
                                _ => None,
                            },
                            |e, start| e.child(format!("Starting at {}", start)),
                        )
                        .child(
                            div()
                                .text_size(type_size(cx, 12.))
                                .text_color(rgb(semantic(Role::TextMuted)))
                                .child(op.help()),
                        )
                        .when_some(issue, |e, issue| {
                            e.child(
                                div()
                                    .text_color(rgb(semantic(Role::StatusErrorFg)))
                                    .child(issue),
                            )
                        }),
                )
                .on_ok(move |_, w, cx| {
                    view.update(cx, |v, cx| {
                        if v.state.epoch != epoch {
                            v.state.error = true;
                            v.state.notice =
                                "Repository changed. Review this operation again.".into();
                            v.operation = None;
                            cx.notify();
                            return true;
                        }
                        if v.state.busy {
                            return false;
                        }
                        v.confirm_operation(op.clone(), epoch, w, cx);
                        false
                    })
                    .unwrap_or(true)
                })
                .on_cancel(move |_, _, cx| {
                    cancel
                        .update(cx, |v, cx| {
                            if v.state.busy {
                                return false;
                            }
                            v.operation = None;
                            v.set_operation_feedback(None, false, cx);
                            cx.notify();
                            true
                        })
                        .unwrap_or(true)
                })
        });
        self.target.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }
    fn set_operation_feedback(
        &mut self,
        error: Option<String>,
        busy: bool,
        cx: &mut Context<Self>,
    ) {
        self.operation_feedback.update(cx, |state, cx| {
            state.error = error;
            state.busy = busy;
            cx.notify();
        });
    }
    fn confirm_operation(
        &mut self,
        op: Operation,
        epoch: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.busy || self.state.epoch != epoch {
            return;
        }
        let target = self.target.read(cx).value().to_string();
        if let Err(issue) = op.validate_target(&target) {
            self.set_operation_feedback(Some(issue), false, cx);
            self.target.update(cx, |s, cx| s.focus(window, cx));
            cx.notify();
            return;
        }
        self.set_operation_feedback(None, true, cx);
        self.mutate(Action::Operation { op, target }, window, cx);
    }
    fn back(&mut self, _: &Back, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            self.settings_open = false;
            self.focus.focus(window);
            cx.notify();
            return;
        }
        if window.has_active_dialog(cx) {
            window.close_all_dialogs(cx);
            self.operation = None;
            cx.notify();
            return;
        }
        self.operation = None;
        if !self.search.read(cx).value().is_empty() {
            self.search.update(cx, |s, cx| s.set_value("", window, cx));
            self.search_matches.clear();
        }
        self.state.close_diff();
        self.focus.focus(window);
        cx.notify();
    }
    fn move_selection(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open
            || window.has_focused_input(cx)
            || self.operation.is_some()
            || self.state.diff_context.is_some()
        {
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
        if let Some(patch) = &self.state.patch {
            let range = self.selection.clone().unwrap_or(0..0);
            let text = range
                .filter_map(|i| {
                    if self.split {
                        patch.split.get(i).map(|(a, b)| {
                            format!(
                                "{}\t{}",
                                a.as_ref().map(|l| l.text.as_str()).unwrap_or(""),
                                b.as_ref().map(|l| l.text.as_str()).unwrap_or("")
                            )
                        })
                    } else {
                        patch.lines.get(i).map(|l| l.text.clone())
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        } else if let Some(id) = &self.state.selected {
            cx.write_to_clipboard(ClipboardItem::new_string(id.clone()));
        }
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            return;
        }
        if let Some(p) = &self.state.patch {
            self.selection = Some(
                0..if self.split {
                    p.split.len()
                } else {
                    p.lines.len()
                },
            );
            cx.notify();
        }
    }
    fn button(&self, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
        components::sized(Button::new(id).label(label).ghost(), ControlSize::Compact)
            .tab_stop(!self.settings_open)
    }
    // Actions and icon-only targets are separate from quiet row navigation.
    fn action_button(&self, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
        self.button(id, label)
            .with_variant(ButtonVariant::Secondary)
            .outline()
    }
    fn primary_button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        disabled: bool,
    ) -> Button {
        let label = label.into();
        self.button(id, if disabled { label.clone() } else { "".into() })
            .primary()
            .disabled(disabled)
            .when(!disabled, |button| {
                button.child(
                    div()
                        .text_color(rgb(semantic(Role::SurfaceChrome)))
                        .child(label),
                )
            })
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
                    .bg(rgb(semantic(Role::InteractionSelected)))
                    .text_color(rgb(semantic(Role::TextDefault)))
            })
    }

    fn icon_button(
        &self,
        id: impl Into<ElementId>,
        glyph: &str,
        name: impl Into<SharedString>,
    ) -> Button {
        self.button(id, "")
            .icon(icon(glyph).size(px(GLYPH_SIZE)))
            .size(px(CONTROL_HEIGHT))
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
            let value = input.read(cx).value();
            div()
                .w(px(width))
                .h(px(height))
                .px_2()
                .rounded(px(6.))
                .bg(rgb(semantic(Role::SurfaceChrome)))
                .overflow_hidden()
                .text_color(rgb(semantic(Role::TextMuted)))
                .child(if value.is_empty() {
                    placeholder.to_string()
                } else {
                    value.to_string()
                })
                .into_any_element()
        } else {
            Input::new(input)
                .w(px(width))
                .h(px(height))
                .into_any_element()
        }
    }
    fn highlight_sidebar_row(button: Button, highlighted: bool) -> Button {
        button.selected(highlighted).when(highlighted, |button| {
            button
                .bg(rgb(semantic(Role::ActionAccentTintHover)))
                .text_color(rgb(semantic(Role::TextDefault)))
        })
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.state.snapshot.is_none() {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .bg(rgb(semantic(Role::SurfaceNav)))
                .child(div().flex_1())
                .child(
                    self.button("startup-appearance", "Appearance")
                        .icon(icon("settings"))
                        .h(px(32.))
                        .on_click(cx.listener(|v, _, w, cx| {
                            v.open_settings(w, cx);
                        })),
                )
                .into_any_element();
        }
        let changes = self
            .state
            .snapshot
            .as_ref()
            .map(|s| s.status.len())
            .unwrap_or(0);
        let query = self.reference_search.read(cx).value().to_lowercase();
        let nav = div();
        #[cfg(feature = "native-evidence")]
        let nav = nav.on_children_prepainted(|bounds, _, _| {
            if let Some(path) = std::env::var_os("GITCRAB_SIDEBAR_LAYOUT_PROBE") {
                if let [filter, working, local, ..] = bounds.as_slice() {
                    let data = serde_json::json!({
                        "above": f32::from(working.top() - filter.bottom()),
                        "below": f32::from(local.top() - working.bottom()),
                        "row_height": f32::from(working.size.height),
                        "units": "logical pixels"
                    });
                    let _ = std::fs::write(path, serde_json::to_vec(&data).unwrap());
                }
            }
        });
        let mut nav = nav
            .id("navigation")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .p_2()
            .gap_1()
            .overflow_y_scroll()
            .child(
                Input::new(&self.reference_search)
                    .h(px(28.))
                    .disabled(self.settings_open),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        self.button("working", "Working changes")
                            .icon(icon("changes"))
                            .flex_1()
                            .justify_start()
                            .h(px(32.))
                            .map(|button| {
                                Self::highlight_sidebar_row(button, self.state.selected.is_none())
                            })
                            .on_click(cx.listener(|v, _, w, cx| v.select(None, w, cx))),
                    )
                    .child(badge(
                        changes.to_string(),
                        if changes > 0 {
                            semantic(Role::ActionPrimary)
                        } else {
                            semantic(Role::TextMuted)
                        },
                    )),
            );
        if let Some(s) = &self.state.snapshot {
            let local_key = format!("{}|", self.state.path);
            let local_collapsed = self.preferences.collapsed.contains(&local_key);
            nav = nav.child(
                self.button("local-branches", "Local")
                    .w_full()
                    .h(px(28.))
                    .px_2()
                    .justify_start()
                    .icon(icon(if local_collapsed { "right" } else { "down" }))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(semantic(Role::TextDefault)))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_color(rgb(semantic(Role::TextMuted)))
                            .child(s.branches.len().to_string()),
                    )
                    .border_b_1()
                    .border_color(rgb(semantic(Role::BorderDivider)))
                    .tooltip(if local_collapsed {
                        "Expand local branches"
                    } else {
                        "Collapse local branches"
                    })
                    .on_click(cx.listener(move |v, _, w, cx| {
                        if !v.preferences.collapsed.remove(&local_key) {
                            v.preferences.collapsed.insert(local_key.clone());
                        }
                        v.persist_preferences(w, cx);
                        cx.notify();
                    })),
            );
            let mut tree = div().flex().flex_col().min_w_0();
            let mut groups = std::collections::BTreeSet::new();
            for b in s
                .branches
                .iter()
                .map(|branch| &branch.name)
                .filter(|b| !local_collapsed && b.to_lowercase().contains(&query))
            {
                let parts = b.split('/').collect::<Vec<_>>();
                let mut hidden = false;
                for depth in 1..parts.len() {
                    let prefix = parts[..depth].join("/");
                    let key = format!("{}|{}", self.state.path, prefix);
                    let collapsed = self.preferences.collapsed.contains(&key);
                    if groups.insert(prefix.clone()) {
                        let label = parts[depth - 1].to_string();
                        tree = tree.child(
                            div().pl(px(8. + (depth - 1) as f32 * 12.)).child(
                                self.button(
                                    SharedString::from(format!("branch-group-{prefix}")),
                                    label,
                                )
                                .icon(icon(if collapsed { "right" } else { "down" }))
                                .tooltip(format!(
                                    "{} {prefix}",
                                    if collapsed { "Expand" } else { "Collapse" }
                                ))
                                .w_full()
                                .justify_start()
                                .h(px(24.))
                                .text_color(rgb(semantic(Role::TextMuted)))
                                .on_click(cx.listener(
                                    move |v, _, w, cx| {
                                        if !v.preferences.collapsed.remove(&key) {
                                            v.preferences.collapsed.insert(key.clone());
                                        }
                                        v.persist_preferences(w, cx);
                                        cx.notify();
                                    },
                                )),
                            ),
                        );
                    }
                    if collapsed {
                        hidden = true;
                        break;
                    }
                }
                if hidden {
                    continue;
                }
                let branch = b.clone();
                let current = s.checked_out_branch() == Some(b.as_str());
                let previewed = self.state.is_previewing(ReferenceKind::LocalBranch, b);
                let hint = match (current, previewed) {
                    (true, true) => "Checked out · Previewing",
                    (true, false) => "Checked out · Click to preview",
                    (false, true) => "Previewing · Double-click to checkout",
                    (false, false) => "Click to preview · Double-click to checkout",
                };
                tree = tree.child(
                    self.button(SharedString::from(format!("local-branch-{b}")), "")
                        .icon(
                            icon(if current { "check" } else { "branch" }).text_color(rgb(
                                if current {
                                    semantic(Role::StatusSuccessFg)
                                } else {
                                    semantic(Role::TextMuted)
                                },
                            )),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .child(parts.last().unwrap().to_string()),
                        )
                        .w_full()
                        .justify_start()
                        .h(px(24.))
                        .pl(px(20. + (parts.len() - 1) as f32 * 12.))
                        .pr_2()
                        .text_color(rgb(if current || previewed {
                            semantic(Role::TextDefault)
                        } else {
                            semantic(Role::TextMuted)
                        }))
                        .when(current, |b| b.font_weight(FontWeight::SEMIBOLD))
                        .when(current && !previewed, |button| {
                            button.bg(rgb(semantic(Role::StatusSuccessBg)))
                        })
                        .map(|button| Self::highlight_sidebar_row(button, previewed))
                        .tooltip(format!("{b} · {hint}"))
                        .on_click(cx.listener(move |v, e: &ClickEvent, w, cx| {
                            if e.click_count() == 2 {
                                v.review(Operation::Checkout, branch.clone(), w, cx);
                            } else {
                                v.preview_ref(ReferenceKind::LocalBranch, &branch, w, cx);
                            }
                        })),
                );
            }
            nav = nav.child(tree);
            for (kind, glyph, ref_kind, refs) in [
                ("Remotes", "remote", ReferenceKind::Remote, &s.remote_refs),
                ("Tags", "tag", ReferenceKind::Tag, &s.tags),
            ] {
                let key = format!("{}|{kind}", self.state.path);
                let collapsed = self.preferences.collapsed.contains(&key);
                nav = nav.child(
                    self.button(
                        SharedString::from(format!("ref-heading-{kind}")),
                        format!("{kind} · {}", refs.len()),
                    )
                    .icon(icon(if collapsed { "right" } else { "down" }))
                    .w_full()
                    .justify_start()
                    .h(type_size(cx, 30.))
                    .on_click(cx.listener(move |v, _, w, cx| {
                        if !v.preferences.collapsed.remove(&key) {
                            v.preferences.collapsed.insert(key.clone());
                        }
                        v.persist_preferences(w, cx);
                        cx.notify();
                    })),
                );
                for (i, reference) in refs
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| !collapsed && r.to_lowercase().contains(&query))
                {
                    let previewed = self.state.is_previewing(ref_kind, reference);
                    let reference = reference.clone();
                    nav = nav.child(
                        self.button(SharedString::from(format!("{kind}-{i}")), reference.clone())
                            .icon(icon(glyph))
                            .w_full()
                            .justify_start()
                            .h(type_size(cx, 26.))
                            .tooltip(reference.clone())
                            .map(|button| Self::highlight_sidebar_row(button, previewed))
                            .on_click(cx.listener(move |v, _, w, cx| {
                                v.preview_ref(ref_kind, &reference, w, cx);
                            })),
                    );
                }
            }
            nav = nav.child(heading(format!("Stashes · {}", s.stash_count)));
        }
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(semantic(Role::SurfaceNav)))
            .overflow_hidden()
            .child(nav)
            .child(
                div()
                    .p_2()
                    .border_t_1()
                    .border_color(rgb(semantic(Role::BorderDivider)))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        self.action_button("stash-pop", "Restore latest stash…")
                            .icon(icon("stash"))
                            .w_full()
                            .justify_start()
                            .disabled(
                                self.state.busy
                                    || !self
                                        .state
                                        .snapshot
                                        .as_ref()
                                        .is_some_and(|s| s.stash_count > 0),
                            )
                            .on_click(cx.listener(|v, _, w, cx| {
                                v.review(Operation::PopStash, String::new(), w, cx)
                            })),
                    )
                    .child(
                        self.button("settings", "Appearance")
                            .icon(icon("settings"))
                            .w_full()
                            .justify_start()
                            .on_click(cx.listener(|v, _, w, cx| {
                                v.open_settings(w, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
    fn new_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let loading = self.state.loading();
        let recent = Preferences::current(cx).recent;
        let notice = self
            .open_failure
            .as_ref()
            .map(|failure| (failure.path.as_str(), failure.details.as_str()))
            .or_else(|| self.state.error.then_some(("", self.state.notice.as_str())));
        panel()
            .relative()
            .track_focus(&self.focus)
            .bg(rgb(semantic(Role::SurfaceCanvas)))
            .text_color(rgb(semantic(Role::TextDefault)))
            .text_size(px(self.preferences.font_size))
            .items_center()
            .justify_center()
            .gap_3()
            .p_4()
            .child(super::assets::logo(!Theme::global(cx).is_dark()).size(px(52.)))
            .child(
                div()
                    .text_size(type_size(cx, 22.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if loading {
                        "Opening repository…"
                    } else {
                        "Open a repository"
                    }),
            )
            .when(!loading, |el| {
                el.child(
                    div().text_color(rgb(semantic(Role::TextSecondary))).child(
                        "Choose a local Git repository to browse history and review changes.",
                    ),
                )
                .child(
                    Button::new("browse-repository")
                        .primary()
                        .tab_stop(true)
                        .icon(icon("folder"))
                        .label("Browse…")
                        .disabled(self.settings_open)
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
                                    .text_color(rgb(semantic(Role::TextMuted)))
                                    .child("Recent repositories"),
                            )
                            .child(
                                div()
                                    .id("new-tab-recent-list")
                                    .max_h(px(112.))
                                    .overflow_y_scroll()
                                    .children(recent.into_iter().enumerate().map(
                                        |(index, path)| {
                                            let name = std::path::Path::new(&path)
                                                .file_name()
                                                .map(|name| name.to_string_lossy().into_owned())
                                                .unwrap_or_else(|| path.clone());
                                            self.button(("new-tab-recent", index), "")
                                                .icon(icon("history"))
                                                .justify_start()
                                                .overflow_hidden()
                                                .child(div().min_w_0().truncate().child(name))
                                                .tooltip(path.clone())
                                                .tab_stop(true)
                                                .disabled(self.settings_open)
                                                .on_click(move |_, w, cx| {
                                                    w.dispatch_action(
                                                        Box::new(super::workspace::OpenRecent(
                                                            path.clone(),
                                                        )),
                                                        cx,
                                                    );
                                                })
                                        },
                                    )),
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
                        .bg(rgb(semantic(Role::SurfacePanel)))
                        .border_1()
                        .border_color(rgb(semantic(Role::BorderDivider)))
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
                                        .bg(rgb(semantic(Role::StatusWarningFg))),
                                )
                                .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(
                                    if path.is_empty() {
                                        "Action failed"
                                    } else {
                                        "Couldn’t open repository"
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
                                    .text_size(type_size(cx, 12.))
                                    .child(name),
                            )
                        })
                        .child(
                            div()
                                .id("open-failure-details")
                                .mt_1()
                                .max_h(px(80.))
                                .overflow_y_scroll()
                                .text_size(type_size(cx, 12.))
                                .text_color(rgb(semantic(Role::TextSecondary)))
                                .child(details.to_owned()),
                        ),
                )
            })
            .into_any_element()
    }
}
impl Render for GitCrab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(feature = "native-evidence")]
        if let Some((start, kind)) = self.feedback_start.take() {
            window.on_next_frame(move |_, cx| super::trace::record(kind, start, cx));
        }
        #[cfg(feature = "native-evidence")]
        if self.evidence_phase == 3 {
            self.evidence_phase = 4;
            let data = serde_json::json!({"loaded":self.state.graph.rows.len(),"view":std::env::var("GITCRAB_CAPTURE_VIEW").unwrap_or_else(|_|"history".into()),"width":f32::from(window.viewport_size().width),"height":f32::from(window.viewport_size().height),"scale":window.scale_factor(),"worker_ms":self.state.worker_ms,"diff_rows":self.state.patch.as_ref().map(|p|p.lines.len()),"dialog_layer":window.has_active_dialog(cx),"renderer":"GPUI 0.2.2 native","present_time_measured":false});
            let path = std::env::var("GITCRAB_NATIVE_READY").unwrap();
            window.on_next_frame(move |_, _| {
                let _ = std::fs::write(path, serde_json::to_vec_pretty(&data).unwrap());
            });
            window.request_animation_frame();
        }
        #[cfg(feature = "native-evidence")]
        if self.evidence_phase == 4 && std::env::var_os("GITCRAB_NATIVE_BENCH").is_some() {
            if let Some(previous) = self.benchmark.0.replace(std::time::Instant::now()) {
                self.benchmark
                    .1
                    .push(previous.elapsed().as_secs_f64() * 1000.);
            }
            if self.benchmark.1.len() < 240 {
                let i = self.benchmark.1.len();
                if let Some(p) = &self.state.patch {
                    self.diff_scroll.scroll_to_item(
                        (i * 2).min(p.lines.len().saturating_sub(1)),
                        ScrollStrategy::Top,
                    );
                } else {
                    self.history_scroll.scroll_to_item(
                        (i * 2).min(self.state.graph.rows.len()),
                        ScrollStrategy::Top,
                    );
                }
                let weak = cx.weak_entity();
                window.on_next_frame(move |_, cx| {
                    let _ = weak.update(cx, |_, cx| cx.notify());
                });
                window.request_animation_frame();
            } else {
                self.evidence_phase = 5;
                let data = serde_json::json!({"samples_ms":self.benchmark.1,"conditions":"240 GPUI render/animation intervals with synthetic scroll; not physical presentation or input latency","loaded":self.state.graph.rows.len(),"diff_rows":self.state.patch.as_ref().map(|p|p.lines.len())});
                let path = format!(
                    "{}.bench.json",
                    std::env::var("GITCRAB_NATIVE_READY").unwrap()
                );
                let _ = std::fs::write(path, serde_json::to_vec_pretty(&data).unwrap());
            }
        }
        #[cfg(feature = "native-evidence")]
        if let Some(path) = std::env::var_os("GITCRAB_DRAFT_PROBE") {
            let data = serde_json::json!({"draft":self.state.draft,"revision":self.state.draft_revision,"wip":self.wip_summary.read(cx).value().to_string(),"summary":self.draft.read(cx).value().to_string(),"body":self.description.read(cx).value().to_string(),"wip_focused":self.wip_summary.read(cx).focus_handle(cx).is_focused(window),"summary_focused":self.draft.read(cx).focus_handle(cx).is_focused(window),"activity":self.activity.iter().map(|r|&r.summary).collect::<Vec<_>>()});
            let _ = std::fs::write(path, serde_json::to_vec_pretty(&data).unwrap());
        }
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
        let ui_scale = self.preferences.font_size / 13.;
        let single_pane = f32::from(window.viewport_size().width) < 648. * ui_scale + 44.;
        let rail = in_diff || f32::from(window.viewport_size().width) < 1200. * ui_scale;
        let sidebar = if rail {
            div()
                .flex()
                .flex_col()
                .items_center()
                .size_full()
                .bg(rgb(semantic(Role::SurfaceChrome)))
                .gap_2()
                .child(
                    self.icon_button("rail-back", "back", "Return to history · Escape")
                        .on_click(cx.listener(|v, _, w, cx| v.back(&Back, w, cx))),
                )
                .child(
                    self.icon_button("rail-references", "branch", "Open reference navigation")
                        .on_click(cx.listener(|v, _, w, cx| v.show_references(w, cx))),
                )
                .child(
                    self.icon_button("rail-working", "changes", "Working changes")
                        .on_click(cx.listener(|v, _, w, cx| v.select(None, w, cx))),
                )
                .into_any_element()
        } else {
            self.sidebar(cx)
        };
        let inspector = self.inspector(cx);
        let center = if self.state.patch.is_some() {
            self.diff(cx)
        } else if in_diff {
            self.review_placeholder(cx)
        } else {
            self.history(window, cx)
        };
        let branch = self
            .state
            .snapshot
            .as_ref()
            .map(|s| s.head_label())
            .unwrap_or_default();
        let recent = Preferences::current(cx)
            .recent
            .iter()
            .filter(|path| *path != &self.state.path)
            .cloned()
            .collect::<Vec<_>>();

        let menu_focus = self.focus.clone();
        let mut toolbar = div()
            .h(type_size(cx, TOOLBAR_HEIGHT))
            .bg(rgb(semantic(Role::SurfaceChrome)))
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .px_3()
            .border_b_1()
            .border_color(rgb(semantic(Role::BorderDivider)))
            .child(
                components::selector("repository-selector", "Repository", title.clone(), "folder")
                    .tooltip(format!(
                        "{} · Open repository · Ctrl/Cmd+O",
                        self.state.path
                    ))
                    .disabled(self.state.busy)
                    .dropdown_menu(move |menu, _, _| {
                        let mut menu = menu.action_context(menu_focus.clone());
                        menu = menu
                            .item(PopupMenuItem::new("Open repository…").action(Box::new(Open)));
                        for path in &recent {
                            let path = path.clone();
                            let name = std::path::Path::new(&path)
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy();
                            menu = menu.item(
                                PopupMenuItem::element({
                                    let label = format!(
                                        "{} · {}",
                                        name,
                                        std::path::Path::new(&path)
                                            .parent()
                                            .and_then(|p| p.file_name())
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                    );
                                    let full = path.clone();
                                    move |_, _| {
                                        div()
                                            .id(SharedString::from(format!("recent-{full}")))
                                            .child(label.clone())
                                            .tooltip({
                                                let full = full.clone();
                                                move |w, cx| Tooltip::new(full.clone()).build(w, cx)
                                            })
                                    }
                                })
                                .action(Box::new(super::workspace::OpenRecent(path))),
                            );
                        }
                        menu
                    }),
            )
            .child(separator())
            .child({
                let branches = self
                    .state
                    .snapshot
                    .as_ref()
                    .map(|s| {
                        s.branches
                            .iter()
                            .map(|b| {
                                let name = b.name.clone();
                                let current = s.checked_out_branch() == Some(name.as_str());
                                let previewed =
                                    self.state.is_previewing(ReferenceKind::LocalBranch, &name);
                                (name, current, previewed)
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let weak = cx.weak_entity();
                components::selector("branch-selector", "Checked out", branch.clone(), "branch")
                    .tooltip(format!("{branch} · Choose a branch to preview its history"))
                    .dropdown_menu(move |mut menu, _, _| {
                        for (name, current, previewed) in &branches {
                            let label = if *previewed {
                                format!("{name} · Previewing")
                            } else {
                                name.clone()
                            };
                            let name = name.clone();
                            let weak = weak.clone();
                            menu = menu.item(PopupMenuItem::new(label).checked(*current).on_click(
                                move |_, w, cx| {
                                    let _ = weak.update(cx, |v, cx| {
                                        v.preview_ref(ReferenceKind::LocalBranch, &name, w, cx);
                                    });
                                },
                            ));
                        }
                        menu
                    })
            });
        let narrow_toolbar =
            f32::from(window.viewport_size().width) < 1120. * (self.preferences.font_size / 13.);
        if !narrow_toolbar {
            toolbar = toolbar.child(separator());
            for (label, glyph) in [("Fetch", "fetch"), ("Pull", "pull"), ("Push", "push")] {
                toolbar=toolbar.child(components::toolbar_command(SharedString::from(format!("remote-{glyph}")),label,glyph).disabled(true).tooltip(format!("{label} unavailable: network and authentication support is not implemented")));
            }
            toolbar = toolbar.child(separator());
            for (i, (op, label, glyph)) in [
                (Operation::CreateBranch, "Branch", "branch-plus"),
                (Operation::Merge, "Merge", "merge"),
                (Operation::Stash, "Stash", "stash"),
                (Operation::PopStash, "Pop", "pop"),
            ]
            .into_iter()
            .enumerate()
            {
                if i == 2 {
                    toolbar = toolbar.child(separator());
                }
                toolbar = toolbar.child(
                    components::toolbar_command(("toolbar", i), label, glyph)
                        .disabled(
                            self.state.busy
                                || (i == 3
                                    && self
                                        .state
                                        .snapshot
                                        .as_ref()
                                        .is_none_or(|s| s.stash_count == 0)),
                        )
                        .tooltip(format!("{label} · Review before applying"))
                        .on_click(cx.listener(move |v, _, w, cx| {
                            v.review(op.clone(), String::new(), w, cx)
                        })),
                );
            }
        }
        toolbar = toolbar
            .child(div().flex_1())
            .child(div().w(px(180.)).child(self.editor(
                &self.search,
                32.,
                180.,
                "Search history…",
                cx,
            )))
            .when(!self.search.read(cx).value().is_empty(), |bar| {
                bar.child(
                    div()
                        .text_size(type_size(cx, 12.))
                        .text_color(rgb(semantic(Role::TextMuted)))
                        .child(format!("{} matches", self.search_matches.len())),
                )
                .child(
                    self.icon_button("search-prev", "up", "Previous match · Shift+F3")
                        .tooltip("Previous match · Shift+Enter")
                        .disabled(self.search_matches.is_empty())
                        .on_click(cx.listener(|v, _, w, cx| v.search_step(-1, w, cx))),
                )
                .child(
                    self.icon_button("search-next", "down", "Next match · F3")
                        .tooltip("Next match · Enter. Searches loaded history.")
                        .disabled(self.search_matches.is_empty())
                        .on_click(cx.listener(|v, _, w, cx| v.search_step(1, w, cx))),
                )
            });
        let weak = cx.weak_entity();
        toolbar = toolbar.child(
            self.icon_button("toolbar-more", "more", "More repository actions")
                .dropdown_menu(move |mut menu, _, _| {
                    menu = menu.item(
                        PopupMenuItem::new("Refresh repository · F5").action(Box::new(Refresh)),
                    );
                    for (op, label) in [
                        (Operation::CreateBranch, "Create branch…"),
                        (Operation::Checkout, "Checkout…"),
                        (Operation::Merge, "Merge…"),
                        (Operation::Stash, "Stash…"),
                        (Operation::PopStash, "Pop latest stash…"),
                    ] {
                        let weak = weak.clone();
                        menu = menu.item(PopupMenuItem::new(label).on_click(move |_, w, cx| {
                            let _ =
                                weak.update(cx, |v, cx| v.review(op.clone(), String::new(), w, cx));
                        }));
                    }
                    let appearance = weak.clone();
                    let refs = weak.clone();
                    menu = menu
                        .separator()
                        .item(PopupMenuItem::new("References…").on_click(move |_, w, cx| {
                            let _ = refs.update(cx, |v, cx| v.show_references(w, cx));
                        }))
                        .item(PopupMenuItem::new("Appearance…").on_click(move |_, w, cx| {
                            let _ = appearance.update(cx, |v, cx| {
                                v.open_settings(w, cx);
                            });
                        }));
                    menu.separator().item(
                        PopupMenuItem::new("Fetch / Pull / Push: network support unavailable")
                            .disabled(true),
                    )
                }),
        );
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
                .bg(rgb(semantic(Role::BorderDivider)))
                .child(
                    h_resizable(SharedString::from(format!(
                        "workspace-{rail}-{}",
                        self.layout_revision
                    )))
                    .with_state(&self.panels[usize::from(rail)])
                    .on_resize(cx.listener(
                        move |v,
                              state: &Entity<gpui_component::resizable::ResizableState>,
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
                    .child(
                        resizable_panel()
                            .size(px(self.preferences.inspector_width))
                            .size_range(px(260.)..px(560.))
                            .child(div().size_full().pl(px(SEAM)).child(inspector)),
                    ),
                )
                .into_any_element()
        };
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(semantic(Role::SurfaceCanvas)))
            .text_color(rgb(semantic(Role::TextDefault)))
            .text_size(px(self.preferences.font_size))
            .track_focus(&self.focus)
            .key_context("GitCrab")
            .on_action(cx.listener(Self::refresh))
            .on_action(cx.listener(Self::back))
            .on_action(cx.listener(|v, _: &Next, w, cx| v.move_selection(1, w, cx)))
            .on_action(cx.listener(|v, _: &Previous, w, cx| v.move_selection(-1, w, cx)))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(|v, _: &SplitBefore, w, cx| v.keyboard_pan_diff(false, w, cx)))
            .on_action(cx.listener(|v, _: &SplitAfter, w, cx| v.keyboard_pan_diff(true, w, cx)))
            .on_action(cx.listener(Self::commit_staged))
            .on_key_down(cx.listener(|v, event: &KeyDownEvent, w, cx| {
                if event.keystroke.key == "enter"
                    && event.keystroke.modifiers.shift
                    && v.search.read(cx).focus_handle(cx).is_focused(w)
                {
                    v.search_step(-1, w, cx);
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|v, _: &Find, w, cx| {
                if !w.has_active_dialog(cx) && !v.settings_open {
                    v.search.update(cx, |s, cx| s.focus(w, cx));
                }
            }))
            .on_action(cx.listener(|v, _: &SearchNext, w, cx| v.search_step(1, w, cx)))
            .on_action(cx.listener(|v, _: &SearchPrevious, w, cx| v.search_step(-1, w, cx)))
            .child(self.column_drag_events(cx))
            .child(toolbar)
            .when(single_pane, |el| {
                el.child(
                    section_bar()
                        .h(type_size(cx, 36.))
                        .child(self.button("compact-history", "History / Diff").on_click(
                            cx.listener(|v, _, _, cx| {
                                v.constrained_inspector = false;
                                cx.notify();
                            }),
                        ))
                        .child(
                            self.button("compact-inspector", "Changes / Details")
                                .on_click(cx.listener(|v, _, _, cx| {
                                    v.constrained_inspector = true;
                                    cx.notify();
                                })),
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
                        .bg(rgb(semantic(Role::SurfacePanel)))
                        .border_1()
                        .border_color(rgb(semantic(Role::BorderDivider)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .size(px(6.))
                                        .rounded_full()
                                        .bg(rgb(semantic(Role::StatusErrorFg))),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("Action failed"),
                                )
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
                                        v.state.error = false;
                                        v.state.notice.clear();
                                        cx.notify();
                                    }),
                                )),
                        )
                        .child(
                            div()
                                .id("error-details")
                                .max_h(px(80.))
                                .overflow_y_scroll()
                                .text_size(type_size(cx, 12.))
                                .text_color(rgb(semantic(Role::TextSecondary)))
                                .child(self.state.notice.clone()),
                        ),
                )
            })
            .child(main_content)
            .child(
                div()
                    .h(type_size(cx, 28.))
                    .bg(rgb(semantic(Role::SurfaceChrome)))
                    .border_t_1()
                    .border_color(rgb(semantic(Role::BorderDivider)))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .gap_2()
                    .text_size(type_size(cx, 12.))
                    .text_color(rgb(semantic(Role::TextMuted)))
                    .child(div().flex_1().truncate().child(if self.state.busy {
                        "Running Git operation…".into()
                    } else if self.state.loading() {
                        "Loading repository…".into()
                    } else if self.state.error {
                        String::new()
                    } else {
                        if self.state.notice.is_empty() {
                            self.activity
                                .front()
                                .map(|r| r.summary.clone())
                                .unwrap_or_default()
                        } else {
                            self.state.notice.clone()
                        }
                    }))
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
                                .text_color(rgb(semantic(Role::StatusWarningFg)))
                                .tooltip(warning),
                        )
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod date_tests {
    #[::core::prelude::v1::test]
    fn commit_dates_use_day_month_year_and_24_hour_time() {
        assert_eq!(
            super::display_date("2026-11-05T00:04:59-03:00"),
            "05/11/2026 @ 00:04"
        );
        assert_eq!(
            super::display_date("2026-01-02T23:07:00+00:00"),
            "02/01/2026 @ 23:07"
        );
        for value in ["", "unknown", "not a timestamp at all", "日付なし"] {
            assert_eq!(super::display_date(value), value);
        }
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
