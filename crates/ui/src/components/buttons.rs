//! Buttons: role palettes, placement sizes and the toolbar controls.
use crate::theme::Role;
use crate::theme::*;
use gpui::component::{
    button::{Button, ButtonCustomVariant, ButtonVariant, ButtonVariants},
    Disableable, Sizable, WindowExt,
};
use gpui::{prelude::*, *};

#[derive(Clone, Copy)]
pub enum ControlSize {
    Regular,
    Compact,
    DenseBulk,
    DenseHunk,
    FileAction,
}
/// Applies the dimensions and typography for a button's placement.
pub fn sized(button: Button, size: ControlSize) -> Button {
    use ControlSize::*;
    let (h, padding, font) = match size {
        Regular => (32., 12., 13.),
        Compact => (28., 8., 12.),
        DenseBulk => (
            geometry::SIZE_BULK_STAGE_HEIGHT,
            geometry::SIZE_BULK_STAGE_PADDING_X,
            geometry::SIZE_BULK_STAGE_FONT_SIZE,
        ),
        DenseHunk => (
            geometry::SIZE_HUNK_ACTION_HEIGHT,
            geometry::SIZE_HUNK_ACTION_PADDING_X,
            geometry::SIZE_HUNK_ACTION_FONT_SIZE,
        ),
        FileAction => (geometry::SIZE_FILE_ACTION_TARGET, 0., 11.),
    };
    button
        .small()
        .h(scaled(h))
        .px(scaled(padding))
        .text_size(scaled(font))
        // Library buttons default to a 1.25 line height; GitCrab's two-line
        // buttons (selectors, recent repositories) are laid out for GPUI's default.
        .line_height(phi())
        .rounded(px(3.))
        // Enabled buttons show a pointer; `disabled_when` and `set_disabled`
        // swap it for not-allowed.
        .cursor_pointer()
}
/// The role a button plays, which selects its palette everywhere it appears.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// The main call to action of a surface, such as Commit.
    Primary,
    /// An ordinary outlined action.
    Secondary,
    /// A borderless action that blends into its toolbar.
    Ghost,
    /// Adds changes to the next commit.
    Stage,
    /// Moves changes out of the next commit and keeps them in the working copy.
    Unstage,
    /// Permanently removes working-copy content or records an unusual resolution.
    Discard,
}

/// Applies the palette for a role that needs no theme access.
///
/// Staging roles fall back to Secondary here; use [`style`] for those.
pub fn basic(button: Button, kind: ButtonKind, size: ControlSize) -> Button {
    let button = sized(button, size);
    match kind {
        // Library primary buttons have no border; the disabled look needs one.
        ButtonKind::Primary => button.primary().border_1().text_role(Role::SurfaceChrome),
        ButtonKind::Ghost => button.ghost(),
        _ => button.with_variant(ButtonVariant::Secondary).outline(),
    }
}

/// Disabled behavior shared by every button.
pub trait ButtonExt: Sized {
    /// Sets the disabled state. When disabled the button gets the shared
    /// unavailable look: muted text, flat muted fill, no hover highlight and a
    /// not-allowed cursor.
    fn disabled_when(self, disabled: bool) -> Self;
    /// Sets the disabled state without the muted look: a pointer cursor when
    /// enabled, not-allowed when disabled. The library leaves the cursor as an
    /// arrow either way, so GitCrab buttons set it here.
    fn set_disabled(self, disabled: bool) -> Self;
}

impl ButtonExt for Button {
    fn disabled_when(self, disabled: bool) -> Self {
        let button = Disableable::disabled(self, disabled);
        if !disabled {
            return button.cursor_pointer();
        }
        button
            .bg_role(Role::SurfaceDisabled)
            .text_role(Role::TextDisabled)
            .border_role(Role::BorderDivider)
            .cursor_not_allowed()
    }
    fn set_disabled(self, disabled: bool) -> Self {
        let button = Disableable::disabled(self, disabled);
        if disabled {
            button.cursor_not_allowed()
        } else {
            button.cursor_pointer()
        }
    }
}

/// Applies sizing and the palette of a button's role.
///
/// Stage uses an accent fill, Unstage a neutral outline so it reads as
/// reversible, and Discard the warning palette for permanent changes. Every
/// button of the same role and size is identical.
pub fn style(button: Button, kind: ButtonKind, size: ControlSize, cx: &App) -> Button {
    let (fg, bg, border, hover) = match kind {
        ButtonKind::Stage => (
            Role::ActionStageFg,
            Role::ActionStageBg,
            Role::ActionStageFg,
            Role::ActionStageBg,
        ),
        ButtonKind::Unstage => (
            Role::TextDefault,
            Role::SurfaceOverlay,
            Role::BorderControl,
            Role::InteractionPressed,
        ),
        ButtonKind::Discard => (
            Role::ActionDiscardFg,
            Role::ActionDiscardBg,
            Role::ActionDiscardFg,
            Role::ActionDiscardBg,
        ),
        _ => return basic(button, kind, size),
    };
    sized(button, size)
        .custom(
            ButtonCustomVariant::new(cx)
                .foreground(role_color(fg).into())
                .color(role_color(bg).into())
                .hover(role_color(hover).into())
                .active(role_color(bg).into()),
        )
        // Custom variants no longer carry a border color.
        .border_1()
        .border_color(role_color(border))
}

/// Suppresses a library button's own hover and press fill, for buttons inside
/// a surface that draws its own feedback, such as a list row.
pub fn without_interaction_fill(button: Button, foreground: Role, cx: &App) -> Button {
    let clear: Hsla = gpui::transparent_black();
    button
        .custom(
            ButtonCustomVariant::new(cx)
                .foreground(role_color(foreground).into())
                .color(clear)
                .hover(clear)
                .active(clear),
        )
        .cursor_pointer()
}

