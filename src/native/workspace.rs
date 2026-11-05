use super::components::{self, ControlSize};
use super::{
    preferences::Preferences,
    theme::*,
    view::{GitCrab, Open},
};
use gpui::{prelude::*, *};
use gpui_component::{button::*, Disableable, Sizable, WindowExt};
use std::{cell::Cell, rc::Rc};

mod tab_order;

actions!(gitcrab, [NewTab, CloseTab, NextTab, PreviousTab]);

#[derive(Clone, PartialEq, Debug, serde::Deserialize, Action)]
#[action(namespace = gitcrab, no_json)]
pub struct OpenRecent(pub String);

#[derive(Clone)]
struct DraggedTab {
    workspace: EntityId,
    tab: EntityId,
    label: String,
    selected: bool,
    width: Rc<Cell<Pixels>>,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Keep the source tab's size and appearance without a close control.
        tab_chrome(self.selected)
            // GPUI measures drag roots with minimum-content space. Use the
            // laid-out source width instead of letting a truncated label shrink.
            .w(self.width.get())
            .child(div().child(tab_label("dragged-tab-label", self.label.clone()).tab_stop(false)))
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
    tab_drop_target: Option<(EntityId, bool)>,
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
            tab_drop_target: None,
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

    fn drop_tab(
        &mut self,
        drag: &DraggedTab,
        target: Option<EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if drag.workspace != cx.entity_id() || self.blocked(window, cx) {
            return;
        }
        let Some(from) = self.tabs.iter().position(|tab| tab.entity_id() == drag.tab) else {
            return;
        };
        let slot = if let Some(target) = target {
            let Some(index) = self.tabs.iter().position(|tab| tab.entity_id() == target) else {
                return;
            };
            let after = self.tab_drop_target == Some((target, true));
            index + usize::from(after)
        } else {
            self.tabs.len()
        };
        self.tab_drop_target = None;
        // Move existing entities without activating, recreating, or clearing them.
        if tab_order::move_to_slot(&mut self.tabs, &mut self.active, from, slot) {
            self.scroll.scroll_to_item(self.active);
        }
        cx.notify();
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
        let mut strip = div()
            .id("repository-tabs")
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(4.))
            .overflow_x_scroll()
            .track_scroll(&self.scroll);
        for (index, entity) in self.tabs.iter().enumerate() {
            let tab_id = entity.entity_id();
            let tab_width = Rc::new(Cell::new(px(0.)));
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
            strip = strip.child(
                tab_chrome(selected)
                    .id(("repository-tab", tab_id))
                    .child({
                        let tab_width = tab_width.clone();
                        canvas(
                            move |bounds, _, _| tab_width.set(bounds.size.width),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full()
                    })
                    .when(!blocked, |el| {
                        let after = self.tab_drop_target == Some((tab_id, true));
                        el.can_drop(move |value, _, _| {
                            value.downcast_ref::<DraggedTab>().is_some_and(|drag| {
                                drag.workspace == workspace_id && drag.tab != tab_id
                            })
                        })
                        .on_drag_move(cx.listener(
                            move |v, event: &DragMoveEvent<DraggedTab>, w, cx| {
                                if event.drag(cx).workspace != cx.entity_id()
                                    || !event.bounds.contains(&event.event.position)
                                    || v.blocked(w, cx)
                                {
                                    return;
                                }
                                let after = event.event.position.x
                                    >= event.bounds.origin.x + event.bounds.size.width / 2.;
                                let target = Some((tab_id, after));
                                if v.tab_drop_target != target {
                                    v.tab_drop_target = target;
                                    cx.notify();
                                }
                            },
                        ))
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .w(px(2.))
                                .when(after, |el| el.right_0())
                                .when(!after, |el| el.left_0())
                                .bg(rgb(semantic(Role::ActionPrimary)))
                                .invisible()
                                .group_drag_over::<DraggedTab>("repository-tab-drop", |el| {
                                    el.visible()
                                }),
                        )
                        .group("repository-tab-drop")
                        .on_drop(cx.listener(
                            move |v, drag: &DraggedTab, w, cx| {
                                v.drop_tab(drag, Some(tab_id), w, cx);
                            },
                        ))
                    })
                    .child(
                        div()
                            .id(("drag-repository-tab", tab_id))
                            .when(!blocked, |el| {
                                el.on_drag(
                                    DraggedTab {
                                        workspace: workspace_id,
                                        tab: tab_id,
                                        label: label.clone(),
                                        selected,
                                        width: tab_width.clone(),
                                    },
                                    |drag, _, _, cx| {
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
                                        if !v.blocked(w, cx) {
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
            );
        }
        strip = strip.child(
            div()
                .id("repository-tabs-end")
                .flex_1()
                .min_w(rems(32. / 13.))
                .h(rems(28. / 13.))
                .when(!blocked, |el| {
                    el.can_drop(move |value, _, _| {
                        value
                            .downcast_ref::<DraggedTab>()
                            .is_some_and(|drag| drag.workspace == workspace_id)
                    })
                    .drag_over::<DraggedTab>(|style, _, _, _| {
                        style
                            .border_l_2()
                            .border_color(rgb(semantic(Role::ActionPrimary)))
                    })
                    .on_drop(cx.listener(|v, drag: &DraggedTab, w, cx| {
                        v.drop_tab(drag, None, w, cx);
                    }))
                })
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
