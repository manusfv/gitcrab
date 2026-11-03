mod inspector;
mod review;
mod settings;
use super::preferences::Preferences;
use super::theme::*;
use gitcrab_core::{
    controller::{Controller, Lane},
    service::{Action, Failure, Operation, Request, Response, Service},
};
use gpui::{prelude::*, *};
use gpui_component::{
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt, PopupMenuItem},
    resizable::{h_resizable, resizable_panel},
    tooltip::Tooltip,
    Disableable, Sizable, WindowExt,
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
    let month = date
        .get(5..7)
        .and_then(|s| s.parse::<usize>().ok())
        .and_then(|m| {
            [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ]
            .get(m.wrapping_sub(1))
        })
        .copied();
    match (month, date.get(8..10), date.get(..4), date.get(11..16)) {
        (Some(month), Some(day), Some(year), Some(time)) => {
            format!("{day} {month} {year} · {time}")
        }
        _ => date.to_string(),
    }
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
        Quit,
        CommitStaged,
        Find,
        SearchNext,
        SearchPrevious
    ]
);
pub struct GitCrab {
    state: Controller,
    service: Service,
    focus: FocusHandle,
    draft: Entity<InputState>,
    description: Entity<InputState>,
    search: Entity<InputState>,
    target: Entity<InputState>,
    operation: Option<Operation>,
    split: bool,
    compact: bool,
    dim_whitespace: bool,
    active_hunk: usize,
    history_scroll: UniformListScrollHandle,
    diff_scroll: UniformListScrollHandle,
    selection: Option<Range<usize>>,
    anchor: usize,
    _subscriptions: Vec<Subscription>,
    preferences: Preferences,
    settings_open: bool,
    saving: bool,
    preference_save: Option<Task<()>>,
    layout_revision: u64,
    panels: [Entity<gpui_component::resizable::ResizableState>; 2],
    panel_subscriptions: Vec<Subscription>,
    actual_inspector_width: [f32; 2],
    watcher: Option<notify::RecommendedWatcher>,
    watch_epoch: u32,
    watch_warning: Option<String>,
    external_dirty: bool,
    search_matches: Vec<String>,
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
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let draft = cx.new(|cx| InputState::new(window, cx).placeholder("Commit summary"));
        let description = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("Description (optional)")
        });
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search history…"));
        let target = cx
            .new(|cx| InputState::new(window, cx).placeholder("Branch, revision or stash message"));
        let subscriptions = vec![
            cx.subscribe(&draft, |v, input, event: &InputEvent, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                v.state.draft = compose_message(
                    input.read(cx).value().as_ref(),
                    v.description.read(cx).value().as_ref(),
                );
                cx.notify();
            }),
            cx.subscribe(&description, |v, input, event: &InputEvent, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                v.state.draft = compose_message(
                    v.draft.read(cx).value().as_ref(),
                    input.read(cx).value().as_ref(),
                );
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
                if w.is_window_active() && !v.state.busy {
                    v.external_dirty = false;
                    v.refresh(&Refresh, w, cx);
                }
            }),
        ];
        let preferences = Preferences::load();
        gpui_component::Theme::global_mut(cx).font_size = px(preferences.font_size);
        let mut v = Self {
            state: Controller::default(),
            service: Service::new(),
            focus: cx.focus_handle(),
            draft,
            description,
            search,
            target,
            operation: None,
            split: preferences.split,
            compact: preferences.compact,
            dim_whitespace: false,
            active_hunk: 0,
            history_scroll: UniformListScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            selection: None,
            anchor: 0,
            _subscriptions: subscriptions,
            preferences,
            settings_open: false,
            saving: false,
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
        v.focus.focus(window);
        if let Some(path) = std::env::args().nth(1) {
            let mut job = v.state.open(path);
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
        let jobs = self.state.complete(lane, response);
        #[cfg(feature = "native-evidence")]
        if lane == Lane::Snapshot && !self.state.loading() {
            if let Some(start) = self.watch_start.take() {
                super::trace::record("external-event-to-state", start, cx);
            }
        }
        let (summary, body) = split_message(&self.state.draft);
        let summary = summary.to_string();
        let body = body.to_string();
        if summary != self.draft.read(cx).value().as_ref() {
            self.draft
                .update(cx, |s, cx| s.set_value(summary, window, cx));
        }
        if body != self.description.read(cx).value().as_ref() {
            self.description
                .update(cx, |s, cx| s.set_value(body, window, cx));
        }
        if lane == Lane::Snapshot && self.state.snapshot.is_some() {
            self.update_search(cx);
            self.install_watch(window, cx);
            if !self.preferences.recent.contains(&self.state.path) {
                self.preferences.recent.insert(0, self.state.path.clone());
                self.preferences.recent.truncate(10);
                self.persist_preferences(window, cx);
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
                    if !v.state.busy && w.is_window_active() {
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
            let Ok(preferences) = view.update_in(cx, |v, _, _| v.preferences.clone()) else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { preferences.save() })
                .await;
            if let Err(error) = result {
                let _ = view.update_in(cx, |v, _, cx| {
                    v.state.error = true;
                    v.state.notice = format!("Could not save appearance: {error}");
                    cx.notify();
                });
            }
        }));
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
        (self.actual_inspector_width[usize::from(self.state.diff_context.is_some())] - 32.)
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
            .child(icon(if loading { "refresh" } else { "check" }).text_color(rgb(ACCENT)))
            .child(div().text_size(type_size(cx, 16.)).child(if loading {
                "Loading changes…"
            } else if self.state.error {
                "Changes could not be loaded"
            } else if staged {
                "No staged changes to review"
            } else {
                "Working changes reviewed"
            }))
            .child(div().text_color(rgb(MUTED)).child(if loading {
                "Your selection is being updated."
            } else {
                "Choose another file, review staged changes, or create your commit."
            }))
            .child(
                self.button("review-history", "Return to history")
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
                if let Some(path) = self.state.files.first().cloned() {
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
    fn pick(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.busy || self.settings_open || window.has_active_dialog(cx) {
            return;
        }
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Git repository".into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = rx.await;
            let _ = view.update_in(cx, |v, window, cx| match result {
                Ok(Ok(Some(paths))) => {
                    if let Some(p) = paths.first() {
                        let j = v.state.open(p.to_string_lossy().into_owned());
                        v.dispatch(j, window, cx);
                    }
                }
                Ok(Err(e)) => {
                    v.state.notice = e.to_string();
                    v.state.error = true;
                    cx.notify();
                }
                _ => {}
            });
        })
        .detach();
    }
    fn refresh(&mut self, _: &Refresh, window: &mut Window, cx: &mut Context<Self>) {
        let j = self.state.refresh();
        self.dispatch(j, window, cx);
    }
    fn select(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(feature = "native-evidence")]
        {
            self.feedback_start = Some((std::time::Instant::now(), "selection-to-next-frame"));
        }
        self.selection = None;
        self.focus.focus(window);
        if let Some(snapshot) = &self.state.snapshot {
            if let Some(index) = id
                .as_ref()
                .and_then(|id| snapshot.commits.iter().position(|c| &c.id == id))
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
        let j = self.state.select(id);
        self.dispatch(j, window, cx);
    }
    fn file(&mut self, path: String, staged: bool, window: &mut Window, cx: &mut Context<Self>) {
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
                s.status.iter().any(|f| f.index != ' ' && f.index != '?')
                    && !s.status.iter().any(|f| {
                        f.index == 'U'
                            || f.worktree == 'U'
                            || matches!((f.index, f.worktree), ('A', 'A') | ('D', 'D'))
                    })
            })
    }
    fn commit_staged(&mut self, _: &CommitStaged, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open || window.has_active_dialog(cx) {
            return;
        }
        if self.can_commit() {
            self.mutate(
                Action::Commit {
                    message: self.state.draft.clone(),
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
        self.operation = Some(op.clone());
        self.target.update(cx, |s, cx| {
            s.set_value(target, window, cx);
        });
        let target = self.target.clone();
        let view = cx.weak_entity();
        let cancel = view.clone();
        let epoch = self.state.epoch;
        let repo = self.state.path.clone();
        let head = self.state.snapshot.as_ref().unwrap().head.clone();
        let label = match op {
            Operation::CreateBranch | Operation::CreateBranchAt { .. } => "Create branch",
            Operation::Checkout => "Checkout",
            Operation::Merge => "Merge",
            Operation::CherryPick => "Cherry-pick",
            Operation::Revert => "Revert",
            Operation::Stash => "Stash",
            Operation::PopStash => "Pop stash",
        };
        window.open_dialog(cx,move|dialog,_,cx|{
            let view=view.clone();let cancel=cancel.clone();let target=target.clone();let op=op.clone();
            dialog.title(label).confirm().button_props(gpui_component::dialog::DialogButtonProps::default().ok_text(label))
                .child(div().flex().flex_col().gap_3()
                    .child(div().p_3().rounded(px(6.)).bg(rgb(CANVAS)).border_1().border_color(rgb(BORDER))
                        .child(div().text_size(type_size(cx, 12.)).text_color(rgb(SUBTLE)).child("REPOSITORY"))
                        .child(div().mt_1().text_size(type_size(cx, 12.)).child(repo.clone()))
                        .child(div().mt_2().child(badge(format!("Current branch · {head}"), ACCENT))))
                    .child(div().font_weight(FontWeight::MEDIUM).child(match op {
                        Operation::CreateBranch | Operation::CreateBranchAt { .. } => "New branch name", Operation::Stash => "Stash message (optional)",
                        Operation::PopStash => "Latest stash", _ => "Target branch or commit" }))
                    .when(!matches!(op, Operation::PopStash), |e| e.child(Input::new(&target)))
                    .when_some(match &op { Operation::CreateBranchAt {start}=>Some(start.clone()), _=>None }, |e,start| e.child(format!("Starting at {}",start)))
                    .child(div().text_size(type_size(cx, 12.)).text_color(rgb(MUTED)).child("Review this operation before continuing. Conflicts will appear in Working changes.")))
                .on_ok(move|_,w,cx|view.update(cx,|v,cx|{
                    if v.state.epoch!=epoch {v.state.error=true;v.state.notice="Repository changed. Review this operation again.".into();v.operation=None;cx.notify();return true;}
                    if v.state.busy{return false;}
                    let target=v.target.read(cx).value().to_string();v.operation=None;v.mutate(Action::Operation{op:op.clone(),target},w,cx);true
                }).unwrap_or(true))
                .on_cancel(move|_,_,cx|{let _=cancel.update(cx,|v,cx|{v.operation=None;cx.notify();});true})
        });
        self.target.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
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
        Button::new(id)
            .label(label)
            .small()
            .ghost()
            .rounded(px(6.))
            .tab_stop(!self.settings_open)
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
                .bg(rgb(CHROME))
                .overflow_hidden()
                .text_color(rgb(MUTED))
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
    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let changes = self
            .state
            .snapshot
            .as_ref()
            .map(|s| s.status.len())
            .unwrap_or(0);
        let mut nav = div()
            .id("navigation")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .p_2()
            .gap_1()
            .overflow_y_scroll()
            .child(heading("REPOSITORY"))
            .child(
                self.button("open", "Open repository…")
                    .icon(icon("folder"))
                    .w_full()
                    .justify_start()
                    .h(px(32.))
                    .tooltip("Open a Git repository · Ctrl/Cmd+O")
                    .disabled(self.state.busy)
                    .on_click(cx.listener(|v, _, w, cx| v.pick(&Open, w, cx))),
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
                            .when(self.state.selected.is_none(), |b| {
                                b.bg(rgb(SELECTED)).text_color(rgb(ACCENT))
                            })
                            .on_click(cx.listener(|v, _, w, cx| v.select(None, w, cx))),
                    )
                    .child(badge(
                        changes.to_string(),
                        if changes > 0 { ACCENT } else { MUTED },
                    )),
            )
            .child(
                div()
                    .mt_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(heading("LOCAL BRANCHES"))
                    .child(
                        div()
                            .pr_2()
                            .text_size(type_size(cx, 12.))
                            .text_color(rgb(SUBTLE))
                            .child(
                                self.state
                                    .snapshot
                                    .as_ref()
                                    .map(|s| s.branches.len())
                                    .unwrap_or(0)
                                    .to_string(),
                            ),
                    ),
            );
        if let Some(s) = &self.state.snapshot {
            let mut groups = std::collections::BTreeSet::new();
            for (i, b) in s.branches.iter().enumerate() {
                let parts = b.split('/').collect::<Vec<_>>();
                let mut hidden = false;
                for depth in 1..parts.len() {
                    let prefix = parts[..depth].join("/");
                    let key = format!("{}|{}", self.state.path, prefix);
                    let collapsed = self.preferences.collapsed.contains(&key);
                    if groups.insert(prefix.clone()) {
                        let label =
                            format!("{} {}", if collapsed { "▸" } else { "▾" }, parts[depth - 1]);
                        nav = nav.child(
                            div().pl(px((depth - 1) as f32 * 12.)).child(
                                self.button(
                                    SharedString::from(format!("branch-group-{prefix}")),
                                    label,
                                )
                                .w_full()
                                .justify_start()
                                .h(px(28.))
                                .text_color(rgb(MUTED))
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
                let id = s
                    .commits
                    .iter()
                    .find(|c| c.refs.contains(b))
                    .map(|c| c.id.clone());
                let branch = b.clone();
                nav = nav.child(
                    self.button(("branch", i), parts.last().unwrap().to_string())
                        .icon(icon("branch"))
                        .w_full()
                        .justify_start()
                        .h(px(30.))
                        .ml(px((parts.len()-1) as f32 * 12.))
                        .text_color(rgb(MUTED))
                        .when(b == &s.head, |b| {
                            b.bg(rgb(RAISED))
                                .text_color(rgb(TEXT))
                                .font_weight(FontWeight::MEDIUM)
                        })
                        .when(id.is_some() && id == self.state.selected, |b| b.bg(rgb(SELECTED)).text_color(rgb(TEXT)))
                        .tooltip(format!("{} · Double-click to checkout", b))
                        .on_click(cx.listener(move |v, e: &ClickEvent, w, cx| {
                            if e.click_count() == 2 {
                                v.review(Operation::Checkout, branch.clone(), w, cx);
                            } else {
                                if let Some(id)=id.clone() { v.select(Some(id), w, cx); }
                                else { v.state.notice="Branch tip is outside loaded history. Use Load more commits to reveal it.".into();cx.notify(); }
                            }
                        })),
                );
            }
            nav = nav.child(div().mt_3().child(heading("TAGS & REFERENCES")));
            let mut seen = std::collections::HashSet::new();
            for c in &s.commits {
                for r in &c.refs {
                    if r == "HEAD" || s.branches.contains(r) || !seen.insert(r.clone()) {
                        continue;
                    }
                    let id = c.id.clone();
                    nav = nav.child(
                        self.button(
                            ("ref", seen.len()),
                            r.trim_start_matches("tag: ").to_string(),
                        )
                        .icon(icon("tag"))
                        .w_full()
                        .justify_start()
                        .h(px(30.))
                        .text_color(rgb(MUTED))
                        .tooltip(r.clone())
                        .on_click(
                            cx.listener(move |v, _, w, cx| v.select(Some(id.clone()), w, cx)),
                        ),
                    );
                }
            }
        }
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(CHROME))
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(BORDER))
            .overflow_hidden()
            .child(nav)
            .child(
                div()
                    .p_2()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        self.button("stash-pop", "Restore latest stash…")
                            .icon(icon("stash"))
                            .w_full()
                            .justify_start()
                            .disabled(self.state.busy || self.state.snapshot.is_none())
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
                                v.settings_open = true;
                                v.focus.focus(w);
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
    fn history(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.state.snapshot.is_none() {
            return panel()
                .items_center()
                .justify_center()
                .gap_3()
                .p_4()
                .child(
                    img(ImageSource::Resource(Resource::Embedded(
                        "gitcrab.png".into(),
                    )))
                    .size(px(52.)),
                )
                .child(
                    div()
                        .text_size(px(22.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(if self.state.loading() {
                            "Opening repository…"
                        } else {
                            "Open a repository"
                        }),
                )
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .text_size(type_size(cx, 12.))
                        .text_center()
                        .child("Browse history, review changes, and create your next commit."),
                )
                .child(
                    Button::new("welcome-open")
                        .primary()
                        .icon(icon("folder"))
                        .label("Open a repository")
                        .disabled(self.state.loading() || self.settings_open)
                        .on_click(cx.listener(|v, _, w, cx| v.pick(&Open, w, cx))),
                )
                .child(
                    div()
                        .text_size(type_size(cx, 12.))
                        .text_color(rgb(SUBTLE))
                        .child("Ctrl/Cmd+O  ·  Existing local Git repositories"),
                )
                .into_any_element();
        }
        let count = self.state.graph.rows.len();
        let weak = cx.weak_entity();
        panel()
            .child(
                div()
                    .h(px(26.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px_2()
                    .gap_2()
                    .bg(rgb(CHROME))
                    .text_size(type_size(cx, 12.))
                    .text_color(rgb(SUBTLE))
                    .font_weight(FontWeight::MEDIUM)
                    .child(
                        div()
                            .w(px(110.))
                            .flex_none()
                            .whitespace_nowrap()
                            .child("References"),
                    )
                    .child(
                        div()
                            .w(px((self.state.graph.lanes as f32 * 18. + 24.).max(92.)))
                            .flex_none()
                            .child("Graph"),
                    )
                    .child(div().flex_1().min_w_0().truncate().child("Message"))
                    .child(div().w(px(65.)).flex_none().child("SHA")),
            )
            .child(
                uniform_list("history", count + 1, move |range, w, cx| {
                    weak.update(cx, |v, cx| {
                        range.map(|i| v.history_row(i, w, cx)).collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
                })
                .track_scroll(self.history_scroll.clone())
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .flex_1()
                .min_h_0(),
            )
            .child(
                self.button(
                    "more",
                    if count < self.state.limit {
                        format!("All {count} commits loaded")
                    } else {
                        format!("Load 500 more commits · {count} shown")
                    },
                )
                .h(px(30.))
                .text_color(rgb(MUTED))
                .disabled(self.state.loading() || count < self.state.limit)
                .on_click(cx.listener(|v, _, w, cx| {
                    v.state.limit = (v.state.limit + 500).min(100_000);
                    v.refresh(&Refresh, w, cx);
                })),
            )
            .into_any_element()
    }
    fn history_row(&self, index: usize, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let height = if self.compact { 26. } else { 30. };
        if index == 0 {
            let graph_width = (self.state.graph.lanes as f32 * 18. + 24.).max(92.);
            let has_head = self
                .state
                .snapshot
                .as_ref()
                .is_some_and(|s| !s.head_id.is_empty());
            return div()
                .id("wip-row")
                .w_full()
                .h(px(height))
                .flex()
                .items_center()
                .px_2()
                .gap_2()
                .min_w(px(graph_width + 360.))
                .text_color(rgb(ACCENT))
                .bg(rgb(if self.state.selected.is_none() {
                    SELECTED
                } else {
                    PANEL
                }))
                .hover(|s| s.bg(rgb(HOVER)))
                .child(
                    div()
                        .w(px(110.))
                        .flex_none()
                        .text_size(type_size(cx, 12.))
                        .child(badge("WIP", ACCENT)),
                )
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, w, _| {
                            let x = bounds.origin.x + px(12.);
                            let y = bounds.origin.y + px(height / 2.);
                            if has_head {
                                w.paint_quad(fill(
                                    Bounds::new(
                                        point(x - px(0.8), y + px(5.)),
                                        size(px(1.6), px(height / 2. - 5.)),
                                    ),
                                    rgb(PALETTE[0]),
                                ));
                            }
                            w.paint_quad(
                                outline(
                                    Bounds::new(
                                        point(x - px(5.), y - px(5.)),
                                        size(px(10.), px(10.)),
                                    ),
                                    rgb(PALETTE[0]),
                                    BorderStyle::Dashed,
                                )
                                .corner_radii(px(5.)),
                            );
                        },
                    )
                    .w(px(graph_width))
                    .h(px(height))
                    .flex_none(),
                )
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::MEDIUM)
                        .child("Working changes"),
                )
                .child(badge(
                    self.state
                        .snapshot
                        .as_ref()
                        .map(|s| format!("{} files", s.status.len()))
                        .unwrap_or_else(|| "Clean".into()),
                    MUTED,
                ))
                .on_click(cx.listener(|v, _, w, cx| v.select(None, w, cx)))
                .into_any_element();
        }
        let i = index - 1;
        let Some(snap) = &self.state.snapshot else {
            return div().into_any_element();
        };
        let c = &snap.commits[i];
        let id = c.id.clone();
        let target = c.id.clone();
        let selected = self.state.selected.as_ref() == Some(&c.id);
        let search = self.search.read(cx).value().to_lowercase();
        let matched = search.is_empty()
            || c.subject.to_lowercase().contains(&search)
            || c.id.contains(&search)
            || c.author.to_lowercase().contains(&search);
        let row = self.state.graph.rows[i].clone();
        let mut incoming = i
            .checked_sub(1)
            .map(|p| self.state.graph.rows[p].edges.clone())
            .unwrap_or_default();
        if i == 0 && !snap.head_id.is_empty() {
            incoming.push(gitcrab_core::graph::Edge {
                from_lane: 0,
                to_lane: 0,
                color: 0,
                parent: snap.head_id.clone(),
            });
        }
        let graph_width = (self.state.graph.lanes as f32 * 18. + 24.).max(92.);
        let full_refs = c.refs.join(" · ");
        let ref_count = c.refs.iter().filter(|r| r.as_str() != "HEAD").count();
        let refs = c
            .refs
            .iter()
            .find(|r| r.as_str() != "HEAD" && !r.starts_with("HEAD ->"))
            .or(c.refs.first())
            .cloned()
            .unwrap_or_default();
        let refs = refs
            .trim_start_matches("tag: ")
            .trim_start_matches("HEAD -> ")
            .to_string();
        let color = row.color;
        let merge = c.parents.len() > 1;
        let has_refs = !c.refs.is_empty();
        let mut el = div()
            .id(("commit", i))
            .w_full()
            .h(px(height))
            .flex()
            .items_center()
            .min_w(px(graph_width + 360.))
            .px_2()
            .gap_2()
            .cursor_pointer()
            .overflow_hidden()
            .when(selected, |e| e.bg(rgb(SELECTED)))
            .hover(|s| s.bg(rgb(if selected { SELECTED } else { HOVER })))
            .text_color(rgb(if matched { TEXT } else { MUTED }))
            .child(
                div()
                    .id(("refs", i))
                    .w(px(110.))
                    .flex_none()
                    .truncate()
                    .text_size(type_size(cx, 12.))
                    .text_color(rgb(PALETTE[color % 8]))
                    .when(!refs.is_empty(), |e| {
                        e.child(badge(
                            if ref_count > 1 {
                                format!("{} +{}", refs, ref_count - 1)
                            } else {
                                refs
                            },
                            PALETTE[color % 8],
                        ))
                    })
                    .tooltip(move |w, cx| Tooltip::new(full_refs.clone()).build(w, cx)),
            )
            .child(
                canvas(
                    move |_, _, _| (),
                    move |bounds, _, w, _| {
                        let origin = bounds.origin;
                        let x = |lane: usize| origin.x + px(12. + lane as f32 * 18.);
                        let y = origin.y + px(height / 2.);
                        if has_refs {
                            w.paint_quad(fill(
                                Bounds::new(
                                    point(origin.x - px(8.), y - px(0.75)),
                                    size(px(20. + row.lane as f32 * 18.), px(1.5)),
                                ),
                                rgb(PALETTE[row.color % 8]),
                            ));
                        }
                        for (edge, above) in row
                            .edges
                            .iter()
                            .map(|e| (e, false))
                            .chain(incoming.iter().map(|e| (e, true)))
                        {
                            let mut path = PathBuilder::stroke(px(1.6));
                            let y0 = if above { y - px(height) } else { y };
                            let y1 = y0 + px(height);
                            path.move_to(point(x(edge.from_lane), y0));
                            path.cubic_bezier_to(
                                point(x(edge.to_lane), y1),
                                point(x(edge.from_lane), y0 + px(height * 0.5)),
                                point(x(edge.to_lane), y1 - px(height * 0.5)),
                            );
                            if let Ok(path) = path.build() {
                                w.paint_path(path, rgb(PALETTE[edge.color % 8]));
                            }
                        }
                        if selected || merge {
                            w.paint_quad(
                                outline(
                                    Bounds::new(
                                        point(x(row.lane) - px(7.), y - px(7.)),
                                        size(px(14.), px(14.)),
                                    ),
                                    rgb(PALETTE[row.color % 8]),
                                    BorderStyle::Solid,
                                )
                                .corner_radii(px(7.)),
                            );
                        }
                        w.paint_quad(
                            fill(
                                Bounds::new(
                                    point(x(row.lane) - px(4.), y - px(4.)),
                                    size(px(8.), px(8.)),
                                ),
                                rgb(PALETTE[row.color % 8]),
                            )
                            .corner_radii(px(4.)),
                        );
                    },
                )
                .w(px(graph_width))
                .h(px(height))
                .flex_none(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(selected, |e| e.font_weight(FontWeight::MEDIUM))
                    .child(c.subject.clone()),
            )
            .child(
                div()
                    .w(px(65.))
                    .flex_none()
                    .whitespace_nowrap()
                    .truncate()
                    .text_size(type_size(cx, 12.))
                    .font_family("monospace")
                    .text_color(rgb(SUBTLE))
                    .child(c.id[..8.min(c.id.len())].to_string()),
            )
            .on_click(cx.listener(move |v, _, w, cx| v.select(Some(id.clone()), w, cx)));
        if c.id == snap.head_id {
            el = el.border_l_2().border_color(rgb(ACCENT));
        }
        let view = cx.entity();
        el.context_menu(move |mut menu, _, _| {
            let sha = target.clone();
            menu = menu
                .item(
                    PopupMenuItem::new("Copy commit SHA").on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(sha.clone()))
                    }),
                )
                .separator();
            let create_view = view.clone();
            let start = target.clone();
            menu = menu
                .item(
                    PopupMenuItem::new("Create branch here…").on_click(move |_, w, cx| {
                        create_view.update(cx, |v, cx| {
                            v.review(
                                Operation::CreateBranchAt {
                                    start: start.clone(),
                                },
                                String::new(),
                                w,
                                cx,
                            )
                        })
                    }),
                )
                .separator();
            for (op, label) in [
                (Operation::Checkout, "Checkout…"),
                (Operation::Merge, "Merge…"),
                (Operation::CherryPick, "Cherry-pick…"),
                (Operation::Revert, "Revert…"),
            ] {
                let view = view.clone();
                let target = target.clone();
                menu = menu.item(
                    PopupMenuItem::new(format!(
                        "{} {}",
                        label.trim_end_matches('…'),
                        target.chars().take(8).collect::<String>()
                    ))
                    .on_click(move |_, w, cx| {
                        view.update(cx, |v, cx| v.review(op.clone(), target.clone(), w, cx))
                    }),
                );
            }
            menu
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
        let dialogs = gpui_component::Root::render_dialog_layer(window, cx);
        #[cfg(feature = "native-evidence")]
        if self.evidence_phase == 3 {
            self.evidence_phase = 4;
            let data = serde_json::json!({"loaded":self.state.graph.rows.len(),"view":std::env::var("GITCRAB_CAPTURE_VIEW").unwrap_or_else(|_|"history".into()),"width":f32::from(window.viewport_size().width),"height":f32::from(window.viewport_size().height),"scale":window.scale_factor(),"worker_ms":self.state.worker_ms,"diff_rows":self.state.patch.as_ref().map(|p|p.lines.len()),"dialog_layer":dialogs.is_some(),"renderer":"GPUI 0.2.2 native","present_time_measured":false});
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
        let title = self
            .state
            .path
            .rsplit(['/', '\\'])
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("Repository")
            .to_string();
        let in_diff = self.state.diff_context.is_some();
        let sidebar = if in_diff {
            div()
                .flex()
                .flex_col()
                .size_full()
                .gap_2()
                .child(
                    self.button("rail-back", "‹")
                        .on_click(cx.listener(|v, _, w, cx| v.back(&Back, w, cx))),
                )
                .child(
                    self.button("rail-working", "◇")
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
            self.history(cx)
        };
        let branch = self
            .state
            .snapshot
            .as_ref()
            .map(|s| s.head.clone())
            .unwrap_or_default();
        let recent = self.preferences.recent.clone();
        let menu_view = cx.entity();
        let mut toolbar = div()
            .h(px(42.))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div().w(px(210.)).flex_none().flex().items_center().gap_1()
                    .child(self.button("repository-selector", title.clone()).icon(icon("folder"))
                        .flex_1().min_w_0().justify_start().tooltip("Open repository · Ctrl/Cmd+O. Right-click for recent repositories.")
                        .disabled(self.state.busy).on_click(cx.listener(|v,_,w,cx|v.pick(&Open,w,cx))))
                    .context_menu(move |mut menu,_,_| {
                        for path in recent.iter() {
                            let path=path.clone(); let view=menu_view.clone();
                            menu=menu.item(PopupMenuItem::new(path.clone()).on_click(move|_,w,cx|view.update(cx,|v,cx|{
                                let job=v.state.open(path.clone());v.dispatch(job,w,cx);
                            })));
                        }
                        menu
                    }),
            )
            .child(
                self.button("refresh", "Refresh")
                    .icon(icon("refresh"))
                    .h(px(32.))
                    .tooltip("Refresh repository status · F5")
                    .disabled(self.state.loading())
                    .on_click(cx.listener(|v, _, w, cx| v.refresh(&Refresh, w, cx))),
            )
            .child(separator());
        let narrow_toolbar =
            f32::from(window.viewport_size().width) < 1150. || self.preferences.font_size > 15.;
        for (i, (op, label, glyph)) in [
            (Operation::CreateBranch, "Branch", "branch"),
            (Operation::Checkout, "Checkout", "history"),
            (Operation::Merge, "Merge", "merge"),
            (Operation::Stash, "Stash", "stash"),
        ]
        .into_iter()
        .enumerate()
        {
            toolbar = toolbar.child(
                self.button(("toolbar", i), if narrow_toolbar { "" } else { label })
                    .icon(icon(glyph))
                    .h(px(32.))
                    .tooltip(format!("{label} · Review before applying"))
                    .disabled(self.state.busy || self.state.snapshot.is_none())
                    .on_click(
                        cx.listener(move |v, _, w, cx| v.review(op.clone(), String::new(), w, cx)),
                    ),
            );
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
                        .text_color(rgb(MUTED))
                        .child(format!("{} matches", self.search_matches.len())),
                )
                .child(
                    self.button("search-prev", "↑")
                        .tooltip("Previous match · Shift+Enter")
                        .disabled(self.search_matches.is_empty())
                        .on_click(cx.listener(|v, _, w, cx| v.search_step(-1, w, cx))),
                )
                .child(
                    self.button("search-next", "↓")
                        .tooltip("Next match · Enter. Searches loaded history.")
                        .disabled(self.search_matches.is_empty())
                        .on_click(cx.listener(|v, _, w, cx| v.search_step(1, w, cx))),
                )
            });
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(CANVAS))
            .text_color(rgb(TEXT))
            .text_size(px(self.preferences.font_size))
            .track_focus(&self.focus)
            .key_context("GitCrab")
            .on_action(cx.listener(Self::pick))
            .on_action(cx.listener(Self::refresh))
            .on_action(cx.listener(Self::back))
            .on_action(cx.listener(|v, _: &Next, w, cx| v.move_selection(1, w, cx)))
            .on_action(cx.listener(|v, _: &Previous, w, cx| v.move_selection(-1, w, cx)))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::select_all))
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
            .child(toolbar)
            .when(self.state.error, |el| {
                el.child(
                    div()
                        .id("error-notice")
                        .max_h(px(90.))
                        .overflow_y_scroll()
                        .p_3()
                        .mx_2()
                        .mt_2()
                        .rounded(px(6.))
                        .bg(rgba(0xe06c7514))
                        .text_color(rgb(RED))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("Git could not complete this action"),
                                )
                                .child(self.button("dismiss-error", "Dismiss").on_click(
                                    cx.listener(|v, _, _, cx| {
                                        v.state.error = false;
                                        cx.notify();
                                    }),
                                )),
                        )
                        .child(
                            div()
                                .text_size(type_size(cx, 12.))
                                .child(self.state.notice.clone()),
                        ),
                )
            })
            .child(
                div().flex_1().min_h_0().p_2().child(
                    h_resizable(SharedString::from(format!(
                        "workspace-{in_diff}-{}",
                        self.layout_revision
                    )))
                    .with_state(&self.panels[usize::from(in_diff)])
                    .on_resize(cx.listener(
                        move |v,
                              state: &Entity<gpui_component::resizable::ResizableState>,
                              w,
                              cx| {
                            let sizes = state.read(cx).sizes();
                            if sizes.len() == 3 {
                                if !in_diff {
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
                            .size(px(if in_diff {
                                44.
                            } else {
                                self.preferences.nav_width
                            }))
                            .size_range(if in_diff {
                                px(44.)..px(45.)
                            } else {
                                px(150.)..px(330.)
                            })
                            .child(sidebar),
                    )
                    .child(resizable_panel().child(div().size_full().pl(px(6.)).child(center)))
                    .child(
                        resizable_panel()
                            .size(px(self.preferences.inspector_width))
                            .size_range(px(260.)..px(560.))
                            .child(div().size_full().pl(px(6.)).child(inspector)),
                    ),
                ),
            )
            .child(
                div()
                    .h(px(24.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .gap_2()
                    .text_size(type_size(cx, 12.))
                    .text_color(rgb(MUTED))
                    .child(div().size(px(5.)).rounded_full().bg(rgb(
                        if self.state.busy || self.state.loading() {
                            ACCENT
                        } else {
                            GREEN
                        },
                    )))
                    .child(div().flex_1().truncate().child(if self.state.busy {
                        "Running Git operation…".into()
                    } else if self.state.loading() {
                        "Loading repository…".into()
                    } else {
                        self.state.notice.clone()
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon("branch").small())
                            .child(if branch.is_empty() {
                                "No repository".into()
                            } else {
                                branch
                            }),
                    )
                    .when_some(self.watch_warning.clone(), |bar, warning| {
                        bar.child(
                            self.button("watch-warning", "Live refresh unavailable")
                                .text_color(rgb(0xe5c07b))
                                .tooltip(warning),
                        )
                    }),
            )
            .when(self.settings_open, |el| el.child(self.settings_dialog(cx)))
            .children(dialogs)
    }
}
