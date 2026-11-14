//! GitCrab's reusable UI components.
//!
//! Views build their panes from these pieces instead of styling raw elements,
//! so the same kind of control looks and behaves the same everywhere:
//!
//! - [`buttons`]: button roles and sizes, toolbar commands and selectors.
//! - [`list_row`]: rows for lists and trees, with hover, selection and slots.
//! - [`badge`]: tinted labels for counts, statuses and markers.
//! - [`layout`]: pane surfaces, header strips, groups and dividers.
//! - [`rail`]: drag-to-reorder for tab strips and column headers.
//!
//! Views import everything through [`prelude`].
pub mod badge;
pub mod buttons;
pub mod layout;
pub mod list_row;
pub mod rail;

pub use buttons::*;

/// The components and helpers views use, for a single glob import.
pub mod prelude {
    pub use super::badge::Badge;
    pub use super::buttons::{ButtonExt as _, ControlSize};
    pub use super::layout::*;
    pub use super::list_row::{disclosure, ListRow};
}

/// Applies a caller's style overrides on top of a component's defaults.
///
/// Text properties merge one by one. A plain refinement would replace the
/// whole text style, so setting only a font weight would drop the
/// component's text color and size.
pub(crate) fn apply_style<E: gpui::Styled>(mut el: E, style: &gpui::StyleRefinement) -> E {
    use gpui::Refineable as _;
    let mut style = style.clone();
    let text = style.text.take();
    el.style().refine(&style);
    if let Some(text) = text {
        el.text_style()
            .get_or_insert_with(Default::default)
            .refine(&text);
    }
    el
}
