use super::*;
use gitcrab_core::{
    git::FileChange,
    presentation::{ordinal, reflow_body},
};

const INSPECTOR_BAR_HEIGHT: f32 = 32.;
/// Vertical gap between the parts of the commit composer.
const COMPOSER_GAP: f32 = 8.;

fn inspector_bar() -> Div {
    section_bar()
        .h(scaled(INSPECTOR_BAR_HEIGHT))
        .px(scaled(INSPECTOR_INSET))
}

/// Working-changes list that a file row belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Conflicts,
    Unstaged,
    Staged,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Self::Conflicts => "Conflicts",
            Self::Unstaged => "Unstaged",
            Self::Staged => "Staged",
        }
    }
    fn staged(self) -> bool {
        self == Self::Staged
    }
}

/// One fixed-height row in the working-changes scroller.
#[derive(Clone, Copy)]
enum FileRow {
    Header(Section, usize),
    File(Section, usize),
    Empty(Section),
}

impl FileRow {
    fn section(self) -> Section {
        match self {
            Self::Header(section, _) | Self::File(section, _) | Self::Empty(section) => section,
        }
    }
}

/// Names the status in text and pairs it with the status color.
fn status_badge(file: &FileChange, staged: bool) -> impl IntoElement {
    let status = if file.index == '?' {
        '?'
    } else if staged {
        file.index
    } else {
        file.worktree
    };
    let (color, letter) = if file.is_unmerged() {
        (semantic(Role::StatusConflictFg), '!')
    } else {
        (file_status_color(status), status)
    };
    // A square single-letter chip; the full name is in the tooltip.
    let name = if file.is_unmerged() {
        format!("Conflict: {}", file.status_name(staged))
    } else if status == '?' {
        "Untracked: Git isn't tracking this file yet".to_string()
    } else {
        format!(
            "{} ({})",
            file.status_name(staged),
            if staged { "staged" } else { "unstaged" }
        )
    };
    Badge::new(letter.to_string(), color)
        .fixed_width(20.)
        .text_size(scaled(TEXT_SIZE_CAPTION))
        .px_0()
        .tooltip(SharedString::from(format!("status-{}", file.path)), name)
}

impl GitCrab {
    pub(super) fn inspector(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let mut el = panel().id("inspector-scroll").overflow_y_scroll();
        if self.state.snapshot.is_none() {
            return el.into_any_element();
        }
        if let Some(id) = &self.state.selected {
            if let Some((index, c)) = self
                .state
                .snapshot
                .as_ref()
                .and_then(|s| s.commits.iter().enumerate().find(|(_, c)| &c.id == id))
            {
                let commands = self.command_context(window, cx);
                el = self.commit_details(el, window, index, c, &commands, cx);
            }
        } else {
            // Working changes is a fixed-height column: its file list scrolls on
            // its own and the composer stays pinned below it. Inside a scroll
            // container the list would lose its definite height and collapse.
            return self
                .working_changes(panel().id("inspector-working"), window, cx)
                .into_any_element();
        }
        el.into_any_element()
    }

