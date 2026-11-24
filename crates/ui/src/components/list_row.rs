//! The shared row for lists and trees: references, changed files and pickers.
use super::buttons::ButtonExt as _;
use crate::theme::Role;
use crate::theme::*;
use gpui::component::{
    button::Button,
    menu::{ContextMenuExt as _, PopupMenu},
    tooltip::Tooltip,
    Sizable,
};
use gpui::{prelude::*, *};
use std::rc::Rc;

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type MenuBuilder = Rc<dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu>;

/// One row of a list.
///
/// A row has a click target holding `leading` items, a truncating label and
/// `trailing` items. Optional `start` and `end` items sit outside the target,
/// for elements with their own tooltip or action, such as a status badge or a
/// Stage button. The row owns hover, selection and the selection rail, so
/// every list looks and responds the same way.
///
/// By default the target is a keyboard-focusable button. A row in a list with
/// its own keyboard navigation, such as a picker, uses `.focusable(false)`.
///
/// Sizes are pixels at the default font size and scale with it.
#[derive(IntoElement)]
pub struct ListRow {
    id: ElementId,
    height: f32,
    inset: f32,
    indent: f32,
    leading: Vec<AnyElement>,
    label: Option<AnyElement>,
    trailing: Vec<AnyElement>,
    start: Option<AnyElement>,
    end: Option<AnyElement>,
    foreground: Role,
    selected: bool,
    rail: bool,
    highlighted: bool,
    background: Option<Role>,
    disabled: bool,
    focusable: bool,
    tab_stop: bool,
    tooltip: Option<SharedString>,
    group: Option<SharedString>,
    focus: Option<FocusHandle>,
    on_click: Option<ClickHandler>,
    menu: Option<MenuBuilder>,
    style: StyleRefinement,
}

