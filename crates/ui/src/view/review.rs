use super::*;
use gitcrab_core::highlight::Span;
use std::ops::Range;

const TEXT_ONLY_LAYOUTS: &str = "Layouts apply to text changes";

const DIFF_ROW_HEIGHT_COMPACT: f32 = 18.;
const DIFF_ROW_HEIGHT: f32 = geometry::SIZE_ROW_DIFF;
/// Space a diff row keeps above and below code at large code sizes.
const DIFF_ROW_LEADING: f32 = 5.;
/// Unified rows reserve this beside their text at [`BASE_CODE_SIZE`], and are
/// never narrower than [`UNIFIED_ROW_MIN_WIDTH`].
const UNIFIED_ROW_CHROME: f32 = 160.;
const UNIFIED_ROW_MIN_WIDTH: f32 = 600.;
/// How far one horizontal pan moves the diff.
const PAN_STEP: f32 = 240.;
/// How long a file's latest outcome stays beside its diff controls.
const OUTCOME_SHOWN_FOR: std::time::Duration = std::time::Duration::from_secs(60);
const TOOLBAR_DIVIDER_HEIGHT: f32 = 14.;

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
                color: Some(role_color(Role::TextMuted).into()),
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
            DIFF_ROW_HEIGHT_COMPACT
        } else {
            DIFF_ROW_HEIGHT
        })
        .max(self.preferences.code_size + DIFF_ROW_LEADING)
    }

    /// Width every Unified row shares, so the list scrolls to its longest line.
    fn unified_row_width(&self, p: &gitcrab_core::controller::Patch) -> f32 {
        let scale = self.preferences.code_size / BASE_CODE_SIZE;
        let advance = self.preferences.code_size * CODE_CHAR_WIDTH;
        (p.content.columns as f32 * advance + UNIFIED_ROW_CHROME * scale).max(UNIFIED_ROW_MIN_WIDTH)
    }

    /// Rebuilds the diff's visual row layout when the patch, layout or pane
    /// width changes. In Split, each side wraps its long lines at the columns
    /// it can show, measured from the code font, as GitHub Desktop's split
    /// diff does.
    pub(super) fn update_diff_wrap(&mut self, window: &mut Window) {
        let Some(p) = self.state.patch.as_ref() else {
            self.diff_wrap = None;
            return;
        };
        if !self.split_active {
            if !self
                .diff_wrap
                .as_ref()
                .is_some_and(|(token, wrap)| *token == p.token && wrap.columns == usize::MAX)
            {
                self.diff_wrap = Some((p.token, gitcrab_core::diff::Wrap::unified(&p.content)));
            }
            return;
        }
        let size = self.preferences.code_size;
        let scale = size / BASE_CODE_SIZE;
        let text = window.text_system();
        let advance = text
            .advance(text.resolve_font(&font(CODE_FONT)), px(size), 'm')
            .map(|advance| f32::from(advance.width))
            .unwrap_or(size * CODE_CHAR_WIDTH)
            .max(1.);
        // Each side: 8 px padding on both edges, 16 px around the marker and
        // gutter rule, the line number and marker columns, and a little slack
        // for rounding.
        let side = self.history_available / 2.
            - DIFF_SIDE_PADDING
            - DIFF_SIDE_GAPS
            - DIFF_SPLIT_GUTTER * scale
            - 6.;
        let columns = ((side / advance).floor() as usize).max(8);
        if self
            .diff_wrap
            .as_ref()
            .is_some_and(|(token, wrap)| *token == p.token && wrap.columns == columns)
        {
            return;
        }
        self.diff_wrap = Some((
            p.token,
            gitcrab_core::diff::Wrap::split(&p.content, columns),
        ));
    }

    /// The visual row layout for the patch and layout on screen.
    fn wrap(&self) -> Option<&gitcrab_core::diff::Wrap> {
        let p = self.state.patch.as_ref()?;
        self.diff_wrap
            .as_ref()
            .filter(|(token, wrap)| {
                *token == p.token && (wrap.columns == usize::MAX) != self.split_active
            })
            .map(|(_, wrap)| wrap)
    }

    /// Rows the diff list draws, counting gaps between hunks and, in Split,
    /// wrapped segments.
    fn diff_visual_rows(&self) -> usize {
        let Some(p) = self.state.patch.as_ref() else {
            return 0;
        };
        match self.wrap() {
            Some(wrap) => wrap.rows(),
            None if self.split_active => p.content.split.len(),
            None => p.content.lines.len(),
        }
    }

    /// The displayed row (selection, find and hunk index) a drawn row shows,
    /// and which wrapped segment of it.
    fn row_of_visual(&self, visual: usize) -> (usize, usize) {
        self.wrap().map_or((visual, 0), |wrap| wrap.locate(visual))
    }

    /// Scrolls a displayed row into view, at its first wrapped segment.
    pub(super) fn scroll_diff_to(&self, row: usize, strategy: ScrollStrategy) {
        let visual = self.wrap().map_or(row, |wrap| wrap.first(row));
        self.diff_scroll.scroll_to_item(visual, strategy);
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
                f32::from(handle.max_offset().y),
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
        let top = self.row_of_visual(top).0;
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
            self.scroll_diff_to(row, ScrollStrategy::Top);
            cx.notify();
        }
    }

    /// Names what each side of the comparison shows.
    fn comparison_sides(&self, p: &gitcrab_core::controller::Patch) -> (String, String) {
        let before_path = p
            .original_path
            .as_ref()
            .map(|old| format!(" ({})", gitcrab_core::git::display_path(old)))
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
            format!("Before: {before}{before_path}"),
            format!("After: {after}"),
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
            Preview::Renamed => ("Renamed without changes", "Only the path changed.".into()),
            Preview::ModeChange => (
                "File mode changed",
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
                        .text_role(Role::TextMuted)
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
            .child(icon("changes").text_role(Role::TextMuted))
            .child(div().text_size(scaled(TEXT_SIZE_HEADING)).child(title))
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
                    .text_role(Role::TextMuted)
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
        let n = self.diff_visual_rows();
        let weak = cx.weak_entity();
        let display = gitcrab_core::git::display_path(&p.path);
        let (directory, name) = match display.rfind(['/', '\\']) {
            Some(cut) => (display[..=cut].to_string(), display[cut + 1..].to_string()),
            None => (String::new(), display.clone()),
        };
        let path_tooltip = match &p.original_path {
            Some(old) => format!("{} → {display}", gitcrab_core::git::display_path(old)),
            None => display.clone(),
        };
        // File identity, its file action and close, as GitKraken's title row.
        let header = section_bar()
            .child(
                div()
                    .id("diff-path")
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .overflow_hidden()
                    .when_some(
                        p.id.is_none()
                            .then_some(self.state.snapshot.as_ref())
                            .flatten()
                            .and_then(|s| s.status.iter().find(|f| f.path == p.path)),
                        |path, file| {
                            path.child(
                                div()
                                    .flex_none()
                                    .mr_1()
                                    .child(super::inspector::status_badge(file, staged)),
                            )
                        },
                    )
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
                            .text_role(Role::TextMuted)
                            .child(match &p.original_path {
                                Some(old) => format!(
                                    "{directory} (renamed from {})",
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
                        .child(Badge::role("Updated on disk", Role::StatusInfoFg))
                        .tooltip(|w, cx| {
                            Tooltip::new(
                                "The file changed outside GitCrab and the diff was reloaded. Selected lines stay selected where they did not change.",
                            )
                            .build(w, cx)
                        }),
                )
            })
            .when(conflict, |bar| {
                bar.child(Badge::role("Conflict", Role::StatusConflictFg))
            })
            .when(conflict, |el| {
                let path = p.path.clone();
                el.child(
                    self.action_button("resolve-file", "Mark resolved…")
                        .flex_none()
                        .disabled_when(self.state.blocking_write())
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
                    .flex_none()
                    .tooltip_with_action(
                        if staged { "Unstage file" } else { "Stage file" },
                        &StageSelectedFile,
                        Some(KEY_CONTEXT),
                    )
                    .disabled_when(
                        self.state.blocking_write()
                            || !self.state.snapshot.as_ref().is_some_and(|s| {
                                s.status
                                    .iter()
                                    .any(|f| f.path == p.path && Controller::on_side(f, staged))
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
            })
            .child(
                self.icon_button("close-diff", "x", "Close diff")
                    .flex_none()
                    .tooltip_with_action("Close diff", &Back, Some(KEY_CONTEXT))
                    .on_click(cx.listener(|v, _, w, cx| v.leave_diff(w, cx))),
            );
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
                .when(!available, |button| {
                    button.tooltip(format!("No {} changes in this file", label.to_lowercase()))
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
        let layout_note =
            (self.split && text && !self.split_active).then_some("Too narrow for Split");
        let has_rows = text && !p.content.lines.is_empty();
        // GitKraken's controls row: status on the left, the compared side in
        // the middle, view tools on the right.
        let side = || div().flex_1().min_w_0().flex().items_center().gap_2();
        let tools = side()
            .justify_end()
            .when(has_rows, |bar| self.diff_tools(bar, cx))
            .when_some(layout_note, |bar, note| {
                bar.child(
                    div()
                        .id("split-fallback")
                        .text_size(scaled(TEXT_SIZE_SMALL))
                        .text_role(Role::StatusWarningFg)
                        .child(note)
                        .tooltip(|w, cx| {
                            Tooltip::new("Widen the window to compare side by side.").build(w, cx)
                        }),
                )
            })
            .child(
                segmented_group()
                    .child(
                        self.segment_button("unified-mode", "Unified", text && !self.split)
                            .disabled_when(!text)
                            .when(!text, |button| button.tooltip(TEXT_ONLY_LAYOUTS))
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
                            .when(!text, |button| button.tooltip(TEXT_ONLY_LAYOUTS))
                            .on_click(cx.listener(|v, _, w, cx| {
                                v.split = true;
                                v.selection = None;
                                v.persist_preferences(w, cx);
                                cx.notify();
                            })),
                    ),
            )
            .when(has_rows, |bar| {
                bar.child(
                    segmented_group().child(
                        self.segment_button("selection-help", "Shortcuts", false)
                            .on_click(|_, w, cx| {
                                let scroll = ScrollHandle::new();
                                w.open_modal(cx, move |d, _, cx| diff_help(d, &scroll, cx))
                            }),
                    ),
                )
            });
        let controls = section_bar()
            .bg_role(Role::SurfaceCanvas)
            .child(side().child(self.diff_status(p)))
            .when_some(source, |bar, source| bar.child(source))
            .child(tools);
        let el = panel()
            .bg_role(Role::SurfaceCanvas)
            .child(header)
            .child(controls);
        if !has_rows {
            return el.child(self.preview_panel(p, cx)).into_any_element();
        }
        let diff_over = overflows_vertically(&self.diff_scroll);
        el.when(self.diff_find_open, |el| el.child(self.diff_find_bar(cx)))
            .when(self.split_active, |el| {
                let (before, after) = self.comparison_sides(p);
                let label = |text: String| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_2()
                        .truncate()
                        .text_size(scaled(TEXT_SIZE_SMALL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_role(Role::TextSecondary)
                        .child(text)
                };
                el.child(
                    div()
                        .h(scaled(24.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .bg_role(Role::SurfaceChrome)
                        .border_b_1()
                        .border_role(Role::BorderDivider)
                        .child(label(before))
                        .child(div().w(px(1.)).h_full().bg_role(Role::BorderDivider))
                        .child(label(after)),
                )
            })
            .child(
                track_overflow(div(), &self.diff_scroll, diff_over)
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
                        .when(self.split_active && diff_over, |list| {
                            list.pr(SCROLLBAR_GUTTER)
                        })
                        .track_scroll(&self.diff_scroll)
                        .size_full(),
                    )
                    .child(pinned_scrollbar_overlay("diff", &self.diff_scroll, px(0.)))
                    .when(!self.split_active, |el| {
                        // Shows where a wide unified diff is scrolled horizontally.
                        el.child(
                            div().absolute().inset_0().child(slim_scrollbar(
                                gpui::component::scroll::Scrollbar::horizontal(&self.diff_scroll)
                                    .id("diff-horizontal-scrollbar")
                                    .mode(gpui::component::scroll::ScrollbarMode::Always),
                            )),
                        )
                    }),
            )
            .into_any_element()
    }

    /// The diff's write status and outcome, for the left of its controls row.
    fn diff_status(&self, p: &gitcrab_core::controller::Patch) -> Div {
        let path = gitcrab_core::git::display_path(&p.path);
        // The latest outcome for this file, next to the controls that caused it.
        let outcome = self
            .activity
            .front()
            .filter(|record| {
                record.target.as_deref() == Some(path.as_str())
                    && record.at.elapsed() < OUTCOME_SHOWN_FOR
            })
            .cloned();
        // Writes to other files leave this diff as it is and say nothing here.
        let writing = self.state.write_touches_patch();
        let refreshing = !writing && !self.state.patch_ready();
        let note = |role: Role, text: String| {
            div()
                .min_w_0()
                .truncate()
                .text_size(scaled(TEXT_SIZE_SMALL))
                .text_role(role)
                .child(text)
        };
        div()
            .min_w_0()
            .flex()
            .items_center()
            .gap_2()
            .overflow_hidden()
            .when(writing || refreshing, |status| {
                status.child(note(
                    Role::TextSecondary,
                    if refreshing {
                        "Updating diff…".to_string()
                    } else {
                        self.running_label()
                    },
                ))
            })
            .when_some(outcome.filter(|_| !writing), |status, record| {
                status.child(note(
                    if record.failure.is_some() {
                        Role::StatusErrorFg
                    } else {
                        Role::StatusSuccessFg
                    },
                    record.details.clone(),
                ))
            })
            .when_some(p.hunk_blocked.clone(), |status, reason| {
                status.child(note(Role::TextDefault, reason))
            })
    }

    /// Adds find, whitespace and hunk navigation to the diff's controls row.
    fn diff_tools(&self, bar: Div, cx: &mut Context<Self>) -> Div {
        let bar = bar.child(
            segmented_group()
                .child(
                    self.icon_button("diff-find", "search", "Find in diff")
                        .size(scaled(CONTROL_HEIGHT - 2.))
                        .tooltip_with_action("Find in diff", &Find, Some(KEY_CONTEXT))
                        .selected(self.diff_find_open)
                        .when(self.diff_find_open, |button| {
                            button.bg_role(Role::InteractionSelected)
                        })
                        .on_click(cx.listener(|v, _, w, cx| {
                            if v.diff_find_open {
                                v.close_diff_find(w, cx)
                            } else {
                                v.open_diff_find(w, cx)
                            }
                        })),
                )
                .child(
                    self.segment_button("show-whitespace", "Whitespace", self.show_whitespace)
                        .tooltip("Show spaces and tabs")
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.show_whitespace = !v.show_whitespace;
                            cx.notify();
                        })),
                ),
        );
        let Some((position, _)) = self.viewport_hunk() else {
            return bar;
        };
        let hunk_nav: [(usize, bool, &str, &str, &dyn gpui::Action, _); 2] = [
            (
                0,
                false,
                "up",
                "Previous hunk",
                &PreviousHunk,
                position.previous,
            ),
            (1, true, "down", "Next hunk", &NextHunk, position.next),
        ];
        bar.child(segmented_group().children(hunk_nav.into_iter().map(
            |(index, forward, glyph, name, action, target)| {
                self.icon_button(("hunk-nav", index), glyph, name)
                    .size(scaled(CONTROL_HEIGHT - 2.))
                    .tooltip_with_action(name, action, Some(KEY_CONTEXT))
                    .disabled_when(target.is_none())
                    .on_click(cx.listener(move |v, _, w, cx| v.step_hunk(forward, w, cx)))
            },
        )))
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
                    .text_size(scaled(TEXT_SIZE_SMALL))
                    .text_role(Role::TextMuted)
                    .child(status),
            )
            .child(
                self.icon_button("diff-find-prev", "up", "Previous match")
                    .tooltip_with_action("Previous match", &SearchPrevious, Some(KEY_CONTEXT))
                    .disabled_when(self.diff_matches.is_empty())
                    .on_click(cx.listener(|v, _, w, cx| v.diff_find_step(-1, w, cx))),
            )
            .child(
                self.icon_button("diff-find-next", "down", "Next match")
                    .tooltip_with_action("Next match", &SearchNext, Some(KEY_CONTEXT))
                    .disabled_when(self.diff_matches.is_empty())
                    .on_click(cx.listener(|v, _, w, cx| v.diff_find_step(1, w, cx))),
            )
            .child(
                self.button("diff-find-close", "×")
                    .tooltip_with_action("Close", &Back, Some(KEY_CONTEXT))
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
            .child(icon(if loading { "refresh" } else { "check" }).text_role(Role::ActionPrimary))
            .child(div().text_size(scaled(TEXT_SIZE_HEADING)).child(title))
            .child(div().text_role(Role::TextMuted).child(detail))
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
        size: ControlSize,
        cx: &mut Context<Self>,
    ) -> E {
        let token = p.token;
        let unstage = p.staged;
        let ready = self.diff_actions_ready();
        let count = lines.len();
        let noun = if count == 1 { "line" } else { "lines" };
        let rows = lines.clone();
        bar.when(!unstage, |bar| {
            bar.child(
                self.discard_button(
                    SharedString::from(format!("{place}-lines-discard")),
                    format!("Discard {count} {noun}…"),
                    size,
                    p,
                    cx,
                )
                .on_click(cx.listener(move |v, _, w, cx| {
                    v.review_discard(token, DiscardTarget::Lines(lines.clone()), w, cx)
                })),
            )
        })
        .child(
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
                size,
                cx,
            )
            .disabled_when(!ready)
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
    }

    /// A Discard button for `p`'s hunks or lines. It is disabled while the
    /// patch is not ready, and also, with the reason as its tooltip, when
    /// Discard does not support this change type.
    fn discard_button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        size: ControlSize,
        p: &gitcrab_core::controller::Patch,
        cx: &App,
    ) -> Button {
        let button = self
            .semantic_button(id, label, components::ButtonKind::Discard, size, cx)
            .disabled_when(!self.diff_actions_ready() || p.discard_blocked.is_some());
        match p.discard_blocked.clone() {
            Some(reason) => button.tooltip(reason),
            None => button,
        }
    }

    fn review_discard(
        &mut self,
        token: u32,
        target: DiscardTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = self.state.patch.as_ref().filter(|p| {
            p.id.is_none() && !p.staged && p.token == token && p.discard_blocked.is_none()
        }) else {
            return;
        };
        if self.state.busy {
            return;
        }
        let path = gitcrab_core::git::display_path(&p.path);
        let epoch = self.state.epoch;
        let repo = gitcrab_core::presentation::display_path(&self.state.path);
        let weak = cx.weak_entity();
        let cancel_focus = cx.focus_handle();
        let scroll = ScrollHandle::new();
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
        window.open_alert(cx, move |dialog, _, cx| {
            let weak = weak.clone();
            let target = target.clone();
            let discard = components::style(
                Button::new("confirm-discard-hunk")
                    .label(confirm.clone())
                    .tab_stop(true),
                components::ButtonKind::Discard,
                ControlSize::Regular,
                cx,
            )
            .on_click(move |_, w, cx| {
                let action = match &target {
                    DiscardTarget::Hunk(index) => Action::DiscardHunk { token, index: *index },
                    DiscardTarget::Lines(rows) => Action::DiscardLines {
                        token,
                        rows: rows.clone(),
                    },
                };
                let _ = weak.update(cx, |v, cx| {
                    if v.state.epoch == epoch && !v.state.busy {
                        v.mutate(action, w, cx);
                    }
                });
                w.close_dialog(cx);
            });
            dialog
                .frame(
                    ModalHeader::new(title.clone())
                        .description(format!(
                            "This permanently removes {what} from the working copy."
                        ))
                        .tone(Tone::Danger),
                    ModalWidth::Small,
                    cx,
                )
                .body(
                    &scroll,
                    modal_body()
                        .child(modal::subject(path.clone(), Some(repo.clone().into())))
                        .child(modal::paragraph(
                            "Only these changes are removed. Staged content stays in the index, and changes with unsupported encodings or change types are refused.",
                        )),
                )
                .on_ok(|_, _, _| true)
                .footer(
                    ModalFooter::new()
                        .action(components::dialog_cancel_focused(&cancel_focus, cx))
                        .action(discard),
                )
        });
        window.on_next_frame(move |w, cx| initial_focus.focus(w, cx));
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
        let step = px(PAN_STEP);
        offset.x = if right {
            (offset.x - step).max(-handle.max_offset().x)
        } else {
            (offset.x + step).min(px(0.))
        };
        handle.set_offset(offset);
    }
    /// A hunk header spanning the whole row in either layout, ruled off from
    /// its lines as GitKraken's is. The scope truncates first, so a long scope
    /// line or a narrow Split pane cannot hide the actions.
    fn hunk_header_row(
        &self,
        i: usize,
        line: usize,
        rows: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.state.patch.as_ref().unwrap();
        let l = &p.content.lines[line];
        let index = p.content.hunk_rows.binary_search(&line).ok();
        let (ranges, scope) = gitcrab_core::diff::hunk_header_parts(&l.text);
        let conflict = self
            .state
            .snapshot
            .as_ref()
            .is_some_and(|s| s.status.iter().any(|f| f.path == p.path && f.is_unmerged()));
        let actions =
            index.filter(|&index| self.hunk_actions_allowed(p, conflict) && index < p.hunks.len());
        let ready = self.diff_actions_ready();
        let unified_width = self.unified_row_width(p);
        // Buttons sit at the right edge, as GitKraken's do, unless that edge
        // is scrolled out of the pane by a long unified line.
        let actions_right = self.split_active || unified_width <= self.history_available;
        let mut row = div()
            .id(("diff-line", i))
            .w_full()
            .when(!self.split_active, |row| row.min_w(px(unified_width)))
            .h(px(self.diff_row_height() * rows))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .overflow_hidden()
            .border_b_1()
            .border_role(Role::BorderDivider)
            .text_size(scaled(TEXT_SIZE_SMALL))
            .child(
                div()
                    .flex_none()
                    .font_family(CODE_FONT)
                    .text_size(px(self.preferences.code_size))
                    .text_role(Role::TextMuted)
                    .child(format!("@@ {ranges} @@")),
            )
            .child(
                div()
                    .when(actions_right, |scope| scope.flex_1())
                    .min_w_0()
                    .truncate()
                    .font_family(CODE_FONT)
                    .text_size(px(self.preferences.code_size))
                    .text_role(Role::TextSecondary)
                    .child(scope.to_string()),
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
                    .text_role(Role::TextSecondary)
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
                row = self.line_actions(row, p, selected, "hunk", ControlSize::DenseHunk, cx);
                row = row.child(
                    div()
                        .flex_none()
                        .w(px(1.))
                        .h(px(TOOLBAR_DIVIDER_HEIGHT))
                        .mx_1()
                        .bg_role(Role::BorderDivider),
                );
            }
            // Discard then Stage, as GitKraken orders them, so the common
            // action sits at the edge.
            if !unstage {
                row = row.child(
                    self.discard_button(
                        ("discard-hunk", i),
                        "Discard hunk…",
                        ControlSize::DenseHunk,
                        p,
                        cx,
                    )
                    .on_click(cx.listener(move |v, _, w, cx| {
                        v.review_discard(token, DiscardTarget::Hunk(index), w, cx)
                    })),
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
        }
        row.into_any_element()
    }

    fn diff_row(&self, visual: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = self.state.patch.as_ref().unwrap();
        // `i` is the displayed row; `segment` is which wrapped part of it
        // this drawn row shows. Only Split wraps, so Unified segments are 0.
        let (i, segment) = self.row_of_visual(visual);
        let split = self.split_active;
        if self
            .wrap()
            .is_some_and(|wrap| wrap.is_gap(visual, self.hunk_rows()))
        {
            return div()
                .id(("diff-line", visual))
                .w_full()
                .when(!split, |row| row.min_w(px(self.unified_row_width(p))))
                .when(split, |row| {
                    row.flex()
                        .child(div().flex_1())
                        .child(div().flex_1().border_l_1().border_role(Role::BorderDivider))
                })
                .h(px(self.diff_row_height()))
                .into_any_element();
        }
        let wrap_columns = self.wrap().map(|wrap| wrap.columns).filter(|_| split);
        let header = if split { p.content.split[i].0 } else { Some(i) }
            .filter(|&line| p.content.lines[line].text.starts_with("@@ "));
        if let Some(line) = header {
            let Some(wrap) = self.wrap() else {
                return self.hunk_header_row(visual, line, 1., cx);
            };
            let row = px(self.diff_row_height());
            // The header covers its two rows from the second one, which paints
            // last, so the row above can't draw over it or take its clicks.
            if visual + 1 < wrap.first(i + 1) {
                return div().h(row).into_any_element();
            }
            return div()
                .relative()
                .w_full()
                .when(!split, |r| r.min_w(px(self.unified_row_width(p))))
                .h(row)
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .child(self.hunk_header_row(visual, line, 2., cx)),
                )
                .into_any_element();
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
        let scale = self.preferences.code_size / BASE_CODE_SIZE;
        let mut el = div()
            .id(("diff-line", visual))
            .cursor_text()
            .w_full()
            .when(!split, |row| row.min_w(px(self.unified_row_width(p))))
            .h(px(self.diff_row_height()))
            .flex()
            .items_center()
            .font_family(CODE_FONT)
            .text_size(px(self.preferences.code_size))
            .overflow_hidden()
            .when(pending, |row| row.opacity(0.35))
            .when(found, |row| {
                row.border_l_2().border_role(if current_match {
                    Role::InteractionFocus
                } else {
                    Role::StatusWarningFg
                })
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
                .when(side == 1, |cell| {
                    cell.border_l_1().border_role(Role::BorderDivider)
                })
                .when(split, |cell| {
                    // Each side wraps its long lines onto further rows; the
                    // clip only guards against a font wider than measured.
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
            let highlighted = selected && focused_side && line.is_some();
            let kind = line.map_or(' ', |l| l.kind);
            // As in GitKraken, numbers and the +/− marker sit in a plain
            // gutter ruled off from the code, and only the code is tinted.
            let numbers = match line {
                Some(_) if segment > 0 => String::new(),
                Some(l) if split => (if side == 0 { l.old } else { l.new })
                    .map(|n| format!("{n:>4}"))
                    .unwrap_or_default(),
                Some(l) => format!(
                    "{:>4} {:>4}",
                    l.old.map(|n| n.to_string()).unwrap_or_default(),
                    l.new.map(|n| n.to_string()).unwrap_or_default()
                ),
                None => String::new(),
            };
            cell = cell
                .bg_role(if highlighted {
                    Role::InteractionSelected
                } else {
                    Role::SurfaceCanvas
                })
                .child(
                    div()
                        .flex_none()
                        .h_full()
                        .flex()
                        .items_center()
                        .gap_1()
                        .pl_2()
                        .pr_1()
                        .border_r_1()
                        .border_role(Role::BorderDivider)
                        .child(
                            div()
                                .w(px(if split {
                                    DIFF_SPLIT_NUMBER_WIDTH * scale
                                } else {
                                    DIFF_UNIFIED_NUMBER_WIDTH * scale
                                }))
                                .flex_none()
                                .whitespace_nowrap()
                                .text_role(Role::TextMuted)
                                .child(numbers),
                        )
                        .child(
                            div()
                                .w(px(DIFF_MARKER_WIDTH * scale))
                                .flex_none()
                                .text_role(match kind {
                                    '+' => Role::StatusSuccessFg,
                                    '-' => Role::StatusErrorFg,
                                    _ => Role::TextMuted,
                                })
                                .child(match kind {
                                    _ if segment > 0 => "",
                                    '+' => "+",
                                    '-' => "−",
                                    _ => "",
                                }),
                        ),
                );
            if let Some(l) = line {
                let tint = match l.kind {
                    '+' => Some(Role::DiffAddedBg),
                    '-' => Some(Role::DiffRemovedBg),
                    '@' => Some(Role::DiffHunkBg),
                    _ => None,
                }
                .filter(|_| !highlighted);
                cell = cell.child(
                    div()
                        .flex_1()
                        .h_full()
                        .flex()
                        .items_center()
                        .px_2()
                        .when_some(tint, |code, tint| code.bg_role(tint))
                        .child({
                            let (prefix, shown) = gitcrab_core::diff::shown_text(l);
                            let mut highlights =
                                diff_highlights(shown, &l.highlights, prefix, self.show_whitespace);
                            // In Split, this row shows one wrapped segment of the line.
                            let range = match wrap_columns {
                                Some(columns) => gitcrab_core::diff::wrap_ranges(shown, columns)
                                    .get(segment)
                                    .cloned()
                                    .unwrap_or(shown.len()..shown.len()),
                                None => 0..shown.len(),
                            };
                            if range != (0..shown.len()) {
                                highlights = highlights
                                    .into_iter()
                                    .filter_map(|(span, style)| {
                                        let start = span.start.max(range.start);
                                        let end = span.end.min(range.end);
                                        (start < end).then(|| {
                                            (start - range.start..end - range.start, style)
                                        })
                                    })
                                    .collect();
                            }
                            let text = SharedString::from(shown[range].to_string());
                            div()
                                .whitespace_nowrap()
                                .flex_none()
                                .child(StyledText::new(text).with_highlights(highlights))
                        }),
                );
            }
            el = el.child(cell);
        }
        el.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                v.focus.focus(w, cx);
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

/// The diff's selection, copy and navigation shortcuts.
fn diff_help(
    dialog: gpui::component::dialog::Dialog,
    scroll: &ScrollHandle,
    cx: &mut App,
) -> gpui::component::dialog::Dialog {
    use modal::{shortcut_row as row, COMMAND_KEY as CMD};
    dialog
        .frame(
            ModalHeader::new("Diff shortcuts")
                .description("Select, copy and move through the diff from the keyboard.")
                .closable(),
            ModalWidth::Medium,
            cx,
        )
        .body(
            scroll,
            modal_body()
                .gap(scaled(18.))
                .child(
                    modal::section("Selection")
                        .child(row(
                            &["Click", "/", "Shift", "+", "Click"],
                            "Select a row, or extend the selection",
                        ))
                        .child(row(&[CMD, "+", "A"], "Select every row"))
                        .child(modal::fine_print(
                            "With rows selected in working changes, Stage, Unstage and Discard act on just those added and removed lines. In Split, a selected row includes both sides. Changing layouts clears the selection.",
                        )),
                )
                .child(
                    modal::section("Copy")
                        .child(row(
                            &[CMD, "+", "C"],
                            "Copy source text; in Split, from the side you clicked",
                        ))
                        .child(row(
                            &[CMD, "+", "Shift", "+", "C"],
                            "Copy the selection as a patch with +/- markers",
                        ))
                        .child(modal::fine_print(
                            "Copying with nothing selected leaves the clipboard unchanged.",
                        )),
                )
                .child(
                    modal::section("Navigation")
                        .child(row(&["Alt", "+", "↑", "/", "↓"], "Previous or next hunk"))
                        .child(row(&[CMD, "+", "F"], "Find text in the diff"))
                        .child(row(&["Enter", "/", "F3"], "Next match"))
                        .child(row(
                            &["Alt", "+", "←", "/", "→"],
                            "Scroll a wide Unified diff",
                        )),
                ),
        )
        .footer(ModalFooter::new().action(components::dialog_ok(
            "Close",
            components::ButtonKind::Primary,
            cx,
        )))
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
