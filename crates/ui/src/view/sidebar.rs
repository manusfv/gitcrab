use super::*;
use crate::workspace::OpenRecent;
use components::{ContextMenu, MenuTarget};
use gitcrab_core::presentation::display_path;
use gitcrab_core::sidebar::{section, section_key, tree_rows, RefFilter, TreeRow};
use std::rc::Rc;

/// Horizontal inset shared by every sidebar row, header and the filter box.
const INSET: f32 = 8.;
/// Extra indent per folder level.
const INDENT: f32 = 14.;
const HEADER_HEIGHT: f32 = 28.;
const ROW_HEIGHT: f32 = 26.;
const WORKING_ROW_HEIGHT: f32 = geometry::SIZE_TARGET_REGULAR;
const ICON_SIZE: f32 = 14.;
/// Indent of folder rows and of entries, which clear the folders' disclosure arrows.
const FOLDER_INDENT: f32 = 4.;
const ENTRY_INDENT: f32 = 18.;

type EntryClick = Rc<dyn Fn(&mut GitCrab, &ClickEvent, &mut Window, &mut Context<GitCrab>)>;
type EntryMenu = Rc<dyn Fn(&mut Window, &mut App) -> ContextMenu>;

/// The reference a sidebar context menu acts on.
struct MenuReference {
    kind: ReferenceKind,
    /// What Checkout, Merge and Create branch resolve: a branch name or full ref.
    target: String,
    /// The short name people see and copy.
    label: String,
    tip: String,
    checked_out: bool,
}

/// A menu entry that runs `command` on `target` through the shared command model.
fn command_item(
    menu: ContextMenu,
    view: &MenuTarget<GitCrab>,
    commands: &CommandContext,
    command: Command,
    label: impl Into<SharedString>,
    target: String,
) -> ContextMenu {
    menu.item(
        label,
        command.availability(commands).enabled(),
        view.command(MenuCommand::Run(command, target)),
    )
}

/// Checkout, Merge, Pull and Push for a branch or tag, then Create branch and copy actions.
fn reference_menu(view: Entity<GitCrab>, reference: MenuReference) -> EntryMenu {
    Rc::new(move |w, cx| {
        let (commands, head) = view.update(cx, |v, cx| {
            let head = v
                .state
                .snapshot
                .as_ref()
                .and_then(|s| s.checked_out_branch().map(str::to_owned));
            (v.command_context(w, cx), head)
        });
        let target_view = MenuTarget::new(w, view.downgrade());
        let MenuReference {
            kind,
            target,
            label,
            tip,
            checked_out,
        } = &reference;
        let mut menu = ContextMenu::new();
        if *checked_out {
            for command in [Command::Pull, Command::Push] {
                menu = command_item(
                    menu,
                    &target_view,
                    &commands,
                    command,
                    command.label(),
                    String::new(),
                );
            }
        } else {
            let into = head.as_deref().unwrap_or("HEAD");
            menu = command_item(
                menu,
                &target_view,
                &commands,
                Command::Checkout,
                format!("Checkout {label}"),
                target.clone(),
            );
            menu = command_item(
                menu,
                &target_view,
                &commands,
                Command::Merge,
                format!("Merge {label} into {into}"),
                target.clone(),
            );
        }
        let noun = if *kind == ReferenceKind::Tag {
            "tag"
        } else {
            "branch"
        };
        menu.separator()
            .item(
                "Create branch here…",
                true,
                target_view.command(MenuCommand::CreateBranch(target.clone())),
            )
            .separator()
            .copy(format!("Copy {noun} name"), label.clone())
            .copy("Copy commit SHA", tip.clone())
    })
}

/// A section header's menu: one command, such as Fetch for Remotes.
fn command_menu(view: Entity<GitCrab>, commands: &'static [Command]) -> EntryMenu {
    Rc::new(move |w, cx| {
        let context = view.update(cx, |v, cx| v.command_context(w, cx));
        let target = MenuTarget::new(w, view.downgrade());
        commands.iter().fold(ContextMenu::new(), |menu, &command| {
            command_item(
                menu,
                &target,
                &context,
                command,
                command.menu_label(),
                String::new(),
            )
        })
    })
}

