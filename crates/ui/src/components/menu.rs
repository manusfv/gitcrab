//! Menu entries.
use gpui::component::menu::PopupMenuItem;
use gpui::{prelude::*, *};

/// A popup menu entry whose row shows the pointer cursor while it is enabled.
///
/// Library menu entries keep the arrow cursor and accept no styling, so the
/// label is rendered as a custom element. The library still handles clicks,
/// keyboard navigation, `checked`, `action` shortcuts and the disabled look.
pub fn menu_item(label: impl Into<SharedString>, enabled: bool) -> PopupMenuItem {
    let label = label.into();
    PopupMenuItem::element(move |_, _| {
        div()
            // Reach over the entry's own padding (`px_2`, `py_1`) so the
            // whole row shows the cursor, not just the text.
            .mx(rems(-0.5))
            .my(rems(-0.25))
            .px(rems(0.5))
            .py(rems(0.25))
            .flex_1()
            .min_w_0()
            .when(enabled, |row| row.cursor_pointer())
            .child(label.clone())
    })
    .disabled(!enabled)
}
