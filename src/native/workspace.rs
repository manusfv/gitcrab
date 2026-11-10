use super::components::{self, ControlSize};
use super::{
    preferences::Preferences,
    theme::*,
    view::{GitCrab, Open},
};
use gpui::{prelude::*, *};
use gpui_component::{button::*, Disableable, Sizable, WindowExt};
use std::{cell::RefCell, rc::Rc};

mod tab_order;
use components::rail::RailDrag;

actions!(gitcrab, [NewTab, CloseTab, NextTab, PreviousTab]);

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
        .border_color(rgb(semantic(if selected {
            Role::ActionPrimary
        } else {
            Role::SurfaceChrome
        })))
        .bg(rgb(semantic(if selected {
            Role::SurfaceOverlay
        } else {
            Role::SurfaceChrome
        })))
}

fn tab_label(id: impl Into<ElementId>, label: String) -> Button {
    Button::new(id)
        .ghost()
        .cursor_default()
        .interaction_highlight(false)
        .small()
        .h(rems(28. / 13.))
        .child(
            div()
                .max_w(px(170.))
                .truncate()
                .text_size(rems(14. / 13.))
                .line_height(rems(20. / 13.))
                .child(label),
        )
}

fn tab_close(id: impl Into<ElementId>) -> Button {
    components::sized(
        Button::new(id).child(
            div()
                .text_size(rems(16. / 13.))
                .line_height(rems(16. / 13.))
                .child("×"),
        ),
        ControlSize::TabClose,
    )
    .ghost()
    .cursor_default()
    .w(rems(32. / 13.))
}

