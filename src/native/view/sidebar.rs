use super::*;
use std::rc::Rc;

/// Horizontal inset shared by every sidebar row, header and the filter box.
const INSET: f32 = 8.;
/// Extra indent per folder level.
const INDENT: f32 = 14.;
const HEADER_HEIGHT: f32 = 28.;
const ROW_HEIGHT: f32 = 26.;

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
    fn sidebar_divider() -> Div {
        div()
            .w_full()
            .h(px(SEAM))
            .flex_none()
            .bg(rgb(semantic(Role::BorderDivider)))
    }

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
        cx: &App,
    ) -> Button {
        self.button(id, "")
            .icon(
                icon(if collapsed { "right" } else { "down" })
                    .size(px(12.))
                    .text_color(rgb(semantic(Role::TextMuted))),
            )
            .w_full()
            .h(type_size(cx, HEADER_HEIGHT))
            .px(px(INSET))
            .justify_start()
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(type_size(cx, 11.))
            .text_color(rgb(semantic(Role::TextSecondary)))
            .tooltip(format!(
                "{} {}",
                if collapsed { "Expand" } else { "Collapse" },
                label.to_lowercase()
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon(glyph).size(px(14.)))
                    .child(div().truncate().child(label.to_uppercase())),
            )
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(rgb(semantic(Role::TextMuted)))
                    .child(count),
            )
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
        let mut seen = std::collections::BTreeSet::new();
        for entry in entries {
            let parts = entry.label.split('/').collect::<Vec<_>>();
            let mut hidden = false;
            for depth in 1..parts.len() {
                let prefix = parts[..depth].join("/");
                let key = if scope.is_empty() {
                    format!("{}|{prefix}", self.state.path)
                } else {
                    format!("{}|{scope}/{prefix}", self.state.path)
                };
                let collapsed = self.preferences.collapsed.contains(&key) && !filtering;
                if seen.insert(key.clone()) {
                    tree = tree.child(
                        self.button(SharedString::from(format!("folder-{key}")), "")
                            .icon(
                                icon(if collapsed { "right" } else { "down" })
                                    .size(px(12.))
                                    .text_color(rgb(semantic(Role::TextMuted))),
                            )
                            .w_full()
                            .h(type_size(cx, ROW_HEIGHT))
                            .justify_start()
                            .pl(px(INSET + 4. + (depth - 1) as f32 * INDENT))
                            .pr(px(INSET))
                            .text_color(rgb(semantic(Role::TextSecondary)))
                            .tooltip(format!(
                                "{} {prefix}/",
                                if collapsed { "Expand" } else { "Collapse" }
                            ))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        icon("folder")
                                            .size(px(14.))
                                            .text_color(rgb(semantic(Role::TextMuted))),
                                    )
                                    .child(
                                        div().truncate().child(format!("{}/", parts[depth - 1])),
                                    ),
                            )
                            .on_click(cx.listener(move |v, _, w, cx| {
                                Self::toggle_collapsed(v, &key, w, cx)
                            })),
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
            let depth = parts.len() - 1;
            let leaf = parts[depth].to_string();
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
                self.button(SharedString::from(id), "")
                    .icon(icon(lead).size(px(14.)).text_color(rgb(if previewed {
                        semantic(Role::ActionPrimary)
                    } else {
                        semantic(Role::TextMuted)
                    })))
                    .child(div().flex_1().min_w_0().truncate().child(leaf))
                    .when(current, |row| {
                        row.child(
                            icon("check")
                                .size(px(14.))
                                .text_color(rgb(semantic(Role::StatusSuccessFg))),
                        )
                    })
                    .w_full()
                    .h(type_size(cx, ROW_HEIGHT))
                    .justify_start()
                    .pl(px(INSET + 18. + depth as f32 * INDENT))
                    .pr(px(INSET))
                    .text_color(rgb(if current || previewed {
                        semantic(Role::TextDefault)
                    } else {
                        semantic(Role::TextMuted)
                    }))
                    .when(current, |b| b.font_weight(FontWeight::SEMIBOLD))
                    .when(current && !previewed, |b| {
                        b.bg(rgb(semantic(Role::StatusSuccessBg)))
                    })
                    .map(|b| Self::highlight_sidebar_row(b, previewed))
                    .tooltip(tooltip)
                    .on_click(cx.listener(move |v, e: &ClickEvent, w, cx| on_click(v, e, w, cx))),
            );
        }
        tree
    }

    pub(super) fn sidebar(&self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.state.snapshot.is_none() {
            return div()
                .size_full()
                .bg(rgb(semantic(Role::SurfaceNav)))
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
            .overflow_y_scroll()
            .child(
                div().px(px(INSET)).py(px(INSET)).child(
                    Input::new(&self.reference_search)
                        .h(px(CONTROL_HEIGHT))
                        .disabled(self.settings_open),
                ),
            )
            .child(
                self.button("working", "")
                    .icon(icon("changes").size(px(14.)))
                    .child(div().flex_1().min_w_0().truncate().child("Working changes"))
                    .child(badge(
                        changes.to_string(),
                        if changes > 0 {
                            semantic(Role::ActionPrimary)
                        } else {
                            semantic(Role::TextMuted)
                        },
                    ))
                    .w_full()
                    .h(type_size(cx, 32.))
                    .px(px(INSET))
                    .justify_start()
                    .map(|button| {
                        Self::highlight_sidebar_row(button, self.state.selected.is_none())
                    })
                    .on_click(cx.listener(|v, _, w, cx| {
                        v.select(None, w, cx);
                        v.close_references(w, cx);
                    })),
            );
        if let Some(s) = &self.state.snapshot {
            // A filter shows matches inside collapsed groups and counts them.
            let filtering = !query.is_empty();
            let count_label = |matched: usize, total: usize| {
                if filtering {
                    format!("{matched} of {total}")
                } else {
                    total.to_string()
                }
            };
            let matches_query = |name: &str| name.to_lowercase().contains(&query);

            // Local branches.
            let local_key = format!("{}|", self.state.path);
            let local_collapsed = self.preferences.collapsed.contains(&local_key) && !filtering;
            let local_matches = s.branches.iter().filter(|b| matches_query(&b.name)).count();
            let entries = if local_collapsed {
                vec![]
            } else {
                s.branches
                    .iter()
                    .filter(|b| matches_query(&b.name))
                    .map(|b| {
                        let name = b.name.clone();
                        let current = s.checked_out_branch() == Some(name.as_str());
                        let previewed = self.state.is_previewing(ReferenceKind::LocalBranch, &name);
                        let hint = match (current, previewed) {
                            (true, true) => "Checked out · Previewing",
                            (true, false) => "Checked out · Click to preview",
                            (false, true) => "Previewing · Double-click to checkout",
                            (false, false) => "Click to preview · Double-click to checkout",
                        };
                        TreeEntry {
                            label: name.clone(),
                            id: format!("local-branch-{name}"),
                            glyph: "branch",
                            current,
                            previewed,
                            tooltip: format!("{name} · {hint}"),
                            on_click: Rc::new(move |v, e, w, cx| {
                                if e.click_count() == 2 {
                                    v.review(Operation::Checkout, name.clone(), w, cx);
                                } else {
                                    v.preview_ref(ReferenceKind::LocalBranch, &name, w, cx);
                                }
                            }),
                        }
                    })
                    .collect()
            };
            nav = nav.child(
                div()
                    .flex()
                    .flex_col()
                    .child(Self::sidebar_divider())
                    .child(
                        self.sidebar_header(
                            "local-branches",
                            "branch",
                            "Local",
                            count_label(local_matches, s.branches.len()),
                            local_collapsed,
                            cx,
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
                let key = format!("{}|{kind}", self.state.path);
                let collapsed = self.preferences.collapsed.contains(&key) && !filtering;
                let matches = refs.iter().filter(|r| matches_query(&r.label)).count();
                let entries = if collapsed {
                    vec![]
                } else {
                    refs.iter()
                        .filter(|r| matches_query(&r.label))
                        .map(|reference| {
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
                        .collect()
                };
                nav = nav.child(
                    div()
                        .flex()
                        .flex_col()
                        .child(Self::sidebar_divider())
                        .child(
                            self.sidebar_header(
                                SharedString::from(format!("ref-heading-{kind}")),
                                glyph,
                                kind,
                                count_label(matches, refs.len()),
                                collapsed,
                                cx,
                            )
                            .on_click(cx.listener(
                                move |v, _, w, cx| Self::toggle_collapsed(v, &key, w, cx),
                            )),
                        )
                        .child(self.sidebar_tree(kind, entries, filtering, cx)),
                );
            }

            // Stashes.
            let key = format!("{}|Stashes", self.state.path);
            let collapsed = self.preferences.collapsed.contains(&key);
            nav = nav.child(
                div()
                    .flex()
                    .flex_col()
                    .child(Self::sidebar_divider())
                    .child(
                        self.sidebar_header(
                            "ref-heading-Stashes",
                            "stash",
                            "Stashes",
                            s.stash_count.to_string(),
                            collapsed,
                            cx,
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
                                    .pl(px(INSET + 18.))
                                    .pr(px(INSET))
                                    .pb_2()
                                    .flex()
                                    .flex_col()
                                    .text_size(type_size(cx, 12.))
                                    .child(
                                        div()
                                            .truncate()
                                            .child(format!("Latest · {}", stash.message)),
                                    )
                                    .child(
                                        div()
                                            .text_color(rgb(semantic(Role::TextMuted)))
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
            .bg(rgb(semantic(Role::SurfaceNav)))
            .overflow_hidden()
            .child(nav)
            .into_any_element()
    }
}
