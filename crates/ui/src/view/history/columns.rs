use super::*;
use components::rail::RailDrag;
use gitcrab_core::history_columns::ColumnLayout;

const RESIZE_HALF_WIDTH: f32 = 5.;
/// Room at the default font size kept clear at the right of the history table
/// for the floating Columns button, so it never covers a column header.
const COLUMNS_BUTTON_ROOM: f32 = 32.;
const GRAPH_LEGEND: &str = "Graph · Each colored lane is a line of history. A dot is a commit, a diamond is a merge or branch point, and the dashed circle is Working changes. Commit details list each parent.";

#[derive(Clone)]
struct DraggedColumn {
    owner: EntityId,
    column: HistoryColumn,
}
impl Render for DraggedColumn {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
#[derive(Clone)]
struct ResizeColumn {
    owner: EntityId,
    column: HistoryColumn,
}
impl Render for ResizeColumn {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Sizes a header or row cell. The message takes any space beyond its width.
fn column_frame<E: Styled>(el: E, entry: &ColumnLayout) -> E {
    if entry.column == HistoryColumn::Message {
        el.flex_1().min_w(px(entry.width))
    } else {
        el.w(px(entry.width)).flex_none()
    }
}

impl GitCrab {
    /// Saves the displayed, width-adapted layout as the user's own before editing it.
    fn materialize_columns(&mut self) {
        if !self.history_columns.customized {
            self.history_columns = self.history_layout.clone();
            self.history_columns.customized = true;
        }
    }

    /// Column visibility, order and width controls shared by the header menu and
    /// the Columns button, so every drag and resize action has a keyboard route.
    pub(super) fn columns_menu(
        weak: WeakEntity<Self>,
        mut menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let Some(view) = weak.upgrade() else {
            return menu;
        };
        let layout = view.read(cx).history_layout.clone();
        menu = menu.item(PopupMenuItem::label("Show columns"));
        for column in HistoryColumn::ALL {
            let weak = weak.clone();
            let visible = layout
                .columns
                .iter()
                .any(|e| e.column == column && e.visible);
            menu = menu.item(
                components::menu_item(column.label(), column != HistoryColumn::Message)
                    .checked(visible)
                    .on_click(move |_, w, cx| {
                        let _ = weak.update(cx, |v, cx| {
                            v.materialize_columns();
                            v.history_columns.toggle(column);
                            v.columns_changed(w, cx);
                        });
                    }),
            );
        }
        let visible: Vec<_> = layout.visible().map(|e| e.column).collect();
        let order = weak.clone();
        let order_columns = visible.clone();
        menu = menu
            .separator()
            .submenu("Move column", window, cx, move |mut menu, _, _| {
                for (index, column) in order_columns.iter().copied().enumerate() {
                    for (delta, direction) in [(-1, "left"), (1, "right")] {
                        let weak = order.clone();
                        let at_end = (delta < 0 && index == 0)
                            || (delta > 0 && index + 1 == order_columns.len());
                        menu = menu.item(
                            components::menu_item(
                                format!("Move {} {direction}", column.label()),
                                !at_end,
                            )
                            .on_click(move |_, w, cx| {
                                let _ = weak.update(cx, |v, cx| {
                                    v.materialize_columns();
                                    v.history_columns.move_visible(column, delta);
                                    v.columns_changed(w, cx);
                                });
                            }),
                        );
                    }
                }
                menu
            });
        let widths = weak.clone();
        menu = menu.submenu("Column width", window, cx, move |mut menu, _, _| {
            for column in visible.iter().copied() {
                for (delta, verb) in [(32., "Widen"), (-32., "Narrow")] {
                    let weak = widths.clone();
                    menu = menu.item(
                        components::menu_item(format!("{verb} {}", column.label()), true).on_click(
                            move |_, w, cx| {
                                let _ = weak.update(cx, |v, cx| {
                                    v.materialize_columns();
                                    v.history_columns.resize_by(column, delta);
                                    v.columns_changed(w, cx);
                                });
                            },
                        ),
                    );
                }
            }
            menu
        });
        let weak = weak.clone();
        menu.separator().item(
            components::menu_item("Reset columns · fit to window", true).on_click(
                move |_, w, cx| {
                    let _ = weak.update(cx, |v, cx| {
                        v.history_columns = HistoryColumns::default();
                        v.columns_changed(w, cx);
                    });
                },
            ),
        )
    }

    /// The graph column's own horizontal scrollbar, along the bottom of the
    /// table under that column only.
    pub(super) fn graph_scrollbar(&self) -> AnyElement {
        div()
            .absolute()
            .left_0()
            .bottom_0()
            .w_full()
            .h(px(10.))
            .pr(px(self.history_gutter()))
            .flex()
            .children(self.history_layout.visible().map(|entry| {
                column_frame(div(), entry).h_full().relative().when(
                    entry.column == HistoryColumn::Graph,
                    |cell| {
                        cell.child(
                            div().absolute().inset_0().child(slim_scrollbar(
                                gpui::component::scroll::Scrollbar::horizontal(&self.graph_scroll)
                                    .id("history-graph-scrollbar")
                                    .viewport_from_layout()
                                    .mode(gpui::component::scroll::ScrollbarMode::Always),
                            )),
                        )
                    },
                )
            }))
            .into_any_element()
    }

