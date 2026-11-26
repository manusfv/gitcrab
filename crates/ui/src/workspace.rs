use super::components;
use super::{
    components::{
        prelude::{Picker, PickerEntry, PickerItem},
        ButtonExt as _,
    },
    theme::{Role, *},
    view::{CloneRepository, CreateRepository, GitCrab, Open, Quit},
};
use gitcrab_core::presentation::{display_path, path_labels};
use gpui::component::{button::*, tooltip::Tooltip, Sizable, WindowExt};
use gpui::{prelude::*, *};
use std::{cell::RefCell, rc::Rc};

use components::rail::RailDrag;
use components::shortcut_text;
use components::{ContextMenu, MenuTarget};

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

/// A tab context menu item.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = gitcrab, no_json)]
struct TabMenu(MenuTarget<Workspace>, TabCommand);

#[derive(Clone, PartialEq, Debug)]
enum TabCommand {
    Close(Vec<EntityId>),
    NewTab,
}

/// Installs the named release from the Settings dialog, which closes just
/// before installing.
#[derive(Clone, PartialEq, Debug, serde::Deserialize, Action)]
#[action(namespace = gitcrab, no_json)]
pub struct InstallUpdateFromSettings(pub String);

/// Where the user asked to install an update.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum InstallOrigin {
    Notice,
    Settings,
}

/// Why an install can't go ahead now, if it can't. An install started from
/// Settings may find Settings open, because the workspace closes it before
/// installing; any other dialog, or Settings during an install started from
/// the notice, is left for the user to close.
fn install_refusal(
    origin: InstallOrigin,
    git_running: bool,
    settings_open: bool,
    other_dialog_open: bool,
) -> Option<&'static str> {
    if git_running {
        Some("A Git operation is running. Try again when it finishes.")
    } else if other_dialog_open || (settings_open && origin != InstallOrigin::Settings) {
        Some("Close the open dialog, then try again.")
    } else {
        None
    }
}

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

/// Repository tab metrics, after VS Code's editor tabs.
const TAB_LABEL_MAX_WIDTH: f32 = 170.;
const TAB_LABEL_PAD_LEFT: f32 = 12.;
const TAB_LABEL_PAD_RIGHT: f32 = 4.;
const TAB_TEXT_SIZE: f32 = 13.;
const TAB_CLOSE_SIZE: f32 = 20.;
const TAB_CLOSE_GLYPH: f32 = 16.;
const TAB_CLOSE_MARGIN_RIGHT: f32 = 6.;
const TAB_UNREAD_DOT: f32 = 8.;
/// Height of the accent line along the active tab's top edge.
const TAB_ACCENT: f32 = 1.;
/// Room kept at the end of the strip so the last tab never touches the buttons.
const TAB_STRIP_END_MIN: f32 = 8.;
const TAB_STRIP_BUTTON: f32 = 28.;
/// Space on each side of the New tab button, between the last tab and the
/// rest of the strip.
const TAB_STRIP_BUTTON_INSET: f32 = 4.;
/// Tabs sit flush, so a drag places them with no gap between neighbours.
const TAB_DRAG_GAP: f32 = 0.;
const TAB_PICKER_WIDTH: f32 = 280.;

/// A tab in the strip. The active tab takes the toolbar's surface and has no
/// bottom edge, so it reads as part of the content below; the others sit on
/// the strip and only light up on hover, or while their context menu is open.
fn tab_chrome(selected: bool, held: bool, group: SharedString) -> Div {
    div()
        .relative()
        .flex_none()
        .flex()
        .items_center()
        .h_full()
        .group(group)
        .border_r_1()
        .border_role(Role::BorderDivider)
        .map(|tab| {
            if selected {
                tab.bg_role(Role::SurfaceChrome)
                    .text_role(Role::TextDefault)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h(px(TAB_ACCENT))
                            .bg_role(Role::ActionPrimary),
                    )
            } else {
                tab.bg_role(Role::SurfaceCanvas)
                    .text_role(Role::TextMuted)
                    .border_b_1()
                    .hover(|s| {
                        s.bg_role(Role::InteractionHover)
                            .text_role(Role::TextSecondary)
                    })
                    .when(held, |tab| {
                        tab.bg_role(Role::InteractionHover)
                            .text_role(Role::TextSecondary)
                    })
            }
        })
}