/// One leaf of a slash-separated reference tree (branch, remote ref or tag).
struct TreeEntry {
    /// Slash-separated name; every segment but the last becomes a folder row.
    label: String,
    id: String,
    glyph: &'static str,
    current: bool,
    previewed: bool,
    tooltip: String,
    on_click: EntryClick,
    menu: EntryMenu,
}

impl GitCrab {
    fn toggle_collapsed(v: &mut Self, key: &str, w: &mut Window, cx: &mut Context<Self>) {
        if !v.preferences.collapsed.remove(key) {
            v.preferences.collapsed.insert(key.to_string());
        }
        v.persist_preferences(w, cx);
        cx.notify();
    }

    /// A section header: chevron, section icon, uppercase label and a right-aligned count.
    fn sidebar_header(
        &self,
        id: impl Into<ElementId>,
        glyph: &str,
        label: &str,
        count: String,
        collapsed: bool,
    ) -> ListRow {
        ListRow::new(id)
            .height(HEADER_HEIGHT)
            .inset(INSET)
            .tab_stop(!self.settings_open)
            .foreground(Role::TextSecondary)
            .font_weight(FontWeight::SEMIBOLD)
            .leading(disclosure(collapsed))
            .leading(icon(glyph).size(px(ICON_SIZE)))
            .label(label.to_uppercase())
            .trailing(
                div()
                    .flex_none()
                    .font_weight(FontWeight::NORMAL)
                    .text_role(Role::TextMuted)
                    .child(count),
            )
    }

    /// One reference section. Open sections share the sidebar's height evenly
    /// and scroll on their own, so every section header stays in view.
    fn sidebar_section(
        &self,
        id: &'static str,
        header: ListRow,
        collapsed: bool,
        body: impl IntoElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .map(|section| {
                if collapsed {
                    section.flex_none()
                } else {
                    section.flex_1().min_h_0()
                }
            })
            .child(divider())
            .child(header)
            .when(!collapsed, |section| {
                section.child(
                    scroll_area(
                        id,
                        &scroll_handle(window, cx, id),
                        div().id(id).flex_1().child(body),
                    )
                    .flex_1(),
                )
            })
    }

