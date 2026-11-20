use super::components;
use super::{
    components::{
        prelude::{Picker, PickerEntry, PickerItem},
        ButtonExt as _,
    },
    theme::{Role, *},
    view::{GitCrab, Open, Quit},
};
use gitcrab_core::presentation::{display_path, path_labels};
use gpui::component::{button::*, tooltip::Tooltip, Sizable, WindowExt};
use gpui::{prelude::*, *};
use std::{cell::RefCell, rc::Rc};

use components::rail::RailDrag;

actions!(
    gitcrab,
    [
        NewTab,
        CloseTab,
        NextTab,
        PreviousTab,
        MoveTabLeft,
        MoveTabRight
    ]
);

#[derive(Clone, PartialEq, Debug, serde::Deserialize, Action)]
#[action(namespace = gitcrab, no_json)]
pub struct OpenRecent(pub String);

#[derive(Clone)]
struct DraggedTab {
    workspace: EntityId,
    tab: EntityId,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn tab_chrome(selected: bool) -> Div {
    div()
        .relative()
        .flex_none()
        .flex()
        .items_center()
        .rounded(px(4.))
        .border_b_1()
        .border_role(if selected {
            Role::ActionPrimary
        } else {
            Role::SurfaceChrome
        })
        .bg_role(if selected {
            Role::SurfaceOverlay
        } else {
            Role::SurfaceChrome
        })
}

/// The tab's name. It is part of the tab, which handles clicks, drags and focus.
fn tab_label(label: String) -> Div {
    div()
        .h(scaled(28.))
        .pl(scaled(10.))
        .pr(scaled(2.))
        .flex()
        .items_center()
        .child(
            div()
                .max_w(px(170.))
                .truncate()
                .text_size(scaled(14.))
                .line_height(scaled(20.))
                .child(label),
        )
}

/// The tab's close mark. It is not a separate focus stop: the tab is one control.
fn tab_close(id: impl Into<ElementId>, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .w(scaled(24.))
        .h(scaled(24.))
        .mr(scaled(4.))
        .rounded(px(4.))
        .text_size(scaled(16.))
        .line_height(scaled(16.))
        .map(|el| {
            if enabled {
                el.text_role(Role::TextSecondary).hover(|s| {
                    s.bg_role(Role::InteractionHover)
                        .text_role(Role::TextDefault)
                })
            } else {
                el.text_role(Role::TextDisabled)
            }
        })
        .child("×")
}

/// The window's repository tabs and their shared open and close actions.
pub struct Workspace {
    tabs: Vec<Entity<GitCrab>>,
    last_session: Option<Vec<String>>,
    active: usize,
    picking: bool,
    picker_error: Option<String>,
    confirming_draft_loss: bool,
    scroll: ScrollHandle,
    tab_drag: Option<RailDrag<EntityId>>,
    tab_drag_start: Option<(EntityId, Pixels)>,
    /// One keyboard focus stop per tab, kept across renders.
    tab_focus: std::collections::HashMap<EntityId, FocusHandle>,
    /// Redraws the update notice when a check finishes.
    _updates: Subscription,
}

/// The notice announcing an available release, above the repository panel.
/// It never blocks the window: the user can act on it or close it.
fn update_notice(
    update: crate::updates::AvailableUpdate,
    cx: &mut Context<Workspace>,
) -> Stateful<Div> {
    let button = |id: &'static str, label: &'static str| {
        components::basic(
            Button::new(id).label(label),
            components::ButtonKind::Secondary,
            components::ControlSize::Compact,
        )
        .tab_stop(true)
    };
    let url = update.release_url.clone();
    let skip = update.version.clone();
    let dismiss = update.version.clone();
    div()
        .id("update-notice")
        .flex_none()
        .mx_2()
        .mt_2()
        .p_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(CONTROL_RADIUS))
        .border_1()
        .border_role(Role::StatusInfoFg)
        .bg_role(Role::StatusInfoBg)
        .child(
            div()
                .size(px(8.))
                .flex_none()
                .rounded_full()
                .bg_role(Role::StatusInfoFg),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .text_role(Role::TextDefault)
                .child(format!("GitCrab {} is available", update.version)),
        )
        .child(
            button("update-release-notes", "Release notes")
                .tooltip(update.release_url.clone())
                .on_click(move |_, _, cx| cx.open_url(&url)),
        )
        .child(
            button("update-skip", "Skip this version")
                .tooltip("Don't announce this version again")
                .on_click(cx.listener(move |_, _, _, cx| {
                    crate::updates::skip(&skip, cx);
                    cx.notify();
                })),
        )
        .child(
            Button::new("update-dismiss")
                .ghost()
                .small()
                .tab_stop(true)
                .w(scaled(28.))
                .h(scaled(28.))
                .child(
                    div()
                        .text_size(scaled(16.))
                        .line_height(scaled(16.))
                        .child("×"),
                )
                .tooltip("Close until GitCrab restarts")
                .on_click(cx.listener(move |_, _, _, cx| {
                    crate::updates::dismiss(&dismiss, cx);
                    cx.notify();
                })),
        )
}