/// The tab's name. It is part of the tab, which handles clicks, drags and focus.
fn tab_label(label: String) -> Div {
    div()
        .pl(scaled(TAB_LABEL_PAD_LEFT))
        .pr(scaled(TAB_LABEL_PAD_RIGHT))
        .flex()
        .items_center()
        .child(
            div()
                .max_w(px(TAB_LABEL_MAX_WIDTH))
                .truncate()
                .text_size(scaled(TAB_TEXT_SIZE))
                .child(label),
        )
}

/// The tab's close button. It is not a separate focus stop: the tab is one
/// control. The active tab always shows it; the others show it on hover. A
/// tab with an unseen result shows a dot in its place until hovered.
fn tab_close(
    id: impl Into<ElementId>,
    group: SharedString,
    enabled: bool,
    selected: bool,
    unread: bool,
) -> Stateful<Div> {
    let on_hover_only = unread || !selected;
    div()
        .id(id)
        .relative()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(scaled(TAB_CLOSE_SIZE))
        .mr(scaled(TAB_CLOSE_MARGIN_RIGHT))
        .rounded(px(CONTROL_RADIUS))
        .map(|el| {
            if enabled {
                el.hover(|s| {
                    s.bg_role(Role::InteractionPressed)
                        .text_role(Role::TextDefault)
                })
                .tooltip(|w, cx| Tooltip::new("Close").action(&CloseTab, None).build(w, cx))
            } else {
                el.text_role(Role::TextDisabled)
            }
        })
        .child(
            div()
                .flex_none()
                .size(scaled(TAB_CLOSE_GLYPH))
                .when(on_hover_only, |glyph| {
                    glyph
                        .invisible()
                        .group_hover(group.clone(), |s| s.visible())
                })
                .child(icon("x").size(scaled(TAB_CLOSE_GLYPH))),
        )
        .when(unread, |el| {
            el.child(
                div()
                    .absolute()
                    .size(scaled(TAB_UNREAD_DOT))
                    .rounded_full()
                    .bg_role(Role::TextSecondary)
                    .group_hover(group, |s| s.invisible()),
            )
        })
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
    /// The tab whose context menu is open. It keeps its hover look.
    menu_tab: Option<EntityId>,
    /// Redraws the update notice when a check finishes.
    _updates: Subscription,
}

