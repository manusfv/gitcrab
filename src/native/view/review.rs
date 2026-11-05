use super::*;
use gitcrab_core::highlight::Span;
use std::ops::Range;

fn syntax_style(kind: char) -> HighlightStyle {
    HighlightStyle {
        color: Some(
            rgb(match kind {
                'c' => MUTED,
                's' => SYNTAX_STRING,
                'k' => SYNTAX_KEYWORD,
                'n' => SYNTAX_NUMBER,
                _ => TEXT,
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
                color: Some(rgb(MUTED).into()),
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

impl GitCrab {
    pub(super) fn diff(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.state.patch.as_ref().unwrap();
        let staged = p.staged;
        let n = if self.split {
            p.split.len()
        } else {
            p.lines.len()
        };
        let weak = cx.weak_entity();
        let mut el = panel()
            .bg(rgb(CANVAS))
            .child(
                section_bar()
                    .child(
                        self.button("back", "History")
                            .icon(icon("back"))
                            .on_click(cx.listener(|v, _, w, cx| v.back(&Back, w, cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(
                                p.original_path
                                    .as_ref()
                                    .map(|old| format!("{old} → {}", p.path))
                                    .unwrap_or_else(|| p.path.clone()),
                            ),
                    )
                    .child(
                        segmented_group()
                            .child(
                                self.segment_button("unified-mode", "Unified", !self.split)
                                    .on_click(cx.listener(|v, _, w, cx| {
                                        v.split = false;
                                        v.selection = None;
                                        v.persist_preferences(w, cx);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                self.segment_button("split-mode", "Split", self.split)
                                    .on_click(cx.listener(|v, _, w, cx| {
                                        v.split = true;
                                        v.selection = None;
                                        v.persist_preferences(w, cx);
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                section_bar()
                    .when(p.id.is_none(), |bar| {
                        bar.child(segmented_group().children([false, true].into_iter().map(
                            |side| {
                                self.segment_button(
                                    if side {
                                        "source-staged"
                                    } else {
                                        "source-unstaged"
                                    },
                                    if side { "Staged" } else { "Unstaged" },
                                    side == staged,
                                )
                                .on_click(cx.listener(
                                    move |v, _, w, cx| {
                                        if let Some(p) = &v.state.patch {
                                            v.file(p.path.clone(), side, w, cx);
                                        }
                                    },
                                ))
                            },
                        )))
                    })
                    .when(p.id.is_some(), |bar| {
                        bar.child(badge(
                            format!(
                                "Commit {}",
                                p.id.as_ref().unwrap().chars().take(8).collect::<String>()
                            ),
                            ACCENT,
                        ))
                    })
                    .child(
                        div()
                            .text_size(type_size(cx, 12.))
                            .text_color(rgb(MUTED))
                            .child(if p.id.is_some() {
                                "First-parent comparison"
                            } else if staged {
                                "Compared to HEAD"
                            } else {
                                "Compared to index"
                            }),
                    )
                    .child(div().flex_1())
                    .when(p.id.is_none(), |el| {
                        el.child(
                            self.action_button(
                                "stage-file",
                                if staged { "Unstage file" } else { "Stage file" },
                            )
                            .outline()
                            .icon(icon("check"))
                            .disabled(
                                self.state.busy
                                    || !self.state.snapshot.as_ref().is_some_and(|s| {
                                        s.status.iter().any(|f| {
                                            f.path == p.path && Controller::on_side(f, staged)
                                        })
                                    }),
                            )
                            .on_click(cx.listener(|v, _, w, cx| {
                                if let Some(p) = &v.state.patch {
                                    v.mutate(
                                        Action::Stage {
                                            path: p.path.clone(),
                                            unstage: p.staged,
                                        },
                                        w,
                                        cx,
                                    );
                                }
                            })),
                        )
                    }),
            );
        let path = p.path.clone();
        if p.lines
            .iter()
            .any(|line| line.text.starts_with("Binary files "))
        {
            return el.child(div().flex_1().flex().flex_col().items_center().justify_center().gap_3().p_4()
                .child(icon("changes").text_color(rgb(MUTED)))
                .child(div().text_size(type_size(cx,16.)).child("Binary file changed"))
                .child(div().text_color(rgb(MUTED)).child(if p.id.is_some() {"Binary content has no text preview."} else {"A text preview is not available. You can stage or unstage the whole file."})))
                .into_any_element();
        }
        if p.lines.is_empty() {
            return el.child(div().flex_1().flex().flex_col().items_center().justify_center().gap_3().p_4()
                .child(div().text_size(type_size(cx,16.)).child("No text changes on this side"))
                .child(div().text_color(rgb(MUTED)).child("Choose the other side or another file. Empty files can be staged as a whole.")))
                .into_any_element();
        }
        if p.hunks.is_empty()
            && p.lines
                .iter()
                .any(|line| line.text.starts_with("rename to "))
        {
            let metadata = p
                .lines
                .iter()
                .filter(|line| {
                    line.text.starts_with("rename from ") || line.text.starts_with("rename to ")
                })
                .map(|line| line.text.clone())
                .collect::<Vec<_>>();
            return el
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .p_4()
                        .child(
                            div()
                                .text_size(type_size(cx, 16.))
                                .child("File renamed · contents unchanged"),
                        )
                        .children(
                            metadata
                                .into_iter()
                                .map(|line| div().text_color(rgb(MUTED)).child(line)),
                        ),
                )
                .into_any_element();
        }
        if !p.hunks.is_empty() {
            let active = self.active_hunk.min(p.hunks.len() - 1);
            let mut bar = div()
                .h(px(32.))
                .flex()
                .items_center()
                .px_2()
                .gap_2()
                .border_b_1()
                .border_color(rgb(BORDER));
            for (index, delta, label) in [(0, -1isize, "up"), (1, 1isize, "down")] {
                bar = bar.child(
                    self.icon_button(
                        ("hunk-nav", index as usize),
                        label,
                        if delta < 0 {
                            "Previous hunk"
                        } else {
                            "Next hunk"
                        },
                    )
                    .disabled(if delta < 0 {
                        active == 0
                    } else {
                        active + 1 == p.hunks.len()
                    })
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if let Some(p) = &v.state.patch {
                            v.active_hunk = (v.active_hunk as isize + delta)
                                .clamp(0, p.hunks.len().saturating_sub(1) as isize)
                                as usize;
                            let rows = if v.split {
                                &p.split_hunk_rows
                            } else {
                                &p.hunk_rows
                            };
                            if let Some(row) = rows.get(v.active_hunk) {
                                v.diff_scroll.scroll_to_item(*row, ScrollStrategy::Top);
                            }
                            cx.notify();
                        }
                    })),
                );
            }
            el = el.child(
                bar.child(
                    div()
                        .text_size(type_size(cx, 12.))
                        .text_color(rgb(MUTED))
                        .child(format!("Hunk {} of {}", active + 1, p.hunks.len())),
                ),
            );
        }
        el=el.child(uniform_list(SharedString::from(format!("diff-{path}-{}",self.split)),n,move|range,_,cx|weak.update(cx,|v,cx|range.map(|i|v.diff_row(i,cx)).collect::<Vec<_>>()).unwrap_or_default()).with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained).track_scroll(self.diff_scroll.clone()).flex_1().min_h_0())
            .child(div().h(px(32.)).flex_none().px_2().flex().items_center().gap_2()
                .when(self.split, |e| e.child(self.action_button("split-before", "Before").icon(icon("back")).on_click(cx.listener(|v,_,_,cx| {v.pan_diff(false);cx.notify();})))
                    .child(self.action_button("split-after", "After").icon(icon("right")).on_click(cx.listener(|v,_,_,cx| {v.pan_diff(true);cx.notify();}))))
                .child(self.action_button("show-whitespace", "Show whitespace").outline().selected(self.show_whitespace).on_click(cx.listener(|v,_,_,cx| {v.show_whitespace=!v.show_whitespace;cx.notify();})))
                .child(self.action_button("selection-help", "Help").on_click(|_,w,cx| { w.open_dialog(cx,|d,_,_| d.title("Diff help").alert().child("Split: Before / After or Alt+Left / Alt+Right reveals either pane. The horizontal wheel scrolls. Click / Shift-click selects lines. Ctrl/Cmd+A selects all; Ctrl/Cmd+C copies original source.")); })));
        el.into_any_element()
    }
    pub(super) fn keyboard_pan_diff(
        &self,
        after: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.split
            && self.state.patch.is_some()
            && !self.settings_open
            && !window.has_active_dialog(cx)
        {
            self.pan_diff(after);
            cx.notify();
        }
    }
    fn pan_diff(&self, after: bool) {
        let state = self.diff_scroll.0.borrow();
        let width = state
            .last_item_size
            .map(|item| item.contents.width)
            .unwrap_or(px(0.));
        let handle = state.base_handle.clone();
        drop(state);
        let mut offset = handle.offset();
        // Reveal the right pane origin, including its source gutter. A long
        // right pane may still need scrolling to inspect the end of a line.
        offset.x = if after {
            -(width / 2.).min(handle.max_offset().width)
        } else {
            px(0.)
        };
        handle.set_offset(offset);
    }
    fn diff_row(&self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = self.state.patch.as_ref().unwrap();
        let selected = self.selection.as_ref().is_some_and(|r| r.contains(&i));
        let advance = self.preferences.code_size * 0.65;
        let scale = self.preferences.code_size / 12.;
        let width = if self.split {
            (p.split_columns as f32 * advance + 130. * scale) * 2.
        } else {
            p.columns as f32 * advance + 160. * scale
        };
        let mut el = div()
            .id(("diff-line", i))
            .w_full()
            .min_w(px(width.max(if self.split { 950. } else { 600. })))
            .h(px(if self.preferences.compact_diff {
                18.
            } else {
                20.
            })
            .max(px(self.preferences.code_size + 5.)))
            .flex()
            .items_center()
            .font_family(CODE_FONT)
            .text_size(px(self.preferences.code_size))
            .overflow_hidden();
        let cells = if self.split {
            let (a, b) = &p.split[i];
            vec![a.as_ref(), b.as_ref()]
        } else {
            vec![Some(&p.lines[i])]
        };
        for (side, line) in cells.into_iter().enumerate() {
            let mut cell = div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .gap_2()
                .px_2();
            if let Some(l) = line {
                cell = cell
                    .bg(rgb(if selected {
                        SELECTED
                    } else {
                        match l.kind {
                            '+' => DIFF_ADDED,
                            '-' => DIFF_REMOVED,
                            '@' => DIFF_HUNK,
                            _ => CANVAS,
                        }
                    }))
                    .child(
                        div()
                            .w(px(if self.split { 45. * scale } else { 90. * scale }))
                            .flex_none()
                            .whitespace_nowrap()
                            .text_color(rgb(MUTED))
                            .child(if self.split {
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
                                '+' => POSITIVE,
                                '-' => DANGER,
                                _ => MUTED,
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
                if l.kind == '@'
                    && l.text.starts_with("@@ ")
                    && p.id.is_none()
                    && (!self.split || side == 0)
                {
                    let index = p.hunks.iter().position(|h| h == &l.text);
                    let token = p.token;
                    let unstage = p.staged;
                    if let Some(index) = index {
                        cell = cell.child(
                            self.action_button(
                                ("hunk", i),
                                if unstage {
                                    "Unstage hunk"
                                } else {
                                    "Stage hunk"
                                },
                            )
                            .outline()
                            .h(px((if self.preferences.compact_diff {
                                18.0_f32
                            } else {
                                20.0_f32
                            })
                            .max(self.preferences.code_size + 5.)))
                            .text_size(px(12.))
                            .disabled(self.state.busy)
                            .on_click(cx.listener(
                                move |v, _, w, cx| {
                                    v.mutate(
                                        Action::Hunk {
                                            token,
                                            index,
                                            unstage,
                                        },
                                        w,
                                        cx,
                                    )
                                },
                            )),
                        );
                    }
                }
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
