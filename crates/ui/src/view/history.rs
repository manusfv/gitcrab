use super::*;
use gitcrab_core::history_columns::{HistoryColumn, HistoryColumns};
mod columns;

const HISTORY_CELL_INSET: Rems = Rems(0.5);

/// Narrowest the WIP summary editor gets when the history pane is cramped.
const WIP_EDITOR_MIN_WIDTH: f32 = 48.;
/// Width of the WIP summary editor while its text is short or empty.
const WIP_EDITOR_DEFAULT_WIDTH: f32 = 120.;
/// Right padding of the WIP editor. The input also keeps a 10px margin right
/// of the caret (gpui-component's `RIGHT_MARGIN`), so together they match the
/// 12px left padding.
const WIP_EDITOR_PAD_RIGHT: f32 = 2.;
const WIP_EDITOR_PAD_LEFT: f32 = 12.;
const WIP_EDITOR_CARET_MARGIN: f32 = 10.;
/// Left and right border of the input.
const WIP_EDITOR_BORDER: f32 = 2.;

/// Width the WIP editor needs to show `text_width` of text without the input
/// scrolling it: padding, border and caret margin, plus 1px against rounding.
/// Short text uses [`WIP_EDITOR_DEFAULT_WIDTH`]; the cell's cap and the badges
/// can still shrink it to [`WIP_EDITOR_MIN_WIDTH`].
fn wip_editor_width(text_width: f32) -> f32 {
    let text = if text_width.is_finite() {
        text_width.max(0.).ceil()
    } else {
        0.
    };
    (text
        + WIP_EDITOR_PAD_LEFT
        + WIP_EDITOR_PAD_RIGHT
        + WIP_EDITOR_BORDER
        + WIP_EDITOR_CARET_MARGIN
        + 1.)
        .max(WIP_EDITOR_DEFAULT_WIDTH)
}

use gitcrab_core::presentation::{resolve_decorations, Decoration, DecorationKind};

/// A type marker drawn before a decoration's name so tags, remotes and HEAD
/// stay distinct.
fn decoration_marker(decoration: &Decoration) -> Option<AnyElement> {
    let glyph = |name: &str| {
        icon(name)
            .size(scaled(12.))
            .flex_none()
            .text_role(Role::TextSecondary)
            .into_any_element()
    };
    match (decoration.kind, decoration.target.as_ref().map(|t| t.0)) {
        (DecorationKind::HeadBranch, _) => Some(
            div()
                .flex_none()
                .px(px(3.))
                .rounded(px(3.))
                .bg_role(Role::ActionPrimary)
                .text_role(Role::TextOnPrimary)
                .text_size(scaled(10.))
                .font_weight(FontWeight::BOLD)
                .child("HEAD")
                .into_any_element(),
        ),
        (DecorationKind::Tag, _) => Some(glyph("tag")),
        (_, Some(ReferenceKind::Remote)) => Some(glyph("remote")),
        _ => None,
    }
}

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

fn paint_lane_corner(
    w: &mut Window,
    junction: Point<Pixels>,
    lane_x: Pixels,
    row_edge: Pixels,
    color: u32,
) {
    let run = lane_x - junction.x;
    let rise = row_edge - junction.y;
    let radius = px(6.).min(run.abs()).min(rise.abs());
    let bend_x = lane_x - if run < px(0.) { -radius } else { radius };
    let bend_y = junction.y + if rise < px(0.) { -radius } else { radius };
    let mut path = PathBuilder::stroke(graph_stroke(w.scale_factor()));
    path.move_to(junction);
    path.line_to(point(bend_x, junction.y));
    path.curve_to(point(lane_x, bend_y), point(lane_x, junction.y));
    path.line_to(point(lane_x, row_edge));
    if let Ok(path) = path.build() {
        w.paint_path(path, rgb(color));
    }
}

impl GitCrab {
    pub(super) fn column_drag_events(&self, cx: &Context<Self>) -> impl IntoElement {
        components::rail::end_events(
            cx.weak_entity(),
            Self::finish_column_drag,
            Self::cancel_column_drag,
        )
    }

    /// Previews a decoration's reference, or selects its commit for a detached HEAD.
    fn activate_decoration(
        &mut self,
        decoration: &Decoration,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &decoration.target {
            Some((kind, name)) => {
                if self.single_pane {
                    self.constrained_inspector = true;
                }
                self.preview_ref(*kind, name, window, cx);
            }
            None => self.activate_commit(id, window, cx),
        }
    }