impl Workspace {
    /// Starts with the command-line worktree or the last saved session path.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (path, picker_error) = match std::env::args_os().nth(1).map(|arg| arg.into_string()) {
            Some(Ok(path)) => (Some(path), None),
            Some(Err(_)) => (
                None,
                Some("Repository folder names must be valid Unicode on this platform.".into()),
            ),
            None => (None, None),
        };
        let paths = match path {
            Some(path) => vec![path],
            None if picker_error.is_none() => crate::preferences::session_repositories(cx),
            None => vec![],
        };
        let mut tabs: Vec<_> = paths
            .into_iter()
            .enumerate()
            .map(|(i, path)| Self::create_tab(Some(path), i == 0, window, cx))
            .collect();
        if tabs.is_empty() {
            tabs.push(Self::create_tab(None, true, window, cx));
        }
        Self {
            tabs,
            last_session: None,
            active: 0,
            picking: false,
            picker_error,
            confirming_draft_loss: false,
            scroll: ScrollHandle::new(),
            tab_drag: None,
            tab_drag_start: None,
            tab_focus: Default::default(),
            _updates: cx.observe_global::<crate::updates::Updates>(|_, cx| cx.notify()),
        }
    }

    fn create_tab(
        path: Option<String>,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<GitCrab> {
        let tab = cx.new(|cx| GitCrab::new(path, active, window, cx));
        cx.observe(&tab, |v, _, cx| {
            v.save_session(cx);
            cx.notify();
        })
        .detach();
        tab
    }

    /// Persists the non-empty tabs, in order, so the next launch reopens them.
    fn save_session(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<String> = self
            .tabs
            .iter()
            .map(|tab| tab.read(cx).repository_path().to_owned())
            .filter(|path| !path.is_empty())
            .collect();
        if self.last_session.as_ref() == Some(&paths) {
            return;
        }
        // Skip clearing the saved session until a repository has opened, so a
        // launch that starts with an empty tab keeps the previous session.
        if paths.is_empty() && self.last_session.is_none() {
            return;
        }
        if crate::preferences::remember_repositories(&paths, cx).is_ok() {
            self.last_session = Some(paths);
        }
    }

    fn blocked(&self, window: &mut Window, cx: &mut App) -> bool {
        self.picking
            || self.confirming_draft_loss
            || self.tabs[self.active].read(cx).awaiting_trust
            || self.tabs[self.active].read(cx).settings_open()
            || window.has_active_dialog(cx)
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index != self.active {
            self.tabs[self.active].update(cx, |tab, _| tab.deactivate());
            self.active = index;
        }
        self.tabs[index].update(cx, |tab, cx| tab.activate(window, cx));
        self.scroll.scroll_to_item(index);
        cx.notify();
    }

    fn add(&mut self, path: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.push(Self::create_tab(path, false, window, cx));
        self.activate(self.tabs.len() - 1, window, cx);
    }

    fn close(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked(window, cx) || self.tabs[index].read(cx).mutating() {
            return;
        }
        if self.tabs[index].read(cx).has_drafts() {
            let id = self.tabs[index].entity_id();
            self.confirm_discard(
                "Close this tab and discard its drafts?",
                window,
                cx,
                move |v, w, cx| {
                    if let Some(index) = v.tabs.iter().position(|tab| tab.entity_id() == id) {
                        v.close_after_flush(index, w, cx);
                    }
                },
            );
            return;
        }
        self.close_after_flush(index, window, cx);
    }

    fn close_after_flush(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.tabs[index].entity_id();
        let identity = self.tabs[index].read(cx).close_identity();
        self.after_persistence(window, cx, move |v, w, cx| {
            if let Some(index) = v.tabs.iter().position(|tab| tab.entity_id() == id) {
                let tab = v.tabs[index].read(cx);
                if tab.mutating() || tab.close_identity() != identity {
                    v.picker_error = Some(
                        "The tab changed while closing. Review it before closing again.".into(),
                    );
                } else {
                    v.close_now(index, w, cx);
                }
            }
        });
    }

    fn close_now(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.remove(index);
        self.save_session(cx);
        if self.tabs.is_empty() {
            self.tabs.push(Self::create_tab(None, true, window, cx));
        } else if index < self.active {
            self.active -= 1;
        } else if index == self.active {
            self.active = index.min(self.tabs.len() - 1);
            self.activate(self.active, window, cx);
        }
        cx.notify();
    }

    fn confirm_discard(
        &mut self,
        message: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        confirmed: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        self.confirm_close(
            message,
            "Commit drafts are kept in memory for the lifetime of their tab.",
            "Discard drafts",
            window,
            cx,
            confirmed,
        );
    }

    fn confirm_close_without_saving(
        &mut self,
        error: String,
        window: &mut Window,
        cx: &mut Context<Self>,
        confirmed: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let detail = format!("Could not save settings: {error}");
        self.picker_error = Some(detail.clone());
        self.confirm_close(
            "Close without saving settings?",
            &detail,
            "Close without saving",
            window,
            cx,
            confirmed,
        );
    }

    fn confirm_close(
        &mut self,
        message: &str,
        detail: &str,
        confirm_label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        confirmed: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        self.confirming_draft_loss = true;
        let answer = window.prompt(
            PromptLevel::Warning,
            message,
            Some(detail),
            &["Cancel", confirm_label],
            cx,
        );
        cx.spawn_in(window, async move |workspace, cx| {
            let accepted = answer.await == Ok(1);
            let _ = workspace.update_in(cx, |v, w, cx| {
                v.confirming_draft_loss = false;
                if accepted {
                    confirmed(v, w, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn after_persistence(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        finished: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let result = self
            .tabs
            .iter()
            .try_for_each(|tab| tab.update(cx, |tab, cx| tab.flush_preferences(cx)))
            .and_then(|()| crate::preferences::flush(cx));
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.confirm_close_without_saving(error, window, cx, finished);
                return;
            }
        };
        self.confirming_draft_loss = true;
        cx.spawn_in(window, async move |workspace, cx| {
            let result = result
                .await
                .unwrap_or_else(|_| Err("Settings writer closed.".into()));
            let _ = workspace.update_in(cx, |v, w, cx| {
                v.confirming_draft_loss = false;
                match result {
                    Ok(()) => finished(v, w, cx),
                    Err(error) => v.confirm_close_without_saving(error, w, cx, finished),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Handles a window close request while keeping the window alive.
    /// The deferred close checks for writes, confirms draft loss and flushes settings.
    pub fn should_close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.blocked(window, cx) || self.tabs.iter().any(|tab| tab.read(cx).mutating()) {
            return false;
        }
        if self.tabs.iter().any(|tab| tab.read(cx).has_drafts()) {
            self.confirm_discard(
                "Quit GitCrab and discard all drafts?",
                window,
                cx,
                |v, w, cx| v.quit_after_flush(w, cx),
            );
            false
        } else {
            self.quit_after_flush(window, cx);
            false
        }
    }

    fn quit_after_flush(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.remember_window_size(window, cx);
        let identities: Vec<_> = self
            .tabs
            .iter()
            .map(|tab| (tab.entity_id(), tab.read(cx).close_identity()))
            .collect();
        self.after_persistence(window, cx, move |v, w, cx| {
            let current: Vec<_> = v
                .tabs
                .iter()
                .map(|tab| (tab.entity_id(), tab.read(cx).close_identity()))
                .collect();
            if current != identities || v.tabs.iter().any(|tab| tab.read(cx).mutating()) {
                v.picker_error =
                    Some("A tab changed while quitting. Review it before quitting again.".into());
            } else {
                w.remove_window();
            }
        });
    }

    /// Queues the window's size so the next launch opens at it. The flush that
    /// follows waits for this write. Full screen keeps the windowed size.
    fn remember_window_size(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let bounds = window.window_bounds();
        let size = bounds.get_bounds().size;
        let saved = crate::preferences::WindowSize {
            width: f32::from(size.width),
            height: f32::from(size.height),
            maximized: matches!(bounds, WindowBounds::Maximized(_)),
        };
        if let Err(error) = crate::preferences::remember_window(saved, cx) {
            self.picker_error = Some(format!("Could not save the window size: {error}"));
        }
    }

    fn cycle(&mut self, direction: isize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.blocked(window, cx) {
            let index = (self.active as isize + direction).rem_euclid(self.tabs.len() as isize);
            self.activate(index as usize, window, cx);
        }
    }

    /// Moves the active tab one position left (`-1`) or right (`1`), the keyboard
    /// counterpart of dragging it in the strip.
    fn move_active_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked(window, cx) {
            return;
        }
        let from = self.active;
        let slot = match delta {
            -1 if from > 0 => from - 1,
            1 if from + 1 < self.tabs.len() => from + 2,
            _ => return,
        };
        if gitcrab_core::tabs::move_to_slot(&mut self.tabs, &mut self.active, from, slot) {
            self.scroll.scroll_to_item(self.active);
            cx.notify();
        }
    }

    fn order_tabs(&mut self, dragged: EntityId, order: &[EntityId]) {
        let Some(from) = self.tabs.iter().position(|tab| tab.entity_id() == dragged) else {
            return;
        };
        let Some(to) = order.iter().position(|id| *id == dragged) else {
            return;
        };
        let slot = to + usize::from(from < to);
        gitcrab_core::tabs::move_to_slot(&mut self.tabs, &mut self.active, from, slot);
    }

    fn move_tab_drag(&mut self, pointer: Pixels, cx: &mut Context<Self>) {
        let Some(rail) = self.tab_drag.as_mut().filter(|rail| !rail.released) else {
            return;
        };
        if rail.update(pointer, std::time::Instant::now()) {
            let dragged = rail.dragged;
            let order: Vec<_> = rail.order().collect();
            self.order_tabs(dragged, &order);
        }
        cx.notify();
    }

    fn finish_tab_drag(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.tab_drag.as_ref().is_some_and(|rail| !rail.released) {
            self.move_tab_drag(event.position.x, cx);
            self.tab_drag
                .as_mut()
                .unwrap()
                .release(std::time::Instant::now());
        }
    }

    fn cancel_tab_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_drag.as_ref().is_some_and(|rail| !rail.released) {
            let rail = self.tab_drag.take().unwrap();
            self.order_tabs(rail.dragged, rail.original());
            cx.stop_active_drag(window);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn open_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.picking = true;
        let backend = crate::backend::get(cx);
        let task = cx
            .background_executor()
            .spawn(async move { backend.discover(std::path::Path::new(&path)) });
        cx.spawn_in(window, async move |workspace, cx| {
            let result = task.await;
            let _ = workspace.update_in(cx, |v, w, cx| {
                v.picking = false;
                match result {
                    Ok(location) => v.open_identity(location, w, cx),
                    Err(error) => v.picker_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_identity(
        &mut self,
        location: gitcrab_core::platform::RepositoryLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.read(cx).repository_identity() == Some(location.worktree.as_path()))
        {
            self.activate(index, window, cx);
            return;
        }
        if self.tabs[self.active].read(cx).has_repository() {
            self.add(None, window, cx);
        }
        self.tabs[self.active].update(cx, |tab, cx| tab.open_repository(location, window, cx));
    }

    fn pick(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked(window, cx) {
            return;
        }
        self.picking = true;
        self.picker_error = None;
        cx.notify();
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Git repository".into()),
        });
        cx.spawn_in(window, async move |workspace, cx| {
            let result = rx.await;
            let _ = workspace.update_in(cx, |v, w, cx| {
                v.picking = false;
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            match path.into_os_string().into_string() {
                                Ok(path) => v.open_path(path, w, cx),
                                Err(_) => v.picker_error = Some("Repository folder names must be valid Unicode on this platform.".into()),
                            }
                        }
                    }
                    Ok(Err(error)) => v.picker_error = Some(error.to_string()),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let blocked = self.blocked(window, cx);
        let workspace_id = cx.entity_id();
        let now = std::time::Instant::now();
        if self.tab_drag.as_ref().is_some_and(|rail| rail.settled(now)) {
            self.tab_drag = None;
        }
        if self
            .tab_drag
            .as_ref()
            .is_some_and(|rail| rail.animating(now))
        {
            let weak = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                let _ = weak.update(cx, |_, cx| cx.notify());
            });
            window.request_animation_frame();
        }
        let drop_bounds = Rc::new(RefCell::new(
            self.tabs
                .iter()
                .map(|tab| (tab.entity_id(), Bounds::<Pixels>::default()))
                .collect::<Vec<_>>(),
        ));

        let mut strip = div()
            .id("repository-tabs")
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(4.))
            .overflow_x_scroll()
            .track_scroll(&self.scroll)
            .on_drag_move(
                cx.listener(move |v, event: &DragMoveEvent<DraggedTab>, w, cx| {
                    if event.drag(cx).workspace != cx.entity_id() || v.blocked(w, cx) {
                        return;
                    }
                    if v.tab_drag
                        .as_ref()
                        .is_none_or(|rail| rail.dragged != event.drag(cx).tab)
                    {
                        return;
                    }
                    if cx.active_drag_cursor_style() != Some(CursorStyle::ClosedHand) {
                        cx.set_active_drag_cursor_style(CursorStyle::ClosedHand, w);
                    }
                    v.move_tab_drag(event.event.position.x, cx);
                }),
            );
        // Same-named repositories show their shortest distinguishing path suffix.
        let paths: Vec<String> = self
            .tabs
            .iter()
            .map(|tab| tab.read(cx).repository_path().to_owned())
            .collect();
        let named: Vec<String> = paths.iter().filter(|p| !p.is_empty()).cloned().collect();
        let mut named_labels = path_labels(&named).into_iter();
        let tab_labels: Vec<(String, String)> = paths
            .iter()
            .map(|path| {
                let label = if path.is_empty() {
                    None
                } else {
                    named_labels.next()
                };
                match label {
                    Some(label) => (label.suffix, display_path(path)),
                    None => ("New tab".to_string(), "New tab".to_string()),
                }
            })
            .collect();
        let tab_ids: Vec<EntityId> = self.tabs.iter().map(|tab| tab.entity_id()).collect();
        self.tab_focus.retain(|id, _| tab_ids.contains(id));
        for id in &tab_ids {
            self.tab_focus
                .entry(*id)
                .or_insert_with(|| cx.focus_handle().tab_stop(true));
        }
        for (index, entity) in self.tabs.iter().enumerate() {
            let tab_id = entity.entity_id();
            let focus = self.tab_focus[&tab_id].clone();

            let tab = entity.read(cx);
            let (mut label, tooltip) = tab_labels[index].clone();
            if tab.unread_result() {
                label = format!("● {label}");
            }
            let selected = index == self.active;
            let closable = !blocked && !tab.mutating();
            // The name, close mark and padding form one tab: it takes the
            // focus, drags and clicks as a whole, like a browser tab.
            strip = strip.child(components::rail::item(
                tab_chrome(selected)
                    .id(("repository-tab", tab_id))
                    .track_focus(&focus)
                    .when(focus.is_focused(window), |el| {
                        // One ring around the whole tab.
                        el.child(
                            div()
                                .absolute()
                                .inset_0()
                                .rounded(px(4.))
                                .border_2()
                                .border_role(Role::InteractionFocus),
                        )
                    })
                    .tooltip({
                        let tooltip = tooltip.clone();
                        move |w, cx| Tooltip::new(tooltip.clone()).build(w, cx)
                    })
                    .on_click(cx.listener(move |v, _, w, cx| {
                        if !cx.has_active_drag() && !v.blocked(w, cx) {
                            v.activate(index, w, cx);
                        }
                    }))
                    .on_key_down(cx.listener(move |v, event: &KeyDownEvent, w, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space")
                            && !event.keystroke.modifiers.modified()
                        {
                            cx.stop_propagation();
                            if !v.blocked(w, cx) {
                                v.activate(index, w, cx);
                            }
                        }
                    }))
                    .when(!blocked, |el| {
                        el.cursor_grab().active(|el| el.cursor_grabbing())
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |v, event: &MouseDownEvent, _, _| {
                            v.tab_drag_start = Some((tab_id, event.position.x));
                        }),
                    )
                    .when(!blocked, |el| {
                        let weak = cx.weak_entity();
                        let bounds = drop_bounds.clone();
                        el.on_drag(
                            DraggedTab {
                                workspace: workspace_id,
                                tab: tab_id,
                            },
                            move |drag, _, w, cx| {
                                let _ = weak.update(cx, |v, cx| {
                                    let pointer = v
                                        .tab_drag_start
                                        .filter(|(id, _)| *id == tab_id)
                                        .map(|(_, x)| x)
                                        .unwrap_or(w.mouse_position().x);
                                    v.tab_drag =
                                        RailDrag::new(&bounds.borrow(), tab_id, pointer, 4.);
                                    v.move_tab_drag(w.mouse_position().x, cx);
                                });
                                cx.stop_propagation();
                                cx.new(|_| drag.clone())
                            },
                        )
                    })
                    // Middle click closes the tab, as in browsers and editors,
                    // without focusing it first.
                    .capture_any_mouse_down(|event, window, _| {
                        if event.button == MouseButton::Middle {
                            window.prevent_default();
                        }
                    })
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(move |v, _, w, cx| {
                            if !cx.has_active_drag() {
                                v.close(index, w, cx);
                            }
                        }),
                    )
                    .child({
                        let drop_bounds = drop_bounds.clone();
                        canvas(
                            move |bounds, _, _| {
                                drop_bounds.borrow_mut()[index].1 = bounds;
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full()
                    })
                    .child(
                        tab_label(label.clone())
                            .when(blocked, |el| el.text_role(Role::TextDisabled)),
                    )
                    .child(
                        tab_close(("close-repository-tab", tab_id), closable)
                            .tooltip({
                                let text = format!("Close {label} tab");
                                move |w, cx| Tooltip::new(text.clone()).build(w, cx)
                            })
                            .on_click(cx.listener(move |v, _, w, cx| {
                                // The close mark acts alone; the tab's own
                                // click would select a tab that is closing.
                                cx.stop_propagation();
                                if !cx.has_active_drag() {
                                    v.close(index, w, cx);
                                }
                            })),
                    ),
                self.tab_drag.as_ref(),
                tab_id,
                now,
            ));
        }
        strip = strip.child(
            div()
                .id("repository-tabs-end")
                .flex_1()
                .min_w(scaled(8.))
                .h(scaled(28.)),
        );
        // New tab and the tab list stay outside the scrolling strip so they
        // remain reachable however many tabs are open.
        let new_tab = Button::new("new-repository-tab")
            .ghost()
            .small()
            .tab_stop(true)
            .w(scaled(28.))
            .h(scaled(28.))
            .px(scaled(4.))
            .child(
                div()
                    .text_size(scaled(16.))
                    .line_height(scaled(16.))
                    .child("+"),
            )
            .set_disabled(blocked)
            .tooltip("New repository tab · Ctrl/Cmd+T")
            .on_click(cx.listener(|v, _, w, cx| {
                if !v.blocked(w, cx) {
                    v.add(None, w, cx);
                }
            }));
        let tab_entries = tab_labels
            .iter()
            .enumerate()
            .map(|(index, (label, _))| (index, label.clone(), index == self.active))
            .collect::<Vec<_>>();
        let tab_count = self.tabs.len();
        let active_index = self.active;
        let weak = cx.weak_entity();
        let tab_focus = self.tab_focus[&self.tabs[self.active].entity_id()].clone();
        let tab_list = {
            let moves = [
                ("Move tab left", "Ctrl+Shift+PgUp", -1, active_index > 0),
                (
                    "Move tab right",
                    "Ctrl+Shift+PgDn",
                    1,
                    active_index + 1 < tab_count,
                ),
            ];
            let entries = tab_entries
                .iter()
                .map(|(_, label, active)| PickerItem::new(label.clone()).checked(*active).into())
                .chain([PickerEntry::Rule])
                .chain(moves.iter().map(|&(label, shortcut, _, available)| {
                    PickerItem::new(label)
                        .note(shortcut)
                        .disabled(!available)
                        .into()
                }))
                .chain([PickerEntry::Rule])
                .chain([PickerItem::new("New tab").note("Ctrl/Cmd+T").into()]);
            Picker::new(
                "repository-tab-picker",
                Button::new("repository-tab-list")
                    .ghost()
                    .small()
                    .tab_stop(true)
                    .h(scaled(28.))
                    .icon(icon("down"))
                    .label(format!("{tab_count}"))
                    .set_disabled(blocked)
                    .tooltip("All tabs and tab order"),
                px(280.),
                &tab_focus,
                move |index, w, cx| {
                    if index < tab_count {
                        let _ = weak.update(cx, |v, cx| {
                            if !v.blocked(w, cx) {
                                v.activate(index, w, cx);
                            }
                        });
                    } else if let Some(&(_, _, delta, _)) = moves.get(index - tab_count) {
                        let _ = weak.update(cx, |v, cx| v.move_active_tab(delta, w, cx));
                    } else {
                        w.dispatch_action(Box::new(NewTab), cx);
                    }
                },
            )
            .anchor(Anchor::TopRight)
            .entries(entries)
            .build(window, cx)
        };
        let active = self.tabs[self.active].clone();
        let update = crate::updates::notice(cx);
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(components::rail::end_events(
                cx.weak_entity(),
                Self::finish_tab_drag,
                Self::cancel_tab_drag,
            ))
            .on_action(cx.listener(Self::pick))
            .on_action(cx.listener(|v, _: &Quit, w, cx| {
                if v.should_close_window(w, cx) {
                    w.remove_window();
                }
            }))
            .on_action(cx.listener(|v, _: &NewTab, w, cx| {
                if !v.blocked(w, cx) {
                    v.add(None, w, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &CloseTab, w, cx| v.close(v.active, w, cx)))
            .on_action(cx.listener(|v, _: &NextTab, w, cx| v.cycle(1, w, cx)))
            .on_action(cx.listener(|v, _: &PreviousTab, w, cx| v.cycle(-1, w, cx)))
            .on_action(cx.listener(|v, _: &MoveTabLeft, w, cx| v.move_active_tab(-1, w, cx)))
            .on_action(cx.listener(|v, _: &MoveTabRight, w, cx| v.move_active_tab(1, w, cx)))
            .on_action(cx.listener(|v, action: &OpenRecent, w, cx| {
                if !v.blocked(w, cx) {
                    v.open_path(action.0.clone(), w, cx);
                }
            }))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .h(px(geometry::SIZE_SHELL_TAB_STRIP))
                    .bg_role(Role::SurfaceChrome)
                    .border_b_1()
                    .border_role(Role::BorderDivider)
                    .child(strip)
                    .child(new_tab)
                    .child(tab_list),
            )
            .when_some(update, |el, update| el.child(update_notice(update, cx)))
            .when_some(self.picker_error.clone(), |el, error| {
                el.child(div().px_3().text_role(Role::StatusErrorFg).child(error))
            })
            .child(
                div()
                    .id(("repository-panel", active.entity_id()))
                    .flex_1()
                    .min_h_0()
                    .child(active),
            )
    }
}
