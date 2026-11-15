//! Horizontal scrolling of the history graph column on its own.
//!
//! Lanes keep their pitch however many there are. When they
//! don't fit, only the graph column scrolls sideways, with its own scrollbar,
//! and the rest of the table stays put.
use gpui::component::scroll::ScrollbarHandle;
use gpui::{point, px, size, Bounds, Pixels, Point, Size};
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Extent {
    /// How far the lanes are scrolled right, in logical pixels.
    scrolled: f32,
    /// Width of the graph column.
    viewport: f32,
    /// Width the lanes need.
    content: f32,
}

/// Shared scroll position of the graph column, read by every row's painter
/// and driven by the wheel and the column's scrollbar.
#[derive(Clone, Default)]
pub(crate) struct GraphScroll(Rc<Cell<Extent>>);

impl GraphScroll {
    /// How far the lanes are scrolled right, in logical pixels.
    pub(crate) fn scrolled(&self) -> f32 {
        self.0.get().scrolled
    }

    /// The largest scroll that still shows the last lane.
    pub(crate) fn max(&self) -> f32 {
        let extent = self.0.get();
        (extent.content - extent.viewport).max(0.)
    }

    /// Records this frame's column and lane widths, keeping the scroll in range.
    pub(crate) fn set_extent(&self, viewport: f32, content: f32) {
        let finite = |value: f32| if value.is_finite() { value.max(0.) } else { 0. };
        let mut extent = self.0.get();
        extent.viewport = finite(viewport);
        extent.content = finite(content);
        self.0.set(extent);
        self.scroll_to(extent.scrolled);
    }

    /// Scrolls by `delta` logical pixels (positive moves toward later lanes).
    /// Returns whether the position changed.
    pub(crate) fn scroll_by(&self, delta: f32) -> bool {
        let before = self.scrolled();
        self.scroll_to(before + delta);
        self.scrolled() != before
    }

    /// Brings a node `center` pixels from the graph's unscrolled left edge into
    /// view when it is less than `margin` inside the column (so pinned to an
    /// edge), centring it in the column, as far as the ends
    /// allow. A node already in view leaves the scroll alone. Returns whether
    /// the position changed.
    pub(crate) fn reveal(&self, center: f32, margin: f32) -> bool {
        let extent = self.0.get();
        let before = extent.scrolled;
        let visible = center - before;
        if visible < margin || visible > extent.viewport - margin {
            self.scroll_to(center - extent.viewport / 2.);
        }
        self.scrolled() != before
    }

    fn scroll_to(&self, scrolled: f32) {
        let mut extent = self.0.get();
        extent.scrolled = if scrolled.is_finite() {
            scrolled.clamp(0., self.max())
        } else {
            0.
        };
        self.0.set(extent);
    }
}

impl ScrollbarHandle for GraphScroll {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        Bounds::new(Point::default(), size(px(self.0.get().viewport), px(0.)))
    }

    fn offset(&self) -> Point<Pixels> {
        point(px(-self.scrolled()), px(0.))
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        self.scroll_to(-f32::from(offset.x));
    }

    fn content_size(&self) -> Size<Pixels> {
        size(px(self.0.get().content), px(0.))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolling_stays_between_the_first_and_last_lane() {
        let scroll = GraphScroll::default();
        scroll.set_extent(100., 340.);
        assert_eq!(scroll.max(), 240.);
        assert!(!scroll.scroll_by(-10.));
        assert!(scroll.scroll_by(500.));
        assert_eq!(scroll.scrolled(), 240.);
        // The scrollbar speaks in negative offsets, like gpui's handles.
        scroll.set_offset(point(px(-60.), px(0.)));
        assert_eq!(scroll.scrolled(), 60.);
        assert_eq!(scroll.offset().x, px(-60.));
        // Fewer lanes after a refresh pull the scroll back into range.
        scroll.set_extent(100., 130.);
        assert_eq!(scroll.scrolled(), 30.);
        scroll.set_extent(100., 80.);
        assert_eq!((scroll.scrolled(), scroll.max()), (0., 0.));
    }

    #[test]
    fn revealing_a_node_centres_it_in_the_column() {
        let scroll = GraphScroll::default();
        scroll.set_extent(100., 400.);
        // Past the right edge: scroll until it sits in the middle.
        assert!(scroll.reveal(235., 15.));
        assert_eq!(scroll.scrolled(), 185.);
        // Already in view, even off-centre: nothing moves.
        assert!(!scroll.reveal(205., 15.));
        // Past the left edge: scroll back until it sits in the middle.
        assert!(scroll.reveal(180., 15.));
        assert_eq!(scroll.scrolled(), 130.);
        // Never past either end.
        scroll.reveal(10., 15.);
        assert_eq!(scroll.scrolled(), 0.);
        scroll.reveal(1000., 15.);
        assert_eq!(scroll.scrolled(), scroll.max());
    }
}
