use super::*;
use gitcrab_core::sidebar::{branch_hint, section, section_key, tree_rows, RefFilter, TreeRow};
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
            .tooltip(format!(
                "{} {}",
                if collapsed { "Expand" } else { "Collapse" },
                label.to_lowercase()
            ))
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
                    path,
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
                            .tooltip(format!(
                                "{} {path}/",
                                if collapsed { "Expand" } else { "Collapse" }
                            ))
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
                    .on_click(cx.listener(move |v, e: &ClickEvent, w, cx| on_click(v, e, w, cx))),
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
            .w_full()
            .flex()
            .flex_col()
            .child(
                div().px(scaled(INSET)).py(scaled(INSET)).child(
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
                    })),
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
                    TreeEntry {
                        label: name.clone(),
                        id: format!("local-branch-{name}"),
                        glyph: "branch",
                        current,
                        previewed,
                        tooltip: format!("{name} · {}", branch_hint(current, previewed)),
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
            nav = nav.child(
                div()
                    .flex()
                    .flex_col()
                    .child(divider())
                    .child(
                        self.sidebar_header(
                            "local-branches",
                            "local",
                            "Local",
                            local.count,
                            local.collapsed,
                        )
                        .on_click(cx.listener(move |v, _, w, cx| {
                            Self::toggle_collapsed(v, &local_key, w, cx)
                        })),
                    )
                    .child(self.sidebar_tree("", entries, filtering, cx)),
            );

            // Remotes and tags.
            for (kind, glyph, ref_kind, refs) in [
                ("Remotes", "remote", ReferenceKind::Remote, &s.remote_refs),
                ("Tags", "tag", ReferenceKind::Tag, &s.tags),
            ] {
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
                        TreeEntry {
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
                nav = nav.child(
                    div()
                        .flex()
                        .flex_col()
                        .child(divider())
                        .child(
                            self.sidebar_header(
                                SharedString::from(format!("ref-heading-{kind}")),
                                glyph,
                                kind,
                                shown.count,
                                shown.collapsed,
                            )
                            .on_click(cx.listener(
                                move |v, _, w, cx| Self::toggle_collapsed(v, &key, w, cx),
                            )),
                        )
                        .child(self.sidebar_tree(kind, entries, filtering, cx)),
                );
            }

            // Stashes.
            let key = section_key(&self.state.path, "Stashes");
            let collapsed = self.preferences.collapsed.contains(&key);
            nav = nav.child(
                div()
                    .flex()
                    .flex_col()
                    .child(divider())
                    .child(
                        self.sidebar_header(
                            "ref-heading-Stashes",
                            "stash",
                            "Stashes",
                            s.stash_count.to_string(),
                            collapsed,
                        )
                        .on_click(
                            cx.listener(move |v, _, w, cx| Self::toggle_collapsed(v, &key, w, cx)),
                        ),
                    )
                    .when_some(
                        s.latest_stash.as_ref().filter(|_| !collapsed),
                        |section, stash| {
                            // Pop stash acts on this entry, so name it where the button is.
                            section.child(
                                div()
                                    .pl(scaled(INSET + 18.))
                                    .pr(scaled(INSET))
                                    .pb_2()
                                    .flex()
                                    .flex_col()
                                    .text_size(scaled(12.))
                                    .child(
                                        div()
                                            .truncate()
                                            .child(format!("Latest · {}", stash.message)),
                                    )
                                    .child(
                                        div()
                                            .text_role(Role::TextMuted)
                                            .child(display_date(&stash.date)),
                                    ),
                            )
                        },
                    ),
            );
        }
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg_role(Role::SurfaceNav)
            .overflow_hidden()
            .child(
                scroll_area(
                    "navigation",
                    &scroll_handle(window, cx, "navigation-scroll"),
                    nav,
                )
                .flex_1(),
            )
            .into_any_element()
    }
}
