use super::*;
use gitcrab_core::history_columns::{HistoryColumn, HistoryColumns};
mod columns;

const HISTORY_CELL_INSET: Rems = Rems(0.5);

#[derive(Clone, Copy)]
enum ReferenceSide {
    Left,
    Right,
}

fn paint_reference_link(w: &mut Window, start: Pixels, end: Pixels, y: Pixels, color: usize) {
    let scale = w.scale_factor();
    let top = px((f32::from(y) * scale - 0.5).round() / scale);
    w.paint_quad(fill(
        Bounds::new(
            point(start.min(end), top),
            size((end - start).abs(), px(1. / scale)),
        ),
        branch_tint(color, 0x70),
    ));
}

impl GitCrab {
    pub(super) fn column_drag_events(&self, cx: &Context<Self>) -> impl IntoElement {
        components::rail::end_events(
            cx.weak_entity(),
            Self::finish_column_drag,
            Self::cancel_column_drag,
        )
    }

    pub(super) fn history(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.prepare_column_drag(window, cx);
        let count = self.state.graph.rows.len();
        let weak = cx.weak_entity();
        panel()
            .bg(rgb(semantic(Role::SurfaceCanvas)))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(
                        div()
                            .id("history-horizontal-scroll")
                            .size_full()
                            .overflow_x_scroll()
                            .map(|mut el| {
                                el.style().restrict_scroll_to_axis = Some(true);
                                el
                            })
                            .track_scroll(&self.history_horizontal_scroll)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .w_full()
                                    .min_w(px(self.history_table_width()))
                                    .h_full()
                                    .child(self.history_header(cx))
                                    .child(
                                        uniform_list("history", count + 1, move |range, w, cx| {
                                            weak.update(cx, |v, cx| {
                                                range
                                                    .map(|i| v.history_row(i, w, cx))
                                                    .collect::<Vec<_>>()
                                            })
                                            .unwrap_or_default()
                                        })
                                        .track_scroll(self.history_scroll.clone())
                                        .map(|mut el| {
                                            el.style().restrict_scroll_to_axis = Some(true);
                                            el
                                        })
                                        .flex_1()
                                        .min_h_0(),
                                    ),
                            ),
                    )
                    .child(
                        div().absolute().inset_0().child(
                            gpui_component::scroll::Scrollbar::horizontal(
                                &self.history_horizontal_scroll,
                            )
                            .id("history-horizontal-scrollbar"),
                        ),
                    ),
            )
            .when(self.state.limit == 100_000 && count == 100_000, |e| {
                e.child(
                    div()
                        .h(px(30.))
                        .px_3()
                        .text_color(rgb(semantic(Role::TextMuted)))
                        .child(format!("{count} commits shown · history limit reached")),
                )
            })
            .when(
                count >= self.state.limit && self.state.limit < 100_000,
                |e| {
                    e.child(
                        self.action_button(
                            "more",
                            format!("Load 500 more commits · {count} shown"),
                        )
                        .disabled(self.state.loading())
                        .on_click(cx.listener(|v, _, w, cx| {
                            v.state.limit = (v.state.limit + 500).min(100_000);
                            v.refresh(&Refresh, w, cx);
                        })),
                    )
                },
            )
            .into_any_element()
    }
    fn history_row(
        &self,
        index: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let height = (if self.compact { 26.0_f32 } else { 30.0_f32 })
            .max(self.preferences.font_size * 20. / 13. + 8.);
        if index == 0 {
            let has_head = self
                .state
                .snapshot
                .as_ref()
                .is_some_and(|s| !s.head_id.is_empty());
            return div()
                .id("wip-row")
                .cursor_pointer()
                .w_full()
                .h(px(height))
                .flex()
                .items_center()
                .min_w(px(self.history_table_width()))
                .text_color(rgb(semantic(Role::ActionPrimary)))
                .bg(rgb(if self.state.selected.is_none() {
                    semantic(Role::SurfaceWipSelected)
                } else {
                    semantic(Role::SurfaceCanvas)
                }))
                .hover(|s| {
                    s.bg(rgb(if self.state.selected.is_none() {
                        semantic(Role::InteractionSelected)
                    } else {
                        semantic(Role::InteractionHover)
                    }))
                })
                .children(self.preferences.history_columns.visible().map(|entry| {
                    let cell = match entry.column {
                        HistoryColumn::Graph => (canvas(
                            |_, _, _| (),
                            move |bounds, _, w, _| {
                                let x = graph_lane_x(bounds, 0, w.scale_factor());
                                let y = bounds.origin.y + px(height / 2.);
                                if has_head {
                                    paint_vertical_lane(
                                        w,
                                        x,
                                        y + px(5.),
                                        bounds.bottom(),
                                        lanes()[0],
                                    );
                                }
                                w.paint_quad(
                                    outline(
                                        Bounds::new(
                                            point(x - px(9.), y - px(9.)),
                                            size(px(18.), px(18.)),
                                        ),
                                        rgb(lanes()[0]),
                                        BorderStyle::Dashed,
                                    )
                                    .corner_radii(px(9.)),
                                );
                            },
                        )
                        .w_full()
                        .h(px(height))
                        .flex_none())
                        .into_any_element(),
                        HistoryColumn::Message => div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .w_full()
                            .child(
                                div().flex_1().min_w(px(96.)).max_w(px(180.)).child(
                                    Input::new(&self.wip_summary)
                                        .disabled(self.settings_open)
                                        .h(type_size(cx, 24.))
                                        .text_size(type_size(cx, 12.)),
                                ),
                            )
                            .children(self.state.snapshot.as_ref().into_iter().flat_map(|s| {
                                s.change_counts()
                                    .into_iter()
                                    .zip([
                                        ("M", "Modified", Role::StatusWarningFg),
                                        ("+", "Added", Role::StatusSuccessFg),
                                        ("−", "Deleted", Role::StatusErrorFg),
                                        ("R", "Renamed", Role::StatusInfoFg),
                                    ])
                                    .map(|(count, (mark, name, role))| {
                                        badge(format!("{mark} {count}"), semantic(role))
                                            .id(SharedString::from(format!("wip-{name}")))
                                            .tooltip(move |w, cx| {
                                                Tooltip::new(format!("{count} {name} paths"))
                                                    .build(w, cx)
                                            })
                                    })
                            }))
                            .into_any_element(),
                        _ => div().into_any_element(),
                    };
                    self.history_cell(entry).child(cell).into_any_element()
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|v, _, w, cx| {
                        if v.state.selected.is_some() {
                            let job = v.state.select(None);
                            v.dispatch(job, w, cx);
                        }
                    }),
                )
                .into_any_element();
        }
        let i = index - 1;
        let Some(snap) = &self.state.snapshot else {
            return div().into_any_element();
        };
        let c = &snap.commits[i];
        let id = c.id.clone();
        let target = c.id.clone();
        let selected = self.state.selected.as_ref() == Some(&c.id);
        let search = self.search.read(cx).value().to_lowercase();
        let matched = search.is_empty()
            || c.subject.to_lowercase().contains(&search)
            || c.id.contains(&search)
            || c.author.to_lowercase().contains(&search);
        let row = self.state.graph.rows[i].clone();
        let mut incoming = i
            .checked_sub(1)
            .map(|p| self.state.graph.rows[p].edges.clone())
            .unwrap_or_default();
        if i == 0 && !snap.head_id.is_empty() {
            incoming.push(gitcrab_core::graph::Edge {
                from_lane: 0,
                to_lane: 0,
                color: 0,
                parent: snap.head_id.clone(),
            });
        }

        let full_refs = c.refs.join(" · ");
        let ref_count = c.refs.iter().filter(|r| r.as_str() != "HEAD").count();
        let refs = c
            .refs
            .iter()
            .find(|r| {
                self.state
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.head_id == c.id && &s.head == *r)
            })
            .or_else(|| {
                c.refs
                    .iter()
                    .find(|r| r.as_str() != "HEAD" && !r.starts_with("HEAD ->"))
            })
            .or(c.refs.first())
            .cloned()
            .unwrap_or_default();
        let refs = refs
            .trim_start_matches("tag: ")
            .trim_start_matches("HEAD -> ")
            .to_string();
        let columns = &self.preferences.history_columns;
        let reference_side = if refs.is_empty() {
            None
        } else {
            columns
                .visible()
                .zip(columns.visible().skip(1))
                .find_map(|(left, right)| match (left.column, right.column) {
                    (HistoryColumn::References, HistoryColumn::Graph) => Some(ReferenceSide::Left),
                    (HistoryColumn::Graph, HistoryColumn::References) => Some(ReferenceSide::Right),
                    _ => None,
                })
        };
        let color = row.color;
        let junction = row.is_junction(&incoming);
        let mut el = div()
            .id(("commit", i))
            .w_full()
            .h(px(height))
            .flex()
            .items_center()
            .min_w(px(self.history_table_width()))
            .cursor_pointer()
            .overflow_hidden()
            .when(selected, |e| e.bg(rgb(semantic(Role::InteractionSelected))))
            .hover(|s| {
                s.bg(rgb(if selected {
                    semantic(Role::InteractionSelected)
                } else {
                    semantic(Role::InteractionHover)
                }))
            })
            .text_color(rgb(if matched {
                semantic(Role::TextDefault)
            } else {
                semantic(Role::TextMuted)
            }))
            .children(self.preferences.history_columns.visible().map(|entry| {
                let row = row.clone();
                let incoming = incoming.clone();
                let refs = refs.clone();
                let full_refs = full_refs.clone();
                let cell = match entry.column {
                    HistoryColumn::References => (div()
                        .id(("refs", i))
                        .w_full()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_1()
                        .overflow_hidden()
                        .text_size(type_size(cx, 12.))
                        .text_color(rgb(semantic(Role::TextDefault)))
                        .when(!refs.is_empty(), |e| {
                            e.child(
                                self.button(("ref-detail", i), "")
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_color(rgb(semantic(Role::TextDefault)))
                                            .child(refs),
                                    )
                                    .flex_1()
                                    .min_w_0()
                                    .justify_start()
                                    .overflow_hidden()
                                    .px_1()
                                    .h(px(REF_BADGE_HEIGHT))
                                    .bg(branch_tint(color, REF_TINT))
                                    .tooltip(full_refs.clone())
                                    .on_click(cx.listener({
                                        let id = c.id.clone();
                                        move |v, _, w, cx| v.select(Some(id.clone()), w, cx)
                                    })),
                            )
                        })
                        .when(ref_count > 1, |e| {
                            e.child(
                                div()
                                    .flex_none()
                                    .whitespace_nowrap()
                                    .h(px(REF_BADGE_HEIGHT))
                                    .flex()
                                    .items_center()
                                    .px(px(4.))
                                    .rounded(px(CONTROL_RADIUS))
                                    .bg(branch_tint(color, REF_TINT))
                                    .child(format!("+{}", ref_count - 1)),
                            )
                        })
                        .tooltip(move |w, cx| Tooltip::new(full_refs.clone()).build(w, cx)))
                    .into_any_element(),
                    HistoryColumn::Graph => {
                        (div().relative().w_full().h(px(height)).flex_none().child(
                            canvas(
                                move |_, _, _| (),
                                move |bounds, _, w, _| {
                                    let origin = bounds.origin;
                                    let scale = w.scale_factor();
                                    let x = |lane| graph_lane_x(bounds, lane, scale);
                                    let y = origin.y + px(height / 2.);
                                    let center_x = x(row.lane);
                                    let band_y = origin.y + px(3.);
                                    let band_height = bounds.size.height - px(6.);
                                    let band_right = bounds.right();
                                    w.paint_quad(fill(
                                        Bounds::new(
                                            point(center_x, band_y),
                                            size(band_right - center_x, band_height),
                                        ),
                                        branch_tint(color, 0x24),
                                    ));
                                    w.paint_quad(fill(
                                        Bounds::new(
                                            point(band_right - px(1.), band_y),
                                            size(px(1.), band_height),
                                        ),
                                        branch_tint(color, 0x70),
                                    ));
                                    if let Some(side) = reference_side {
                                        let edge = match side {
                                            ReferenceSide::Left => bounds.left(),
                                            ReferenceSide::Right => bounds.right(),
                                        };
                                        paint_reference_link(w, edge, center_x, y, color);
                                    }
                                    let edges = || {
                                        incoming
                                            .iter()
                                            .map(|e| (e, true))
                                            .chain(row.edges.iter().map(|e| (e, false)))
                                            .map(|(edge, above)| {
                                                (
                                                    x(edge.from_lane),
                                                    x(edge.to_lane),
                                                    edge.color,
                                                    above,
                                                )
                                            })
                                    };
                                    for (from, _, edge_color, above) in
                                        edges().filter(|(from, to, _, _)| from == to)
                                    {
                                        let (top, bottom) = if above {
                                            (bounds.top(), y)
                                        } else {
                                            (y, bounds.bottom())
                                        };
                                        paint_vertical_lane(
                                            w,
                                            from,
                                            top,
                                            bottom,
                                            lanes()[edge_color % lanes().len()],
                                        );
                                    }
                                    for (from, to, edge_color, above) in
                                        edges().filter(|(from, to, _, _)| from != to)
                                    {
                                        let mut path = PathBuilder::stroke(graph_stroke(scale));
                                        let y0 = if above { y - px(height) } else { y };
                                        let y1 = y0 + px(height);
                                        path.move_to(point(from, y0));
                                        path.cubic_bezier_to(
                                            point(to, y1),
                                            point(from, y0 + px(height * 0.6)),
                                            point(to, y1 - px(height * 0.6)),
                                        );
                                        if let Ok(path) = path.build() {
                                            w.paint_path(
                                                path,
                                                rgb(lanes()[edge_color % lanes().len()]),
                                            );
                                        }
                                    }
                                    if junction {
                                        let radius = px(6.);
                                        let mut diamond = PathBuilder::fill();
                                        diamond.move_to(point(center_x, y - radius));
                                        diamond.line_to(point(center_x + radius, y));
                                        diamond.line_to(point(center_x, y + radius));
                                        diamond.line_to(point(center_x - radius, y));
                                        diamond.close();
                                        if let Ok(path) = diamond.build() {
                                            w.paint_path(path, rgb(lanes()[color % lanes().len()]));
                                        }
                                    } else {
                                        let radius = 4.;
                                        w.paint_quad(
                                            fill(
                                                Bounds::new(
                                                    point(center_x - px(radius), y - px(radius)),
                                                    size(px(radius * 2.), px(radius * 2.)),
                                                ),
                                                rgb(lanes()[color % lanes().len()]),
                                            )
                                            .corner_radii(px(radius)),
                                        );
                                    }
                                },
                            )
                            .size_full(),
                        ))
                        .into_any_element()
                    }
                    HistoryColumn::Message => (div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .when(selected, |e| e.font_weight(FontWeight::MEDIUM))
                        .py(px(3.))
                        .child(c.subject.clone()))
                    .into_any_element(),
                    HistoryColumn::Sha => (div()
                        .w_full()
                        .flex_none()
                        .whitespace_nowrap()
                        .truncate()
                        .text_size(type_size(cx, 12.))
                        .font_family(CODE_FONT)
                        .text_color(rgb(semantic(Role::TextSecondary)))
                        .child(c.id[..8.min(c.id.len())].to_string()))
                    .into_any_element(),
                    HistoryColumn::Author => div()
                        .truncate()
                        .text_color(rgb(semantic(Role::TextSecondary)))
                        .child(c.author.clone())
                        .into_any_element(),
                    HistoryColumn::Date => div()
                        .truncate()
                        .text_size(type_size(cx, 12.))
                        .text_color(rgb(semantic(Role::TextSecondary)))
                        .child(display_date(&c.date))
                        .into_any_element(),
                };
                self.history_cell(entry)
                    .when_some(
                        reference_side.filter(|_| entry.column == HistoryColumn::References),
                        |frame, side| {
                            // This segment spans the reference cell's padding and
                            // meets the graph segment at their shared boundary.
                            frame.relative().child(
                                canvas(
                                    |_, _, _| (),
                                    move |bounds, _, w, _| {
                                        let inset = w.rem_size() * HISTORY_CELL_INSET.0;
                                        let (edge, badge) = match side {
                                            ReferenceSide::Left => {
                                                (bounds.right(), bounds.right() - inset)
                                            }
                                            ReferenceSide::Right => {
                                                (bounds.left(), bounds.left() + inset)
                                            }
                                        };
                                        paint_reference_link(
                                            w,
                                            badge,
                                            edge,
                                            bounds.center().y,
                                            color,
                                        );
                                    },
                                )
                                .absolute()
                                .inset_0(),
                            )
                        },
                    )
                    .child(cell)
                    .into_any_element()
            }))
            .on_click(cx.listener(move |v, _, w, cx| v.select(Some(id.clone()), w, cx)));
        if c.id == snap.head_id {
            // Paint the marker over the row so it cannot shift the lane origin.
            el = el.relative().child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .w(px(2.))
                    .bg(rgb(semantic(Role::ActionPrimary))),
            );
        }
        let view = cx.entity();
        el.context_menu(move |mut menu, _, _| {
            let sha = target.clone();
            menu = menu
                .item(
                    PopupMenuItem::new("Copy commit SHA").on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(sha.clone()))
                    }),
                )
                .separator();
            let create_view = view.clone();
            let start = target.clone();
            menu = menu
                .item(
                    PopupMenuItem::new("Create branch here…").on_click(move |_, w, cx| {
                        create_view.update(cx, |v, cx| {
                            v.review(
                                Operation::CreateBranchAt {
                                    start: start.clone(),
                                },
                                String::new(),
                                w,
                                cx,
                            )
                        })
                    }),
                )
                .separator();
            for (op, label) in [
                (Operation::Checkout, "Checkout…"),
                (Operation::Merge, "Merge…"),
                (Operation::CherryPick, "Cherry-pick…"),
                (Operation::Revert, "Revert…"),
            ] {
                let view = view.clone();
                let target = target.clone();
                menu = menu.item(
                    PopupMenuItem::new(format!(
                        "{} {}",
                        label.trim_end_matches('…'),
                        target.chars().take(8).collect::<String>()
                    ))
                    .on_click(move |_, w, cx| {
                        view.update(cx, |v, cx| v.review(op.clone(), target.clone(), w, cx))
                    }),
                );
            }
            menu
        })
        .into_any_element()
    }
}
