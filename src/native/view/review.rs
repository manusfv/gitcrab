use super::*;
use gitcrab_core::highlight::Span;
use std::ops::Range;

fn syntax_style(kind: char) -> HighlightStyle {
    HighlightStyle {
        color: Some(
            rgb(match kind {
                'c' => semantic(Role::TextMuted),
                's' => semantic(Role::SyntaxString),
                'k' => semantic(Role::SyntaxKeyword),
                'n' => semantic(Role::SyntaxNumber),
                _ => semantic(Role::TextDefault),
            })
            .into(),
        ),
        ..Default::default()
    }
}

fn diff_highlights(
    text: &str,
    spans: &[Span],
    prefix: usize,
    show_whitespace: bool,
) -> Vec<(Range<usize>, HighlightStyle)> {
    if !show_whitespace {
        return spans
            .iter()
            .filter(|span| span.start < text.len() + prefix)
            .map(|span| {
                (
                    span.start - prefix..(span.end - prefix).min(text.len()),
                    syntax_style(span.kind),
                )
            })
            .collect();
    }

    let mut spans = spans.iter().peekable();
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    for (start, character) in text.char_indices() {
        while spans.peek().is_some_and(|span| span.end <= start + prefix) {
            spans.next();
        }
        let end = start + character.len_utf8();
        let mut style = spans
            .peek()
            .filter(|span| span.start <= start + prefix)
            .map_or_else(HighlightStyle::default, |span| syntax_style(span.kind));
        if character.is_whitespace() {
            style.underline = Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(rgb(semantic(Role::TextMuted)).into()),
                wavy: false,
            });
        }
        if style == HighlightStyle::default() {
            continue;
        }
        if let Some((range, previous)) = highlights.last_mut() {
            if range.end == start && *previous == style {
                range.end = end;
                continue;
            }
        }
        highlights.push((start..end, style));
    }
    highlights
}

impl GitCrab {
    /// Height of one diff row in logical pixels.
    fn diff_row_height(&self) -> f32 {
        (if self.preferences.compact_diff {
            18_f32
        } else {
            20.
        })
        .max(self.preferences.code_size + 5.)
    }

    /// Hunk header rows in the displayed layout.
    fn hunk_rows(&self) -> &[usize] {
        match &self.state.patch {
            Some(p) if self.split_active => &p.content.split_hunk_rows,
            Some(p) => &p.content.hunk_rows,
            None => &[],
        }
    }

    /// Locates the reader among hunks from the diff's scroll position.
    ///
    /// Returns the position and whether the diff can scroll at all. Wheel and
    /// keyboard scrolling move the position as well as the hunk arrows.
    fn viewport_hunk(&self) -> Option<(gitcrab_core::diff::HunkPosition, bool)> {
        let rows = self.hunk_rows();
        let (offset, max) = {
            let state = self.diff_scroll.0.borrow();
            let handle = &state.base_handle;
            (
                -f32::from(handle.offset().y),
                f32::from(handle.max_offset().height),
            )
        };
        if max <= 0.5 {
            // Every row is on screen, so there is nowhere to move.
            return rows.first().map(|_| {
                (
                    gitcrab_core::diff::HunkPosition {
                        current: 0,
                        previous: None,
                        next: None,
                    },
                    false,
                )
            });
        }
        let top = (offset / self.diff_row_height() + 0.5).floor().max(0.) as usize;
        gitcrab_core::diff::hunk_position(rows, top, offset >= max - 1.)
            .map(|position| (position, true))
    }

    /// Scrolls to the next or previous hunk relative to the viewport.
    pub(super) fn step_hunk(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open || window.has_active_dialog(cx) || self.state.patch.is_none() {
            return;
        }
        let Some((position, _)) = self.viewport_hunk() else {
            return;
        };
        let target = if forward {
            position.next
        } else {
            position.previous
        };
        if let Some(row) = target.and_then(|index| self.hunk_rows().get(index).copied()) {
            self.diff_scroll.scroll_to_item(row, ScrollStrategy::Top);
            cx.notify();
        }
    }

    /// Names what each side of the comparison shows.
    fn comparison_sides(&self, p: &gitcrab_core::controller::Patch) -> (String, String) {
        let before_path = p
            .original_path
            .as_ref()
            .map(|old| format!(" · {}", gitcrab_core::git::display_path(old)))
            .unwrap_or_default();
        let (before, after) = match &p.id {
            Some(id) => (
                match self
                    .commit_parents(id)
                    .and_then(|parents| parents.first().cloned())
                {
                    Some(parent) => format!("Parent {}", short_id(&parent)),
                    None => "Empty tree".into(),
                },
                format!("Commit {}", short_id(id)),
            ),
            None if p.staged => ("HEAD".into(), "Index".into()),
            None => ("Index".into(), "Working tree".into()),
        };
        (
            format!("Before · {before}{before_path}"),
            format!("After · {after}"),
        )
    }