/// Gives a button a translucent fill that gets lighter on hover and press,
/// such as a reference label tinted with its lane color.
pub fn tinted_fill(button: Button, fill: Rgba, hover: Rgba, foreground: Role, cx: &App) -> Button {
    button
        .custom(
            ButtonCustomVariant::new(cx)
                .foreground(role_color(foreground).into())
                .color(fill.into())
                .hover(hover.into())
                .active(hover.into()),
        )
        // The library fades a custom resting fill to a fifth of its color;
        // set it directly so the button keeps the fill it was given.
        .bg(fill)
        .cursor_pointer()
}

/// Reveals a file action on row hover or keyboard focus, or keeps it always visible.
///
/// Opacity preserves the button's layout and focus target when it is hidden.
pub fn file_action(
    button: Button,
    focus: &FocusHandle,
    persistent: bool,
    window: &Window,
    cx: &App,
) -> Div {
    div()
        .flex_none()
        .opacity(if persistent || focus.contains_focused(window, cx) {
            1.
        } else {
            0.
        })
        .group_hover("file-row", |s| s.opacity(1.))
        .child(
            sized(
                button,
                if persistent {
                    ControlSize::Compact
                } else {
                    ControlSize::FileAction
                },
            )
            .w(scaled(if persistent {
                geometry::SIZE_FILE_ACTION_COARSE_TARGET
            } else {
                geometry::SIZE_FILE_ACTION_TARGET
            })),
        )
}

/// A toolbar action: an icon above a small label, or the icon alone when
/// `labelled` is false. A `caret` marks an action that offers a menu.
///
/// A disabled action is greyed out, shows a not-allowed cursor and keeps its
/// tooltip, so the reason it cannot run stays discoverable.
///
/// A `loading` action is the one Git is running: its icon becomes a spinner
/// and it keeps full strength while the other actions dim.
pub fn toolbar_command(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    glyph: &str,
    disabled: bool,
    loading: bool,
    labelled: bool,
    caret: bool,
) -> Button {
    let label = label.into();
    let disabled = disabled || loading;
    Button::new(id)
        .ghost()
        .tab_stop(true)
        .line_height(phi())
        .set_disabled(disabled)
        .when(disabled && !loading, |button| button.opacity(0.7))
        .h(scaled(42.))
        .min_w(scaled(if labelled { 46. } else { 32. }))
        .px_1()
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .map(|row| {
                            if loading {
                                row.child(spinner(px(16.)))
                            } else {
                                row.child(icon(glyph).size(px(16.)))
                            }
                        })
                        .when(caret, |row| row.child(icon("down").size(px(10.)))),
                )
                .when(labelled, |column| {
                    column.child(div().text_size(scaled(11.)).child(label))
                }),
        )
}

/// A loader that turns at a steady pace while work runs. It stands still
/// when the system asks for reduced motion.
pub fn spinner(size: Pixels) -> gpui::component::spinner::Spinner {
    gpui::component::spinner::Spinner::new()
        .icon(icon("loader"))
        .ease(|delta| delta)
        .with_size(gpui::component::Size::Size(size))
}

pub fn selector(
    id: impl Into<ElementId>,
    kicker: Option<&str>,
    value: impl Into<SharedString>,
    glyph: &str,
    width: Pixels,
    height: Pixels,
) -> Button {
    Button::new(id)
        .ghost()
        .tab_stop(true)
        .line_height(phi())
        .cursor_pointer()
        .h(height)
        .w(width)
        .min_w_0()
        .flex_none()
        .px(px(8.))
        .child(
            div()
                // Button wraps children in an intrinsic-width label. Give this
                // row a definite width so the text and caret can spread out.
                .w(width - px(16.))
                .min_w_0()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(icon(glyph).size(px(16.)).flex_none())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .items_start()
                        .when_some(kicker, |label, kicker| {
                            label.child(
                                div()
                                    .text_size(scaled(11.))
                                    .text_role(Role::TextMuted)
                                    .child(kicker.to_owned()),
                            )
                        })
                        .child(
                            div()
                                .w_full()
                                .truncate()
                                .text_size(rems(1.))
                                .child(value.into()),
                        ),
                )
                .child(icon("down").size(px(12.)).flex_none()),
        )
}

/// The Cancel button of a dialog with a custom footer. It closes the dialog,
/// which runs the dialog's `on_close` callback.
pub fn dialog_cancel(cx: &App) -> Button {
    style(
        Button::new("dialog-cancel").label("Cancel").tab_stop(true),
        ButtonKind::Secondary,
        ControlSize::Regular,
        cx,
    )
    .on_click(|_, window, cx| window.close_dialog(cx))
}

/// A right-aligned row of dialog buttons, for dialogs with a custom footer.
pub fn dialog_footer(buttons: impl IntoIterator<Item = AnyElement>) -> Div {
    div().flex().justify_end().gap_2().pt_2().children(buttons)
}

/// A dialog's confirming button, such as OK. Clicking it confirms the dialog
/// the way Enter does: the dialog's `on_ok` runs, then it closes.
pub fn dialog_ok(label: impl Into<SharedString>, kind: ButtonKind, cx: &App) -> AnyElement {
    // `DialogAction` fills its parent; this wrapper keeps it to the button's width.
    div()
        .flex_none()
        .child(gpui::component::dialog::DialogAction::new().child(style(
            Button::new("dialog-ok").label(label).tab_stop(true),
            kind,
            ControlSize::Regular,
            cx,
        )))
        .into_any_element()
}
