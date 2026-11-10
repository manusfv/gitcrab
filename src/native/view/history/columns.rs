use super::*;
use components::rail::RailDrag;
use gitcrab_core::history_columns::ColumnLayout;

const RESIZE_HALF_WIDTH: f32 = 5.;

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

impl GitCrab {
    pub(super) fn history_table_width(&self) -> f32 {
        self.preferences
            .history_columns
            .visible()
            .map(|entry| entry.width)
            .sum()
    }
    pub(super) fn history_cell(&self, entry: &ColumnLayout) -> Div {
        div()
            .w(px(entry.width))
            .flex_none()
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
        if let Err(error) = self.save_preferences(cx) {
            self.state.error = true;
            self.state.notice = format!("Could not save column layout: {error}");
        }
        cx.notify();
    }
    fn order_columns(&mut self, order: &[HistoryColumn]) {
        // Hidden entries retain their positions and width/visibility settings.
        let mut visible: Vec<_> = self
            .preferences
            .history_columns
            .visible()
            .cloned()
            .collect();
        visible.sort_by_key(|entry| {
            order
                .iter()
                .position(|id| *id == entry.column)
                .unwrap_or(usize::MAX)
        });
        let mut next = visible.into_iter();
        for entry in &mut self.preferences.history_columns.columns {
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
            self.preferences.history_columns.resize(column, width);
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
            self.preferences
                .history_columns
                .visible()
                .map(|entry| (entry.column, Bounds::<Pixels>::default()))
                .collect::<Vec<_>>(),
        ));
        div()
            .id("history-header")
            .h(type_size(cx, 28.))
            .flex_none()
            .flex()
            .items_center()
            .bg(rgb(semantic(Role::SurfaceChrome)))
            .text_size(type_size(cx, 12.))
            .text_color(rgb(semantic(Role::TextSecondary)))
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
                    v.preferences
                        .history_columns
                        .resize(column, width + f32::from(event.event.position.x - start));
                    cx.notify();
                }),
            )
            .children(self.preferences.history_columns.visible().enumerate().map(
                |(index, entry)| {
                    let column = entry.column;
                    let width = entry.width;
                    let minimized_graph =
                        column == HistoryColumn::Graph && width <= column.minimum_width();
                    let moving = self
                        .history_column_drag
                        .as_ref()
                        .is_some_and(|rail| rail.dragged == column);
                    components::rail::item(
                        div()
                            .id(("history-column", column as usize))
                            .relative()
                            .w(px(width))
                            .h_full()
                            .flex_none()
                            .flex()
                            .items_center()
                            .when(moving, |el| el.bg(rgb(semantic(Role::SurfaceOverlay))))
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
                                                .size(type_size(cx, 14.))
                                                .text_color(rgb(semantic(Role::TextSecondary))),
                                        )
                                        .tooltip(|w, cx| Tooltip::new("Graph").build(w, cx))
                                    })
                                    .when(!minimized_graph, |el| {
                                        el.child(div().truncate().child(column.label()))
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
                                            v.history_column_resize =
                                                Some((column, event.position.x, width));
                                        }),
                                    )
                                    .on_drag(ResizeColumn { owner, column }, |drag, _, _, cx| {
                                        cx.stop_propagation();
                                        cx.new(|_| drag.clone())
                                    })
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
                },
            ))
            .context_menu(move |mut menu, _, cx| {
                let Some(view) = weak.upgrade() else {
                    return menu;
                };
                let layout = view.read(cx).preferences.history_columns.clone();
                for column in HistoryColumn::ALL {
                    let weak = weak.clone();
                    let visible = layout
                        .columns
                        .iter()
                        .any(|e| e.column == column && e.visible);
                    menu = menu.item(
                        PopupMenuItem::new(column.label())
                            .checked(visible)
                            .disabled(column == HistoryColumn::Message)
                            .on_click(move |_, w, cx| {
                                let _ = weak.update(cx, |v, cx| {
                                    v.preferences.history_columns.toggle(column);
                                    v.columns_changed(w, cx);
                                });
                            }),
                    );
                }
                let weak = weak.clone();
                menu.separator()
                    .item(
                        PopupMenuItem::new("Reset columns").on_click(move |_, w, cx| {
                            let _ = weak.update(cx, |v, cx| {
                                v.preferences.history_columns = HistoryColumns::default();
                                v.history_horizontal_scroll
                                    .set_offset(point(px(0.), px(0.)));
                                v.columns_changed(w, cx);
                            });
                        }),
                    )
            })
            .into_any_element()
    }
}
