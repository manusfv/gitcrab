//! Containers and dividers that give panes their shared structure.
use crate::theme::*;
use gpui::{prelude::*, *};

/// A full-size pane surface that clips its content.
pub fn panel() -> Div {
    div()
        .flex()
        .flex_col()
        .size_full()
        .min_w_0()
        .min_h_0()
        .bg_role(Role::SurfacePanel)
        .overflow_hidden()
}

/// A pane's header strip, with a divider below.
pub fn section_bar() -> Div {
    div()
        .h(scaled(SECTION_HEIGHT))
        .flex_none()
        .flex()
        .items_center()
        .gap_2()
        .px(scaled(SECTION_INSET))
        .bg_role(Role::SurfaceChrome)
        .border_b_1()
        .border_role(Role::BorderDivider)
}

/// An outlined group of adjacent toggle buttons.
pub fn segmented_group() -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .rounded(px(CONTROL_RADIUS))
        .border_1()
        .border_role(Role::BorderDivider)
        .bg_role(Role::SurfaceChrome)
}

/// A short vertical rule between toolbar groups.
pub fn separator() -> Div {
    div()
        .w(px(1.))
        .h(px(22.))
        .mx_2()
        .flex_none()
        .bg_role(Role::BorderDivider)
}

/// A full-width horizontal rule between stacked sections.
pub fn divider() -> Div {
    div()
        .w_full()
        .h(px(SEAM))
        .flex_none()
        .bg_role(Role::BorderDivider)
}