/// The notice announcing an available release, above the repository panel.
/// It never blocks the window: the user can act on it or close it. When this
/// copy installs updates itself it offers to, and shows the progress and any
/// error in place; otherwise it links to the release page.
fn update_notice(
    update: crate::updates::AvailableUpdate,
    cx: &mut Context<Workspace>,
) -> Stateful<Div> {
    use crate::updates::{Install, InstallSupport};
    let button = |id: &'static str, label: &'static str, kind| {
        components::basic(
            Button::new(id).label(label),
            kind,
            components::ControlSize::Compact,
        )
        .tab_stop(true)
    };
    let install = crate::updates::install_state(cx);
    let support = crate::updates::install_support(cx);
    let busy = install.in_progress();
    let (detail, detail_role) = match &install {
        Install::Idle => (None, Role::TextMuted),
        Install::Downloading(Some(percent)) => {
            (Some(format!("Downloading {percent}%")), Role::TextMuted)
        }
        Install::Downloading(None) => (Some("Downloading…".to_owned()), Role::TextMuted),
        Install::Installing => (Some("Installing…".to_owned()), Role::TextMuted),
        Install::Failed(error) => (Some(error.clone()), Role::StatusErrorFg),
    };
    let notes_tooltip = match &support {
        Some(InstallSupport::Unsupported(reason)) => {
            format!("{reason} Download it from {}", update.release_url)
        }
        _ => update.release_url.clone(),
    };
    let install_button = (support == Some(InstallSupport::Supported) && !busy).then(|| {
        let version = update.version.clone();
        let failed = matches!(install, Install::Failed(_));
        button(
            "update-install",
            if failed {
                "Try again"
            } else {
                "Install and restart"
            },
            components::ButtonKind::Primary,
        )
        .tooltip("Download, verify and install this version, then restart GitCrab")
        .on_click(cx.listener(move |v, _, w, cx| {
            v.install_update(version.clone(), InstallOrigin::Notice, w, cx)
        }))
    });
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
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_role(Role::TextDefault)
                        .child(format!("GitCrab {} is available", update.version)),
                )
                .when_some(detail, |el, detail| {
                    el.child(
                        div()
                            .id("update-install-status")
                            .text_size(scaled(TEXT_SIZE_SMALL))
                            .text_role(detail_role)
                            .child(detail),
                    )
                }),
        )
        .when_some(install_button, |el, install| el.child(install))
        .child(
            button(
                "update-release-notes",
                "Release notes",
                components::ButtonKind::Secondary,
            )
            .tooltip(notes_tooltip)
            .on_click(move |_, _, cx| cx.open_url(&url)),
        )
        .when(!busy, |el| {
            el.child(
                button(
                    "update-skip",
                    "Skip this version",
                    components::ButtonKind::Secondary,
                )
                .tooltip("Don't announce this version again")
                .on_click(cx.listener(move |_, _, _, cx| {
                    crate::updates::skip(&skip, cx);
                    cx.notify();
                })),
            )
        })
        .when(!busy, |el| {
            el.child(
                Button::new("update-dismiss")
                    .ghost()
                    .small()
                    .tab_stop(true)
                    .size(scaled(CONTROL_HEIGHT))
                    .child(
                        div()
                            .text_size(scaled(GLYPH_SIZE))
                            .line_height(scaled(GLYPH_SIZE))
                            .child("×"),
                    )
                    .tooltip("Close until GitCrab restarts")
                    .on_click(cx.listener(move |_, _, _, cx| {
                        crate::updates::dismiss(&dismiss, cx);
                        cx.notify();
                    })),
            )
        })
}

