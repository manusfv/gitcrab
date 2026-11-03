use super::*;

impl GitCrab {
    pub(super) fn inspector(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut el = panel();
        if let Some(id) = &self.state.selected {
            if let Some(c) = self
                .state
                .snapshot
                .as_ref()
                .and_then(|s| s.commits.iter().find(|c| &c.id == id))
            {
                let timestamp = c.date.clone();
                let initials = c
                    .author
                    .split_whitespace()
                    .take(2)
                    .filter_map(|s| s.chars().next())
                    .collect::<String>();
                el = el
                    .child(
                        div()
                            .h(px(34.))
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .child(icon("changes").small().text_color(rgb(MUTED)))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Commit details"),
                            ),
                    )
                    .child(
                        div()
                            .px_3()
                            .pt_3()
                            .pb_2()
                            .text_size(type_size(cx, 15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(c.subject.clone()),
                    )
                    .child(
                        div()
                            .id("commit-author")
                            .px_3()
                            .pb_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .size(px(30.))
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
                                    .gap_1()
                                    .child(div().truncate().child(c.author.clone()))
                                    .child(
                                        div()
                                            .text_size(type_size(cx, 12.))
                                            .text_color(rgb(MUTED))
                                            .child(display_date(&c.date)),
                                    ),
                            )
                            .tooltip(move |w, cx| Tooltip::new(timestamp.clone()).build(w, cx)),
                    )
                    .child(
                        self.button("copy-id", c.id.chars().take(10).collect::<String>())
                            .icon(icon("copy"))
                            .font_family("monospace")
                            .text_size(type_size(cx, 12.))
                            .mx_2()
                            .tooltip("Copy full commit SHA")
                            .on_click(cx.listener(|v, _, w, cx| v.copy(&Copy, w, cx))),
                    )
                    .child(div().px_3().flex().flex_wrap().gap_1().children(
                        c.parents.iter().enumerate().map(|(i, parent)| {
                            let id = parent.clone();
                            self.button(
                                ("parent", i),
                                format!("Parent {}", parent.chars().take(8).collect::<String>()),
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
                                cx.listener(move |v, _, w, cx| v.select(Some(id.clone()), w, cx)),
                            )
                        }),
                    ))
                    .child(
                        div()
                            .px_3()
                            .text_size(type_size(cx, 12.))
                            .text_color(rgb(ACCENT))
                            .child(c.refs.join(" · ")),
                    )
                    .child(heading(format!(
                        "Changed files · {}",
                        self.state.files.len()
                    )));
                let weak = cx.weak_entity();
                el = el.child(
                    uniform_list(
                        "committed-files",
                        self.state.files.len(),
                        move |range, _, cx| {
                            weak.update(cx, |v, cx| {
                                range
                                    .map(|i| {
                                        let path = v.state.files[i].clone();
                                        div().h(px(30.)).child(
                                            v.button(("changed", i), path.clone())
                                                .w_full()
                                                .justify_start()
                                                .icon(icon("changes"))
                                                .tooltip(path.clone())
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
                for (i, (op, label)) in [
                    (Operation::Checkout, "Checkout…"),
                    (Operation::CherryPick, "Cherry-pick…"),
                    (Operation::Revert, "Revert…"),
                ]
                .into_iter()
                .enumerate()
                {
                    let id = id.clone();
                    el = el.child(
                        self.button(("commit-op", i), label)
                            .w_full()
                            .justify_start()
                            .px_3()
                            .disabled(self.state.busy)
                            .on_click(cx.listener(move |v, _, w, cx| {
                                v.review(op.clone(), id.clone(), w, cx)
                            })),
                    );
                }
            }
        } else {
            el = el.child(
                div()
                    .h(px(34.))
                    .px_3()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(icon("changes").small().text_color(rgb(ACCENT)))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Working changes"),
                    )
                    .child(div().flex_1())
                    .child(badge(
                        self.state
                            .snapshot
                            .as_ref()
                            .map(|s| s.status.len())
                            .unwrap_or(0)
                            .to_string(),
                        ACCENT,
                    )),
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
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_2()
                        .child(heading(format!(
                            "{}  ·  {}",
                            if staged { "STAGED" } else { "UNSTAGED" },
                            file_count
                        )))
                        .child(
                            self.button(
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
                            .mx_3()
                            .mb_2()
                            .p_3()
                            .rounded(px(6.))
                            .bg(rgb(CHROME))
                            .text_color(rgb(SUBTLE))
                            .text_size(type_size(cx, 12.))
                            .child(if staged {
                                "Stage files or individual hunks to build your commit."
                            } else {
                                "Your working tree is clean."
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
                                            'R' => 0xe5c07b,
                                            _ => ACCENT,
                                        };
                                        let active = v.state.patch.as_ref().is_some_and(|p| {
                                            p.path == f.path && p.staged == staged
                                        });
                                        div()
                                            .w_full()
                                            .h(px(30.))
                                            .flex()
                                            .items_center()
                                            .px_2()
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
                                                    .justify_start()
                                                    .tooltip(path.clone())
                                                    .on_click(cx.listener(move |v, _, w, cx| {
                                                        v.file(path.clone(), staged, w, cx)
                                                    })),
                                                ),
                                            )
                                            .child(
                                                v.button(
                                                    (if staged { "minus" } else { "plus" }, i),
                                                    if staged { "−" } else { "+" },
                                                )
                                                .tooltip(if staged {
                                                    "Unstage file"
                                                } else {
                                                    "Stage file"
                                                })
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
                    .h(px((n as f32 * 30.).clamp(30., 150.)))
                    .min_h(px(30.)),
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
                .child(div().flex_1())
                .child(div().mx_3().mt_2().border_t_1().border_color(rgb(BORDER)))
                .child(
                    div()
                        .px_3()
                        .pt_2()
                        .pb_2()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().font_weight(FontWeight::MEDIUM).child("Commit"))
                        .child(badge(
                            format!("{} / 72", summary_length),
                            if summary_length > 72 { 0xe5c07b } else { MUTED },
                        )),
                )
                .child(
                    div()
                        .px_3()
                        .pb_2()
                        .text_size(type_size(cx, 12.))
                        .text_color(rgb(MUTED))
                        .child(if conflict {
                            "Resolve conflicts before committing."
                        } else if staged {
                            "Only staged files are included. Summary required."
                        } else {
                            "Stage changes, then give your commit a clear summary."
                        }),
                )
                .child(div().px_3().child(Input::new(&self.draft).h(px(34.))))
                .child(
                    div()
                        .px_3()
                        .pt_2()
                        .child(Input::new(&self.description).h(px(64.))),
                )
                .when(conflict, |el| {
                    el.child(
                        div()
                            .p_3()
                            .text_color(rgb(RED))
                            .child("Resolve conflicts with Git, then refresh."),
                    )
                })
                .child(
                    Button::new("commit")
                        .label(format!(
                            "Commit {staged_count} staged {}",
                            if staged_count == 1 { "file" } else { "files" }
                        ))
                        .icon(icon("check"))
                        .primary()
                        .m_3()
                        .tooltip("Commit staged changes · Ctrl/Cmd+Enter")
                        .disabled(!self.can_commit())
                        .on_click(cx.listener(|v, _, w, cx| v.commit_staged(&CommitStaged, w, cx))),
                );
        }
        el.into_any_element()
    }
}
