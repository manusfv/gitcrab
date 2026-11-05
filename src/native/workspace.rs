use super::components::{self, ControlSize};
use super::{
    preferences::Preferences,
    theme::*,
    view::{GitCrab, Open},
};
use gpui::{prelude::*, *};
use gpui_component::{button::*, Disableable, Sizable, WindowExt};

actions!(gitcrab, [NewTab, CloseTab, NextTab, PreviousTab]);

#[derive(Clone, PartialEq, Debug, serde::Deserialize, Action)]
#[action(namespace = gitcrab, no_json)]
pub struct OpenRecent(pub String);

pub struct Workspace {
    tabs: Vec<Entity<GitCrab>>,
    active: usize,
    picking: bool,
    picker_error: Option<String>,
    scroll: ScrollHandle,
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
        let blocked = self.blocked(window, cx);
        let mut strip = div()
            .id("repository-tabs")
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap_1()
            .overflow_x_scroll()
            .track_scroll(&self.scroll);
        for (index, entity) in self.tabs.iter().enumerate() {
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
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .rounded(px(4.))
                    .border_b_1()
                    .border_color(rgb(if selected {
                        semantic(Role::ActionPrimary)
                    } else {
                        semantic(Role::SurfaceChrome)
                    }))
                    .when(selected, |el| el.bg(rgb(semantic(Role::SurfaceOverlay))))
                    .child(
                        Button::new(("select-repository-tab", entity.entity_id()))
                            .ghost()
                            .small()
                            .tab_stop(true)
                            .disabled(blocked)
                            .child(div().max_w(px(170.)).truncate().child(label.clone()))
                            .tooltip(tooltip)
                            .on_click(cx.listener(move |v, _, w, cx| {
                                if !v.blocked(w, cx) {
                                    v.activate(index, w, cx);
                                }
                            })),
                    )
                    .child(
                        components::sized(
                            Button::new(("close-repository-tab", entity.entity_id())).label("×"),
                            ControlSize::TabClose,
                        )
                        .ghost()
                        .small()
                        .tab_stop(true)
                        .disabled(blocked || tab.mutating())
                        .tooltip(format!("Close {label} tab"))
                        .on_click(cx.listener(move |v, _, w, cx| v.close(index, w, cx))),
                    ),
            );
        }
        let active = self.tabs[self.active].clone();
        div()
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
                    .child(strip)
                    .child(
                        Button::new("new-repository-tab")
                            .label("+")
                            .ghost()
                            .small()
                            .disabled(blocked)
                            .tooltip("New repository tab · Ctrl/Cmd+T")
                            .on_click(cx.listener(|v, _, w, cx| {
                                if !v.blocked(w, cx) {
                                    v.add(None, w, cx);
                                }
                            })),
                    ),
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
    }
}

fn canonical_worktree(path: &str) -> std::path::PathBuf {
    gitcrab_core::platform::worktree_identity(std::path::Path::new(path))
}