impl ListRow {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            height: FILE_ROW_HEIGHT,
            inset: 8.,
            indent: 0.,
            leading: vec![],
            label: None,
            trailing: vec![],
            start: None,
            end: None,
            foreground: Role::TextDefault,
            selected: false,
            rail: false,
            highlighted: false,
            background: None,
            disabled: false,
            focusable: true,
            tab_stop: true,
            tooltip: None,
            group: None,
            focus: None,
            on_click: None,
            menu: None,
            style: StyleRefinement::default(),
        }
    }

    /// Row height in pixels at the default font size.
    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }
    /// Horizontal padding on both sides.
    pub fn inset(mut self, inset: f32) -> Self {
        self.inset = inset;
        self
    }
    /// Extra left padding, for tree depth.
    pub fn indent(mut self, indent: f32) -> Self {
        self.indent = indent;
        self
    }
    /// Adds an item before the label inside the click target, such as an icon.
    pub fn leading(mut self, item: impl IntoElement) -> Self {
        self.leading.push(item.into_any_element());
        self
    }
    /// Sets a single-line label that truncates to the available width.
    pub fn label(mut self, text: impl Into<SharedString>) -> Self {
        self.label = Some(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(text.into())
                .into_any_element(),
        );
        self
    }
    /// Sets custom label content, which should truncate itself.
    pub fn content(mut self, content: impl IntoElement) -> Self {
        self.label = Some(content.into_any_element());
        self
    }
    /// Adds an item after the label inside the click target, such as a count.
    pub fn trailing(mut self, item: impl IntoElement) -> Self {
        self.trailing.push(item.into_any_element());
        self
    }
    /// Puts an item before the click target, outside it.
    pub fn start(mut self, item: impl IntoElement) -> Self {
        self.start = Some(item.into_any_element());
        self
    }
    /// Puts an item after the click target, outside it, such as an action.
    pub fn end(mut self, item: impl IntoElement) -> Self {
        self.end = Some(item.into_any_element());
        self
    }
    /// The text color of the row.
    pub fn foreground(mut self, role: Role) -> Self {
        self.foreground = role;
        self
    }
    /// Marks the row as the current selection.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
    /// Marks the row as the one shown elsewhere, with a selection fill and a
    /// rail on its left edge.
    pub fn active(mut self, active: bool) -> Self {
        self.selected |= active;
        self.rail = active;
        self
    }
    /// Gives the row the accent tint used for a previewed item.
    pub fn highlighted(mut self, highlighted: bool) -> Self {
        self.highlighted = highlighted;
        self
    }
    /// A resting fill for a row that stands out without being selected.
    pub fn background(mut self, role: Role) -> Self {
        self.background = Some(role);
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    /// Whether the click target takes keyboard focus.
    pub fn focusable(mut self, focusable: bool) -> Self {
        self.focusable = focusable;
        self
    }
    /// Whether Tab stops on the click target.
    pub fn tab_stop(mut self, tab_stop: bool) -> Self {
        self.tab_stop = tab_stop;
        self
    }
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
    /// Names a hover group, so items can appear while the row is hovered.
    pub fn group(mut self, name: impl Into<SharedString>) -> Self {
        self.group = Some(name.into());
        self
    }
    /// Tracks a focus handle on the whole row, so items can appear while any
    /// part of it has keyboard focus.
    pub fn track_focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
    /// Opens a context menu on right-click. The row keeps its hover fill
    /// while the menu is open.
    pub fn context_menu(
        mut self,
        builder: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> Self {
        self.menu = Some(Rc::new(builder));
        self
    }
}

impl Styled for ListRow {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for ListRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let row_id = SharedString::from(format!("list-row-{}", self.id));
        let menu_open = self.menu.as_ref().map(|_| {
            window.use_keyed_state(
                SharedString::from(format!("{row_id}-menu-open")),
                cx,
                |_, _| false,
            )
        });
        let held = !self.disabled && menu_open.as_ref().is_some_and(|open| *open.read(cx));
        let fill = if self.highlighted {
            Some(Role::ActionAccentTintPressed)
        } else if self.selected {
            Some(Role::InteractionSelected)
        } else {
            self.background
        };
        let hover = if self.highlighted || self.selected {
            fill
        } else {
            Some(Role::InteractionHover)
        };
        let foreground = if self.disabled {
            Role::TextDisabled
        } else {
            self.foreground
        };
        // Padding sits inside the click target unless an outside item takes
        // that edge, so the whole row width stays clickable where it can.
        let left = self.inset + self.indent;
        let right = self.inset;
        let pad_inner_left = self.start.is_none();
        let pad_inner_right = self.end.is_none();

        let body = self
            .leading
            .into_iter()
            .chain(self.label)
            .chain(self.trailing);
        let target = if self.focusable {
            super::buttons::without_interaction_fill(Button::new(self.id), foreground, cx)
                .small()
                .tab_stop(self.tab_stop && !self.disabled)
                .set_disabled(self.disabled)
                .flex_1()
                .min_w_0()
                .h_full()
                .px_0()
                .justify_start()
                .rounded(px(0.))
                .bg(transparent_black())
                .border_color(transparent_black())
                .text_role(foreground)
                .when(pad_inner_left, |b| b.pl(scaled(left)))
                .when(pad_inner_right, |b| b.pr(scaled(right)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .children(body),
                )
                .when_some(self.tooltip, |b, tooltip| b.tooltip(tooltip))
                .when_some(self.on_click, |b, handler| b.on_click(handler))
                .into_any_element()
        } else {
            div()
                .id(self.id)
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .gap_2()
                .when(pad_inner_left, |d| d.pl(scaled(left)))
                .when(pad_inner_right, |d| d.pr(scaled(right)))
                .children(body)
                .when(!self.disabled, |d| d.cursor_pointer())
                .when_some(self.tooltip, |d, text| {
                    d.tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
                })
                .when_some(self.on_click.filter(|_| !self.disabled), |d, handler| {
                    d.on_click(handler)
                })
                .into_any_element()
        };

        let row = div()
            .id(ElementId::Name(row_id))
            .when_some(self.group, |row, group| row.group(group))
            .when_some(self.focus, |row, focus| row.track_focus(&focus))
            .w_full()
            .h(scaled(self.height))
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .when(!pad_inner_left, |row| row.pl(scaled(left)))
            .when(!pad_inner_right, |row| row.pr(scaled(right)))
            .text_role(foreground)
            .when_some(fill, |row, fill| row.bg_role(fill))
            .when(self.rail, |row| {
                row.border_l_2().border_role(Role::InteractionSelectionRail)
            })
            .when(!self.disabled, |row| {
                row.when_some(hover, |row, hover| row.hover(|s| s.bg_role(hover)))
            })
            .children(self.start)
            .child(target)
            .children(self.end)
            .when(held, |row| {
                row.when_some(hover, |row, hover| row.bg_role(hover))
            })
            .map(|row| super::apply_style(row, &self.style));
        match (self.menu, menu_open) {
            (Some(builder), Some(open)) => row
                .context_menu(move |menu, w, cx| {
                    open.update(cx, |open, cx| {
                        *open = true;
                        cx.notify();
                    });
                    let closed = open.clone();
                    cx.subscribe_self(move |_, _: &DismissEvent, cx| {
                        closed.update(cx, |open, cx| {
                            *open = false;
                            cx.notify();
                        })
                    })
                    .detach();
                    builder(menu, w, cx)
                })
                .into_any_element(),
            _ => row.into_any_element(),
        }
    }
}

/// The chevron of a collapsible row.
pub fn disclosure(collapsed: bool) -> gpui::component::Icon {
    icon(if collapsed { "right" } else { "down" })
        .size(px(geometry::SIZE_ICON_SM))
        .text_role(Role::TextMuted)
}
