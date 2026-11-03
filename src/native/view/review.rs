use super::*;

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
            .child(
                div()
                    .h(px(42.))
                    .flex()
                    .items_center()
                    .px_2()
                    .gap_2()
                    .child(
                        self.button("back", "‹ History")
                            .on_click(cx.listener(|v, _, w, cx| v.back(&Back, w, cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(p.path.clone()),
                    )
                    .child(
                        self.button(
                            "dim-whitespace",
                            if self.dim_whitespace {
                                "Normal whitespace"
                            } else {
                                "Dim whitespace"
                            },
                        )
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.dim_whitespace = !v.dim_whitespace;
                            cx.notify();
                        })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .p(px(2.))
                            .rounded(px(6.))
                            .bg(rgb(CHROME))
                            .child(
                                self.button("unified-mode", "Unified")
                                    .when(!self.split, |b| b.bg(rgb(RAISED)).text_color(rgb(TEXT)))
                                    .on_click(cx.listener(|v, _, w, cx| {
                                        v.split = false;
                                        v.selection = None;
                                        v.persist_preferences(w, cx);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                self.button("split-mode", "Split")
                                    .when(self.split, |b| b.bg(rgb(RAISED)).text_color(rgb(TEXT)))
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
                div()
                    .h(px(34.))
                    .flex()
                    .items_center()
                    .px_2()
                    .gap_2()
                    .bg(rgb(CHROME))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .when(p.id.is_none(), |bar| {
                        bar.child(div().flex().items_center().gap_1().children(
                            [false, true].into_iter().map(|side| {
                                self.button(
                                    if side {
                                        "source-staged"
                                    } else {
                                        "source-unstaged"
                                    },
                                    if side { "Staged" } else { "Unstaged" },
                                )
                                .when(side == staged, |b| b.bg(rgb(RAISED)).text_color(rgb(TEXT)))
                                .on_click(cx.listener(
                                    move |v, _, w, cx| {
                                        if let Some(p) = &v.state.patch {
                                            v.file(p.path.clone(), side, w, cx);
                                        }
                                    },
                                ))
                            }),
                        ))
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
                            self.button(
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
                        .child(div().text_size(type_size(cx, 16.)).child("File renamed"))
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
            for (index, delta, label) in [(0, -1isize, "↑"), (1, 1isize, "↓")] {
                bar = bar.child(
                    self.button(("hunk-nav", index as usize), label)
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
            .child(div().h(px(26.)).px_3().text_size(type_size(cx, 12.)).text_color(rgb(MUTED)).child("Click / Shift-click selects lines · Ctrl/Cmd+A selects all · Ctrl/Cmd+C copies"));
        el.into_any_element()
    }
    fn diff_row(&self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = self.state.patch.as_ref().unwrap();
        let selected = self.selection.as_ref().is_some_and(|r| r.contains(&i));
        let width = if self.split {
            (p.split_columns as f32 * 7.5 + 130.) * 2.
        } else {
            p.columns as f32 * 7.5 + 160.
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
            .font_family(if cfg!(windows) {
                "Consolas"
            } else {
                "DejaVu Sans Mono"
            })
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
                            _ => PANEL,
                        }
                    }))
                    .child(
                        div()
                            .w(px(if self.split { 45. } else { 90. }))
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
                    .child({
                        let mut end = l.text.len().min(8192);
                        while !l.text.is_char_boundary(end) {
                            end -= 1;
                        }
                        let text = SharedString::from(l.text[..end].to_string());
                        let highlights = l.highlights.iter().filter(|s| s.end <= end).map(|s| {
                            (
                                s.start..s.end,
                                HighlightStyle {
                                    color: Some(
                                        rgb(match s.kind {
                                            'c' => MUTED,
                                            's' => GREEN,
                                            'k' => 0xc678dd,
                                            'n' => 0xd19a66,
                                            _ => TEXT,
                                        })
                                        .into(),
                                    ),
                                    ..Default::default()
                                },
                            )
                        });
                        div()
                            .whitespace_nowrap()
                            .when(self.dim_whitespace, |el| el.opacity(0.8))
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
                            self.button(
                                ("hunk", i),
                                if unstage {
                                    "Unstage hunk"
                                } else {
                                    "Stage hunk"
                                },
                            )
                            .xsmall()
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