pub struct Workspace {
    tabs: Vec<Entity<GitCrab>>,
    active: usize,
    picking: bool,
    picker_error: Option<String>,
    scroll: ScrollHandle,
    tab_drag: Option<RailDrag<EntityId>>,
    tab_drag_start: Option<(EntityId, Pixels)>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let path = std::env::args()
            .nth(1)
            .or_else(Preferences::last_repository);
        let tab = Self::create_tab(path, true, window, cx);
        Self {
            tabs: vec![tab],
            active: 0,
            picking: false,
            picker_error: None,
            scroll: ScrollHandle::new(),
            tab_drag: None,
            tab_drag_start: None,
        }
    }

    fn create_tab(
        path: Option<String>,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<GitCrab> {
        let tab = cx.new(|cx| GitCrab::new(path, active, window, cx));
        cx.observe(&tab, |_, _, cx| cx.notify()).detach();
        tab
    }

    fn blocked(&self, window: &mut Window, cx: &mut App) -> bool {
        self.picking
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
        self.tabs.remove(index);
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

    fn cycle(&mut self, direction: isize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.blocked(window, cx) {
            let index = (self.active as isize + direction).rem_euclid(self.tabs.len() as isize);
            self.activate(index as usize, window, cx);
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
        tab_order::move_to_slot(&mut self.tabs, &mut self.active, from, slot);
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
        let identity = canonical_worktree(&path);
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| canonical_worktree(tab.read(cx).repository_path()) == identity)
        {
            self.activate(index, window, cx);
            return;
        }
        if self.tabs[self.active].read(cx).has_repository() {
            self.add(Some(path), window, cx);
        } else {
            self.tabs[self.active].update(cx, |tab, cx| tab.open_path(path, window, cx));
        }
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
                        if let Some(path) = paths.first() {
                            v.open_path(path.to_string_lossy().into_owned(), w, cx);
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
        // Dialog builders can update repository views. Build them outside those
        // views' render calls to avoid leasing the same entity twice.
        let dialogs = gpui_component::Root::render_dialog_layer(window, cx);
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
        for (index, entity) in self.tabs.iter().enumerate() {
            let tab_id = entity.entity_id();

            let tab = entity.read(cx);
            let path = tab.repository_path();
            let mut label = std::path::Path::new(path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| {
                    if path.is_empty() {
                        "New tab".into()
                    } else {
                        path.into()
                    }
                });
            if !path.is_empty()
                && self
                    .tabs
                    .iter()
                    .filter(|other| {
                        std::path::Path::new(other.read(cx).repository_path()).file_name()
                            == std::path::Path::new(path).file_name()
                    })
                    .count()
                    > 1
            {
                let parent = std::path::Path::new(path)
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                label = format!("{label} · {parent}");
            }
            if tab.unread_result() {
                label = format!("● {label}");
            }
            let selected = index == self.active;
            let tooltip = if path.is_empty() {
                label.clone()
            } else {
                path.to_owned()
            };
            strip = strip.child(components::rail::item(
                tab_chrome(selected)
                    .id(("repository-tab", tab_id))
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
                        div()
                            .id(("drag-repository-tab", tab_id))
                            .cursor_grab()
                            .active(|el| el.cursor_grabbing())
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
                                            v.tab_drag = RailDrag::new(
                                                &bounds.borrow(),
                                                tab_id,
                                                pointer,
                                                4.,
                                            );
                                            v.move_tab_drag(w.mouse_position().x, cx);
                                        });
                                        cx.stop_propagation();
                                        cx.new(|_| drag.clone())
                                    },
                                )
                            })
                            .child(
                                tab_label(
                                    ("select-repository-tab", entity.entity_id()),
                                    label.clone(),
                                )
                                .tab_stop(true)
                                .disabled(blocked)
                                .tooltip(tooltip)
                                .on_click(cx.listener(
                                    move |v, _, w, cx| {
                                        if !cx.has_active_drag() && !v.blocked(w, cx) {
                                            v.activate(index, w, cx);
                                        }
                                    },
                                )),
                            ),
                    )
                    .child(
                        tab_close(("close-repository-tab", entity.entity_id()))
                            .tab_stop(true)
                            .disabled(blocked || tab.mutating())
                            .tooltip(format!("Close {label} tab"))
                            .on_click(cx.listener(move |v, _, w, cx| v.close(index, w, cx))),
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
                .min_w(rems(32. / 13.))
                .h(rems(28. / 13.))
                .child(
                    Button::new("new-repository-tab")
                        .ghost()
                        .small()
                        .w(rems(28. / 13.))
                        .h(rems(28. / 13.))
                        .px(rems(4. / 13.))
                        .child(
                            div()
                                .text_size(rems(16. / 13.))
                                .line_height(rems(16. / 13.))
                                .child("+"),
                        )
                        .disabled(blocked)
                        .tooltip("New repository tab · Ctrl/Cmd+T")
                        .on_click(cx.listener(|v, _, w, cx| {
                            if !v.blocked(w, cx) {
                                v.add(None, w, cx);
                            }
                        })),
                ),
        );
        let active = self.tabs[self.active].clone();
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
            .on_action(cx.listener(|v, _: &NewTab, w, cx| {
                if !v.blocked(w, cx) {
                    v.add(None, w, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &CloseTab, w, cx| v.close(v.active, w, cx)))
            .on_action(cx.listener(|v, _: &NextTab, w, cx| v.cycle(1, w, cx)))
            .on_action(cx.listener(|v, _: &PreviousTab, w, cx| v.cycle(-1, w, cx)))
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
                    .bg(rgb(semantic(Role::SurfaceChrome)))
                    .border_b_1()
                    .border_color(rgb(semantic(Role::BorderDivider)))
                    .child(strip),
            )
            .when_some(self.picker_error.clone(), |el, error| {
                el.child(
                    div()
                        .px_3()
                        .text_color(rgb(semantic(Role::StatusErrorFg)))
                        .child(error),
                )
            })
            .child(
                div()
                    .id(("repository-panel", active.entity_id()))
                    .flex_1()
                    .min_h_0()
                    .child(active),
            )
            .children(
                dialogs
                    .map(|dialog| deferred(dialog).with_priority(geometry::LAYER_MODAL as usize)),
            )
    }
}

fn canonical_worktree(path: &str) -> std::path::PathBuf {
    gitcrab_core::platform::worktree_identity(std::path::Path::new(path))
}
