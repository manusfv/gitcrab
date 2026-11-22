//! Containers and dividers that give panes their shared structure.
use crate::theme::Role;
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

/// Keeps a scroll position across renders for the region identified by `key`.
pub fn scroll_handle(window: &mut Window, cx: &mut App, key: &'static str) -> ScrollHandle {
    window
        .use_keyed_state(key, cx, |_, _| ScrollHandle::new())
        .read(cx)
        .clone()
}

/// Room kept at the right edge of a scrolling region so content stops short of
/// the scrollbar instead of running underneath it.
pub const SCROLLBAR_GUTTER: Pixels = px(8.);

/// Wraps a vertically scrolling element with a scrollbar that stays visible
/// whenever the content overflows, so people can tell there is more to see.
///
/// The wrapper is a flex column and `content` shrinks to fit it, so a height
/// cap (`max_h`, including a percentage) or flex sizing set on the returned
/// element limits the content. A cap on `content` itself works for pixel
/// values only, because the wrapper has no definite height of its own.
///
/// While the content overflows it gets [`SCROLLBAR_GUTTER`] of right padding,
/// unless it already sets its own right padding, which is kept.
pub fn scroll_area(id: &'static str, handle: &ScrollHandle, content: Stateful<Div>) -> Div {
    let overflowing = overflows_vertically(handle);
    let mut content = content.overflow_y_scroll();
    if overflowing && content.style().padding.right.is_none() {
        content = content.pr(SCROLLBAR_GUTTER);
    }
    track_overflow(div(), handle, overflowing)
        .relative()
        .flex()
        .flex_col()
        .min_h_0()
        .child(content.min_h_0().track_scroll(handle))
        .child(scrollbar_overlay(id, handle))
}

/// An always-visible vertical scrollbar to lay over a scrolling element's
/// parent, for lists that track their own scroll handle.
pub fn scrollbar_overlay<H>(id: &'static str, handle: &H) -> Div
where
    H: gpui::component::scroll::ScrollbarHandle + Clone,
{
    div().absolute().inset_0().child(vertical_bar(id, handle))
}

/// The always-visible slim vertical scrollbar every overlay is built from.
fn vertical_bar<H>(id: &'static str, handle: &H) -> gpui::component::scroll::Scrollbar
where
    H: gpui::component::scroll::ScrollbarHandle + Clone,
{
    slim_scrollbar(
        gpui::component::scroll::Scrollbar::vertical(handle)
            .id(SharedString::from(format!("{id}-scrollbar")))
            .mode(gpui::component::scroll::ScrollbarMode::Always),
    )
}

/// Gives a scrollbar GitCrab's slim look: a rounded thumb with no gutter that
/// thickens under the pointer. Every scrollbar goes through this so they match.
pub fn slim_scrollbar(
    bar: gpui::component::scroll::Scrollbar,
) -> gpui::component::scroll::Scrollbar {
    bar.styles(slim_styles)
}

/// The slim look as scrollbar styles, shared by [`slim_scrollbar`] and the
/// library-drawn scrollbars so the two cannot drift apart.
pub fn slim_styles(styles: gpui::base::ScrollbarStyles) -> gpui::base::ScrollbarStyles {
    styles
        .track(|t| t.width(SCROLLBAR_GUTTER).bg(gpui::transparent_black()))
        .track_hover(|t| t.bg(gpui::transparent_black()))
        .track_active(|t| {
            t.bg(gpui::transparent_black())
                .border_color(gpui::transparent_black())
        })
        .thumb(|t| {
            t.width(px(4.))
                .inset(px(2.))
                .radius(px(2.))
                .min_length(px(32.))
        })
        .thumb_hover(|t| t.width(px(6.)).inset(px(1.)).radius(px(3.)))
        .thumb_active(|t| t.width(px(6.)).inset(px(1.)).radius(px(3.)))
}

/// A vertical scrollbar for a list wider than the pane it scrolls in, such as
/// the history table. The bar follows this overlay's own box (`top` clears any
/// fixed header) instead of the list's, which would put it at the far end of
/// the content, out of sight until scrolled sideways.
pub fn pinned_scrollbar_overlay<H>(
    id: &'static str,
    handle: &H,
    top: impl Into<Length> + Clone,
) -> Div
where
    H: gpui::component::scroll::ScrollbarHandle + Clone,
{
    div()
        .absolute()
        .top(top)
        .left_0()
        .right_0()
        .bottom_0()
        .child(vertical_bar(id, handle).viewport_from_layout())
}

/// Whether a region has more content than fits, as of the last layout.
pub fn overflows_vertically<H: gpui::component::scroll::ScrollbarHandle>(handle: &H) -> bool {
    handle.content_size().height > handle.viewport_bounds().size.height + px(0.5)
}

/// Reserves [`SCROLLBAR_GUTTER`] only while a region overflows, like a classic
/// browser scrollbar: a list that fits keeps its full width.
///
/// `was` is what this frame laid out with. Overflow is only known after layout,
/// so when it turns out different the window repaints once to settle. Narrowing
/// a region can only make its content taller, so the answer never flips back.
pub fn track_overflow<H>(el: Div, handle: &H, was: bool) -> Div
where
    H: gpui::component::scroll::ScrollbarHandle + Clone,
{
    let handle = handle.clone();
    // One request per render. If the mismatch outlives the repaint (a cached
    // view that is not rebuilt), asking again every frame would never settle.
    let asked = std::cell::Cell::new(false);
    el.on_children_prepainted(move |_, _, cx| {
        if overflows_vertically(&handle) != was && !asked.replace(true) {
            // `Window::refresh` is ignored while the window is drawing, so
            // ask for the repaint once this frame is done.
            cx.defer(|cx| cx.refresh_windows());
        }
    })
}