    fn commit_parents(&self, id: &str) -> Option<Vec<String>> {
        self.state
            .snapshot
            .as_ref()?
            .commits
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.parents.clone())
    }

    /// Describes the comparison baseline, naming a commit's parent.
    fn comparison_label(&self, p: &gitcrab_core::controller::Patch, conflict: bool) -> String {
        if let Some(id) = &p.id {
            return match self.commit_parents(id).as_deref() {
                Some([]) => "Initial commit · compared to an empty tree".into(),
                Some([parent]) => format!("Compared to parent {}", short_id(parent)),
                Some([first, ..]) => {
                    format!(
                        "Merge commit · compared to first parent {}",
                        short_id(first)
                    )
                }
                None => "Compared to its first parent".into(),
            };
        }
        if conflict {
            "Combined diff of both sides · edit the file, then mark it resolved".into()
        } else if p.staged {
            "Staged · HEAD → index".into()
        } else {
            "Unstaged · index → working tree".into()
        }
    }

    /// A purpose-built panel for comparisons without text hunks.
    fn preview_panel(
        &self,
        p: &gitcrab_core::controller::Patch,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use gitcrab_core::diff::Preview;
        let working = p.id.is_none();
        let path = gitcrab_core::git::display_path(&p.path);
        let (title, detail): (&str, String) = match p.preview {
            Preview::Binary => (
                "Binary file changed",
                if working {
                    "Binary content has no text preview. Stage file above acts on the whole file."
                        .into()
                } else {
                    "Binary content has no text preview.".into()
                },
            ),
            Preview::Renamed => (
                "Renamed · contents unchanged",
                "Only the path changed.".into(),
            ),
            Preview::ModeChange => (
                "File mode changed · contents unchanged",
                "Only the file's permissions or type bits changed.".into(),
            ),
            Preview::EmptyAdded => (
                "New empty file",
                if working {
                    "The file has no content. Stage file above adds it to the index.".into()
                } else {
                    "This commit added the file with no content.".into()
                },
            ),
            Preview::EmptyDeleted => ("Empty file deleted", "The file had no content.".into()),
            Preview::Oversized => (
                "Too large to preview",
                if working {
                    "The patch exceeds the preview limit (8 MiB, 50,000 lines, or 8,192 bytes per line). Stage file above still acts on the whole file.".into()
                } else {
                    "The patch exceeds the preview limit (8 MiB, 50,000 lines, or 8,192 bytes per line). Inspect it with Git from a terminal.".into()
                },
            ),
            Preview::NoChanges | Preview::Text => (
                "No changes on this side",
                format!(
                    "{path} has no {} changes.",
                    if p.staged { "staged" } else { "unstaged" }
                ),
            ),
        };
        let other_side =
            (working && p.preview == Preview::NoChanges && self.state.has_side(&p.path, !p.staged))
                .then(|| (p.path.clone(), !p.staged));
        let field = |label: &'static str, value: String| {
            div()
                .flex()
                .gap_2()
                .child(
                    div()
                        .w(px(80.))
                        .flex_none()
                        .text_color(rgb(semantic(Role::TextMuted)))
                        .child(label),
                )
                .child(div().min_w_0().font_family(CODE_FONT).child(value))
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .p_4()
            .child(icon("changes").text_color(rgb(semantic(Role::TextMuted))))
            .child(div().text_size(type_size(cx, 16.)).child(title))
            .when(p.preview == Preview::Renamed, |panel| {
                panel.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(field(
                            "Old path",
                            p.original_path
                                .as_deref()
                                .map(gitcrab_core::git::display_path)
                                .unwrap_or_default(),
                        ))
                        .child(field("New path", path.clone())),
                )
            })
            .child(
                div()
                    .max_w(px(520.))
                    .text_color(rgb(semantic(Role::TextMuted)))
                    .child(detail),
            )
            .when_some(other_side, |panel, (path, staged)| {
                panel.child(
                    self.action_button(
                        "preview-other-side",
                        if staged {
                            "View staged changes"
                        } else {
                            "View unstaged changes"
                        },
                    )
                    .on_click(cx.listener(move |v, _, w, cx| v.file(path.clone(), staged, w, cx))),
                )
            })
            .into_any_element()
    }

    /// Renders the loaded patch in unified or split mode.
    ///
    /// # Panics
    ///
    /// Panics if called before a patch has loaded.
    pub(super) fn diff(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.state.patch.as_ref().unwrap();
        let staged = p.staged;
        let text = p.preview.has_layout();
        let conflict = p.id.is_none()
            && self
                .state
                .snapshot
                .as_ref()
                .is_some_and(|s| s.status.iter().any(|f| f.path == p.path && f.is_unmerged()));
        let n = if self.split_active {
            p.content.split.len()
        } else {
            p.content.lines.len()
        };
        let weak = cx.weak_entity();
        let display = gitcrab_core::git::display_path(&p.path);
        let (directory, name) = match display.rfind(['/', '\\']) {
            Some(cut) => (display[..=cut].to_string(), display[cut + 1..].to_string()),
            None => (String::new(), display.clone()),
        };
        let path_tooltip = match &p.original_path {
            Some(old) => format!(
                "{} → {display} · repository-relative paths",
                gitcrab_core::git::display_path(old)
            ),
            None => format!("{display} · repository-relative path"),
        };
        let back_label = match &p.id {
            Some(id) => format!("Commit {}", short_id(id)),
            None => "Working changes".into(),
        };
        let back_tooltip = match &p.id {
            Some(id) => format!(
                "Close the diff and return to commit {} in history · Escape",
                short_id(id)
            ),
            None => "Close the diff and return to working changes · Escape".into(),
        };
        // Route back, file identity and the inspector toggle.
        let header = section_bar()
            .child(
                self.button("back", back_label)
                    .icon(icon("back"))
                    .flex_none()
                    .tooltip(back_tooltip)
                    .on_click(cx.listener(|v, _, w, cx| v.leave_diff(w, cx))),
            )
            .child(
                div()
                    .id("diff-path")
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::MEDIUM)
                            .child(name),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(rgb(semantic(Role::TextMuted)))
                            .child(match &p.original_path {
                                Some(old) => format!(
                                    "{directory} · renamed from {}",
                                    gitcrab_core::git::display_path(old)
                                ),
                                None => directory,
                            }),
                    )
                    .tooltip(move |w, cx| Tooltip::new(path_tooltip.clone()).build(w, cx)),
            )
            .when(self.diff_updated, |bar| {
                bar.child(
                    div()
                        .id("diff-updated")
                        .child(badge("Updated on disk", semantic(Role::StatusInfoFg)))
                        .tooltip(|w, cx| {
                            Tooltip::new(
                                "The file changed outside GitCrab. The diff was reloaded; a selection stays only where its lines are unchanged.",
                            )
                            .build(w, cx)
                        }),
                )
            })
            .when(!self.single_pane, |bar| {
                bar.child(
                    self.button(
                        "toggle-inspector",
                        if self.inspector_collapsed {
                            "Show details"
                        } else {
                            "Hide details"
                        },
                    )
                    .flex_none()
                    .tooltip("Show or hide the details pane to give the diff more room")
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.inspector_collapsed = !v.inspector_collapsed;
                        cx.notify();
                    })),
                )
            });
        // Comparison controls, grouped: source, baseline and layout.
        let source = (p.id.is_none() && !conflict).then(|| {
            segmented_group().children([false, true].into_iter().map(|side| {
                let available = side == staged || self.state.has_side(&p.path, side);
                let label = if side { "Staged" } else { "Unstaged" };
                self.segment_button(
                    if side {
                        "source-staged"
                    } else {
                        "source-unstaged"
                    },
                    label,
                    side == staged,
                )
                .disabled_when(!available)
                .tooltip(if available {
                    format!("Show {} changes", label.to_lowercase())
                } else {
                    format!("This file has no {} changes", label.to_lowercase())
                })
                .on_click(cx.listener(move |v, _, w, cx| {
                    if let Some(p) = &v.state.patch {
                        if p.staged != side {
                            v.file(p.path.clone(), side, w, cx);
                        }
                    }
                }))
            }))
        });
        let layout_note = (self.split && text && !self.split_active)
            .then_some("Split needs a wider pane · showing Unified");
        let comparison = section_bar()
            .h_auto()
            .min_h(px(SECTION_HEIGHT))
            .py_1()
            .flex_wrap()
            .when(conflict, |bar| {
                bar.child(badge("Conflict", semantic(Role::StatusConflictFg)))
            })
            .when_some(source, |bar, source| bar.child(source))
            .child(
                div()
                    .text_size(type_size(cx, 12.))
                    .text_color(rgb(semantic(Role::TextMuted)))
                    .child(self.comparison_label(p, conflict)),
            )
            .child(div().flex_1())
            .when_some(layout_note, |bar, note| {
                bar.child(
                    div()
                        .id("split-fallback")
                        .text_size(type_size(cx, 12.))
                        .text_color(rgb(semantic(Role::StatusWarningFg)))
                        .child(note)
                        .tooltip(|w, cx| {
                            Tooltip::new(
                                "Both sides do not fit. Hide details or widen the window to compare side by side.",
                            )
                            .build(w, cx)
                        }),
                )
            })
            .child(
                segmented_group()
                    .child(
                        self.segment_button("unified-mode", "Unified", text && !self.split)
                            .disabled_when(!text)
                            .tooltip(if text {
                                "One column with removals and additions"
                            } else {
                                "Layouts apply to text changes"
                            })
                            .on_click(cx.listener(|v, _, w, cx| {
                                v.split = false;
                                v.selection = None;
                                v.persist_preferences(w, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        self.segment_button("split-mode", "Split", text && self.split)
                            .disabled_when(!text)
                            .tooltip(if text {
                                "Before and After side by side when the pane is wide enough"
                            } else {
                                "Layouts apply to text changes"
                            })
                            .on_click(cx.listener(|v, _, w, cx| {
                                v.split = true;
                                v.selection = None;
                                v.persist_preferences(w, cx);
                                cx.notify();
                            })),
                    ),
            )
            .when(conflict, |el| {
                let path = p.path.clone();
                el.child(
                    self.action_button("resolve-file", "Mark resolved…")
                        .disabled_when(self.state.blocking_write())
                        .tooltip("Review staging this working-tree file as the resolution")
                        .on_click(cx.listener(move |v, _, w, cx| {
                            v.review_resolution(path.clone(), w, cx)
                        })),
                )
            })
            .when(p.id.is_none() && !conflict, |el| {
                el.child(
                    self.semantic_button(
                        "stage-file",
                        if staged { "Unstage file" } else { "Stage file" },
                        if staged {
                            components::ButtonKind::Unstage
                        } else {
                            components::ButtonKind::Stage
                        },
                        ControlSize::Compact,
                        cx,
                    )
                    .icon(icon("check"))
                    .tooltip(if staged {
                        "Unstage the whole file · Alt+S"
                    } else {
                        "Stage the whole file · Alt+S"
                    })
                    .disabled_when(
                        self.state.blocking_write()
                            || !self.state.snapshot.as_ref().is_some_and(|s| {
                                s.status.iter().any(|f| {
                                    f.path == p.path && Controller::on_side(f, staged)
                                })
                            }),
                    )
                    .on_click(cx.listener(|v, _, w, cx| {
                        if let Some(p) = &v.state.patch {
                            let Some(reviewed) = v
                                .state
                                .snapshot
                                .as_ref()
                                .and_then(|s| {
                                    s.status
                                        .iter()
                                        .find(|f| f.path == p.path && f.on_side(p.staged))
                                })
                                .cloned()
                            else {
                                return;
                            };
                            v.mutate(
                                Action::StageFile {
                                    reviewed,
                                    unstage: p.staged,
                                },
                                w,
                                cx,
                            );
                        }
                    })),
                )
            });
        let el = panel()
            .bg(rgb(semantic(Role::SurfaceCanvas)))
            .child(header)
            .child(comparison);
        if !text || p.content.lines.is_empty() {
            return el.child(self.preview_panel(p, cx)).into_any_element();
        }
        el.child(self.diff_tools(p, conflict, cx))
            .when(self.diff_find_open, |el| el.child(self.diff_find_bar(cx)))
            .when(self.split_active, |el| {
                let (before, after) = self.comparison_sides(p);
                let label = |text: String| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_2()
                        .truncate()
                        .text_size(type_size(cx, 12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(semantic(Role::TextSecondary)))
                        .child(text)
                };
                el.child(
                    div()
                        .h(type_size(cx, 24.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .bg(rgb(semantic(Role::SurfaceChrome)))
                        .border_b_1()
                        .border_color(rgb(semantic(Role::BorderDivider)))
                        .child(label(before))
                        .child(
                            div()
                                .w(px(1.))
                                .h_full()
                                .bg(rgb(semantic(Role::BorderDivider))),
                        )
                        .child(label(after)),
                )
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    // The previous patch stays in place, dimmed, until this
                    // tab's write is reflected, instead of blanking the pane.
                    // A hunk or line write fades only the rows it changes.
                    .when(
                        !self.state.patch_ready()
                            && (self.state.busy || self.own_write)
                            && self.pending_on_screen().is_none(),
                        |el| el.opacity(0.55),
                    )
                    .child(
                        uniform_list(
                            SharedString::from(format!("diff-{}-{}", p.path, self.split_active)),
                            n,
                            move |range, _, cx| {
                                weak.update(cx, |v, cx| {
                                    range.map(|i| v.diff_row(i, cx)).collect::<Vec<_>>()
                                })
                                .unwrap_or_default()
                            },
                        )
                        .with_horizontal_sizing_behavior(if self.split_active {
                            ListHorizontalSizingBehavior::FitList
                        } else {
                            ListHorizontalSizingBehavior::Unconstrained
                        })
                        .track_scroll(self.diff_scroll.clone())
                        .size_full(),
                    )
                    .when(!self.split_active, |el| {
                        // Shows where a wide unified diff is scrolled horizontally.
                        el.child(
                            div().absolute().inset_0().child(
                                gpui_component::scroll::Scrollbar::horizontal(&self.diff_scroll)
                                    .id("diff-horizontal-scrollbar")
                                    .scrollbar_show(gpui_component::scroll::ScrollbarShow::Always),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }

    /// Hunk navigation, actions for the hunk in view, and content tools.
    fn diff_tools(
        &self,
        p: &gitcrab_core::controller::Patch,
        conflict: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let total = self.hunk_rows().len();
        let position = self.viewport_hunk();
        let mut bar = section_bar()
            .h_auto()
            .min_h(px(SECTION_HEIGHT))
            .py_1()
            .flex_wrap();
        if let Some((position, scrollable)) = position {
            for (index, forward, glyph, name, target) in [
                (0, false, "up", "Previous hunk · Alt+Up", position.previous),
                (1, true, "down", "Next hunk · Alt+Down", position.next),
            ] {
                bar = bar.child(
                    self.icon_button(("hunk-nav", index as usize), glyph, name)
                        .disabled_when(target.is_none())
                        .on_click(cx.listener(move |v, _, w, cx| v.step_hunk(forward, w, cx))),
                );
            }
            bar = bar.child(
                div()
                    .text_size(type_size(cx, 12.))
                    .text_color(rgb(semantic(Role::TextMuted)))
                    .child(if scrollable || total == 1 {
                        format!("Hunk {} of {total}", position.current + 1)
                    } else {
                        format!("{total} hunks, all in view")
                    }),
            );
            // Full-size actions: the selected lines when rows with changes are
            // selected, otherwise the hunk in view. Header buttons stay compact.
            let index = position.current;
            let lines = self.selected_changes();
            if self.hunk_actions_allowed(p, conflict) && !lines.is_empty() {
                bar = self.line_actions(bar.child(separator()), p, lines, "toolbar", cx);
            } else if self.hunk_actions_allowed(p, conflict) && index < p.hunks.len() {
                let token = p.token;
                let unstage = p.staged;
                let ready = self.diff_actions_ready();
                let title = p.hunks[index].clone();
                bar = bar
                    .child(separator())
                    .child(
                        self.semantic_button(
                            "current-hunk-stage",
                            format!(
                                "{} hunk {}",
                                if unstage { "Unstage" } else { "Stage" },
                                index + 1
                            ),
                            if unstage {
                                components::ButtonKind::Unstage
                            } else {
                                components::ButtonKind::Stage
                            },
                            ControlSize::Compact,
                            cx,
                        )
                        .disabled_when(!ready)
                        .tooltip(format!("{title} · select rows to act on single lines"))
                        .on_click(cx.listener(move |v, _, w, cx| {
                            v.mutate(
                                Action::Hunk {
                                    token,
                                    index,
                                    unstage,
                                },
                                w,
                                cx,
                            )
                        })),
                    )
                    .when(!unstage, |bar| {
                        bar.child(
                            self.semantic_button(
                                "current-hunk-discard",
                                format!("Discard hunk {}…", index + 1),
                                components::ButtonKind::Discard,
                                ControlSize::Compact,
                                cx,
                            )
                            .disabled_when(!ready)
                            .tooltip(format!(
                                "Review discarding only this working-copy hunk · {title}"
                            ))
                            .on_click(cx.listener(
                                move |v, _, w, cx| {
                                    v.review_discard(token, DiscardTarget::Hunk(index), w, cx)
                                },
                            )),
                        )
                    });
            }
        }
        let path = gitcrab_core::git::display_path(&p.path);
        // The latest outcome for this file, next to the controls that caused it.
        let outcome = self
            .activity
            .front()
            .filter(|record| {
                record.target.as_deref() == Some(path.as_str())
                    && record.at.elapsed() < std::time::Duration::from_secs(60)
            })
            .cloned();
        // Writes to other files leave this diff as it is and say nothing here.
        let writing = self.state.write_touches_patch();
        let refreshing = !writing && !self.state.patch_ready();
        bar.when(writing || refreshing, |bar| {
            bar.child(
                div()
                    .text_size(type_size(cx, 12.))
                    .text_color(rgb(semantic(Role::TextSecondary)))
                    .child(if refreshing {
                        "Updating diff…".to_string()
                    } else {
                        self.running_label()
                    }),
            )
        })
        .when_some(outcome.filter(|_| !writing), |bar, record| {
            bar.child(
                div()
                    .text_size(type_size(cx, 12.))
                    .text_color(rgb(semantic(if record.failure.is_some() {
                        Role::StatusErrorFg
                    } else {
                        Role::StatusSuccessFg
                    })))
                    .child(record.details.clone()),
            )
        })
        .when_some(p.hunk_blocked.clone(), |bar, reason| {
            bar.child(div().text_size(type_size(cx, 12.)).child(reason))
        })
        .child(div().flex_1())
        .child(
            self.icon_button("diff-find", "search", "Find in diff · Ctrl/Cmd+F")
                .selected(self.diff_find_open)
                .on_click(cx.listener(|v, _, w, cx| {
                    if v.diff_find_open {
                        v.close_diff_find(w, cx)
                    } else {
                        v.open_diff_find(w, cx)
                    }
                })),
        )
        .child(
            self.button("show-whitespace", "Whitespace")
                .selected(self.show_whitespace)
                .tooltip("Mark spaces and tabs")
                .on_click(cx.listener(|v, _, _, cx| {
                    v.show_whitespace = !v.show_whitespace;
                    cx.notify();
                })),
        )
        .child(
            self.button("selection-help", "Help")
                .tooltip("Selection, copy and navigation shortcuts")
                .on_click(|_, w, cx| {
                    w.open_dialog(cx, |d, _, _| {
                        d.title("Diff help").alert().child(
                            "Click or Shift-click to select rows; Ctrl/Cmd+A selects all. \
                             With rows selected in working changes, Stage, Unstage or Discard acts on just those added and removed lines; \
                             in Split a selected row includes both sides. \
                             Ctrl/Cmd+C copies source text: in Split it copies the side you clicked. \
                             Ctrl/Cmd+Shift+C copies the selection as a patch with +/- markers. \
                             Copy leaves the clipboard unchanged when nothing is selected. \
                             Alt+Up and Alt+Down move between hunks; Ctrl/Cmd+F finds text, Enter or F3 moves to the next match. \
                             Alt+Left and Alt+Right scroll a wide Unified diff. Changing layouts clears the selection.",
                        )
                    });
                }),
        )
        .into_any_element()
    }

    /// The Find in diff field with its match position.
    fn diff_find_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self.diff_search.read(cx).value();
        let status = match (self.diff_match, self.diff_matches.len()) {
            _ if query.trim().is_empty() => "Type to find".to_string(),
            (_, 0) => "No matches".to_string(),
            (Some(index), total) => format!("{} of {total}", index + 1),
            (None, total) => format!("{total} matches"),
        };
        section_bar()
            .child(div().w(px(260.)).child(Input::new(&self.diff_search)))
            .child(
                div()
                    .text_size(type_size(cx, 12.))
                    .text_color(rgb(semantic(Role::TextMuted)))
                    .child(status),
            )
            .child(
                self.icon_button(
                    "diff-find-prev",
                    "up",
                    "Previous match · Shift+Enter or Shift+F3",
                )
                .disabled_when(self.diff_matches.is_empty())
                .on_click(cx.listener(|v, _, w, cx| v.diff_find_step(-1, w, cx))),
            )
            .child(
                self.icon_button("diff-find-next", "down", "Next match · Enter or F3")
                    .disabled_when(self.diff_matches.is_empty())
                    .on_click(cx.listener(|v, _, w, cx| v.diff_find_step(1, w, cx))),
            )
            .child(
                self.button("diff-find-close", "×")
                    .tooltip("Close Find · Escape")
                    .on_click(cx.listener(|v, _, w, cx| v.close_diff_find(w, cx))),
            )
            .into_any_element()
    }
    /// Shows the selected file once its patch is loading, unavailable, or done on this side.
    ///
    /// A finished file stays selected with explicit routes to its other side
    /// and to the next unstaged file, instead of opening a different file.
    pub(super) fn review_placeholder(&self, cx: &mut Context<Self>) -> AnyElement {
        let loading = self.state.diff_loading();
        let completed = self.state.completed_review().cloned();
        let conflict = self.state.snapshot.as_ref().and_then(|s| {
            s.conflicts()
                .first()
                .map(|&index| (s.status[index].path.clone(), s.conflict_count()))
        });
        let other_side = completed
            .as_ref()
            .filter(|done| self.state.has_side(&done.path, !done.staged))
            .map(|done| (done.path.clone(), !done.staged));
        let next = completed
            .as_ref()
            .and_then(|done| self.state.next_file(false, &done.path));
        let (title, detail) = if loading && self.state.busy {
            (
                self.running_label(),
                "The diff reloads when Git finishes.".to_string(),
            )
        } else if loading {
            (
                "Loading changes…".to_string(),
                "Your selection is being updated.".to_string(),
            )
        } else if self.state.error {
            (
                "Changes could not be loaded".to_string(),
                "Choose the file again or refresh the repository.".to_string(),
            )
        } else if let Some(done) = &completed {
            let side = if done.staged { "staged" } else { "unstaged" };
            let path = gitcrab_core::git::display_path(&done.path);
            let detail = match (&other_side, &next, &conflict) {
                (Some((_, true)), _, _) => "All of its changes are staged now.".to_string(),
                (Some((_, false)), _, _) => "All of its changes are unstaged now.".to_string(),
                _ => "The file has no remaining changes.".to_string(),
            };
            let detail = match (&next, &conflict) {
                (_, Some((_, count))) => format!(
                    "{detail} {count} conflicted {} still need resolution.",
                    if *count == 1 { "file" } else { "files" }
                ),
                (None, None) if !done.staged => format!("{detail} No other unstaged files remain."),
                _ => detail,
            };
            (format!("No {side} changes left in {path}"), detail)
        } else {
            (
                "No text changes to show".to_string(),
                "Choose another file from Working changes.".to_string(),
            )
        };
        panel()
            .items_center()
            .justify_center()
            .gap_3()
            .p_4()
            .child(
                icon(if loading { "refresh" } else { "check" })
                    .text_color(rgb(semantic(Role::ActionPrimary))),
            )
            .child(div().text_size(type_size(cx, 16.)).child(title))
            .child(
                div()
                    .text_color(rgb(semantic(Role::TextMuted)))
                    .child(detail),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap_2()
                    .when_some(other_side.filter(|_| !loading), |row, (path, staged)| {
                        row.child(
                            self.action_button(
                                "review-other-side",
                                if staged {
                                    "View staged diff"
                                } else {
                                    "View unstaged diff"
                                },
                            )
                            .on_click(
                                cx.listener(move |v, _, w, cx| v.file(path.clone(), staged, w, cx)),
                            ),
                        )
                    })
                    .when_some(next.filter(|_| !loading), |row, path| {
                        row.child(
                            self.action_button("review-next-file", "Next unstaged file")
                                .tooltip(gitcrab_core::git::display_path(&path))
                                .on_click(cx.listener(move |v, _, w, cx| {
                                    v.file(path.clone(), false, w, cx)
                                })),
                        )
                    })
                    .when_some(conflict.filter(|_| !loading), |row, (path, _)| {
                        row.child(
                            self.action_button("review-next-conflict", "Review conflicts")
                                .on_click(cx.listener(move |v, _, w, cx| {
                                    v.file(path.clone(), false, w, cx)
                                })),
                        )
                    })
                    .child(
                        self.action_button("review-history", "Return to history")
                            .on_click(cx.listener(|v, _, w, cx| v.leave_diff(w, cx))),
                    ),
            )
            .into_any_element()
    }
    /// Whether this comparison offers hunk and line actions at all.
    fn hunk_actions_allowed(&self, p: &gitcrab_core::controller::Patch, conflict: bool) -> bool {
        p.id.is_none() && !conflict && p.hunk_blocked.is_none() && !p.hunks.is_empty()
    }

    /// Whether the patch on screen is current and no write is running.
    fn diff_actions_ready(&self) -> bool {
        self.state.patch_ready() && !self.state.blocking_write()
    }

    /// Changed lines in the selected rows, as indices into the patch lines.
    fn selected_changes(&self) -> Vec<usize> {
        match (&self.state.patch, &self.selection) {
            (Some(p), Some(range)) => {
                gitcrab_core::diff::selected_changes(&p.content, self.split_active, range.clone())
            }
            _ => vec![],
        }
    }

    /// Stage or Unstage, and Discard, for the selected changed `lines`.
    fn line_actions<E: ParentElement + FluentBuilder>(
        &self,
        bar: E,
        p: &gitcrab_core::controller::Patch,
        lines: Vec<usize>,
        place: &'static str,
        cx: &mut Context<Self>,
    ) -> E {
        let token = p.token;
        let unstage = p.staged;
        let ready = self.diff_actions_ready();
        let count = lines.len();
        let noun = if count == 1 { "line" } else { "lines" };
        let rows = lines.clone();
        bar.child(
            self.semantic_button(
                SharedString::from(format!("{place}-lines-stage")),
                format!(
                    "{} {count} {noun}",
                    if unstage { "Unstage" } else { "Stage" }
                ),
                if unstage {
                    components::ButtonKind::Unstage
                } else {
                    components::ButtonKind::Stage
                },
                ControlSize::Compact,
                cx,
            )
            .disabled_when(!ready)
            .tooltip(if unstage {
                "Move only the selected added and removed lines out of the index"
            } else {
                "Add only the selected added and removed lines to the index"
            })
            .on_click(cx.listener(move |v, _, w, cx| {
                v.mutate(
                    Action::Lines {
                        token,
                        rows: rows.clone(),
                        unstage,
                    },
                    w,
                    cx,
                )
            })),
        )
        .when(!unstage, |bar| {
            bar.child(
                self.semantic_button(
                    SharedString::from(format!("{place}-lines-discard")),
                    format!("Discard {count} {noun}…"),
                    components::ButtonKind::Discard,
                    ControlSize::Compact,
                    cx,
                )
                .disabled_when(!ready)
                .tooltip("Review discarding only the selected working-copy lines")
                .on_click(cx.listener(move |v, _, w, cx| {
                    v.review_discard(token, DiscardTarget::Lines(lines.clone()), w, cx)
                })),
            )
        })
    }

    fn review_discard(
        &mut self,
        token: u32,
        target: DiscardTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = self
            .state
            .patch
            .as_ref()
            .filter(|p| p.id.is_none() && !p.staged && p.token == token)
        else {
            return;
        };
        if self.state.busy {
            return;
        }
        let path = p.path.clone();
        let epoch = self.state.epoch;
        let repo = self.state.path.clone();
        let weak = cx.weak_entity();
        let cancel_focus = cx.focus_handle();
        let initial_focus = cancel_focus.clone();
        let (title, what, confirm) = match &target {
            DiscardTarget::Hunk(_) => (
                "Discard working-copy hunk?".to_string(),
                "the reviewed hunk".to_string(),
                "Discard hunk".to_string(),
            ),
            DiscardTarget::Lines(lines) => {
                let noun = if lines.len() == 1 { "line" } else { "lines" };
                (
                    format!("Discard {} working-copy {noun}?", lines.len()),
                    format!("the {} selected {noun}", lines.len()),
                    format!("Discard {noun}"),
                )
            }
        };
        window.open_dialog(cx,move |dialog,_,_| {
            let weak=weak.clone();
            let cancel_focus=cancel_focus.clone();
            let target=target.clone();
            let confirm=confirm.clone();
            dialog.title(title.clone()).confirm().p_4()
                .child(div().flex().flex_col().gap_2().child(repo.clone()).child(path.clone())
                    .child(format!("Permanently remove only {what} from the working copy. Staged content stays in the index. Unsupported encodings or change types will be rejected."))
                    .child(format!("Enter cancels. Choose {confirm} explicitly to remove these changes.")))
                .on_ok(|_,_,_| true)
                .footer(move |_,cancel,w,cx| {
                    let cancel=div().track_focus(&cancel_focus).rounded(px(4.))
                        .focus(|s|s.border_2().border_color(rgb(semantic(Role::InteractionFocus))))
                        .child(cancel(w,cx)).into_any_element();
                    let weak=weak.clone();
                    let target=target.clone();
                    let discard=components::style(
                        Button::new("confirm-discard-hunk").label(confirm.clone()).tab_stop(true),
                        components::ButtonKind::Discard,ControlSize::Regular,cx)
                        .on_click(move |_,w,cx| {
                            let action = match &target {
                                DiscardTarget::Hunk(index) => Action::DiscardHunk{token,index:*index},
                                DiscardTarget::Lines(rows) => Action::DiscardLines{token,rows:rows.clone()},
                            };
                            let _=weak.update(cx,|v,cx| {
                                if v.state.epoch==epoch && !v.state.busy {v.mutate(action,w,cx);}
                            });
                            w.close_dialog(cx);
                        }).into_any_element();
                    vec![cancel,discard]
                })
        });
        window.on_next_frame(move |w, _| initial_focus.focus(w));
    }
    /// Scrolls a wide Unified diff horizontally. Split always fits its pane.
    pub(super) fn keyboard_pan_diff(
        &self,
        after: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.split_active
            && self.state.patch.is_some()
            && !self.settings_open
            && !window.has_active_dialog(cx)
        {
            self.pan_diff(after);
            cx.notify();
        }
    }
    fn pan_diff(&self, right: bool) {
        let handle = self.diff_scroll.0.borrow().base_handle.clone();
        let mut offset = handle.offset();
        let step = px(240.);
        offset.x = if right {
            (offset.x - step).max(-handle.max_offset().width)
        } else {
            (offset.x + step).min(px(0.))
        };
        handle.set_offset(offset);
    }
    /// A hunk header spanning the whole row in either layout. Its actions
    /// come first so a long scope line or a narrow Split pane cannot hide them.
    fn hunk_header_row(&self, i: usize, line: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = self.state.patch.as_ref().unwrap();
        let l = &p.content.lines[line];
        let index = p.content.hunk_rows.binary_search(&line).ok();
        let total = p.content.hunk_rows.len();
        let (ranges, scope) = gitcrab_core::diff::hunk_header_parts(&l.text);
        let conflict = self
            .state
            .snapshot
            .as_ref()
            .is_some_and(|s| s.status.iter().any(|f| f.path == p.path && f.is_unmerged()));
        let actions =
            index.filter(|&index| self.hunk_actions_allowed(p, conflict) && index < p.hunks.len());
        let ready = self.diff_actions_ready();
        let scale = self.preferences.code_size / 12.;
        let advance = self.preferences.code_size * 0.65;
        let mut row = div()
            .id(("diff-line", i))
            .w_full()
            .when(!self.split_active, |row| {
                row.min_w(px(
                    (p.content.columns as f32 * advance + 160. * scale).max(600.)
                ))
            })
            .h(px(self.diff_row_height()))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .overflow_hidden()
            .bg(rgb(semantic(Role::DiffHunkBg)))
            .when(line > 0, |row| {
                row.border_t_1()
                    .border_color(rgb(semantic(Role::BorderDivider)))
            })
            .text_size(type_size(cx, 12.))
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(semantic(Role::TextSecondary)))
                    .child(match index {
                        Some(index) => format!("Hunk {} of {total}", index + 1),
                        None => "Hunk".into(),
                    }),
            );
        let pending = index.and_then(|index| {
            self.pending_on_screen()
                .filter(|change| change.touches_hunk(&p.content, index))
                .map(|change| change.label)
        });
        if let Some(label) = pending {
            // The write is under way: say so where its buttons were.
            row = row.child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(semantic(Role::TextSecondary)))
                    .child(label),
            );
        } else if let Some(index) = actions {
            let token = p.token;
            let unstage = p.staged;
            let selected: Vec<usize> = self
                .selected_changes()
                .into_iter()
                .filter(|&s| gitcrab_core::diff::hunk_of(&p.content, s) == Some(index))
                .collect();
            if !selected.is_empty() {
                row = self.line_actions(row, p, selected, "hunk", cx);
                row = row.child(
                    div()
                        .flex_none()
                        .w(px(1.))
                        .h(px(14.))
                        .mx_1()
                        .bg(rgb(semantic(Role::BorderDivider))),
                );
            }
            row = row.child(
                self.semantic_button(
                    ("hunk", i),
                    if unstage {
                        "Unstage hunk"
                    } else {
                        "Stage hunk"
                    },
                    if unstage {
                        components::ButtonKind::Unstage
                    } else {
                        components::ButtonKind::Stage
                    },
                    ControlSize::DenseHunk,
                    cx,
                )
                .disabled_when(!ready)
                .on_click(cx.listener(move |v, _, w, cx| {
                    v.mutate(
                        Action::Hunk {
                            token,
                            index,
                            unstage,
                        },
                        w,
                        cx,
                    )
                })),
            );
            // Keep the permanent action apart from the common staging action.
            if !unstage {
                row = row
                    .child(
                        div()
                            .flex_none()
                            .w(px(1.))
                            .h(px(14.))
                            .mx_1()
                            .bg(rgb(semantic(Role::BorderDivider))),
                    )
                    .child(
                        self.semantic_button(
                            ("discard-hunk", i),
                            "Discard hunk…",
                            components::ButtonKind::Discard,
                            ControlSize::DenseHunk,
                            cx,
                        )
                        .disabled_when(!ready)
                        .tooltip("Review discarding only this working-copy hunk")
                        .on_click(cx.listener(move |v, _, w, cx| {
                            v.review_discard(token, DiscardTarget::Hunk(index), w, cx)
                        })),
                    );
            }
        }
        row.child(
            div()
                .flex_none()
                .font_family(CODE_FONT)
                .text_size(px(self.preferences.code_size))
                .text_color(rgb(semantic(Role::TextMuted)))
                .child(format!("@@ {ranges} @@")),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_family(CODE_FONT)
                .text_size(px(self.preferences.code_size))
                .text_color(rgb(semantic(Role::TextSecondary)))
                .child(scope.to_string()),
        )
        .into_any_element()
    }

    fn diff_row(&self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = self.state.patch.as_ref().unwrap();
        let header = if self.split_active {
            p.content.split[i].0
        } else {
            Some(i)
        }
        .filter(|&line| p.content.lines[line].text.starts_with("@@ "));
        if let Some(line) = header {
            return self.hunk_header_row(i, line, cx);
        }
        let selected = self.selection.as_ref().is_some_and(|r| r.contains(&i));
        // Rows that a hunk or line write in flight changes fade at once.
        let pending = self.pending_on_screen().is_some_and(|change| {
            let (a, b) = if self.split_active {
                p.content.split[i]
            } else {
                (Some(i), None)
            };
            [a, b]
                .into_iter()
                .flatten()
                .any(|line| change.covers(&p.content, line))
        });
        let found = self.diff_matches.binary_search(&i).is_ok();
        let current_match = self
            .diff_match
            .and_then(|index| self.diff_matches.get(index))
            == Some(&i);
        let advance = self.preferences.code_size * 0.65;
        let scale = self.preferences.code_size / 12.;
        let split = self.split_active;
        let mut el = div()
            .id(("diff-line", i))
            .cursor_text()
            .w_full()
            .when(!split, |row| {
                row.min_w(px(
                    (p.content.columns as f32 * advance + 160. * scale).max(600.)
                ))
            })
            .h(px(self.diff_row_height()))
            .flex()
            .items_center()
            .font_family(CODE_FONT)
            .text_size(px(self.preferences.code_size))
            .overflow_hidden()
            .when(pending, |row| row.opacity(0.35))
            .when(found, |row| {
                row.border_l_2()
                    .border_color(rgb(semantic(if current_match {
                        Role::InteractionFocus
                    } else {
                        Role::StatusWarningFg
                    })))
            });
        let cells = if split {
            let (a, b) = p.content.split[i];
            vec![
                a.map(|index| &p.content.lines[index]),
                b.map(|index| &p.content.lines[index]),
            ]
        } else {
            vec![Some(&p.content.lines[i])]
        };
        for (side, line) in cells.into_iter().enumerate() {
            let mut cell = div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .when(split, |cell| {
                    // Each side clips its own long lines; Unified shows them in full.
                    cell.overflow_hidden().on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |v, _: &MouseDownEvent, _, _| {
                            v.copy_side = if side == 0 {
                                gitcrab_core::diff::Side::Before
                            } else {
                                gitcrab_core::diff::Side::After
                            };
                        }),
                    )
                });
            let focused_side =
                !split || (side == 0) == (self.copy_side == gitcrab_core::diff::Side::Before);
            if let Some(l) = line {
                cell = cell
                    .bg(rgb(if selected && focused_side {
                        semantic(Role::InteractionSelected)
                    } else {
                        match l.kind {
                            '+' => semantic(Role::DiffAddedBg),
                            '-' => semantic(Role::DiffRemovedBg),
                            '@' => semantic(Role::DiffHunkBg),
                            _ => semantic(Role::SurfaceCanvas),
                        }
                    }))
                    .child(
                        div()
                            .w(px(if split { 45. * scale } else { 90. * scale }))
                            .flex_none()
                            .whitespace_nowrap()
                            .text_color(rgb(semantic(Role::TextMuted)))
                            .child(if split {
                                (if side == 0 { l.old } else { l.new })
                                    .map(|n| format!("{n:>4}"))
                                    .unwrap_or_default()
                            } else {
                                format!(
                                    "{:>4} {:>4}",
                                    l.old.map(|n| n.to_string()).unwrap_or_default(),
                                    l.new.map(|n| n.to_string()).unwrap_or_default()
                                )
                            }),
                    )
                    .child(
                        div()
                            .w(px(9. * scale))
                            .flex_none()
                            .text_color(rgb(match l.kind {
                                '+' => semantic(Role::StatusSuccessFg),
                                '-' => semantic(Role::StatusErrorFg),
                                _ => semantic(Role::TextMuted),
                            }))
                            .child(match l.kind {
                                '+' => "+",
                                '-' => "−",
                                _ => "",
                            }),
                    )
                    .child({
                        let mut end = l.text.len().min(8192);
                        while !l.text.is_char_boundary(end) {
                            end -= 1;
                        }
                        let prefix = usize::from(matches!(l.kind, ' ' | '+' | '-') && end > 0);
                        let text = SharedString::from(l.text[prefix..end].to_string());
                        let highlights =
                            diff_highlights(&text, &l.highlights, prefix, self.show_whitespace);
                        div()
                            .whitespace_nowrap()
                            .flex_none()
                            .child(StyledText::new(text).with_highlights(highlights))
                    });
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
}

/// What a working-copy discard removes.
#[derive(Clone)]
enum DiscardTarget {
    /// A zero-based hunk of the reviewed patch.
    Hunk(usize),
    /// Changed lines, as indices into the reviewed patch lines.
    Lines(Vec<usize>),
}

fn short_id(id: &str) -> &str {
    &id[..id.len().min(8)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn marks_only_whitespace_without_changing_syntax_or_byte_offsets() {
        let text = "é // 🦀\ttext";
        let patch = gitcrab_core::diff::parse(&format!("@@ -1 +1 @@\n+{text}"));
        let spans = &patch[1].highlights;
        let marked = diff_highlights(text, spans, 1, true);
        let whitespace: Vec<_> = marked
            .iter()
            .filter(|(_, style)| style.underline.is_some())
            .map(|(range, _)| &text[range.clone()])
            .collect();
        assert_eq!(whitespace, [" ", " ", "\t"]);
        assert!(marked.iter().any(|(range, style)| {
            &text[range.clone()] == "🦀" && *style == syntax_style('c')
        }));
        assert!(diff_highlights(text, spans, 1, false)
            .iter()
            .all(|(_, style)| style.underline.is_none()));
    }
}
