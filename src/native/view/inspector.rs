use super::*;

fn inspector_bar() -> Div {
    section_bar().h(px(32.)).px(px(INSPECTOR_INSET))
}

impl GitCrab {
    pub(super) fn inspector(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut el = panel();
        if self.state.snapshot.is_none() {
            return el.into_any_element();
        }
        if let Some(id) = &self.state.selected {
            if let Some(c) = self
                .state
                .snapshot
                .as_ref()
                .and_then(|s| s.commits.iter().find(|c| &c.id == id))
            {
                let timestamp = c.date.clone();
                let copy_id = c.id.clone();
                let initials = c
                    .author
                    .split_whitespace()
                    .take(2)
                    .filter_map(|s| s.chars().next())
                    .collect::<String>();
                el = el
                    .child(
                        inspector_bar()
                            .child(icon("changes").small().text_color(rgb(MUTED)))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Commit details"),
                            ),
                    )
                    .child(
                        div()
                            .id("commit-information")
                            .flex()
                            .flex_col()
                            .flex_none()
                            .max_h(relative(0.5))
                            .overflow_y_scroll()
                            .child(
                                div()
                                    .px(px(INSPECTOR_INSET))
                                    .pt_2()
                                    .pb_1()
                                    .text_size(type_size(cx, 15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(c.subject.clone()),
                            )
                            .child(
                                div()
                                    .id("commit-author")
                                    .px(px(INSPECTOR_INSET))
                                    .pb_2()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .size(px(26.))
                                            .flex_none()
                                            .rounded_full()
                                            .bg(rgb(RAISED))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_color(rgb(ACCENT))
                                            .child(initials),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .flex_col()
                                            .child(div().truncate().child(c.author.clone()))
                                            .child(
                                                div()
                                                    .text_size(type_size(cx, 12.))
                                                    .text_color(rgb(MUTED))
                                                    .child(display_date(&c.date)),
                                            ),
                                    )
                                    .tooltip(move |w, cx| {
                                        Tooltip::new(timestamp.clone()).build(w, cx)
                                    }),
                            )
                            .child(
                                self.button("copy-id", c.id.chars().take(10).collect::<String>())
                                    .icon(icon("copy"))
                                    .font_family(CODE_FONT)
                                    .text_size(type_size(cx, 12.))
                                    .mx(px(INSPECTOR_INSET))
                                    .justify_start()
                                    .tooltip("Copy full commit SHA")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy_id.clone(),
                                        ))
                                    }),
                            )
                            .child(
                                div()
                                    .px(px(INSPECTOR_INSET))
                                    .flex()
                                    .flex_wrap()
                                    .gap_1()
                                    .children(c.parents.iter().enumerate().map(|(i, parent)| {
                                        let id = parent.clone();
                                        self.button(
                                            ("parent", i),
                                            format!(
                                                "Parent {}",
                                                parent.chars().take(8).collect::<String>()
                                            ),
                                        )
                                        .disabled(
                                            !self
                                                .state
                                                .snapshot
                                                .as_ref()
                                                .unwrap()
                                                .commits
                                                .iter()
                                                .any(|c| c.id == *parent),
                                        )
                                        .tooltip(parent.clone())
                                        .on_click(
                                            cx.listener(move |v, _, w, cx| {
                                                v.select(Some(id.clone()), w, cx)
                                            }),
                                        )
                                    })),
                            )
                            .child(
                                div()
                                    .px(px(INSPECTOR_INSET))
                                    .text_size(type_size(cx, 12.))
                                    .text_color(rgb(ACCENT))
                                    .child(c.refs.join(" · ")),
                            ),
                    )
                    .child(
                        inspector_bar()
                            .child(format!("Changed files · {}", self.state.files.len())),
                    );
                let weak = cx.weak_entity();
                el = el.child(
                    uniform_list(
                        "committed-files",
                        self.state.files.len(),
                        move |range, _, cx| {
                            weak.update(cx, |v, cx| {
                                range
                                    .map(|i| {
                                        let change = &v.state.files[i];
                                        let path = change.path.clone();
                                        let label = change
                                            .original_path
                                            .as_ref()
                                            .map(|old| format!("{old} → {path}"))
                                            .unwrap_or_else(|| path.clone());
                                        div().h(px(FILE_ROW_HEIGHT)).child(
                                            v.button(("changed", i), label.clone())
                                                .w_full()
                                                .px(px(INSPECTOR_INSET))
                                                .justify_start()
                                                .icon(icon("changes"))
                                                .tooltip(label.clone())
                                                .on_click(cx.listener(move |v, _, w, cx| {
                                                    v.file(path.clone(), false, w, cx)
                                                })),
                                        )
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                        },
                    )
                    .flex_1()
                    .min_h_0(),
                );
                el = el.child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .px(px(INSPECTOR_INSET))
                        .py(px(6.))
                        .border_t_1()
                        .border_color(rgb(BORDER))
                        .children(
                            [
                                (Operation::Checkout, "Checkout…"),
                                (Operation::CherryPick, "Cherry-pick…"),
                                (Operation::Revert, "Revert…"),
                            ]
                            .into_iter()
                            .enumerate()
                            .map(|(i, (op, label))| {
                                let id = id.clone();
                                self.action_button(("commit-op", i), label)
                                    .flex_none()
                                    .disabled(self.state.busy)
                                    .on_click(cx.listener(move |v, _, w, cx| {
                                        v.review(op.clone(), id.clone(), w, cx)
                                    }))
                            }),
                        ),
                );
            }
        } else {
            el = el.child(
                inspector_bar()
                    .child(icon("changes").small().text_color(rgb(ACCENT)))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Working changes"),
                    )
                    .child(div().flex_1()),
            );
            for staged in [false, true] {
                let file_count = self
                    .state
                    .snapshot
                    .as_ref()
                    .map(|s| {
                        s.status
                            .iter()
                            .filter(|f| {
                                if staged {
                                    f.index != ' ' && f.index != '?'
                                } else {
                                    f.worktree != ' ' || f.index == '?'
                                }
                            })
                            .count()
                    })
                    .unwrap_or(0);
                el = el.child(
                    inspector_bar()
                        .bg(rgb(PANEL))
                        .border_b_0()
                        .justify_between()
                        .child(div().font_weight(FontWeight::MEDIUM).child(format!(
                            "{} ({file_count})",
                            if staged {
                                "Staged Files"
                            } else {
                                "Unstaged Files"
                            }
                        )))
                        .child(
                            self.action_button(
                                if staged { "unstage-all" } else { "stage-all" },
                                if staged { "Unstage all" } else { "Stage all" },
                            )
                            .tooltip(if staged {
                                "Return all staged changes to the working tree"
                            } else {
                                "Stage all working changes for the next commit"
                            })
                            .disabled(self.state.busy || file_count == 0)
                            .on_click(cx.listener(
                                move |v, _, w, cx| {
                                    v.mutate(
                                        Action::Stage {
                                            path: ".".into(),
                                            unstage: staged,
                                        },
                                        w,
                                        cx,
                                    )
                                },
                            )),
                        ),
                );
                let files = self
                    .state
                    .snapshot
                    .as_ref()
                    .map(|s| {
                        s.status
                            .iter()
                            .filter(|f| {
                                if staged {
                                    f.index != ' ' && f.index != '?'
                                } else {
                                    f.worktree != ' ' || f.index == '?'
                                }
                            })
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let weak = cx.weak_entity();
                let n = files.len();
                if n == 0 {
                    el = el.child(
                        div()
                            .flex_none()
                            .px(px(INSPECTOR_INSET))
                            .py_1()
                            .text_color(rgb(SUBTLE))
                            .text_size(type_size(cx, 12.))
                            .child(if staged {
                                "No staged changes."
                            } else {
                                "No unstaged changes."
                            }),
                    );
                    continue;
                }
                el = el.child(
                    uniform_list(
                        if staged {
                            "staged-files"
                        } else {
                            "unstaged-files"
                        },
                        n,
                        move |range, _, cx| {
                            weak.update(cx, |v, cx| {
                                range
                                    .map(|i| {
                                        let f = &files[i];
                                        let path = f.path.clone();
                                        let stage_path = path.clone();
                                        let status = if f.index == '?' {
                                            '?'
                                        } else if staged {
                                            f.index
                                        } else {
                                            f.worktree
                                        };
                                        let tint = match status {
                                            'A' | '?' => GREEN,
                                            'D' | 'U' => RED,
                                            'R' => WARNING,
                                            _ => ACCENT,
                                        };
                                        let active = v.state.patch.as_ref().is_some_and(|p| {
                                            p.path == f.path && p.staged == staged
                                        });
                                        div()
                                            .w_full()
                                            .h(px(FILE_ROW_HEIGHT))
                                            .flex()
                                            .items_center()
                                            .px(px(INSPECTOR_INSET))
                                            .gap_1()
                                            .when(active, |e| e.bg(rgb(SELECTED)))
                                            .hover(|e| e.bg(rgb(HOVER)))
                                            .child(badge(status.to_string(), tint))
                                            .child(
                                                div().flex_1().min_w_0().child(
                                                    v.button(
                                                        (
                                                            if staged {
                                                                "index"
                                                            } else {
                                                                "worktree"
                                                            },
                                                            i,
                                                        ),
                                                        path.clone(),
                                                    )
                                                    .w_full()
                                                    .px_1()
                                                    .justify_start()
                                                    .tooltip(path.clone())
                                                    .on_click(cx.listener(move |v, _, w, cx| {
                                                        v.file(path.clone(), staged, w, cx)
                                                    })),
                                                ),
                                            )
                                            .child(
                                                v.icon_button(
                                                    (if staged { "minus" } else { "plus" }, i),
                                                    if staged { "minus" } else { "plus" },
                                                    format!(
                                                        "{} {}",
                                                        if staged { "Unstage" } else { "Stage" },
                                                        f.path
                                                    ),
                                                )
                                                .disabled(v.state.busy)
                                                .on_click(cx.listener(move |v, _, w, cx| {
                                                    v.mutate(
                                                        Action::Stage {
                                                            path: stage_path.clone(),
                                                            unstage: staged,
                                                        },
                                                        w,
                                                        cx,
                                                    )
                                                })),
                                            )
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                        },
                    )
                    .h(px(
                        (n as f32 * FILE_ROW_HEIGHT).clamp(FILE_ROW_HEIGHT, FILE_ROW_HEIGHT * 4.)
                    )),
                );
            }
            let staged_count = self
                .state
                .snapshot
                .as_ref()
                .map(|s| {
                    s.status
                        .iter()
                        .filter(|f| Controller::on_side(f, true))
                        .count()
                })
                .unwrap_or(0);
            let summary_length = self.draft.read(cx).value().chars().count();
            let staged = self
                .state
                .snapshot
                .as_ref()
                .is_some_and(|s| s.status.iter().any(|f| f.index != ' ' && f.index != '?'));
            let conflict = self.state.snapshot.as_ref().is_some_and(|s| {
                s.status.iter().any(|f| {
                    f.index == 'U'
                        || f.worktree == 'U'
                        || matches!((f.index, f.worktree), ('A', 'A') | ('D', 'D'))
                })
            });
            el = el
                .child(div().flex_1().min_h_0())
                .child(div().border_t_1().border_color(rgb(BORDER)))
                .child(
                    inspector_bar()
                        .justify_between()
                        .child(div().font_weight(FontWeight::MEDIUM).child("Commit"))
                        .when(summary_length >= 50, |e| {
                            e.child(badge(
                                format!("{} / 72", summary_length),
                                if summary_length > 72 { WARNING } else { MUTED },
                            ))
                        }),
                )
                .when(
                    conflict || !staged || self.draft.read(cx).value().trim().is_empty(),
                    |el| {
                        el.child(
                            div()
                                .px(px(INSPECTOR_INSET))
                                .py_1()
                                .text_size(type_size(cx, 12.))
                                .text_color(rgb(MUTED))
                                .child(if conflict {
                                    "Resolve conflicts with Git, then refresh."
                                } else if staged {
                                    "Enter a summary to commit staged changes."
                                } else {
                                    "Stage changes to create a commit."
                                }),
                        )
                    },
                )
                .child(
                    div()
                        .mx(px(INSPECTOR_INSET))
                        .flex_none()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .rounded(px(CONTROL_RADIUS))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(CHROME))
                        .child(if self.settings_open {
                            self.editor(
                                &self.draft,
                                30.,
                                self.inspector_editor_width(),
                                "Commit summary",
                                cx,
                            )
                        } else {
                            Input::new(&self.draft)
                                .bordered(false)
                                .w_full()
                                .h(px(30.))
                                .into_any_element()
                        })
                        .child(div().border_t_1().border_color(rgb(BORDER)))
                        .child(if self.settings_open {
                            self.editor(
                                &self.description,
                                76.,
                                self.inspector_editor_width(),
                                "Description (optional)",
                                cx,
                            )
                        } else {
                            Input::new(&self.description)
                                .bordered(false)
                                .w_full()
                                .h(px(76.))
                                .into_any_element()
                        }),
                )
                .child(
                    self.primary_button(
                        "commit",
                        format!(
                            "Commit {staged_count} staged {}",
                            if staged_count == 1 { "file" } else { "files" }
                        ),
                    )
                    .icon(icon("check"))
                    .mx(px(INSPECTOR_INSET))
                    .my(px(6.))
                    .tooltip("Commit staged changes · Ctrl/Cmd+Enter")
                    .disabled(!self.can_commit())
                    .on_click(cx.listener(|v, _, w, cx| v.commit_staged(&CommitStaged, w, cx))),
                );
        }
        el.into_any_element()
    }
}
