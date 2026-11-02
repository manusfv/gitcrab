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

actions!(
    gitcrab,
    [Open, Refresh, Back, Next, Previous, Copy, SelectAll, Quit]
);
pub struct GitCrab {
    state: Controller,
    service: Service,
    focus: FocusHandle,
    draft: Entity<InputState>,
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
    #[cfg(feature = "native-evidence")]
    evidence_phase: u8,
    #[cfg(feature = "native-evidence")]
    benchmark: (Option<std::time::Instant>, Vec<f64>),
}
impl GitCrab {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let draft = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("Summary, then optional description…")
        });
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search history…"));
        let target = cx
            .new(|cx| InputState::new(window, cx).placeholder("Branch, revision or stash message"));
        let subscriptions = vec![
            cx.subscribe(&draft, |v, input, _: &InputEvent, cx| {
                v.state.draft = input.read(cx).value().to_string();
                cx.notify();
            }),
            cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        let preferences = Preferences::load();
        gpui_component::Theme::global_mut(cx).font_size = px(preferences.font_size);
        let mut v = Self {
            state: Controller::default(),
            service: Service::new(),
            focus: cx.focus_handle(),
            draft,
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
            #[cfg(feature = "native-evidence")]
            evidence_phase: 0,
            #[cfg(feature = "native-evidence")]
            benchmark: (None, vec![]),
        };
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
        if self.state.draft != self.draft.read(cx).value().as_ref() {
            let text = self.state.draft.clone();
            self.draft.update(cx, |s, cx| s.set_value(text, window, cx));
        }
        for job in jobs {
            self.dispatch(Some(job), window, cx);
        }
        #[cfg(feature = "native-evidence")]
        self.advance_evidence(lane, window, cx);
        cx.notify();
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
        if self.state.busy {
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
        self.selection = None;
        self.focus.focus(window);
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
        let j = self.state.mutate(action);
        self.selection = None;
        self.dispatch(j, window, cx);
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
            s.focus(window, cx);
        });
        let target = self.target.clone();
        let view = cx.weak_entity();
        let cancel = view.clone();
        let epoch = self.state.epoch;
        let repo = self.state.path.clone();
        let head = self.state.snapshot.as_ref().unwrap().head.clone();
        let label = match op {
            Operation::CreateBranch => "Create branch",
            Operation::Checkout => "Checkout",
            Operation::Merge => "Merge",
            Operation::CherryPick => "Cherry-pick",
            Operation::Revert => "Revert",
            Operation::Stash => "Stash",
            Operation::PopStash => "Pop stash",
        };
        window.open_dialog(cx,move|dialog,_,_|{
            let view=view.clone();let cancel=cancel.clone();let target=target.clone();let op=op.clone();
            dialog.title(label).confirm().button_props(gpui_component::dialog::DialogButtonProps::default().ok_text(label))
                .child(div().flex().flex_col().gap_3().child(repo.clone()).child(format!("HEAD: {head}"))
                    .child(Input::new(&target)).child("Review the target. Git protections apply; failures can leave conflicts and always refresh status."))
                .on_ok(move|_,w,cx|view.update(cx,|v,cx|{
                    if v.state.epoch!=epoch {v.state.error=true;v.state.notice="Repository changed. Review this operation again.".into();v.operation=None;cx.notify();return true;}
                    if v.state.busy{return false;}
                    let target=v.target.read(cx).value().to_string();v.operation=None;v.mutate(Action::Operation{op:op.clone(),target},w,cx);true
                }).unwrap_or(true))
                .on_cancel(move|_,_,cx|{let _=cancel.update(cx,|v,cx|{v.operation=None;cx.notify();});true})
        });
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
        self.state.close_diff();
        self.focus.focus(window);
        cx.notify();
    }
    fn move_selection(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_focused_input(cx) || self.operation.is_some() || self.state.patch.is_some() {
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
        Button::new(id).label(label).small().ghost()
    }
    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut nav = div()
            .id("navigation")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(CHROME))
            .p_2()
            .gap_1()
            .overflow_y_scroll()
            .child(
                div()
                    .h(px(40.))
                    .px_2()
                    .flex()
                    .items_center()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("◉  GitCrab"),
            )
            .child(
                self.button("open", "Open repository…")
                    .disabled(self.state.busy)
                    .on_click(cx.listener(|v, _, w, cx| v.pick(&Open, w, cx))),
            )
            .child(
                self.button(
                    "working",
                    format!(
                        "◇  Working directory  {}",
                        self.state
                            .snapshot
                            .as_ref()
                            .map(|s| s.status.len())
                            .unwrap_or(0)
                    ),
                )
                .on_click(cx.listener(|v, _, w, cx| v.select(None, w, cx))),
            )
            .child(heading("LOCAL BRANCHES"));
        if let Some(s) = &self.state.snapshot {
            for (i, b) in s.branches.iter().enumerate() {
                let id = s
                    .commits
                    .iter()
                    .find(|c| c.refs.contains(b))
                    .map(|c| c.id.clone());
                let branch = b.clone();
                nav = nav.child(
                    self.button(
                        ("branch", i),
                        format!("{} {}", if b == &s.head { "●" } else { "⑂" }, b),
                    )
                    .on_click(cx.listener(move |v, e: &ClickEvent, w, cx| {
                        if e.click_count() == 2 {
                            v.review(Operation::Checkout, branch.clone(), w, cx);
                        } else {
                            v.select(id.clone(), w, cx);
                        }
                    })),
                );
            }
            nav = nav.child(heading("TAGS & REFERENCES"));
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
                            format!("◇ {}", r.trim_start_matches("tag: ")),
                        )
                        .on_click(
                            cx.listener(move |v, _, w, cx| v.select(Some(id.clone()), w, cx)),
                        ),
                    );
                }
            }
        }
        nav.child(div().flex_1())
            .child(heading("WORKSPACE"))
            .child(
                self.button("settings", "Appearance…")
                    .on_click(cx.listener(|v, _, w, cx| {
                        v.settings_open = true;
                        v.focus.focus(w);
                        cx.notify();
                    })),
            )
            .child(
                self.button(
                    "density",
                    if self.compact {
                        "Comfortable rows"
                    } else {
                        "Compact rows"
                    },
                )
                .on_click(cx.listener(|v, _, _, cx| {
                    v.compact = !v.compact;
                    cx.notify();
                })),
            )
            .child(
                self.button("stash-pop", "Pop latest stash…")
                    .disabled(self.state.busy)
                    .on_click(cx.listener(|v, _, w, cx| {
                        v.review(Operation::PopStash, String::new(), w, cx)
                    })),
            )
            .into_any_element()
    }
    fn history(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.state.graph.rows.len();
        let weak = cx.weak_entity();
        panel()
            .child(heading("HISTORY     ·     REFERENCES / GRAPH / MESSAGE"))
            .child(
                uniform_list("history", count + 1, move |range, w, cx| {
                    weak.update(cx, |v, cx| {
                        range.map(|i| v.history_row(i, w, cx)).collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
                })
                .track_scroll(self.history_scroll.clone())
                .flex_1()
                .min_h_0(),
            )
            .child(
                self.button("more", format!("{} loaded  ·  Load 500 more", count))
                    .disabled(self.state.loading() || count < self.state.limit)
                    .on_click(cx.listener(|v, _, w, cx| {
                        v.state.limit = (v.state.limit + 500).min(100_000);
                        v.refresh(&Refresh, w, cx);
                    })),
            )
            .into_any_element()
    }
    fn history_row(&self, index: usize, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let height = if self.compact { 24. } else { 30. };
        if index == 0 {
            return div()
                .id("wip-row")
                .h(px(height))
                .flex()
                .items_center()
                .px_3()
                .gap_3()
                .text_color(rgb(ACCENT))
                .hover(|s| s.bg(rgb(SELECTED)))
                .child("◌")
                .child("Working directory / index")
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
        let incoming = i
            .checked_sub(1)
            .map(|p| self.state.graph.rows[p].edges.clone())
            .unwrap_or_default();
        let graph_width = (self.state.graph.lanes as f32 * 18. + 24.).max(110.);
        let refs = c.refs.join(" · ");
        let full_refs = refs.clone();
        let color = row.color;
        let mut el = div()
            .id(("commit", i))
            .h(px(height))
            .flex()
            .items_center()
            .min_w_0()
            .px_2()
            .gap_2()
            .cursor_pointer()
            .overflow_hidden()
            .when(selected, |e| e.bg(rgb(SELECTED)))
            .hover(|s| s.bg(rgb(SELECTED)))
            .text_color(rgb(if matched { TEXT } else { MUTED }))
            .child(
                div()
                    .id(("refs", i))
                    .w(px(110.))
                    .flex_none()
                    .truncate()
                    .text_size(px(11.))
                    .text_color(rgb(PALETTE[color % 8]))
                    .child(refs)
                    .tooltip(move |w, cx| Tooltip::new(full_refs.clone()).build(w, cx)),
            )
            .child(
                canvas(
                    move |_, _, _| (),
                    move |bounds, _, w, _| {
                        let origin = bounds.origin;
                        let x = |lane: usize| origin.x + px(12. + lane as f32 * 18.);
                        let y = origin.y + px(height / 2.);
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
            .child(div().flex_1().min_w_0().truncate().child(c.subject.clone()))
            .child(
                div()
                    .w(px(65.))
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(c.id[..8.min(c.id.len())].to_string()),
            )
            .on_click(cx.listener(move |v, _, w, cx| v.select(Some(id.clone()), w, cx)));
        if c.id == snap.head_id {
            el = el.border_l_2().border_color(rgb(ACCENT));
        }
        let view = cx.entity();
        el.context_menu(move |mut menu, _, _| {
            for (op, label) in [
                (Operation::Checkout, "Checkout…"),
                (Operation::Merge, "Merge…"),
                (Operation::CherryPick, "Cherry-pick…"),
                (Operation::Revert, "Revert…"),
            ] {
                let view = view.clone();
                let target = target.clone();
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, w, cx| {
                    view.update(cx, |v, cx| v.review(op.clone(), target.clone(), w, cx))
                }));
            }
            menu
        })
        .into_any_element()
    }
    fn inspector(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut el = panel();
        if let Some(id) = &self.state.selected {
            if let Some(c) = self
                .state
                .snapshot
                .as_ref()
                .and_then(|s| s.commits.iter().find(|c| &c.id == id))
            {
                el = el
                    .child(heading("COMMIT DETAILS"))
                    .child(
                        div()
                            .px_3()
                            .pb_2()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(c.subject.clone()),
                    )
                    .child(
                        div()
                            .px_3()
                            .text_color(rgb(MUTED))
                            .child(c.author.clone())
                            .child(c.date.clone()),
                    )
                    .child(
                        self.button("copy-id", c.id.clone())
                            .on_click(cx.listener(|v, _, w, cx| v.copy(&Copy, w, cx))),
                    )
                    .child(div().px_3().text_size(px(11.)).child(format!(
                            "Parents: {}",
                            c.parents
                                .iter()
                                .map(|p| &p[..8])
                                .collect::<Vec<_>>()
                                .join(", ")
                        )))
                    .child(
                        div()
                            .px_3()
                            .text_size(px(11.))
                            .text_color(rgb(ACCENT))
                            .child(c.refs.join(" · ")),
                    )
                    .child(heading("FIRST-PARENT CHANGED FILES"));
                let weak = cx.weak_entity();
                el = el.child(
                    uniform_list(
                        "committed-files",
                        self.state.files.len(),
                        move |range, _, cx| {
                            weak.update(cx, |v, cx| {
                                range
                                    .map(|i| {
                                        let path = v.state.files[i].clone();
                                        div().h(px(30.)).child(
                                            v.button(("changed", i), path.clone()).on_click(
                                                cx.listener(move |v, _, w, cx| {
                                                    v.file(path.clone(), false, w, cx)
                                                }),
                                            ),
                                        )
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                        },
                    )
                    .flex_1()
                    .min_h_0(),
                );
                for (i, (op, label)) in [
                    (Operation::Checkout, "Checkout…"),
                    (Operation::CherryPick, "Cherry-pick…"),
                    (Operation::Revert, "Revert…"),
                ]
                .into_iter()
                .enumerate()
                {
                    let id = id.clone();
                    el = el.child(
                        self.button(("commit-op", i), label)
                            .disabled(self.state.busy)
                            .on_click(cx.listener(move |v, _, w, cx| {
                                v.review(op.clone(), id.clone(), w, cx)
                            })),
                    );
                }
            }
        } else {
            el = el.child(heading("WORKING DIRECTORY / INDEX"));
            for staged in [false, true] {
                el = el.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_2()
                        .child(heading(if staged { "STAGED" } else { "UNSTAGED" }))
                        .child(
                            self.button(
                                if staged { "unstage-all" } else { "stage-all" },
                                if staged { "Unstage all" } else { "Stage all" },
                            )
                            .disabled(self.state.busy)
                            .on_click(cx.listener(
                                move |v, _, w, cx| {
                                    v.mutate(
                                        Action::Stage {
                                            path: ".".into(),
                                            unstage: staged,
                                        },
                                        w,
                                        cx,
                                    )
                                },
                            )),
                        ),
                );
                let files = self
                    .state
                    .snapshot
                    .as_ref()
                    .map(|s| {
                        s.status
                            .iter()
                            .filter(|f| {
                                if staged {
                                    f.index != ' ' && f.index != '?'
                                } else {
                                    f.worktree != ' ' || f.index == '?'
                                }
                            })
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let weak = cx.weak_entity();
                let n = files.len();
                el = el.child(
                    uniform_list(
                        if staged {
                            "staged-files"
                        } else {
                            "unstaged-files"
                        },
                        n,
                        move |range, _, cx| {
                            weak.update(cx, |v, cx| {
                                range
                                    .map(|i| {
                                        let f = &files[i];
                                        let path = f.path.clone();
                                        let stage_path = path.clone();
                                        div()
                                            .h(px(30.))
                                            .flex()
                                            .items_center()
                                            .px_2()
                                            .gap_1()
                                            .child(
                                                div().flex_1().min_w_0().child(
                                                    v.button(
                                                        (
                                                            if staged {
                                                                "index"
                                                            } else {
                                                                "worktree"
                                                            },
                                                            i,
                                                        ),
                                                        format!(
                                                            "{} {}",
                                                            if staged {
                                                                f.index
                                                            } else {
                                                                f.worktree
                                                            },
                                                            path
                                                        ),
                                                    )
                                                    .on_click(cx.listener(move |v, _, w, cx| {
                                                        v.file(path.clone(), staged, w, cx)
                                                    })),
                                                ),
                                            )
                                            .child(
                                                v.button(
                                                    (if staged { "minus" } else { "plus" }, i),
                                                    if staged { "−" } else { "+" },
                                                )
                                                .disabled(v.state.busy)
                                                .on_click(cx.listener(move |v, _, w, cx| {
                                                    v.mutate(
                                                        Action::Stage {
                                                            path: stage_path.clone(),
                                                            unstage: staged,
                                                        },
                                                        w,
                                                        cx,
                                                    )
                                                })),
                                            )
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                        },
                    )
                    .h(px((n as f32 * 30.).clamp(30., 150.)))
                    .min_h_0(),
                );
            }
            let staged = self
                .state
                .snapshot
                .as_ref()
                .is_some_and(|s| s.status.iter().any(|f| f.index != ' ' && f.index != '?'));
            let conflict = self.state.snapshot.as_ref().is_some_and(|s| {
                s.status.iter().any(|f| {
                    f.index == 'U'
                        || f.worktree == 'U'
                        || matches!((f.index, f.worktree), ('A', 'A') | ('D', 'D'))
                })
            });
            el = el
                .child(div().flex_1())
                .child(heading("COMMIT"))
                .child(div().px_3().child(Input::new(&self.draft).h(px(100.))))
                .when(conflict, |el| {
                    el.child(
                        div()
                            .p_3()
                            .text_color(rgb(RED))
                            .child("Resolve conflicts with Git, then refresh."),
                    )
                })
                .child(
                    Button::new("commit")
                        .label("Commit staged changes")
                        .primary()
                        .m_3()
                        .disabled(
                            self.state.busy
                                || !staged
                                || conflict
                                || self.state.draft.trim().is_empty(),
                        )
                        .on_click(cx.listener(|v, _, w, cx| {
                            v.mutate(
                                Action::Commit {
                                    message: v.state.draft.clone(),
                                },
                                w,
                                cx,
                            )
                        })),
                );
        }
        el.into_any_element()
    }
    fn diff(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.state.patch.as_ref().unwrap();
        let staged = p.staged;
        let n = if self.split {
            p.split.len()
        } else {
            p.lines.len()
        };
        let weak = cx.weak_entity();
        let mut el = panel()
            .child(
                div()
                    .h(px(38.))
                    .flex()
                    .items_center()
                    .px_2()
                    .gap_2()
                    .child(
                        self.button("back", "‹ History")
                            .on_click(cx.listener(|v, _, w, cx| v.back(&Back, w, cx))),
                    )
                    .child(div().flex_1().truncate().child(p.path.clone()))
                    .child(
                        self.button(
                            "dim-whitespace",
                            if self.dim_whitespace {
                                "Normal whitespace"
                            } else {
                                "Dim whitespace"
                            },
                        )
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.dim_whitespace = !v.dim_whitespace;
                            cx.notify();
                        })),
                    )
                    .child(
                        self.button("split", if self.split { "Unified" } else { "Split" })
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.split = !v.split;
                                v.selection = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .h(px(34.))
                    .flex()
                    .items_center()
                    .px_2()
                    .gap_2()
                    .child(if p.id.is_some() {
                        "Committed · first parent"
                    } else if staged {
                        "Index → HEAD"
                    } else {
                        "Worktree → Index"
                    })
                    .when(p.id.is_none(), |el| {
                        el.child(
                            self.button(
                                "source",
                                if staged {
                                    "View unstaged"
                                } else {
                                    "View staged"
                                },
                            )
                            .on_click(cx.listener(
                                move |v, _, w, cx| {
                                    if let Some(p) = &v.state.patch {
                                        v.file(p.path.clone(), !staged, w, cx);
                                    }
                                },
                            )),
                        )
                        .child(
                            self.button(
                                "stage-file",
                                if staged { "Unstage file" } else { "Stage file" },
                            )
                            .disabled(self.state.busy)
                            .on_click(cx.listener(|v, _, w, cx| {
                                if let Some(p) = &v.state.patch {
                                    v.mutate(
                                        Action::Stage {
                                            path: p.path.clone(),
                                            unstage: p.staged,
                                        },
                                        w,
                                        cx,
                                    );
                                }
                            })),
                        )
                    }),
            );
        let path = p.path.clone();
        if !p.hunks.is_empty() {
            let active = self.active_hunk.min(p.hunks.len() - 1);
            let token = p.token;
            let unstage = p.staged;
            let mut bar = div().h(px(30.)).flex().items_center().px_2().gap_2();
            for (index, delta, label) in [(0, -1isize, "↑"), (1, 1isize, "↓")] {
                bar = bar.child(
                    self.button(("hunk-nav", index), label)
                        .disabled(if delta < 0 {
                            active == 0
                        } else {
                            active + 1 == p.hunks.len()
                        })
                        .on_click(cx.listener(move |v, _, _, cx| {
                            if let Some(p) = &v.state.patch {
                                v.active_hunk = (v.active_hunk as isize + delta)
                                    .clamp(0, p.hunks.len().saturating_sub(1) as isize)
                                    as usize;
                                let rows = if v.split {
                                    &p.split_hunk_rows
                                } else {
                                    &p.hunk_rows
                                };
                                if let Some(row) = rows.get(v.active_hunk) {
                                    v.diff_scroll.scroll_to_item(*row, ScrollStrategy::Top);
                                }
                                cx.notify();
                            }
                        })),
                );
            }
            el = el.child(
                bar.child(
                    self.button(
                        "active-hunk",
                        format!(
                            "{} hunk {} / {}",
                            if staged { "Unstage" } else { "Stage" },
                            active + 1,
                            p.hunks.len()
                        ),
                    )
                    .disabled(self.state.busy)
                    .on_click(cx.listener(move |v, _, w, cx| {
                        v.mutate(
                            Action::Hunk {
                                token,
                                index: active,
                                unstage,
                            },
                            w,
                            cx,
                        )
                    })),
                ),
            );
        }
        el=el.child(uniform_list(SharedString::from(format!("diff-{path}-{}",self.split)),n,move|range,_,cx|weak.update(cx,|v,cx|range.map(|i|v.diff_row(i,cx)).collect::<Vec<_>>()).unwrap_or_default()).with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained).track_scroll(self.diff_scroll.clone()).flex_1().min_h_0())
            .child(div().h(px(26.)).px_3().text_size(px(11.)).text_color(rgb(MUTED)).child("Click / Shift-click selects lines · Ctrl/Cmd+A selects all · Ctrl/Cmd+C copies"));
        el.into_any_element()
    }
    fn diff_row(&self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = self.state.patch.as_ref().unwrap();
        let selected = self.selection.as_ref().is_some_and(|r| r.contains(&i));
        let width = if self.split {
            (p.split_columns as f32 * 7.5 + 130.) * 2.
        } else {
            p.columns as f32 * 7.5 + 160.
        };
        let mut el = div()
            .id(("diff-line", i))
            .w_full()
            .min_w(px(width.max(if self.split { 950. } else { 600. })))
            .h(px(26.))
            .flex()
            .items_center()
            .font_family(if cfg!(windows) {
                "Consolas"
            } else {
                "DejaVu Sans Mono"
            })
            .text_size(px(12.))
            .overflow_hidden();
        let cells = if self.split {
            let (a, b) = &p.split[i];
            vec![a.as_ref(), b.as_ref()]
        } else {
            vec![Some(&p.lines[i])]
        };
        for line in cells {
            let mut cell = div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .gap_2()
                .px_2();
            if let Some(l) = line {
                cell = cell
                    .bg(rgb(if selected {
                        SELECTED
                    } else {
                        match l.kind {
                            '+' => 0x2d3a30,
                            '-' => 0x3e2c32,
                            '@' => 0x303747,
                            _ => PANEL,
                        }
                    }))
                    .child(
                        div()
                            .w(px(if self.split { 45. } else { 80. }))
                            .flex_none()
                            .text_color(rgb(MUTED))
                            .child(format!(
                                "{:>4} {:>4}",
                                l.old.map(|n| n.to_string()).unwrap_or_default(),
                                l.new.map(|n| n.to_string()).unwrap_or_default()
                            )),
                    )
                    .child({
                        let mut end = l.text.len().min(8192);
                        while !l.text.is_char_boundary(end) {
                            end -= 1;
                        }
                        let text = SharedString::from(l.text[..end].to_string());
                        let highlights = l.highlights.iter().filter(|s| s.end <= end).map(|s| {
                            (
                                s.start..s.end,
                                HighlightStyle {
                                    color: Some(
                                        rgb(match s.kind {
                                            'c' => MUTED,
                                            's' => GREEN,
                                            'k' => 0xc678dd,
                                            'n' => 0xd19a66,
                                            _ => TEXT,
                                        })
                                        .into(),
                                    ),
                                    ..Default::default()
                                },
                            )
                        });
                        div()
                            .when(self.dim_whitespace, |el| el.opacity(0.8))
                            .flex_none()
                            .child(StyledText::new(text).with_highlights(highlights))
                    });
                if l.kind == '@' && l.text.starts_with("@@ ") && p.id.is_none() {
                    let index = p.hunks.iter().position(|h| h == &l.text);
                    let token = p.token;
                    let unstage = p.staged;
                    if let Some(index) = index {
                        cell = cell.child(
                            self.button(
                                ("hunk", i),
                                if unstage {
                                    "Unstage hunk"
                                } else {
                                    "Stage hunk"
                                },
                            )
                            .disabled(self.state.busy)
                            .on_click(cx.listener(
                                move |v, _, w, cx| {
                                    v.mutate(
                                        Action::Hunk {
                                            token,
                                            index,
                                            unstage,
                                        },
                                        w,
                                        cx,
                                    )
                                },
                            )),
                        );
                    }
                }
            }
            el = el.child(cell);
        }
        el.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                v.focus.focus(w);
                if !e.modifiers.shift {
                    v.anchor = i;
                }
                v.selection = Some(v.anchor.min(i)..v.anchor.max(i) + 1);
                cx.notify();
            }),
        )
        .on_mouse_move(cx.listener(move |v, e: &MouseMoveEvent, _, cx| {
            if e.dragging() && v.selection.is_some() {
                v.selection = Some(v.anchor.min(i)..v.anchor.max(i) + 1);
                cx.notify();
            }
        }))
        .into_any_element()
    }
    fn settings_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut content = div()
            .w(px(430.))
            .rounded(px(10.))
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(BORDER))
            .p_5()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_lg().child("Appearance"))
            .child("Atom One Dark · native system fonts")
            .child(
                self.button(
                    "settings-density",
                    if self.compact {
                        "Density: Compact"
                    } else {
                        "Density: Comfortable"
                    },
                )
                .on_click(cx.listener(|v, _, _, cx| {
                    v.compact = !v.compact;
                    cx.notify();
                })),
            );
        for (index, label, value) in [
            (0, "Text size", self.preferences.font_size),
            (1, "Navigation width", self.preferences.nav_width),
            (2, "Inspector width", self.preferences.inspector_width),
        ] {
            let mut row = div()
                .flex()
                .items_center()
                .justify_between()
                .child(format!("{label}: {value:.0}"));
            for direction in [-1., 1.] {
                row = row.child(
                    self.button(
                        ("appearance", index * 2 + usize::from(direction > 0.)),
                        if direction > 0. { "+" } else { "−" },
                    )
                    .on_click(cx.listener(move |v, _, _, cx| {
                        match index {
                            0 => {
                                v.preferences.font_size =
                                    (v.preferences.font_size + direction).clamp(11., 18.);
                                gpui_component::Theme::global_mut(cx).font_size =
                                    px(v.preferences.font_size);
                            }
                            1 => {
                                v.preferences.nav_width =
                                    (v.preferences.nav_width + direction * 10.).clamp(150., 330.)
                            }
                            _ => {
                                v.preferences.inspector_width = (v.preferences.inspector_width
                                    + direction * 10.)
                                    .clamp(260., 560.)
                            }
                        };
                        cx.notify();
                    })),
                );
            }
            content = content.child(row);
        }
        content = content
            .child(
                "Panel widths can also be dragged. These controls provide keyboard alternatives.",
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        self.button("settings-close", "Close")
                            .on_click(cx.listener(|v, _, w, cx| v.back(&Back, w, cx))),
                    )
                    .child(
                        Button::new("save-appearance")
                            .label("Save appearance")
                            .primary()
                            .disabled(self.saving)
                            .on_click(cx.listener(|v, _, w, cx| {
                                v.preferences.compact = v.compact;
                                v.preferences.split = v.split;
                                let p = v.preferences.clone();
                                v.saving = true;
                                let task = cx.background_executor().spawn(async move { p.save() });
                                cx.spawn_in(w, async move |view, cx| {
                                    let result = task.await;
                                    let _ = view.update_in(cx, |v, _, cx| {
                                        v.saving = false;
                                        match result {
                                            Ok(()) => {
                                                v.state.notice = "Appearance saved".into();
                                                v.settings_open = false;
                                            }
                                            Err(e) => {
                                                v.state.error = true;
                                                v.state.notice = e;
                                            }
                                        }
                                        cx.notify();
                                    });
                                })
                                .detach();
                            })),
                    ),
            );
        div()
            .absolute()
            .inset_0()
            .bg(rgba(0x00000099))
            .flex()
            .items_center()
            .justify_center()
            .child(content)
            .into_any_element()
    }
}
impl Render for GitCrab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(feature = "native-evidence")]
        if self.evidence_phase == 3 {
            self.evidence_phase = 4;
            let data = serde_json::json!({"loaded":self.state.graph.rows.len(),"view":std::env::var("GITCRAB_CAPTURE_VIEW").unwrap_or_else(|_|"history".into()),"width":f32::from(window.viewport_size().width),"height":f32::from(window.viewport_size().height),"scale":window.scale_factor(),"worker_ms":self.state.worker_ms,"diff_rows":self.state.patch.as_ref().map(|p|p.lines.len()),"renderer":"GPUI 0.2.2 native","present_time_measured":false});
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
        let in_diff = self.state.patch.is_some();
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
        } else {
            self.history(cx)
        };
        let mut toolbar = div()
            .h(px(42.))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(title.clone()))
            .child(
                div().text_color(rgb(MUTED)).child(
                    self.state
                        .snapshot
                        .as_ref()
                        .map(|s| s.head.clone())
                        .unwrap_or_default(),
                ),
            )
            .child(
                self.button("refresh", "↻ Refresh")
                    .disabled(self.state.loading())
                    .on_click(cx.listener(|v, _, w, cx| v.refresh(&Refresh, w, cx))),
            );
        for (i, (op, label)) in [
            (Operation::CreateBranch, "Branch"),
            (Operation::Checkout, "Checkout"),
            (Operation::Merge, "Merge"),
            (Operation::Stash, "Stash"),
        ]
        .into_iter()
        .enumerate()
        {
            toolbar = toolbar.child(
                self.button(("toolbar", i), label)
                    .disabled(self.state.busy || self.state.snapshot.is_none())
                    .on_click(
                        cx.listener(move |v, _, w, cx| v.review(op.clone(), String::new(), w, cx)),
                    ),
            );
        }
        toolbar = toolbar
            .child(div().flex_1())
            .child(div().w(px(190.)).child(Input::new(&self.search).small()));
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
            .child(
                div()
                    .h(px(38.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px_3()
                    .gap_2()
                    .bg(rgb(CHROME))
                    .child(
                        div()
                            .w(px(190.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("◉ GitCrab"),
                    )
                    .child(
                        div()
                            .h(px(29.))
                            .px_3()
                            .rounded(px(7.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .bg(rgb(PANEL))
                            .flex()
                            .items_center()
                            .child(title),
                    )
                    .child(
                        self.button("tab-open", "+")
                            .on_click(cx.listener(|v, _, w, cx| v.pick(&Open, w, cx))),
                    ),
            )
            .child(toolbar)
            .when(self.state.error, |el| {
                el.child(
                    div()
                        .p_2()
                        .text_color(rgb(RED))
                        .child(self.state.notice.clone()),
                )
            })
            .child(
                div().flex_1().min_h_0().p_2().child(
                    h_resizable(SharedString::from(format!(
                        "workspace-{in_diff}-{}-{}",
                        self.preferences.nav_width, self.preferences.inspector_width
                    )))
                    .on_resize(cx.listener(
                        move |v,
                              state: &Entity<gpui_component::resizable::ResizableState>,
                              _,
                              cx| {
                            let sizes = state.read(cx).sizes();
                            if sizes.len() == 3 {
                                if !in_diff {
                                    v.preferences.nav_width = f32::from(sizes[0]).clamp(150., 330.);
                                }
                                v.preferences.inspector_width =
                                    f32::from(sizes[2]).clamp(260., 560.);
                            }
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
                    .child(resizable_panel().child(center))
                    .child(
                        resizable_panel()
                            .size(px(self.preferences.inspector_width))
                            .size_range(px(260.)..px(560.))
                            .child(inspector),
                    ),
                ),
            )
            .child(
                div()
                    .h(px(26.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(if self.state.busy {
                        "Running Git operation…".into()
                    } else {
                        self.state.notice.clone()
                    })
                    .child(format!("GPUI · Git worker {:.1} ms", self.state.worker_ms)),
            )
            .when(self.settings_open, |el| el.child(self.settings_dialog(cx)))
    }
}
