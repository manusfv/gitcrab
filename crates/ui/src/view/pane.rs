//! The window's regions as their own views, so scrolling or hovering in one
//! repaints only that region while GPUI reuses the others from the last frame.

use super::*;

#[derive(Clone, Copy)]
pub(super) enum Region {
    Sidebar,
    Center,
    Inspector,
}

pub(super) struct Pane {
    owner: WeakEntity<GitCrab>,
    region: Region,
}

impl Pane {
    pub(super) fn new(owner: &Entity<GitCrab>, region: Region, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            cx.observe(owner, |_, _, cx| cx.notify()).detach();
            Self {
                owner: owner.downgrade(),
                region,
            }
        })
    }
}

impl Render for Pane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let region = self.region;
        self.owner
            .update(cx, |v, cx| v.region(region, window, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}