    /// Scrolls the graph column with a horizontal (or Shift+vertical) wheel,
    /// leaving vertical scrolling to the list.
    pub(super) fn scroll_graph(
        &mut self,
        event: &ScrollEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.graph_scroll.max() <= 0. {
            return;
        }
        let delta = event.delta.pixel_delta(window.line_height());
        let dx = if delta.x != px(0.) {
            delta.x
        } else if event.modifiers.shift {
            delta.y
        } else {
            return;
        };
        if self.graph_scroll.scroll_by(-f32::from(dx)) {
            cx.notify();
        }
        cx.stop_propagation();
    }

    /// The trailing padding of the header and every row, under the Columns button.
    pub(super) fn history_gutter(&self) -> f32 {
        COLUMNS_BUTTON_ROOM * self.preferences.font_size / 13.
    }
    /// The table's minimum width: its visible columns plus the trailing gutter.
    pub(super) fn history_table_width(&self) -> f32 {
        self.history_layout
            .visible()
            .map(|entry| entry.width)
            .sum::<f32>()
            + self.history_gutter()
    }
    pub(super) fn history_cell(&self, entry: &ColumnLayout) -> Div {
        column_frame(div(), entry)
            .h_full()
            .flex()
            .items_center()
            .overflow_hidden()
            .when(entry.column != HistoryColumn::Graph, |el| {
                el.px(HISTORY_CELL_INSET)
            })
    }
    fn columns_changed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.preference_save = None;
        if let Err(error) = self.save_preferences(false, cx) {
            self.state.error = true;
            self.state.notice = format!("Could not save column layout: {error}");
        }
        cx.notify();
    }
    fn order_columns(&mut self, order: &[HistoryColumn]) {
        // Hidden entries retain their positions and width/visibility settings.
        let mut visible: Vec<_> = self.history_columns.visible().cloned().collect();
        visible.sort_by_key(|entry| {
            order
                .iter()
                .position(|id| *id == entry.column)
                .unwrap_or(usize::MAX)
        });
        let mut next = visible.into_iter();
        for entry in &mut self.history_columns.columns {
            if entry.visible {
                *entry = next.next().unwrap();
            }
        }
    }
    fn move_column_drag(&mut self, pointer: Pixels, cx: &mut Context<Self>) {
        let Some(rail) = self
            .history_column_drag
            .as_mut()
            .filter(|rail| !rail.released)
        else {
            return;
        };
        if rail.update(pointer, std::time::Instant::now()) {
            let order: Vec<_> = rail.order().collect();
            self.order_columns(&order);
        }
        cx.notify();
    }
    pub(super) fn prepare_column_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        if self
            .history_column_drag
            .as_ref()
            .is_some_and(|rail| rail.settled(now))
        {
            self.history_column_drag = None;
        }
        if self
            .history_column_drag
            .as_ref()
            .is_some_and(|rail| rail.animating(now))
        {
            let weak = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                let _ = weak.update(cx, |_, cx| cx.notify());
            });
            window.request_animation_frame();
        }
    }

    pub(super) fn finish_column_drag(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((column, start, width)) = self
            .history_column_resize
            .take()
            .filter(|_| cx.has_active_drag())
        {
            let width = width + f32::from(event.position.x - start);
            self.history_columns.resize(column, width);
            self.columns_changed(window, cx);
            return;
        }
        if self
            .history_column_drag
            .as_ref()
            .is_some_and(|rail| !rail.released)
        {
            self.move_column_drag(event.position.x, cx);
            self.history_column_drag
                .as_mut()
                .unwrap()
                .release(std::time::Instant::now());
            self.columns_changed(window, cx);
        }
    }

    pub(super) fn cancel_column_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .history_column_drag
            .as_ref()
            .is_some_and(|rail| !rail.released)
        {
            let rail = self.history_column_drag.take().unwrap();
            self.order_columns(rail.original());
            cx.stop_active_drag(window);
            cx.stop_propagation();
            cx.notify();
        }
    }

    pub(super) fn history_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let owner = cx.entity_id();
        let weak = cx.weak_entity();
        let now = std::time::Instant::now();
        let bounds = std::rc::Rc::new(std::cell::RefCell::new(
            self.history_layout
                .visible()
                .map(|entry| (entry.column, Bounds::<Pixels>::default()))
                .collect::<Vec<_>>(),
        ));
        div()
            .id("history-header")
            .h(scaled(28.))
            .pr(px(self.history_gutter()))
            .flex_none()
            .flex()
            .items_center()
            .bg_role(Role::SurfaceChrome)
            .text_size(scaled(12.))
            .text_role(Role::TextSecondary)
            .font_weight(FontWeight::MEDIUM)
            .on_drag_move(
                cx.listener(move |v, event: &DragMoveEvent<DraggedColumn>, w, cx| {
                    if event.drag(cx).owner != owner
                        || v.history_column_drag
                            .as_ref()
                            .is_none_or(|rail| rail.dragged != event.drag(cx).column)
                    {
                        return;
                    }
                    if cx.active_drag_cursor_style() != Some(CursorStyle::ClosedHand) {
                        cx.set_active_drag_cursor_style(CursorStyle::ClosedHand, w);
                    }
                    v.move_column_drag(event.event.position.x, cx);
                }),
            )
            .on_drag_move(
                cx.listener(move |v, event: &DragMoveEvent<ResizeColumn>, _, cx| {
                    let drag = event.drag(cx);
                    if drag.owner != owner {
                        return;
                    }
                    let Some((column, start, width)) = v.history_column_resize else {
                        return;
                    };
                    if drag.column != column {
                        return;
                    }
                    v.history_columns
                        .resize(column, width + f32::from(event.event.position.x - start));
                    cx.notify();
                }),
            )
            .children(
                self.history_layout
                    .visible()
                    .enumerate()
                    .map(|(index, entry)| {
                        let column = entry.column;
                        let width = entry.width;
                        let minimized_graph =
                            column == HistoryColumn::Graph && width <= column.minimum_width();
                        let moving = self
                            .history_column_drag
                            .as_ref()
                            .is_some_and(|rail| rail.dragged == column);
                        components::rail::item(
                            column_frame(div().id(("history-column", column as usize)), entry)
                                .relative()
                                .h_full()
                                .flex()
                                .items_center()
                                .when(moving, |el| el.bg_role(Role::SurfaceOverlay))
                                .child({
                                    let bounds = bounds.clone();
                                    canvas(
                                        move |b, _, _| bounds.borrow_mut()[index].1 = b,
                                        |_, _, _, _| {},
                                    )
                                    .absolute()
                                    .size_full()
                                })
                                .child(
                                    div()
                                        .id(("history-column-label", column as usize))
                                        .flex_1()
                                        .min_w_0()
                                        .h_full()
                                        .mx(px(RESIZE_HALF_WIDTH))
                                        .px(px(3.))
                                        .flex()
                                        .items_center()
                                        .cursor_grab()
                                        .active(|el| el.cursor_grabbing())
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |v, event: &MouseDownEvent, _, _| {
                                                v.history_column_drag_start =
                                                    Some((column, event.position.x));
                                            }),
                                        )
                                        .on_drag(DraggedColumn { owner, column }, {
                                            let weak = weak.clone();
                                            let bounds = bounds.clone();
                                            move |drag, _, w, cx| {
                                                let _ = weak.update(cx, |v, cx| {
                                                    v.materialize_columns();
                                                    let pointer = v
                                                        .history_column_drag_start
                                                        .filter(|(id, _)| *id == column)
                                                        .map(|(_, x)| x)
                                                        .unwrap_or(w.mouse_position().x);
                                                    v.history_column_drag = RailDrag::new(
                                                        &bounds.borrow(),
                                                        column,
                                                        pointer,
                                                        0.,
                                                    );
                                                    v.move_column_drag(w.mouse_position().x, cx);
                                                });
                                                cx.stop_propagation();
                                                cx.new(|_| drag.clone())
                                            }
                                        })
                                        .when(minimized_graph, |el| {
                                            el.child(
                                                icon("branch")
                                                    .size(scaled(14.))
                                                    .text_role(Role::TextSecondary),
                                            )
                                        })
                                        .when(!minimized_graph, |el| {
                                            el.child(div().truncate().child(column.label()))
                                        })
                                        .when(column == HistoryColumn::Graph, |el| {
                                            el.tooltip(|w, cx| {
                                                Tooltip::new(GRAPH_LEGEND).build(w, cx)
                                            })
                                        }),
                                )
                                .child(
                                    div()
                                        .id(("history-column-resize", column as usize))
                                        .absolute()
                                        .right(px(-RESIZE_HALF_WIDTH))
                                        .top_0()
                                        .bottom_0()
                                        .w(px(RESIZE_HALF_WIDTH * 2.))
                                        .occlude()
                                        .cursor_col_resize()
                                        .group("history-column-resize")
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |v, event: &MouseDownEvent, _, _| {
                                                v.materialize_columns();
                                                v.history_column_resize =
                                                    Some((column, event.position.x, width));
                                            }),
                                        )
                                        .on_drag(
                                            ResizeColumn { owner, column },
                                            |drag, _, _, cx| {
                                                cx.stop_propagation();
                                                cx.new(|_| drag.clone())
                                            },
                                        )
                                        .child(
                                            div()
                                                .absolute()
                                                .left(px(RESIZE_HALF_WIDTH))
                                                .top(px(6.))
                                                .bottom(px(6.))
                                                .w(px(1.))
                                                .bg(tint(semantic(Role::TextSecondary), 0x25))
                                                .group_hover("history-column-resize", |el| {
                                                    el.bg(tint(semantic(Role::TextSecondary), 0x65))
                                                }),
                                        ),
                                ),
                            self.history_column_drag.as_ref(),
                            column,
                            now,
                        )
                    }),
            )
            .context_menu(move |menu, w, cx| Self::columns_menu(weak.clone(), menu, w, cx))
            .into_any_element()
    }
}
