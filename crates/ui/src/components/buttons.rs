//! Buttons: role palettes, placement sizes and the toolbar controls.
use crate::theme::*;
use gpui::{prelude::*, *};
use gpui_component::{
    button::{Button, ButtonCustomVariant, ButtonVariant, ButtonVariants},
    Disableable, Sizable,
};

#[derive(Clone, Copy)]
pub enum ControlSize {
    Regular,
    Compact,
    DenseBulk,
    DenseHunk,
    TabClose,
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
        TabClose => (geometry::SIZE_REPOSITORY_TAB_CLOSE_TARGET, 8., 16.),
        FileAction => (geometry::SIZE_FILE_ACTION_TARGET, 0., 11.),
    };
    button
        .small()
        .h(scaled(h))
        .px(scaled(padding))
        .text_size(scaled(font))
        .rounded(px(3.))
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
        ButtonKind::Primary => button.primary().text_role(Role::SurfaceChrome),
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
}

impl ButtonExt for Button {
    fn disabled_when(self, disabled: bool) -> Self {
        let button = Disableable::disabled(self, disabled);
        if !disabled {
            return button;
        }
        button
            .bg_role(Role::SurfaceDisabled)
            .text_role(Role::TextDisabled)
            .border_role(Role::BorderDivider)
            .cursor_not_allowed()
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
    sized(button, size).custom(
        ButtonCustomVariant::new(cx)
            .foreground(role_color(fg).into())
            .color(role_color(bg).into())
            .border(role_color(border).into())
            .hover(role_color(hover).into())
            .active(role_color(bg).into()),
    )
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
pub fn toolbar_command(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    glyph: &str,
    disabled: bool,
    labelled: bool,
    caret: bool,
) -> Button {
    let label = label.into();
    Button::new(id)
        .ghost()
        .tab_stop(true)
        .disabled(disabled)
        .when(disabled, |button| button.cursor_not_allowed().opacity(0.7))
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
                        .child(icon(glyph).size(px(16.)))
                        .when(caret, |row| row.child(icon("down").size(px(10.)))),
                )
                .when(labelled, |column| {
                    column.child(div().text_size(scaled(11.)).child(label))
                }),
        )
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