    /// Folder rows and leaf rows for one reference kind, indented per depth.
    fn sidebar_tree(
        &self,
        scope: &str,
        entries: Vec<TreeEntry>,
        filtering: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut tree = div().flex().flex_col().min_w_0().pb_1();
        let rows = tree_rows(
            &self.state.path,
            scope,
            entries.iter().map(|entry| entry.label.as_str()),
            &self.preferences.collapsed,
            filtering,
        );
        let mut entries: Vec<Option<TreeEntry>> = entries.into_iter().map(Some).collect();
        for row in rows {
            let (index, depth, leaf) = match row {
                TreeRow::Folder {
                    key,
                    depth,
                    name,
                    path: _,
                    collapsed,
                } => {
                    tree = tree.child(
                        ListRow::new(SharedString::from(format!("folder-{key}")))
                            .height(ROW_HEIGHT)
                            .inset(INSET)
                            .indent(FOLDER_INDENT + (depth - 1) as f32 * INDENT)
                            .tab_stop(!self.settings_open)
                            .foreground(Role::TextSecondary)
                            .leading(disclosure(collapsed))
                            .leading(
                                icon("folder")
                                    .size(px(ICON_SIZE))
                                    .text_role(Role::TextMuted),
                            )
                            .label(format!("{name}/"))
                            .on_click(cx.listener(move |v, _, w, cx| {
                                Self::toggle_collapsed(v, &key, w, cx)
                            })),
                    );
                    continue;
                }
                TreeRow::Leaf { index, depth, name } => (index, depth, name),
            };
            let Some(entry) = entries[index].take() else {
                continue;
            };
            let TreeEntry {
                id,
                glyph,
                current,
                previewed,
                tooltip,
                on_click,
                menu,
                ..
            } = entry;
            let lead = if previewed { "eye" } else { glyph };
            tree = tree.child(
                ListRow::new(SharedString::from(id))
                    .height(ROW_HEIGHT)
                    .inset(INSET)
                    .indent(ENTRY_INDENT + depth as f32 * INDENT)
                    .tab_stop(!self.settings_open)
                    .foreground(if current || previewed {
                        Role::TextDefault
                    } else {
                        Role::TextMuted
                    })
                    .leading(icon(lead).size(px(ICON_SIZE)).text_role(if previewed {
                        Role::ActionPrimary
                    } else {
                        Role::TextMuted
                    }))
                    .label(leaf)
                    .when(current, |row| {
                        row.trailing(
                            icon("check")
                                .size(px(ICON_SIZE))
                                .text_role(Role::StatusSuccessFg),
                        )
                        .font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(current && !previewed, |row| {
                        row.background(Role::StatusSuccessBg)
                    })
                    .when(previewed, |row| row.font_weight(FontWeight::SEMIBOLD))
                    .highlighted(previewed)
                    .tooltip(tooltip)
                    .on_click(cx.listener(move |v, e: &ClickEvent, w, cx| on_click(v, e, w, cx)))
                    .context_menu(move |w, cx| menu(w, cx)),
            );
        }
        tree
    }

    pub(super) fn sidebar(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.state.snapshot.is_none() {
            return div()
                .size_full()
                .bg_role(Role::SurfaceNav)
                .into_any_element();
        }
        let changes = self
            .state
            .snapshot
            .as_ref()
            .map(|s| s.status.len())
            .unwrap_or(0);
        let filter = RefFilter::new(&self.reference_search.read(cx).value());
        let view = cx.entity();
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
            .w_full()
            .flex()
            .flex_col()
            .child(
                div().flex_none().px(scaled(INSET)).py(scaled(INSET)).child(
                    Input::new(&self.reference_search)
                        .h(scaled(CONTROL_HEIGHT))
                        .disabled(self.settings_open),
                ),
            )
            .child(
                ListRow::new("working")
                    .height(WORKING_ROW_HEIGHT)
                    .inset(INSET)
                    .tab_stop(!self.settings_open)
                    .leading(icon("changes").size(px(ICON_SIZE)))
                    .label("Working changes")
                    .trailing(Badge::role(
                        changes.to_string(),
                        if changes > 0 {
                            Role::ActionPrimary
                        } else {
                            Role::TextMuted
                        },
                    ))
                    .highlighted(self.state.selected.is_none())
                    .on_click(cx.listener(|v, _, w, cx| {
                        v.select(None, w, cx);
                        v.close_references(w, cx);
                    }))
                    .context_menu({
                        let menu = command_menu(view.clone(), &[Command::Stash, Command::PopStash]);
                        move |w, cx| menu(w, cx)
                    }),
            );
        if let Some(s) = &self.state.snapshot {
            let filtering = filter.is_active();

            // Local branches.
            let local = section(
                &self.state.path,
                "",
                s.branches.iter().map(|b| b.name.as_str()),
                &self.preferences.collapsed,
                &filter,
            );
            let entries = local
                .visible
                .iter()
                .map(|&index| {
                    let name = s.branches[index].name.clone();
                    let current = s.checked_out_branch() == Some(name.as_str());
                    let previewed = self.state.is_previewing(ReferenceKind::LocalBranch, &name);
                    let menu = reference_menu(
                        view.clone(),
                        MenuReference {
                            kind: ReferenceKind::LocalBranch,
                            target: name.clone(),
                            label: name.clone(),
                            tip: s.branches[index].tip.clone(),
                            checked_out: current,
                        },
                    );
                    TreeEntry {
                        menu,
                        label: name.clone(),
                        id: format!("local-branch-{name}"),
                        glyph: "branch",
                        current,
                        previewed,
                        tooltip: name.clone(),
                        on_click: Rc::new(move |v, e, w, cx| {
                            if e.click_count() == 2 {
                                v.run_operation(Operation::Checkout, name.clone(), w, cx);
                            } else {
                                v.preview_ref(ReferenceKind::LocalBranch, &name, w, cx);
                            }
                        }),
                    }
                })
                .collect();
            let local_key = local.key;
            let header = self
                .sidebar_header(
                    "local-branches",
                    "local",
                    "Local",
                    local.count,
                    local.collapsed,
                )
                .on_click(
                    cx.listener(move |v, _, w, cx| Self::toggle_collapsed(v, &local_key, w, cx)),
                )
                .context_menu({
                    let menu = command_menu(view.clone(), &[Command::CreateBranch]);
                    move |w, cx| menu(w, cx)
                });
            let tree = self.sidebar_tree("", entries, filtering, cx);
            nav = nav.child(self.sidebar_section(
                "sidebar-local",
                header,
                local.collapsed,
                tree,
                window,
                cx,
            ));

            // Remotes and tags.
            let header_menus: [&'static [Command]; 2] = [&[Command::Fetch], &[]];
            for ((kind, scroll_id, glyph, ref_kind, refs), header_commands) in [
                (
                    "Remotes",
                    "sidebar-remotes",
                    "remote",
                    ReferenceKind::Remote,
                    &s.remote_refs,
                ),
                ("Tags", "sidebar-tags", "tag", ReferenceKind::Tag, &s.tags),
            ]
            .into_iter()
            .zip(header_menus)
            {
                let shown = section(
                    &self.state.path,
                    kind,
                    refs.iter().map(|r| r.label.as_str()),
                    &self.preferences.collapsed,
                    &filter,
                );
                let entries = shown
                    .visible
                    .iter()
                    .map(|&index| {
                        let reference = &refs[index];
                        let name = reference.full_name.clone();
                        let menu = reference_menu(
                            view.clone(),
                            MenuReference {
                                kind: ref_kind,
                                target: name.clone(),
                                label: reference.label.clone(),
                                tip: reference.tip.clone(),
                                checked_out: false,
                            },
                        );
                        TreeEntry {
                            menu,
                            label: reference.label.clone(),
                            id: format!("{kind}-{name}"),
                            glyph,
                            current: false,
                            previewed: self.state.is_previewing(ref_kind, &name),
                            tooltip: name.clone(),
                            on_click: Rc::new(move |v, _, w, cx| {
                                v.preview_ref(ref_kind, &name, w, cx);
                            }),
                        }
                    })
                    .collect();
                let key = shown.key;
                let header = self
                    .sidebar_header(
                        SharedString::from(format!("ref-heading-{kind}")),
                        glyph,
                        kind,
                        shown.count,
                        shown.collapsed,
                    )
                    .on_click(
                        cx.listener(move |v, _, w, cx| Self::toggle_collapsed(v, &key, w, cx)),
                    )
                    .when(!header_commands.is_empty(), |header| {
                        let menu = command_menu(view.clone(), header_commands);
                        header.context_menu(move |w, cx| menu(w, cx))
                    });
                let tree = self.sidebar_tree(kind, entries, filtering, cx);
                nav = nav.child(self.sidebar_section(
                    scroll_id,
                    header,
                    shown.collapsed,
                    tree,
                    window,
                    cx,
                ));
            }

            // Worktrees.
            // The filter box is one line, so a newline keeps a query from
            // matching across the branch and folder names.
            let searchable: Vec<String> = s
                .worktrees
                .iter()
                .map(|w| format!("{}\n{}", w.label(), w.folder()))
                .collect();
            let shown = section(
                &self.state.path,
                "Worktrees",
                searchable.iter().map(String::as_str),
                &self.preferences.collapsed,
                &filter,
            );
            let mut rows = div().flex().flex_col().min_w_0().pb_1();
            for &index in &shown.visible {
                let worktree = &s.worktrees[index];
                let path = worktree.path.display().to_string();
                let label = worktree.label();
                let can_open = !worktree.current && !worktree.prunable;
                let mut tooltip = display_path(&path);
                if worktree.prunable {
                    tooltip.push_str(" (missing)");
                }
                let row = ListRow::new(SharedString::from(format!("worktree-{path}")))
                    .height(ROW_HEIGHT)
                    .inset(INSET)
                    .indent(ENTRY_INDENT)
                    .tab_stop(!self.settings_open)
                    .foreground(if worktree.current {
                        Role::TextDefault
                    } else {
                        Role::TextMuted
                    })
                    .leading(
                        icon("folder-git-2")
                            .size(px(ICON_SIZE))
                            .text_role(Role::TextMuted),
                    )
                    .content(
                        div()
                            .flex()
                            .gap_1()
                            .min_w_0()
                            .child(div().flex_shrink_0().max_w_full().truncate().child(label))
                            .when_some(worktree.detail(), |content, folder| {
                                content.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_weight(FontWeight::NORMAL)
                                        .text_role(Role::TextMuted)
                                        .child(folder),
                                )
                            }),
                    )
                    .when(worktree.current, |row| {
                        row.trailing(
                            icon("check")
                                .size(px(ICON_SIZE))
                                .text_role(Role::StatusSuccessFg),
                        )
                        .font_weight(FontWeight::SEMIBOLD)
                        .background(Role::StatusSuccessBg)
                    })
                    .tooltip(tooltip);
                rows = rows.child(if can_open {
                    let target = path.clone();
                    row.on_click(move |e: &ClickEvent, w, cx| {
                        if e.click_count() == 2 {
                            w.dispatch_action(Box::new(OpenRecent(target.clone())), cx);
                        }
                    })
                    .context_menu({
                        let view = view.downgrade();
                        move |w, _| {
                            let target = MenuTarget::new(w, view.clone());
                            ContextMenu::new()
                                .item(
                                    "Open this worktree",
                                    true,
                                    target.command(MenuCommand::OpenWorktree(path.clone())),
                                )
                                .item("Open worktree in new tab", true, OpenRecent(path.clone()))
                        }
                    })
                } else {
                    row
                });
            }
            let key = shown.key;
            let header = self
                .sidebar_header(
                    "ref-heading-Worktrees",
                    "folder-git-2",
                    "Worktrees",
                    shown.count,
                    shown.collapsed,
                )
                .on_click(cx.listener(move |v, _, w, cx| Self::toggle_collapsed(v, &key, w, cx)));
            nav = nav.child(self.sidebar_section(
                "sidebar-worktrees",
                header,
                shown.collapsed,
                rows,
                window,
                cx,
            ));

            // Stashes.
            let key = section_key(&self.state.path, "Stashes");
            let collapsed = self.preferences.collapsed.contains(&key);
            let header = self
                .sidebar_header(
                    "ref-heading-Stashes",
                    "stash",
                    "Stashes",
                    s.stash_count.to_string(),
                    collapsed,
                )
                .on_click(cx.listener(move |v, _, w, cx| Self::toggle_collapsed(v, &key, w, cx)))
                .context_menu({
                    let menu = command_menu(view.clone(), &[Command::Stash, Command::PopStash]);
                    move |w, cx| menu(w, cx)
                });
            let latest = div().when_some(s.latest_stash.as_ref(), |section, stash| {
                // Pop stash acts on this entry, so name it where the button is.
                section.child(
                    div()
                        .pl(scaled(INSET + ENTRY_INDENT))
                        .pr(scaled(INSET))
                        .pb_2()
                        .flex()
                        .flex_col()
                        .text_size(scaled(TEXT_SIZE_SMALL))
                        .child(div().truncate().child(format!("Latest: {}", stash.message)))
                        .child(
                            div()
                                .text_role(Role::TextMuted)
                                .child(display_date(&stash.date)),
                        ),
                )
            });
            nav = nav.child(self.sidebar_section(
                "sidebar-stashes",
                header,
                collapsed,
                latest,
                window,
                cx,
            ));
        }
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg_role(Role::SurfaceNav)
            .overflow_hidden()
            .child(nav)
            .into_any_element()
    }
}
