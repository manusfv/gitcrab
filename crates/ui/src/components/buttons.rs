//! Buttons: role palettes, placement sizes and the toolbar controls.
use crate::theme::Role;
use crate::theme::*;
use gpui::component::{
    button::{Button, ButtonCustomVariant, ButtonVariant, ButtonVariants},
    Disableable, Sizable, WindowExt,
};
use gpui::{prelude::*, *};

const COMMAND_ICON_SIZE: f32 = geometry::SIZE_REPOSITORY_TOOLBAR_COMMAND_ICON;
const SELECTOR_PADDING_X: f32 = 8.;
/// Side padding of Stage, Unstage and Discard buttons at every size.
const STAGING_PADDING_X: f32 = 10.;
/// How far button labels are raised from the line box's centre.
const LABEL_LIFT: f32 = 1.;

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
    let (h, padding, font, line) = match size {
        Regular => (geometry::SIZE_TARGET_REGULAR, 12., TEXT_SIZE_BODY, None),
        Compact => (geometry::SIZE_TARGET_COMPACT, 8., TEXT_SIZE_SMALL, None),
        DenseBulk => (
            geometry::SIZE_BULK_STAGE_HEIGHT,
            geometry::SIZE_BULK_STAGE_PADDING_X,
            geometry::SIZE_BULK_STAGE_FONT_SIZE,
            Some(geometry::SIZE_BULK_STAGE_LINE_HEIGHT),
        ),
        DenseHunk => (
            geometry::SIZE_HUNK_ACTION_HEIGHT,
            geometry::SIZE_HUNK_ACTION_PADDING_X,
            geometry::SIZE_HUNK_ACTION_FONT_SIZE,
            Some(geometry::SIZE_HUNK_ACTION_LINE_HEIGHT),
        ),
        FileAction => (
            geometry::SIZE_FILE_ACTION_TARGET,
            0.,
            TEXT_SIZE_CAPTION,
            None,
        ),
    };
    let button = button
        .small()
        .h(scaled(h))
        .px(scaled(padding))
        .text_size(scaled(font))
        .rounded(px(3.))
        // Enabled buttons show a pointer; `disabled_when` and `set_disabled`
        // swap it for not-allowed.
        .cursor_pointer()
        // The line box centres on the font's full height, which leaves the
        // letters about a pixel low, so labels are lifted to sit centred.
        .when(!matches!(size, FileAction), |button| {
            button.pt(px(0.)).pb(scaled(LABEL_LIFT * 2.))
        });
    match line {
        Some(line) => button.line_height(scaled(line)),
        // Library buttons default to a 1.25 line height; GitCrab's two-line
        // buttons (selectors, recent repositories) are laid out for GPUI's default.
        None => button.line_height(phi()),
    }
}
/// An icon for a button that also has a text label.
///
/// The button centers the icon on the label's line box, but the letters sit
/// below that box's middle, so a centered icon reads as too high. The nudge
/// lines the icon up with the letters instead.
pub fn label_icon(name: &str) -> gpui::component::Icon {
    icon(name).relative().top(px(1.))
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
    let (fg, border) = match kind {
        ButtonKind::Stage => (Role::ActionStageFg, Role::ActionStageFg),
        ButtonKind::Unstage => (Role::TextDefault, Role::BorderControl),
        ButtonKind::Discard => (Role::ActionDiscardFg, Role::ActionDiscardFg),
        _ => return basic(button, kind, size),
    };
    let border: Hsla = role_color(border).into();
    sized(button, size)
        .px(scaled(STAGING_PADDING_X))
        // The library paints a custom button's border in its color on hover,
        // and its resting fill at a fifth of that color. Using the border
        // color keeps the outline on hover over a faint tint of the same hue,
        // which hover and press deepen a little, as GitCrab's neutral
        // buttons do.
        .custom(
            ButtonCustomVariant::new(cx)
                .foreground(role_color(fg).into())
                .color(border)
                .hover(border.opacity(0.3))
                .active(border.opacity(0.4)),
        )
        // Custom variants no longer carry a border color.
        .border_1()
        .border_color(border)
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
        .h(scaled(geometry::SIZE_REPOSITORY_TOOLBAR_COMMAND_HEIGHT))
        .min_w(scaled(if labelled {
            geometry::SIZE_REPOSITORY_TOOLBAR_COMMAND_WIDTH
        } else {
            geometry::SIZE_TARGET_REGULAR
        }))
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
                                row.child(spinner(px(COMMAND_ICON_SIZE)))
                            } else {
                                row.child(icon(glyph).size(px(COMMAND_ICON_SIZE)))
                            }
                        })
                        .when(caret, |row| row.child(icon("down").size(px(CARET_SIZE)))),
                )
                .when(labelled, |column| {
                    column.child(
                        div()
                            .text_size(scaled(geometry::SIZE_REPOSITORY_TOOLBAR_COMMAND_LABEL))
                            .child(label),
                    )
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
        .px(px(SELECTOR_PADDING_X))
        .child(
            div()
                // Button wraps children in an intrinsic-width label. Give this
                // row a definite width so the text and caret can spread out.
                .w(width - px(2. * SELECTOR_PADDING_X))
                .min_w_0()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(icon(glyph).size(px(GLYPH_SIZE)).flex_none())
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
                                    .text_size(scaled(
                                        geometry::SIZE_REPOSITORY_TOOLBAR_SELECTOR_KICKER,
                                    ))
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
                .child(icon("down").size(px(geometry::SIZE_ICON_SM)).flex_none()),
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

/// The Cancel button of a dialog whose confirming action is destructive. It
/// tracks `focus`, which the caller focuses once the dialog opens, so Enter
/// and Space land on the safe choice. The ring sits in a negative margin so
/// focusing it does not shift the footer.
pub fn dialog_cancel_focused(focus: &FocusHandle, cx: &App) -> AnyElement {
    div()
        .track_focus(focus)
        .flex_none()
        .m(px(-2.))
        .rounded(px(5.))
        .border_2()
        .border_color(gpui::transparent_black())
        .focus(|s| s.border_role(Role::InteractionFocus))
        .child(dialog_cancel(cx))
        .into_any_element()
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
