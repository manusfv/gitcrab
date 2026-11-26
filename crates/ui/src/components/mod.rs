//! GitCrab's reusable UI components.
//!
//! Views build their panes from these pieces instead of styling raw elements,
//! so the same kind of control looks and behaves the same everywhere:
//!
//! - [`buttons`]: button roles and sizes, toolbar commands and selectors.
//! - [`list_row`]: rows for lists and trees, with hover, selection and slots.
//! - [`badge`]: tinted labels for counts, statuses and markers.
//! - [`layout`]: pane surfaces, header strips, groups and dividers.
//! - [`menu`]: context menus shown by the operating system, and drawn menu entries.
//! - [`modal`]: the dialog frame (header, body, footer) and its content pieces.
//! - [`picker`]: the searchable popup list for choosing a branch, repository or tab.
//! - [`rail`]: drag-to-reorder for tab strips and column headers.
//!
//! Views import everything through [`prelude`].
pub mod badge;
pub mod buttons;
pub mod layout;
pub mod list_row;
pub mod menu;
pub mod modal;
pub mod picker;
pub mod rail;

pub use buttons::*;
pub use menu::{menu_cursor, menu_item, worktree_file, ContextMenu, MenuTarget};

/// The components and helpers views use, for a single glob import.
pub mod prelude {
    pub use super::badge::Badge;
    pub use super::buttons::{ButtonExt as _, ControlSize};
    pub use super::layout::*;
    pub use super::list_row::{disclosure, ListRow};
    pub use super::modal::{
        modal_body, ModalFooter, ModalFrame as _, ModalHeader, ModalWidth, OpenModal as _, Tone,
    };
    pub use super::picker::{Picker, PickerEntry, PickerItem};
}

/// Applies a caller's style overrides on top of a component's defaults.
///
/// Text properties merge one by one. A plain refinement would replace the
/// whole text style, so setting only a font weight would drop the
/// component's text color and size.
pub(crate) fn apply_style<E: gpui::Styled>(mut el: E, style: &gpui::StyleRefinement) -> E {
    use gpui::Refineable as _;
    let mut style = style.clone();
    let text = std::mem::take(&mut style.text);
    el.style().refine(&style);
    el.text_style().refine(&text);
    el
}

/// Key context of the repository view, where most bindings apply.
pub const KEY_CONTEXT: &str = "GitCrab";

/// The platform's text for the key bound to `action`, such as `Ctrl+O`, for
/// places that cannot show a key badge.
pub fn shortcut_text(action: &dyn gpui::Action, window: &gpui::Window) -> Option<String> {
    use gpui::AsKeystroke as _;
    let context = gpui::KeyContext::parse(KEY_CONTEXT).ok()?;
    let binding = window.highest_precedence_binding_for_action_in_context(action, context)?;
    Some(gpui::component::kbd::Kbd::format(
        binding.keystrokes().first()?.as_keystroke(),
    ))
}