    fn commit_details(
        &self,
        mut el: Stateful<Div>,
        window: &mut Window,
        index: usize,
        c: &gitcrab_core::git::Commit,
        commands: &CommandContext,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = c.id.clone();
        let parents = c.parents.len();
        let lane_color = self.state.graph.rows.get(index).map(|row| row.color);
        let copy_id = c.id.clone();
        let loading = self.state.details_loading();
        let email = self.state.commit_author_email.clone();
        let author = if email.is_empty() {
            c.author.clone()
        } else {
            format!("{} <{email}>", c.author)
        };
        let copy_author = author.clone();
        let copy_info = format!(
            "commit {}\nAuthor: {author}\nDate:   {}\n\n{}{}",
            c.id,
            c.date,
            c.subject,
            if self.state.commit_body.is_empty() {
                String::new()
            } else {
                format!("\n\n{}", self.state.commit_body)
            }
        );
        let initials = c
            .author
            .split_whitespace()
            .take(2)
            .filter_map(|s| s.chars().next())
            .collect::<String>();
        let short_id = c.id.chars().take(7).collect::<String>();
        let date_tooltip = c.date.clone();
        // Counts per kind of change: added, modified, deleted, renamed.
        let mut counts = [0usize; 4];
        for file in &self.state.files {
            match file.index {
                'A' | '?' => counts[0] += 1,
                'D' => counts[2] += 1,
                'R' | 'C' => counts[3] += 1,
                _ => counts[1] += 1,
            }
        }
        let summary = [
            ('A', "added"),
            ('M', "modified"),
            ('D', "deleted"),
            ('R', "renamed"),
        ]
        .into_iter()
        .zip(counts)
        .filter(|(_, n)| *n > 0)
        .map(|((letter, word), n)| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    Badge::new(letter.to_string(), file_status_color(letter))
                        .fixed_width(18.)
                        .text_size(scaled(TEXT_SIZE_CAPTION))
                        .px_0(),
                )
                .child(
                    div()
                        .text_size(scaled(TEXT_SIZE_SMALL))
                        .text_role(Role::TextSecondary)
                        .child(format!("{n} {word}")),
                )
        })
        .collect::<Vec<_>>();
        el = el
            .child(
                inspector_bar()
                    .child(
                        div()
                            .text_size(scaled(TEXT_SIZE_SMALL))
                            .text_role(Role::TextMuted)
                            .child("commit:"),
                    )
                    .child(
                        self.button("copy-id", short_id)
                            .font_family(CODE_FONT)
                            .text_size(scaled(TEXT_SIZE_SMALL))
                            .tooltip(format!("Copy {}", c.id))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy_id.clone()))
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        self.icon_button(
                            "copy-commit-info",
                            "copy",
                            "Copy commit details",
                        )
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_info.clone()))
                        }),
                    ),
            )
            .child(
                scroll_area(
                    "commit-information",
                    &scroll_handle(window, cx, "commit-information-scroll"),
                    div()
                        .id("commit-information")
                        .flex()
                        .flex_col()
                        .gap(scaled(10.))
                        .py(scaled(10.))
                        // The message, framed so it reads as one block.
                        .child(
                            div()
                                .id("commit-message")
                                .mx(scaled(INSPECTOR_INSET))
                                .p(scaled(10.))
                                .flex()
                                .flex_col()
                                .gap(scaled(6.))
                                .rounded(px(CONTROL_RADIUS))
                                .border_1()
                                .border_role(Role::BorderDivider)
                                .bg_role(Role::SurfaceChrome)
                                .child(
                                    div()
                                        .text_size(scaled(TEXT_SIZE_TITLE))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .whitespace_normal()
                                        .child(c.subject.clone()),
                                )
                                .when(!self.state.commit_body.is_empty(), |el| {
                                    el.child(
                                        div()
                                            .id("commit-body")
                                            .text_size(scaled(TEXT_SIZE_SMALL))
                                            .text_role(Role::TextSecondary)
                                            .whitespace_normal()
                                            .children(
                                                reflow_body(&self.state.commit_body)
                                                    .into_iter()
                                                    .map(|line| {
                                                        div().min_h(scaled(16.)).child(line)
                                                    })
                                                    .collect::<Vec<_>>(),
                                            ),
                                    )
                                })
                                .when(loading && self.state.commit_body.is_empty(), |el| {
                                    el.child(
                                        div()
                                            .text_size(scaled(TEXT_SIZE_SMALL))
                                            .text_role(Role::TextMuted)
                                            .child("Loading description…"),
                                    )
                                }),
                        )
                        // Author on the left, parents on the right.
                        .child(
                            div()
                                .id("commit-author")
                                .px(scaled(INSPECTOR_INSET))
                                .flex()
                                .items_start()
                                .gap(scaled(10.))
                                .child(
                                    div()
                                        .size(scaled(36.))
                                        .relative()
                                        .flex_none()
                                        .rounded(px(CONTROL_RADIUS))
                                        .overflow_hidden()
                                        .bg_role(Role::SurfaceOverlay)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_role(Role::ActionPrimary)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(initials)
                                        .when_some(
                                            self.author_images.get(&c.author).cloned(),
                                            |avatar, path| {
                                                avatar.child(
                                                    img(path)
                                                        .absolute()
                                                        .inset_0()
                                                        .size_full()
                                                        .object_fit(ObjectFit::Cover),
                                                )
                                            },
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .truncate()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .child(c.author.clone()),
                                                )
                                                .child(
                                                    self.icon_button(
                                                        "copy-author",
                                                        "copy",
                                                        "Copy author name and email",
                                                    )
                                                    .on_click(move |_, _, cx| {
                                                        cx.write_to_clipboard(
                                                            ClipboardItem::new_string(
                                                                copy_author.clone(),
                                                            ),
                                                        )
                                                    }),
                                                ),
                                        )
                                        .when(!email.is_empty(), |el| {
                                            el.child(
                                                div()
                                                    .truncate()
                                                    .text_size(scaled(TEXT_SIZE_SMALL))
                                                    .text_role(Role::TextMuted)
                                                    .child(email.clone()),
                                            )
                                        })
                                        .child(
                                            div()
                                                .id("commit-date")
                                                .flex()
                                                .gap_1()
                                                .text_size(scaled(TEXT_SIZE_SMALL))
                                                .child(
                                                    div()
                                                        .text_role(Role::TextMuted)
                                                        .child("authored"),
                                                )
                                                .child(
                                                    div()
                                                        .text_role(Role::TextSecondary)
                                                        .child(display_date(&c.date)),
                                                )
                                                .tooltip(move |w, cx| {
                                                    Tooltip::new(date_tooltip.clone()).build(w, cx)
                                                }),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .flex()
                                        .flex_col()
                                        .items_end()
                                        .gap(px(2.))
                                        .when(c.parents.is_empty(), |el| {
                                            el.child(
                                                div()
                                                    .text_size(scaled(TEXT_SIZE_SMALL))
                                                    .text_role(Role::TextMuted)
                                                    .child("root commit"),
                                            )
                                        })
                                        .children(c.parents.iter().enumerate().map(
                                            |(i, parent)| {
                                                let id = parent.clone();
                                                let loaded =
                                                    self.state.snapshot.as_ref().is_some_and(|s| {
                                                        s.commits.iter().any(|c| c.id == *parent)
                                                    });
                                                let short =
                                                    parent.chars().take(7).collect::<String>();
                                                let what = if c.parents.len() > 1 {
                                                    format!("{} parent", ordinal(i).to_lowercase())
                                                } else {
                                                    "parent".to_string()
                                                };
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .when(i == 0, |row| {
                                                        row.child(
                                                            div()
                                                                .text_size(scaled(TEXT_SIZE_SMALL))
                                                                .text_role(Role::TextMuted)
                                                                .child(if c.parents.len() > 1 {
                                                                    "parents:"
                                                                } else {
                                                                    "parent:"
                                                                }),
                                                        )
                                                    })
                                                    .child(
                                                        self.button(("parent", i), short)
                                                            .font_family(CODE_FONT)
                                                            .text_size(scaled(TEXT_SIZE_SMALL))
                                                            .tooltip(if loaded {
                                                                format!("Select {what} {parent}")
                                                            } else {
                                                                format!(
                                                                    "Load more history and select {what} {parent}"
                                                                )
                                                            })
                                                            .on_click(cx.listener(
                                                                move |v, _, w, cx| {
                                                                    v.seek_commit(id.clone(), w, cx)
                                                                },
                                                            )),
                                                    )
                                            },
                                        )),
                                ),
                        )
                        .when(!c.refs.is_empty(), |el| {
                            el.child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_1()
                                    .px(scaled(INSPECTOR_INSET))
                                    .children(c.refs.iter().map(|reference| {
                                        div()
                                            .px_1()
                                            .rounded(px(CONTROL_RADIUS))
                                            .text_size(scaled(TEXT_SIZE_SMALL))
                                            .text_role(Role::TextDefault)
                                            .when_some(lane_color, |badge, color| {
                                                badge.bg(branch_tint(color, REF_TINT))
                                            })
                                            .child(reference.clone())
                                    })),
                            )
                        }),
                )
                .flex_none()
                // Leave room for the changed files and commit actions at large text sizes.
                .max_h(relative(0.45)),
            )
            .child(
                inspector_bar()
                    .border_t_1()
                    .gap(scaled(12.))
                    .when(loading, |bar| bar.child("Loading changes…"))
                    .when(self.state.details_failed && !loading, |bar| {
                        let id = id.clone();
                        bar.child(div().flex_1().child("Changes unavailable")).child(
                            self.button("retry-details", "Retry").on_click(cx.listener(
                                move |v, _, w, cx| v.select(Some(id.clone()), w, cx),
                            )),
                        )
                    })
                    .when(!loading && !self.state.details_failed, |bar| {
                        bar.children(summary).child(div().flex_1()).child(
                            div()
                                .text_size(scaled(TEXT_SIZE_SMALL))
                                .text_role(Role::TextMuted)
                                .child(match self.state.files.len() {
                                    1 => "1 file".to_string(),
                                    n => format!("{n} files"),
                                }),
                        )
                    }),
            );
        let empty_files = if loading {
            Some("Loading the files changed by this commit…")
        } else if self.state.details_failed {
            Some("The changed files could not be loaded. Retry, or refresh the repository.")
        } else if self.state.files.is_empty() {
            Some("This commit changes no files compared with its first parent.")
        } else {
            None
        };
        if let Some(message) = empty_files {
            el = el.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px(scaled(INSPECTOR_INSET))
                    .py_2()
                    .text_size(scaled(TEXT_SIZE_SMALL))
                    .text_role(Role::TextSecondary)
                    .child(message),
            );
        } else {
            let weak = cx.weak_entity();
            let files_scroll = window
                .use_keyed_state("committed-files-scroll", cx, |_, _| {
                    UniformListScrollHandle::new()
                })
                .read(cx)
                .clone();
            let files_over = overflows_vertically(&files_scroll);
            el = el.child(
                track_overflow(div(), &files_scroll, files_over)
                    .relative()
                    .flex_1()
                    // Keep at least two files reachable when details are long.
                    .min_h(scaled(2. * FILE_ROW_HEIGHT))
                    .child(
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
                                                .map(|old| {
                                                    format!(
                                                        "{} → {}",
                                                        gitcrab_core::git::display_path(old),
                                                        change.display_path()
                                                    )
                                                })
                                                .unwrap_or_else(|| change.display_path());
                                            let active = v.state.diff_context.as_ref().is_some_and(
                                                |context| {
                                                    context.id.is_some()
                                                        && context.path == change.path
                                                },
                                            );
                                            ListRow::new(("changed", i))
                                                .inset(INSPECTOR_INSET)
                                                .tab_stop(!v.settings_open)
                                                .start(status_badge(change, true))
                                                .label(label)
                                                .active(active)
                                                .disabled(change.unsupported_path)
                                                .tooltip(change.describe(true))
                                                .on_click(cx.listener(move |v, _, w, cx| {
                                                    v.file(path.clone(), false, w, cx)
                                                }))
                                        })
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default()
                            },
                        )
                        .when(files_over, |list| list.pr(SCROLLBAR_GUTTER))
                        .track_scroll(&files_scroll)
                        .size_full(),
                    )
                    .child(scrollbar_overlay("committed-files", &files_scroll)),
            );
        }
        el.child(
            div()
                .flex_none()
                .flex()
                .flex_wrap()
                .gap_1()
                .px(scaled(INSPECTOR_INSET))
                .py(px(6.))
                .border_t_1()
                .border_role(Role::BorderDivider)
                .children(
                    [Command::Checkout, Command::CherryPick, Command::Revert]
                        .into_iter()
                        .enumerate()
                        .map(|(i, command)| {
                            let id = id.clone();
                            self.action_button(("commit-op", i), command.menu_label())
                                .flex_none()
                                .disabled_when(
                                    !command.availability_for_commit(commands, parents).enabled(),
                                )
                                .when(
                                    !command.availability_for_commit(commands, parents).enabled(),
                                    |button| {
                                        button
                                            .tooltip(command.describe_for_commit(commands, parents))
                                    },
                                )
                                .on_click(cx.listener(move |v, _, w, cx| {
                                    v.run_command(command, id.clone(), w, cx)
                                }))
                        }),
                ),
        )
    }

    fn working_changes(
        &self,
        mut el: Stateful<Div>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let snapshot = self.state.snapshot.as_ref().unwrap();
        let conflicts = snapshot.conflicts();
        let unstaged = snapshot.side(false);
        let staged = snapshot.side(true);
        el = el.child(
            inspector_bar()
                .child(icon("changes").small().text_role(Role::ActionPrimary))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Working changes"),
                )
                .child(div().flex_1()),
        );
        if !conflicts.is_empty() || !snapshot.operations.is_empty() {
            el = el.child(self.operation_banner(conflicts.len(), cx));
        }
        if snapshot.status.is_empty() {
            el = el.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .p_4()
                    .child(icon("check").text_role(Role::StatusSuccessFg))
                    .child(
                        div()
                            .text_size(scaled(TEXT_SIZE_TITLE))
                            .child("No working changes"),
                    )
                    .child(
                        div()
                            .text_size(scaled(TEXT_SIZE_SMALL))
                            .text_role(Role::TextSecondary)
                            .child(if snapshot.head_id.is_empty() {
                                "Add files to this folder to prepare the first commit."
                            } else {
                                "The working tree and index match HEAD."
                            }),
                    ),
            );
        } else {
            let mut rows = vec![];
            for (section, files) in [
                (Section::Conflicts, &conflicts),
                (Section::Unstaged, &unstaged),
                (Section::Staged, &staged),
            ] {
                if section == Section::Conflicts && files.is_empty() {
                    continue;
                }
                rows.push(FileRow::Header(section, files.len()));
                if files.is_empty() {
                    rows.push(FileRow::Empty(section));
                }
                rows.extend(files.iter().map(|&index| FileRow::File(section, index)));
            }
            let row_height = type_size(cx, FILE_ROW_HEIGHT);
            let scroll = window
                .use_keyed_state("working-files-scroll", cx, |_, _| {
                    UniformListScrollHandle::new()
                })
                .read(cx)
                .clone();
            let offset = -f32::from(scroll.0.borrow().base_handle.offset().y);
            let first = (offset / f32::from(row_height)).floor().max(0.) as usize;
            // A header that scrolled out of view stays pinned above its rows.
            let sticky = (offset > 0.5)
                .then(|| rows.get(first).copied())
                .flatten()
                .map(FileRow::section)
                .and_then(|section| {
                    rows.iter().find_map(|row| match row {
                        FileRow::Header(s, count) if *s == section => Some((section, *count)),
                        _ => None,
                    })
                });
            let weak = cx.weak_entity();
            let working_over = overflows_vertically(&scroll);
            let list_rows = rows.clone();
            el = el.child(
                track_overflow(div(), &scroll, working_over)
                    .relative()
                    .flex_1()
                    .min_h(row_height * 3.)
                    .child(
                        uniform_list("working-files", rows.len(), move |range, window, cx| {
                            weak.update(cx, |v, cx| {
                                range
                                    .map(|i| v.working_row(list_rows[i], i, window, cx))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                        })
                        .when(working_over, |list| list.pr(SCROLLBAR_GUTTER))
                        .track_scroll(&scroll)
                        .size_full(),
                    )
                    .when_some(sticky, |list, (section, count)| {
                        list.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .right(if working_over {
                                    SCROLLBAR_GUTTER
                                } else {
                                    px(0.)
                                })
                                .child(self.section_header(section, count, true, cx)),
                        )
                    })
                    .child(scrollbar_overlay("working-files", &scroll)),
            );
        }
        el.child(self.composer(cx))
    }

    fn operation_banner(&self, conflicts: usize, cx: &mut Context<Self>) -> Div {
        let snapshot = self.state.snapshot.as_ref().unwrap();
        let title = match snapshot.operation_label() {
            Some(op) => {
                let mut title = format!("{op} in progress");
                title[..1].make_ascii_uppercase();
                title
            }
            None => "Unresolved conflicts".into(),
        };
        let first_conflict = snapshot
            .conflicts()
            .first()
            .map(|&index| snapshot.status[index].path.clone());
        div()
            .flex_none()
            .mx(scaled(INSPECTOR_INSET))
            .my_2()
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .rounded(px(CONTROL_RADIUS))
            .border_1()
            .border_role(Role::StatusConflictFg)
            .bg_role(Role::StatusConflictBg)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .size(px(8.))
                            .flex_none()
                            .rounded_full()
                            .bg_role(Role::StatusConflictFg),
                    )
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_role(Role::TextDefault)
                            .child(title),
                    )
                    .child(
                        self.button("operation-refresh", "Refresh")
                            .icon(icon("refresh"))
                            .tooltip_with_action(
                                "Refresh after continuing or aborting in a terminal",
                                &Refresh,
                                Some(KEY_CONTEXT),
                            )
                            .on_click(cx.listener(|v, _, w, cx| v.refresh(&Refresh, w, cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_size(scaled(TEXT_SIZE_SMALL))
                            .text_role(Role::TextDefault)
                            .child(match conflicts {
                                0 => "All conflicts are marked resolved.".to_string(),
                                1 => "1 unresolved file".to_string(),
                                n => format!("{n} unresolved files"),
                            }),
                    )
                    .when_some(first_conflict, |row, path| {
                        row.child(
                            self.action_button("review-conflicts", "Review conflicts")
                                .on_click(cx.listener(move |v, _, w, cx| {
                                    v.file(path.clone(), false, w, cx)
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .text_size(scaled(TEXT_SIZE_SMALL))
                    .text_role(Role::TextSecondary)
                    .child(snapshot.operation_recovery()),
            )
    }

    fn section_header(
        &self,
        section: Section,
        count: usize,
        sticky: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let conflicts = self
            .state
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.conflict_count());
        let staged = section.staged();
        let conflict_note = if staged {
            "Conflicted files keep their conflict entries"
        } else {
            "Conflicted files stay unresolved"
        };
        div()
            .w_full()
            .h(scaled(FILE_ROW_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px(scaled(INSPECTOR_INSET))
            .bg_role(Role::SurfaceChrome)
            .border_b_1()
            .border_role(Role::BorderDivider)
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .when(section == Section::Conflicts, |title| {
                        title.text_role(Role::StatusConflictFg)
                    })
                    .child(format!("{} ({count})", section.title())),
            )
            .child(div().flex_1())
            .when(section != Section::Conflicts, |header| {
                let id = match (staged, sticky) {
                    (true, false) => "unstage-all",
                    (true, true) => "unstage-all-pinned",
                    (false, false) => "stage-all",
                    (false, true) => "stage-all-pinned",
                };
                header.child(
                    self.semantic_button(
                        id,
                        if staged { "Unstage all" } else { "Stage all" },
                        if staged {
                            components::ButtonKind::Unstage
                        } else {
                            components::ButtonKind::Stage
                        },
                        ControlSize::DenseBulk,
                        cx,
                    )
                    .flex_none()
                    .when(conflicts > 0, |button| button.tooltip(conflict_note))
                    .disabled_when(self.state.blocking_write() || count == 0)
                    .on_click(cx.listener(move |v, _, w, cx| {
                        v.mutate(Action::StageAll { unstage: staged }, w, cx)
                    })),
                )
            })
    }

    fn working_row(
        &self,
        row: FileRow,
        i: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (section, index) = match row {
            FileRow::Header(section, count) => {
                return self
                    .section_header(section, count, false, cx)
                    .into_any_element()
            }
            FileRow::Empty(section) => {
                return div()
                    .h(scaled(FILE_ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .px(scaled(INSPECTOR_INSET))
                    .text_role(Role::TextSecondary)
                    .text_size(scaled(TEXT_SIZE_SMALL))
                    .child(if section.staged() {
                        "Nothing staged yet."
                    } else {
                        "No unstaged changes."
                    })
                    .into_any_element()
            }
            FileRow::File(section, index) => (section, index),
        };
        let Some(f) = self
            .state
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.status.get(index))
        else {
            return div().h(scaled(FILE_ROW_HEIGHT)).into_any_element();
        };
        let staged = section.staged();
        let conflict = section == Section::Conflicts;
        let row_focus = window
            .use_keyed_state(
                SharedString::from(format!("file-action-focus-{}-{}", section.title(), f.path)),
                cx,
                |_, cx| cx.focus_handle(),
            )
            .read(cx)
            .clone();
        let path_issue = f.path_issue();
        let path = f.path.clone();
        let display_path = f.display_path();
        let reviewed = f.clone();
        let active = self.state.diff_context.as_ref().is_some_and(|context| {
            context.id.is_none() && context.path == f.path && (conflict || context.staged == staged)
        });
        let markers = conflict && self.state.marker_refusals.contains(&f.path);
        // Shown in its predicted list while Git stages or unstages it.
        let moving = self.state.pending_paths.contains(&f.path);
        let action = if conflict {
            let path = f.path.clone();
            self.action_button(("mark-resolved", i), "Mark resolved…")
                .flex_none()
                .disabled_when(self.state.blocking_write() || path_issue.is_some())
                .on_click(cx.listener(move |v, _, w, cx| v.review_resolution(path.clone(), w, cx)))
                .into_any_element()
        } else {
            components::file_action(
                self.icon_button(
                    (if staged { "minus" } else { "plus" }, i),
                    if staged { "minus" } else { "plus" },
                    format!(
                        "{} {}",
                        if staged { "Unstage" } else { "Stage" },
                        display_path
                    ),
                )
                .icon(
                    icon(if staged { "minus" } else { "plus" })
                        .size(px(geometry::SIZE_FILE_ACTION_GLYPH)),
                )
                .disabled_when(self.state.blocking_write() || path_issue.is_some())
                .on_click(cx.listener(move |v, _, w, cx| {
                    v.mutate(
                        Action::StageFile {
                            reviewed: reviewed.clone(),
                            unstage: staged,
                        },
                        w,
                        cx,
                    )
                })),
                &row_focus,
                self.preferences.always_show_file_actions,
                window,
                cx,
            )
            .into_any_element()
        };
        let id = match section {
            Section::Conflicts => "conflict",
            Section::Staged => "index",
            Section::Unstaged => "worktree",
        };
        ListRow::new((id, i))
            .inset(INSPECTOR_INSET)
            .tab_stop(!self.settings_open)
            .group("file-row")
            .track_focus(&row_focus)
            .when(moving, |row| row.opacity(0.55))
            .start(status_badge(f, staged))
            .when(markers, |row| {
                row.leading(
                    Badge::role("Markers", Role::StatusWarningFg)
                        .text_size(scaled(TEXT_SIZE_CAPTION))
                        .px_1(),
                )
            })
            .label(display_path)
            .active(active)
            .disabled(path_issue.is_some())
            .tooltip(f.describe(staged))
            .on_click(cx.listener(move |v, _, w, cx| v.file(path.clone(), staged, w, cx)))
            .end(action)
            .into_any_element()
    }

    /// Confirms staging a conflicted file's working-tree content as its resolution.
    pub(super) fn review_resolution(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.blocking_write() || window.has_active_dialog(cx) {
            return;
        }
        let markers = self.state.marker_refusals.contains(&path);
        let epoch = self.state.epoch;
        let weak = cx.weak_entity();
        let cancel_focus = cx.focus_handle();
        let scroll = ScrollHandle::new();
        let initial_focus = cancel_focus.clone();
        let display = gitcrab_core::git::display_path(&path);
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let weak = weak.clone();
            let path = path.clone();
            let confirm = components::style(
                Button::new("confirm-mark-resolved")
                    .label(if markers {
                        "Keep markers and mark resolved"
                    } else {
                        "Mark resolved"
                    })
                    .tab_stop(true),
                if markers {
                    components::ButtonKind::Discard
                } else {
                    components::ButtonKind::Stage
                },
                ControlSize::Regular,
                cx,
            )
            .on_click(move |_, w, cx| {
                let _ = weak.update(cx, |v, cx| {
                    if v.state.epoch == epoch && !v.state.blocking_write() {
                        v.mutate(
                            Action::MarkResolved {
                                path: path.clone(),
                                allow_markers: markers,
                            },
                            w,
                            cx,
                        );
                    }
                });
                w.close_dialog(cx);
            });
            let header = if markers {
                ModalHeader::new("Mark resolved with conflict markers?")
                    .description("The file still looked unresolved when GitCrab last checked.")
                    .tone(Tone::Warning)
            } else {
                ModalHeader::new("Mark conflict resolved?")
                    .description("Stage this file's working-tree content as the resolution.")
            };
            let note = if markers {
                modal::callout(
                    Tone::Warning,
                    "It contained conflict markers (<<<<<<<, =======, >>>>>>>). Continuing records the markers as file content.",
                )
            } else {
                modal::fine_print(
                    "If the file still contains conflict markers (<<<<<<<, =======, >>>>>>>), GitCrab refuses and keeps the conflict.",
                )
            };
            dialog
                .frame(header, ModalWidth::Small, cx)
                .confirm()
                .body(
                    &scroll,
                    modal_body()
                        .child(modal::subject(display.clone(), None))
                        .child(modal::paragraph(
                            "GitCrab removes the file's conflict entries from the index and stages it as it is in the working tree. A deleted file resolves as a deletion.",
                        ))
                        .child(note),
                )
                .on_ok(|_, _, _| true)
                .footer(
                    ModalFooter::new()
                        .action(components::dialog_cancel_focused(&cancel_focus, cx))
                        .action(confirm),
                )
        });
        window.on_next_frame(move |w, cx| initial_focus.focus(w, cx));
    }

    fn composer(&self, cx: &mut Context<Self>) -> Div {
        let snapshot = self.state.snapshot.as_ref().unwrap();
        let staged_count = snapshot.side(true).len();
        let summary_length = self.draft.read(cx).value().chars().count();
        let no_summary = self.draft.read(cx).value().trim().is_empty();
        let git_blocker = snapshot.commit_blocker();
        // Only blockers the user must resolve elsewhere get a visible note.
        let note = git_blocker
            .clone()
            .filter(|_| snapshot.conflict_count() > 0 || snapshot.operation_label().is_some());
        let only_unstaged =
            git_blocker.is_some() && note.is_none() && !snapshot.is_detached() && staged_count == 0;
        let can_commit = self.can_commit();
        // The reason stays available on the disabled button as a tooltip.
        let blocker = git_blocker.or_else(|| {
            if no_summary {
                Some("Enter a commit summary to commit staged changes.".to_string())
            } else if !can_commit {
                Some(if self.state.blocking_write() {
                    "A Git operation is running. Wait for it to finish.".to_string()
                } else {
                    "Close Appearance to commit.".to_string()
                })
            } else {
                None
            }
        });
        let commit_label = if only_unstaged {
            "Stage Files to Commit".to_string()
        } else if no_summary && note.is_none() && !snapshot.is_detached() {
            "Type a Message to Commit".to_string()
        } else {
            match staged_count {
                0 => "Commit".to_string(),
                1 => "Commit 1 staged file".to_string(),
                n => format!("Commit {n} staged files"),
            }
        };
        let has_draft = !self.state.draft.trim().is_empty();
        let short_head = snapshot.head_id.chars().take(8).collect::<String>();
        let branches_at_head: Vec<String> = if snapshot.is_detached() {
            snapshot
                .branches
                .iter()
                .filter(|branch| branch.tip == snapshot.head_id)
                .map(|branch| branch.name.clone())
                .collect()
        } else {
            vec![]
        };
        let counter_size = scaled(11.);
        let limit = geometry::SIZE_COMMIT_EDITOR_RECOMMENDED_SUMMARY_LENGTH as i64;
        let remaining = limit - summary_length as i64;
        div()
            .flex_none()
            .flex()
            .flex_col()
            .child(div().border_t_1().border_role(Role::BorderDivider))
            .child(
                inspector_bar()
                    .justify_between()
                    .child(div().font_weight(FontWeight::MEDIUM).child("Commit")),
            )
            .when(snapshot.is_detached(), |el| {
                el.child(
                    div()
                        .mx(scaled(INSPECTOR_INSET))
                        .mt(px(COMPOSER_GAP))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .w_full()
                                .text_size(scaled(TEXT_SIZE_SMALL))
                                .text_role(Role::StatusWarningFg)
                                .child(format!("Detached HEAD at {short_head}")),
                        )
                        .child(
                            self.action_button("detached-create-branch", "Create a branch…")
                                .disabled_when(self.state.blocking_write())
                                .on_click(cx.listener(|v, _, w, cx| v.prompt_branch(None, w, cx))),
                        )
                        .children(branches_at_head.into_iter().enumerate().map(|(i, name)| {
                            let target = name.clone();
                            self.action_button(("detached-switch", i), format!("Switch to {name}"))
                                .disabled_when(self.state.blocking_write())
                                .on_click(cx.listener(move |v, _, w, cx| {
                                    v.run_operation(Operation::Checkout, target.clone(), w, cx)
                                }))
                        })),
                )
            })
            .child(
                div()
                    .id("commit-box")
                    .mx(scaled(INSPECTOR_INSET))
                    // Same gap as between the box and the commit button below, so the
                    // box doesn't butt up against the "Commit" header.
                    .mt(px(COMPOSER_GAP))
                    .flex_none()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .rounded(px(CONTROL_RADIUS))
                    .border_1()
                    .border_role(Role::BorderDivider)
                    .bg_role(Role::SurfaceChrome)
                    .when(has_draft, |el| {
                        el.tooltip(|w, cx| Tooltip::new(DRAFT_NOTE).build(w, cx))
                    })
                    .child(
                        // Summary row: the input fills it and the count of characters
                        // left in the suggested limit sits at its right edge.
                        div()
                            .w_full()
                            .flex_none()
                            .flex()
                            .items_center()
                            .h(scaled(geometry::SIZE_COMMIT_EDITOR_SUMMARY))
                            .child(if self.settings_open {
                                self.editor(
                                    &self.draft,
                                    geometry::SIZE_COMMIT_EDITOR_SUMMARY,
                                    self.inspector_editor_width(),
                                    "Commit summary",
                                    cx,
                                )
                            } else {
                                // The wrapper pins the height so the input can never
                                // grow with its content and cover the file lists.
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .h(scaled(geometry::SIZE_COMMIT_EDITOR_SUMMARY))
                                    .child(
                                        Input::new(&self.draft)
                                            .px(scaled(geometry::SIZE_COMMIT_EDITOR_INSET_X))
                                            .appearance(false)
                                            .bordered(false)
                                            .w_full()
                                            .h(scaled(geometry::SIZE_COMMIT_EDITOR_SUMMARY)),
                                    )
                                    .into_any_element()
                            })
                            .child(
                                div()
                                    .id("summary-length")
                                    .flex_none()
                                    .pr(scaled(geometry::SIZE_COMMIT_EDITOR_INSET_X))
                                    .text_size(counter_size)
                                    .text_role(if remaining < 0 {
                                        Role::StatusWarningFg
                                    } else {
                                        Role::TextMuted
                                    })
                                    .child(remaining.to_string())
                                    .tooltip(|w, cx| {
                                        Tooltip::new(
                                            "Characters left in the suggested 72. Longer summaries are still valid but read worse in Git tools.",
                                        )
                                        .build(w, cx)
                                    }),
                            ),
                    )
                    .child(div().border_t_1().border_role(Role::BorderDivider))
                    .child(if self.settings_open {
                        Self::editor_preview(
                            self.description.read(cx).value().as_ref(),
                            geometry::SIZE_COMMIT_EDITOR_BODY_MIN,
                            self.inspector_editor_width(),
                            "Description",
                        )
                    } else {
                        div()
                            .w_full()
                            .flex_none()
                            .overflow_hidden()
                            .h(scaled(geometry::SIZE_COMMIT_EDITOR_BODY_MIN))
                            .child(
                                Textarea::new(&self.description)
                                    // Multi-line inputs pad their text by SIZE_COMMIT_EDITOR_INSET_X
                                    // themselves; an outer inset would double it.
                                    .appearance(false)
                                    .bordered(false)
                                    .w_full()
                                    .h(scaled(geometry::SIZE_COMMIT_EDITOR_BODY_MIN))
                                    .text_size(scaled(TEXT_SIZE_BODY)),
                            )
                            .into_any_element()
                    }),
            )
            .child(
                self.primary_button("commit", commit_label, !can_commit)
                    .when(can_commit, |b| b.icon(icon("check")))
                    .flex_none()
                    .mx(scaled(INSPECTOR_INSET))
                    .mt(px(COMPOSER_GAP))
                    .mb(scaled(if note.is_some() { 0. } else { INSPECTOR_INSET }))
                    .map(|button| match &blocker {
                        Some(reason) => button.tooltip(format!("Commit unavailable. {reason}")),
                        None => {
                            button.tooltip_with_action("Commit", &CommitStaged, Some(KEY_CONTEXT))
                        }
                    })
                    .on_click(cx.listener(|v, _, w, cx| v.commit_staged(&CommitStaged, w, cx))),
            )
            // Blockers that need action elsewhere (conflicts, an operation in
            // progress) stay visible next to the button.
            .when_some(note, |el, reason| {
                el.child(
                    div()
                        // A definite width keeps wrapped text from being measured
                        // at min-content width, which made the composer very tall.
                        .w_full()
                        .px(scaled(INSPECTOR_INSET))
                        .pt(px(COMPOSER_GAP))
                        .pb(scaled(INSPECTOR_INSET))
                        .text_size(scaled(TEXT_SIZE_SMALL))
                        .text_role(Role::StatusWarningFg)
                        .child(reason),
                )
            })
    }
}