impl Workspace {
    /// Starts with the command-line worktree or the last saved session path.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        components::menu::register(cx, |TabMenu(target, command): &TabMenu, cx| {
            let command = command.clone();
            target.update(cx, move |v, w, cx| match command {
                TabCommand::Close(ids) => v.close_many(ids, w, cx),
                TabCommand::NewTab => v.add(None, w, cx),
            });
        });
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
            menu_tab: None,
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

    /// Closes several tabs with one draft confirmation and one settings
    /// flush. Tabs running a Git write stay open.
    fn close_many(&mut self, ids: Vec<EntityId>, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked(window, cx) {
            return;
        }
        let closing: Vec<&Entity<GitCrab>> = self
            .tabs
            .iter()
            .filter(|tab| ids.contains(&tab.entity_id()) && !tab.read(cx).mutating())
            .collect();
        let drafts = closing
            .iter()
            .filter(|tab| tab.read(cx).has_drafts())
            .count();
        let ids: Vec<EntityId> = closing.iter().map(|tab| tab.entity_id()).collect();
        if ids.is_empty() {
            return;
        }
        if drafts > 0 {
            let message = match drafts {
                1 => "Close these tabs and discard one tab's drafts?".to_string(),
                n => format!("Close these tabs and discard drafts in {n} tabs?"),
            };
            self.confirm_discard(&message, window, cx, move |v, w, cx| {
                v.close_many_after_flush(ids, w, cx)
            });
            return;
        }
        self.close_many_after_flush(ids, window, cx);
    }

    fn close_many_after_flush(
        &mut self,
        ids: Vec<EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let identities: Vec<_> = self
            .tabs
            .iter()
            .filter(|tab| ids.contains(&tab.entity_id()))
            .map(|tab| (tab.entity_id(), tab.read(cx).close_identity()))
            .collect();
        self.after_persistence(window, cx, move |v, w, cx| {
            let mut changed = false;
            for (id, identity) in identities {
                let Some(index) = v.tabs.iter().position(|tab| tab.entity_id() == id) else {
                    continue;
                };
                let tab = v.tabs[index].read(cx);
                if tab.mutating() || tab.close_identity() != identity {
                    changed = true;
                } else {
                    v.close_now(index, w, cx);
                }
            }
            if changed {
                v.picker_error =
                    Some("A tab changed while closing. Review it before closing again.".into());
            }
        });
    }

    /// The tab's context menu: close actions, then its repository's path.
    fn tab_menu(&self, tab_id: EntityId, window: &Window, cx: &Context<Self>) -> ContextMenu {
        let menu = ContextMenu::new();
        let Some(index) = self.tabs.iter().position(|tab| tab.entity_id() == tab_id) else {
            return menu;
        };
        let target = MenuTarget::new(window, cx.weak_entity());
        let ids: Vec<EntityId> = self.tabs.iter().map(|tab| tab.entity_id()).collect();
        let tab = self.tabs[index].read(cx);
        let path = tab.repository_path().to_owned();
        let others: Vec<EntityId> = ids.iter().copied().filter(|id| *id != tab_id).collect();
        let right: Vec<EntityId> = ids[index + 1..].to_vec();
        let close = |menu: ContextMenu, label: &'static str, ids: Vec<EntityId>, enabled: bool| {
            menu.item(
                label,
                enabled,
                TabMenu(target.clone(), TabCommand::Close(ids)),
            )
        };
        let menu = close(menu, "Close tab", vec![tab_id], !tab.mutating());
        let menu = close(menu, "Close other tabs", others.clone(), !others.is_empty());
        let menu = close(
            menu,
            "Close tabs to the right",
            right.clone(),
            !right.is_empty(),
        )
        .separator()
        .item("New tab", true, TabMenu(target.clone(), TabCommand::NewTab));
        if path.is_empty() {
            return menu;
        }
        let shown = std::path::PathBuf::from(display_path(&path));
        menu.separator()
            .copy("Copy repository path", shown.to_string_lossy().into_owned())
            .reveal(shown)
    }

    /// Opens a tab's context menu, keeping the tab's hover look until it closes.
    fn show_tab_menu(
        &mut self,
        tab_id: EntityId,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let menu = self.tab_menu(tab_id, window, cx);
        self.menu_tab = Some(tab_id);
        cx.notify();
        let view = cx.weak_entity();
        menu.show(event.position, window, cx, move |cx| {
            let _ = view.update(cx, |v, cx| {
                v.menu_tab = None;
                cx.notify();
            });
        });
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
            "Commit drafts aren't saved to disk.",
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
        self.flush_then(window, cx, move |v, result, w, cx| match result {
            Ok(()) => finished(v, w, cx),
            Err(error) => v.confirm_close_without_saving(error, w, cx, finished),
        });
    }

    /// Saves every tab's preferences and the shared settings, then calls
    /// `finished` with the outcome.
    fn flush_then(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        finished: impl FnOnce(&mut Self, Result<(), String>, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let result = self
            .tabs
            .iter()
            .try_for_each(|tab| tab.update(cx, |tab, cx| tab.flush_preferences(cx)))
            .and_then(|()| crate::preferences::flush(cx));
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                finished(self, Err(error), window, cx);
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
                finished(v, result, w, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Downloads and verifies `version`, then installs it and restarts
    /// GitCrab. Waits for no Git write to be running and asks before
    /// discarding drafts. Progress and errors show in the update notice, the
    /// Settings dialog and the status bar.
    fn install_update(
        &mut self,
        version: String,
        origin: InstallOrigin,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if crate::updates::install_state(cx).in_progress() {
            return;
        }
        let download = crate::updates::download_update(&version, cx);
        cx.notify();
        cx.spawn_in(window, async move |workspace, cx| {
            if download.await.is_ok() {
                let _ = workspace
                    .update_in(cx, |v, w, cx| v.restart_into_update(version, origin, w, cx));
            }
        })
        .detach();
    }

    fn restart_into_update(
        &mut self,
        version: String,
        origin: InstallOrigin,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = self.tabs[self.active].read(cx);
        let (settings_open, awaiting_trust) = (active.settings_open(), active.awaiting_trust);
        let other_dialog_open = self.picking
            || self.confirming_draft_loss
            || awaiting_trust
            || (!settings_open && window.has_active_dialog(cx));
        let git_running = self.tabs.iter().any(|tab| tab.read(cx).mutating());
        if let Some(reason) = install_refusal(origin, git_running, settings_open, other_dialog_open)
        {
            crate::updates::install_failed(reason, cx);
            return;
        }
        if settings_open {
            self.tabs[self.active].update(cx, |tab, cx| tab.close_settings(window, cx));
        }
        if self.tabs.iter().any(|tab| tab.read(cx).has_drafts()) {
            self.confirm_close(
                "Restart GitCrab to install the update and discard all drafts?",
                "Commit drafts aren't saved to disk.",
                "Discard drafts and restart",
                window,
                cx,
                move |v, w, cx| v.install_after_flush(version, w, cx),
            );
        } else {
            self.install_after_flush(version, window, cx);
        }
    }

    /// Saves settings and the window size like quitting does, installs and
    /// restarts. On Windows the installer ends this process and starts the
    /// new version itself.
    fn install_after_flush(
        &mut self,
        version: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remember_window_size(window, cx);
        let identities: Vec<_> = self
            .tabs
            .iter()
            .map(|tab| (tab.entity_id(), tab.read(cx).close_identity()))
            .collect();
        self.flush_then(window, cx, move |v, saved, w, cx| {
            if let Err(error) = saved {
                crate::updates::install_failed(
                    format!("Could not save settings, so nothing was installed: {error}"),
                    cx,
                );
                return;
            }
            let current: Vec<_> = v
                .tabs
                .iter()
                .map(|tab| (tab.entity_id(), tab.read(cx).close_identity()))
                .collect();
            if current != identities || v.tabs.iter().any(|tab| tab.read(cx).mutating()) {
                crate::updates::install_failed(
                    "A tab changed while preparing to restart. Try again.",
                    cx,
                );
                return;
            }
            let installing = crate::updates::install_update(&version, cx);
            cx.spawn_in(w, async move |workspace, cx| {
                if let Ok(program) = installing.await {
                    let _ = workspace.update(cx, |_, cx| {
                        cx.set_restart_path(program);
                        cx.restart();
                    });
                }
            })
            .detach();
        });
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
        if let Some(tab) = self.tab_for(&location, window, cx) {
            tab.update(cx, |tab, cx| tab.open_repository(location, window, cx));
        }
    }

    /// The tab a repository opens in: an empty active tab, or a new one.
    /// `None` when a tab already shows it, which is activated instead.
    fn tab_for(
        &mut self,
        location: &gitcrab_core::platform::RepositoryLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<GitCrab>> {
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.read(cx).repository_identity() == Some(location.worktree.as_path()))
        {
            self.activate(index, window, cx);
            return None;
        }
        if self.tabs[self.active].read(cx).has_repository() {
            self.add(None, window, cx);
        }
        Some(self.tabs[self.active].clone())
    }

    /// Opens the Clone or Create dialog, suggesting the folder that holds the
    /// most recent repository, or the home folder.
    fn start_setup(
        &mut self,
        kind: crate::repository_setup::Setup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.blocked(window, cx) {
            return;
        }
        let parent = crate::preferences::current(cx)
            .recent
            .first()
            .and_then(|path| std::path::Path::new(path).parent())
            .map(|parent| parent.display().to_string())
            .or_else(|| {
                std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                    .map(|home| std::path::PathBuf::from(home).display().to_string())
            })
            .unwrap_or_default();
        let workspace = cx.weak_entity();
        crate::repository_setup::open(
            kind,
            parent,
            std::rc::Rc::new(move |path, window, cx| {
                let _ = workspace.update(cx, |v, cx| v.open_created(path, window, cx));
            }),
            window,
            cx,
        );
    }

    /// Opens a repository the Clone or Create dialog just made.
    fn open_created(
        &mut self,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picking = true;
        self.picker_error = None;
        let backend = crate::backend::get(cx);
        let task = cx
            .background_executor()
            .spawn(async move { backend.discover(&path) });
        cx.spawn_in(window, async move |workspace, cx| {
            let result = task.await;
            let _ = workspace.update_in(cx, |v, w, cx| {
                v.picking = false;
                match result {
                    Ok(location) => {
                        if let Some(tab) = v.tab_for(&location, w, cx) {
                            tab.update(cx, |tab, cx| tab.open_created(location, w, cx));
                        }
                    }
                    Err(error) => v.picker_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
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
            .h_full()
            .flex()
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
            let (label, tooltip) = tab_labels[index].clone();
            let unread = tab.unread_result();
            let selected = index == self.active;
            let group = SharedString::from(format!("repository-tab-{tab_id}"));
            let closable = !blocked && !tab.mutating();
            // The name, close mark and padding form one tab: it takes the
            // focus, drags and clicks as a whole, like a browser tab.
            strip = strip.child(components::rail::item(
                tab_chrome(selected, self.menu_tab == Some(tab_id), group.clone())
                    .id(("repository-tab", tab_id))
                    .track_focus(&focus)
                    .when(
                        focus.is_focused(window) && window.last_input_was_keyboard(),
                        |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .border_1()
                                    .border_role(Role::InteractionFocus),
                            )
                        },
                    )
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
                                    v.tab_drag = RailDrag::new(
                                        &bounds.borrow(),
                                        tab_id,
                                        pointer,
                                        TAB_DRAG_GAP,
                                    );
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
                        tab_close(
                            ("close-repository-tab", tab_id),
                            group,
                            closable,
                            selected,
                            unread,
                        )
                        .on_click(cx.listener(move |v, _, w, cx| {
                            // The close mark acts alone; the tab's own
                            // click would select a tab that is closing.
                            cx.stop_propagation();
                            if !cx.has_active_drag() {
                                v.close(index, w, cx);
                            }
                        })),
                    )
                    .on_mouse_up(
                        MouseButton::Right,
                        cx.listener(move |v, event, w, cx| v.show_tab_menu(tab_id, event, w, cx)),
                    ),
                self.tab_drag.as_ref(),
                tab_id,
                now,
            ));
        }
        let new_tab = Button::new("new-repository-tab")
            .ghost()
            .small()
            .tab_stop(true)
            .size(scaled(TAB_STRIP_BUTTON))
            .px(scaled(4.))
            .child(
                div()
                    .text_size(scaled(GLYPH_SIZE))
                    .line_height(scaled(GLYPH_SIZE))
                    .child("+"),
            )
            .set_disabled(blocked)
            .tooltip_with_action("New tab", &NewTab, None)
            .on_click(cx.listener(|v, _, w, cx| {
                if !v.blocked(w, cx) {
                    v.add(None, w, cx);
                }
            }));
        // New tab follows the last tab, as in a browser. The tab list stays
        // outside the scrolling strip so it remains reachable however many
        // tabs are open, and it offers New tab too.
        strip = strip
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .px(scaled(TAB_STRIP_BUTTON_INSET))
                    .border_b_1()
                    .border_role(Role::BorderDivider)
                    .child(new_tab),
            )
            .child(
                div()
                    .id("repository-tabs-end")
                    .flex_1()
                    .min_w(scaled(TAB_STRIP_END_MIN))
                    .border_b_1()
                    .border_role(Role::BorderDivider),
            );
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
            let moves: [(&str, &dyn Action, isize, bool); 2] = [
                ("Move tab left", &MoveTabLeft, -1, active_index > 0),
                (
                    "Move tab right",
                    &MoveTabRight,
                    1,
                    active_index + 1 < tab_count,
                ),
            ];
            let entries = tab_entries
                .iter()
                .map(|(_, label, active)| PickerItem::new(label.clone()).checked(*active).into())
                .chain([PickerEntry::Rule])
                .chain(moves.iter().map(|&(label, action, _, available)| {
                    PickerItem::new(label)
                        .when_some(shortcut_text(action, window), PickerItem::note)
                        .disabled(!available)
                        .into()
                }))
                .chain([PickerEntry::Rule])
                .chain([PickerItem::new("New tab")
                    .when_some(shortcut_text(&NewTab, window), PickerItem::note)
                    .into()]);
            Picker::new(
                "repository-tab-picker",
                Button::new("repository-tab-list")
                    .ghost()
                    .small()
                    .tab_stop(true)
                    .h(scaled(TAB_STRIP_BUTTON))
                    .icon(icon("down"))
                    .label(format!("{tab_count}"))
                    .set_disabled(blocked)
                    .tooltip("All tabs"),
                px(TAB_PICKER_WIDTH),
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
            .on_action(cx.listener(|v, _: &CloneRepository, w, cx| {
                v.start_setup(crate::repository_setup::Setup::Clone, w, cx)
            }))
            .on_action(cx.listener(|v, _: &CreateRepository, w, cx| {
                v.start_setup(crate::repository_setup::Setup::Create, w, cx)
            }))
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
            .on_action(cx.listener(|v, action: &InstallUpdateFromSettings, w, cx| {
                v.install_update(action.0.clone(), InstallOrigin::Settings, w, cx)
            }))
            .on_action(cx.listener(|v, action: &OpenRecent, w, cx| {
                if !v.blocked(w, cx) {
                    v.open_path(action.0.clone(), w, cx);
                }
            }))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .h(px(geometry::SIZE_SHELL_TAB_STRIP))
                    .bg_role(Role::SurfaceCanvas)
                    .child(strip)
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .border_b_1()
                            .border_role(Role::BorderDivider)
                            .child(tab_list),
                    ),
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
            .child(crate::components::menu_cursor())
    }
}

#[cfg(test)]
mod tests {
    use super::{install_refusal, InstallOrigin};

    const GIT: &str = "A Git operation is running. Try again when it finishes.";
    const DIALOG: &str = "Close the open dialog, then try again.";

    #[test]
    fn settings_only_stays_open_for_an_install_it_started() {
        use InstallOrigin::{Notice, Settings};
        assert_eq!(install_refusal(Settings, false, true, false), None);
        assert_eq!(install_refusal(Notice, false, true, false), Some(DIALOG));
        assert_eq!(install_refusal(Notice, false, false, false), None);
        assert_eq!(install_refusal(Settings, false, false, false), None);
        for origin in [Notice, Settings] {
            assert_eq!(install_refusal(origin, false, false, true), Some(DIALOG));
            assert_eq!(install_refusal(origin, false, true, true), Some(DIALOG));
            assert_eq!(install_refusal(origin, true, true, false), Some(GIT));
            assert_eq!(install_refusal(origin, true, false, false), Some(GIT));
        }
    }
}