    pub(super) fn history(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.prepare_column_drag(window, cx);
        self.history_layout = self.history_columns.adapted(
            self.history_available - self.history_gutter(),
            self.preferences.font_size / 13.,
        );
        let count = self.state.graph.rows.len();
        let weak = cx.weak_entity();
        let unborn = self
            .state
            .snapshot
            .as_ref()
            .filter(|s| s.commits.is_empty() && s.head_id.is_empty())
            .map(|s| s.head.clone());
        let row_height = (if self.compact { 26.0_f32 } else { 30.0_f32 })
            .max(self.preferences.font_size * 20. / 13. + 8.);
        panel()
            .bg_role(Role::SurfaceCanvas)
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
                                        // Size from a commit row, not the WIP row: its
                                        // text input reports an unbounded content width.
                                        .with_width_from_item(Some(1))
                                        .map(|mut el| {
                                            el.style().restrict_scroll_to_axis = Some(true);
                                            el
                                        })
                                        .map(|list| {
                                            if unborn.is_some() {
                                                list.flex_none().h(px(row_height))
                                            } else {
                                                list.flex_1().min_h_0()
                                            }
                                        }),
                                    )
                                    .when_some(unborn, |table, branch| {
                                        table.child(
                                            div()
                                                .flex_1()
                                                .flex()
                                                .flex_col()
                                                .items_center()
                                                .justify_center()
                                                .gap_2()
                                                .p_4()
                                                .child(
                                                    div()
                                                        .text_size(scaled(16.))
                                                        .child("No commits yet"),
                                                )
                                                .child(
                                                    div()
                                                        .text_role(Role::TextMuted)
                                                        .child(format!(
                                                            "{branch} has no commits. Stage files in Working changes and commit them to start its history."
                                                        )),
                                                ),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div().absolute().inset_0().child(
                            gpui_component::scroll::Scrollbar::horizontal(
                                &self.history_horizontal_scroll,
                            )
                            .id("history-horizontal-scrollbar"),
                        ),
                    )
                    .child({
                        // A visible, keyboard-reachable route to column settings.
                        let weak = cx.weak_entity();
                        div()
                            .absolute()
                            .top_0()
                            .right(scaled(4.))
                            .h(scaled(28.))
                            .flex()
                            .items_center()
                            .bg_role(Role::SurfaceChrome)
                            .child(
                                self.button("history-columns", "")
                                    .icon(icon("columns").size(scaled(14.)))
                                    .size(scaled(24.))
                                    .tooltip("Columns: show, order and size history columns")
                                    .dropdown_menu(move |menu, w, cx| {
                                        Self::columns_menu(weak.clone(), menu, w, cx)
                                    }),
                            )
                    }),
            )
            .when(self.state.limit == 100_000 && count == 100_000, |e| {
                e.child(
                    div()
                        .h(px(30.))
                        .px_3()
                        .text_role(Role::TextMuted)
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
                        .disabled_when(self.state.loading())
                        .on_click(cx.listener(|v, _, w, cx| {
                            let plan = v.state.load_more();
                            v.apply_refresh(plan, w, cx);
                        })),
                    )
                },
            )
            .into_any_element()
    }
    fn history_row(&self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let height = (if self.compact { 26.0_f32 } else { 30.0_f32 })
            .max(self.preferences.font_size * 20. / 13. + 8.);
        let lane_count = self.state.graph.lanes;
        if index == 0 {
            // A saved layout can be wider than the pane; cap the message cell to the
            // part on screen so the editor shrinks before the badges are cut off.
            // Measured from the scroll viewport's last painted bounds, which track
            // window and pane resizes exactly (unlike the computed pane widths).
            let scroll = self.history_horizontal_scroll.clone();
            let viewport = f32::from(scroll.bounds().size.width);
            let scrolled = -f32::from(scroll.offset().x);
            let inset = f32::from(HISTORY_CELL_INSET.to_pixels(window.rem_size()));
            let wip_visible_cap = self
                .history_layout
                .visible_message_width(viewport, scrolled)
                .map(|width| (width - 2. * inset).max(WIP_EDITOR_MIN_WIDTH));
            // The editor starts at the width of its text (or placeholder) and grows
            // as the summary is typed; the cap above and the badges bound it.
            let wip_width = {
                let input = self.wip_summary.read(cx);
                let text = match input.value() {
                    value if value.is_empty() => SharedString::from("// WIP"),
                    value => value,
                };
                let font_size = type_size(cx, 12.);
                let run = TextRun {
                    len: text.len(),
                    // The input renders in the theme font, not the window default.
                    font: font(gpui_component::Theme::global(cx).font_family.clone()),
                    color: Hsla::default(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = window
                    .text_system()
                    .shape_line(text, font_size, &[run], None);
                wip_editor_width(f32::from(shaped.width))
            };
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
                .pr(px(self.history_gutter()))
                .text_role(Role::ActionPrimary)
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
                .children(self.history_layout.visible().map(|entry| {
                    let cell = match entry.column {
                        HistoryColumn::Graph => (canvas(
                            |_, _, _| (),
                            move |bounds, _, w, _| {
                                let x =
                                    graph_lane_positions(bounds, lane_count, w.scale_factor())(0);
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
                            .min_w_0()
                            .overflow_hidden()
                            .when_some(wip_visible_cap, |el, cap| el.max_w(px(cap)))
                            // The cap uses the previous frame's viewport. If that
                            // changed (a resize), draw once more with the new size.
                            .child({
                                let scroll = scroll.clone();
                                let used = (viewport, scrolled);
                                canvas(
                                    |_, _, _| (),
                                    move |_, _, w, _| {
                                        let now = (
                                            f32::from(scroll.bounds().size.width),
                                            -f32::from(scroll.offset().x),
                                        );
                                        if now != used {
                                            w.refresh();
                                        }
                                    },
                                )
                                .absolute()
                                .size_0()
                            })
                            // Inline summary editor, mirrored with the composer's
                            // Commit summary field (see the draft observers in view.rs).
                            .child(
                                div()
                                    .id("wip-summary")
                                    // Sized to its text; shrinks before the
                                    // flex_none badges when space runs out.
                                    .flex_shrink()
                                    .w(px(wip_width))
                                    .min_w(px(WIP_EDITOR_MIN_WIDTH))
                                    .overflow_hidden()
                                    .child(
                                        Input::new(&self.wip_summary)
                                            .disabled(self.settings_open)
                                            .py_0()
                                            // The input's 10px caret margin supplies
                                            // the rest, matching the 12px left inset.
                                            .pr(px(WIP_EDITOR_PAD_RIGHT))
                                            .h(px((height - 10.).max(18.)))
                                            .text_size(scaled(12.)),
                                    )
                                    .tooltip(|w, cx| {
                                        Tooltip::new("Working changes · commit summary")
                                            .build(w, cx)
                                    }),
                            )
                            .when(
                                self.state
                                    .snapshot
                                    .as_ref()
                                    .is_some_and(|s| s.status.is_empty()),
                                |row| {
                                    row.child(
                                        div()
                                            .text_size(scaled(12.))
                                            .text_role(Role::TextMuted)
                                            .child("No changes"),
                                    )
                                },
                            )
                            .when_some(
                                self.state
                                    .snapshot
                                    .as_ref()
                                    .map(|s| s.conflict_count())
                                    .filter(|count| *count > 0),
                                |row, count| {
                                    row.child(Badge::role(
                                        format!(
                                            "{count} {}",
                                            if count == 1 { "conflict" } else { "conflicts" }
                                        ),
                                        Role::StatusConflictFg,
                                    ))
                                },
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
                                    .filter(|(count, _)| *count > 0)
                                    .map(|(count, (mark, name, role))| {
                                        Badge::role(format!("{mark} {count}"), role)
                                            .fixed_width(44.)
                                            .tooltip(
                                                SharedString::from(format!("wip-{name}")),
                                                format!("{count} {name} paths"),
                                            )
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
        let matched = c.matches_search(&self.search_query);
        let row = self.state.graph.rows[i].clone();
        let outgoing = self.state.graph.edges_at(i);
        let next_lane = self.state.graph.rows.get(i + 1).map(|row| row.lane);
        let mut incoming = i
            .checked_sub(1)
            .map(|p| self.state.graph.edges_at(p))
            .unwrap_or_default();
        if i == 0 && !snap.head_id.is_empty() {
            incoming.push(gitcrab_core::graph::Edge {
                from_lane: 0,
                to_lane: 0,
                color: 0,
            });
        }

        let decorations = resolve_decorations(snap, &c.refs);
        let primary = decorations
            .iter()
            .position(|d| d.kind == DecorationKind::HeadBranch)
            .or_else(|| {
                decorations
                    .iter()
                    .position(|d| d.kind != DecorationKind::Head)
            })
            .or((!decorations.is_empty()).then_some(0));
        let full_refs = decorations
            .iter()
            .map(Decoration::label)
            .collect::<Vec<_>>()
            .join(" · ");
        let others = decorations.len().saturating_sub(1);
        let primary = primary.map(|index| decorations[index].clone());
        let refs = primary.as_ref().map(|d| d.name.clone()).unwrap_or_default();
        let columns = &self.history_layout;
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
            .pr(px(self.history_gutter()))
            .cursor_pointer()
            .overflow_hidden()
            .when(selected, |e| e.bg_role(Role::InteractionSelected))
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
            .children(self.history_layout.visible().map(|entry| {
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
                        .text_size(scaled(12.))
                        .text_role(Role::TextDefault)
                        .when_some(primary.clone(), |e, primary| {
                            let marker = decoration_marker(&primary);
                            let label = primary.label();
                            e.child(
                                self.button(("ref-detail", i), "")
                                    .children(marker)
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_role(Role::TextDefault)
                                            .child(refs),
                                    )
                                    .flex_1()
                                    .min_w_0()
                                    .justify_start()
                                    .overflow_hidden()
                                    .px_1()
                                    .h(scaled(REF_BADGE_HEIGHT))
                                    .bg(branch_tint(color, REF_TINT))
                                    .tooltip(format!("{label} · Click to preview"))
                                    .on_click(cx.listener({
                                        let id = c.id.clone();
                                        move |v, _, w, cx| {
                                            v.activate_decoration(&primary, id.clone(), w, cx)
                                        }
                                    })),
                            )
                        })
                        .when(others > 0, |e| {
                            let weak = cx.weak_entity();
                            let entries = decorations.clone();
                            let id = c.id.clone();
                            e.child(
                                self.button(("ref-more", i), format!("+{others}"))
                                    .flex_none()
                                    .h(scaled(REF_BADGE_HEIGHT))
                                    .px(px(4.))
                                    .bg(branch_tint(color, REF_TINT))
                                    .tooltip(format!("{others} more · Open the list of references"))
                                    .dropdown_menu(move |mut menu, _, _| {
                                        for entry in &entries {
                                            let weak = weak.clone();
                                            let entry_for_click = entry.clone();
                                            let id = id.clone();
                                            menu = menu.item(
                                                PopupMenuItem::new(entry.label()).on_click(
                                                    move |_, w, cx| {
                                                        let _ = weak.update(cx, |v, cx| {
                                                            v.activate_decoration(
                                                                &entry_for_click,
                                                                id.clone(),
                                                                w,
                                                                cx,
                                                            )
                                                        });
                                                    },
                                                ),
                                            );
                                        }
                                        menu
                                    }),
                            )
                        })
                        .tooltip(move |w, cx| Tooltip::new(full_refs.clone()).build(w, cx)))
                    .into_any_element(),
                    HistoryColumn::Graph => {
                        let outgoing = outgoing.clone();
                        (div().relative().w_full().h(px(height)).flex_none().child(
                            canvas(
                                move |_, _, _| (),
                                move |bounds, _, w, _| {
                                    let origin = bounds.origin;
                                    let scale = w.scale_factor();
                                    let x = graph_lane_positions(bounds, lane_count, scale);
                                    let center_y = bounds.center().y;
                                    let y = snap_graph_stroke_center(center_y, scale);
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
                                        paint_reference_link(w, edge, center_x, center_y, color);
                                    }
                                    let edges = incoming
                                        .iter()
                                        .map(|edge| (edge, true, Some(row.lane)))
                                        .chain(
                                            outgoing.iter().map(|edge| (edge, false, next_lane)),
                                        );
                                    let mut corners = Vec::new();
                                    for (edge, above, lower_lane) in edges {
                                        let from = x(edge.from_lane);
                                        let to = x(edge.to_lane);
                                        let edge_color = lanes()[edge.color % lanes().len()];
                                        // Put a leftward turn on the lower row only when it meets that row's node.
                                        let turns_below =
                                            from > to && lower_lane == Some(edge.to_lane);
                                        let (top, bottom) = if above {
                                            (bounds.top(), y)
                                        } else {
                                            (y, bounds.bottom())
                                        };
                                        if from != to && above == turns_below {
                                            let (junction_x, lane_x, row_edge) = if above {
                                                (to, from, top)
                                            } else {
                                                (from, to, bottom)
                                            };
                                            let span = edge.from_lane.abs_diff(edge.to_lane);
                                            corners.push((
                                                span, junction_x, lane_x, row_edge, edge_color,
                                            ));
                                        } else {
                                            let lane_x = if turns_below { from } else { to };
                                            paint_vertical_lane(w, lane_x, top, bottom, edge_color);
                                        }
                                    }
                                    // Paint longer runs first so each nearer branch keeps its color at the turn.
                                    corners.sort_by_key(|corner| std::cmp::Reverse(corner.0));
                                    for (_, junction_x, lane_x, row_edge, edge_color) in corners {
                                        paint_lane_corner(
                                            w,
                                            point(junction_x, y),
                                            lane_x,
                                            row_edge,
                                            edge_color,
                                        );
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
                    HistoryColumn::Message => {
                        let subject = c.subject.clone();
                        (div()
                            .id(("subject", i))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when(selected, |e| e.font_weight(FontWeight::MEDIUM))
                            .py(px(3.))
                            .child(c.subject.clone())
                            // The full subject stays available when the column clips it.
                            .tooltip(move |w, cx| Tooltip::new(subject.clone()).build(w, cx)))
                        .into_any_element()
                    }
                    HistoryColumn::Sha => (div()
                        .w_full()
                        .flex_none()
                        .whitespace_nowrap()
                        .truncate()
                        .text_size(scaled(12.))
                        .font_family(CODE_FONT)
                        .text_role(Role::TextSecondary)
                        .child(c.id[..8.min(c.id.len())].to_string()))
                    .into_any_element(),
                    HistoryColumn::Author => div()
                        .truncate()
                        .text_role(Role::TextSecondary)
                        .child(c.author.clone())
                        .into_any_element(),
                    HistoryColumn::Date => div()
                        .truncate()
                        .text_size(scaled(12.))
                        .text_role(Role::TextSecondary)
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
            .on_click(cx.listener(move |v, _, w, cx| v.activate_commit(id.clone(), w, cx)));
        if c.id == snap.head_id {
            // Paint the marker over the row so it cannot shift the lane origin.
            el = el.relative().child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .w(px(2.))
                    .bg_role(Role::ActionPrimary),
            );
        }
        let view = cx.entity();
        let parents = c.parents.len();
        el.context_menu(move |mut menu, w, cx| {
            let commands = view.update(cx, |v, cx| v.command_context(w, cx));
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
                        create_view.update(cx, |v, cx| v.prompt_branch(Some(start.clone()), w, cx))
                    }),
                )
                .separator();
            // The shared command model decides availability, including merge commits.
            for command in [
                Command::Checkout,
                Command::Merge,
                Command::CherryPick,
                Command::Revert,
            ] {
                let view = view.clone();
                let target = target.clone();
                let availability = command.availability_for_commit(&commands, parents);
                let short = target.chars().take(8).collect::<String>();
                let label = match availability.reason() {
                    Some(reason) => format!("{} {short} — {reason}", command.label()),
                    None => format!("{} {short}", command.label()),
                };
                menu = menu.item(
                    PopupMenuItem::new(label)
                        .disabled(!availability.enabled())
                        .on_click(move |_, w, cx| {
                            view.update(cx, |v, cx| v.run_command(command, target.clone(), w, cx))
                        }),
                );
            }
            menu
        })
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{wip_editor_width, WIP_EDITOR_DEFAULT_WIDTH};

    #[test]
    fn wip_editor_fits_its_text_without_scrolling() {
        assert_eq!(wip_editor_width(40.), WIP_EDITOR_DEFAULT_WIDTH);
        // 12px left + 2px right padding, 2px border, the input's 10px caret
        // margin and 1px slack; any less and the first glyph gets clipped.
        assert_eq!(wip_editor_width(300.2), 301. + 27.);
    }
}
