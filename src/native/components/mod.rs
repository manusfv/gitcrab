//! P01/P02: native controls retain gpui-component interaction/focus behavior.
pub mod rail;
use super::theme::*;
use gpui::{prelude::*, *};
use gpui_component::{
    button::{Button, ButtonCustomVariant, ButtonVariants},
    Sizable,
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
        .h(rems(h / 13.))
        .px(rems(padding / 13.))
        .text_size(rems(font / 13.))
        .rounded(px(3.))
}
pub fn semantic_action(button: Button, size: ControlSize, destructive: bool, cx: &App) -> Button {
    let (fg, bg) = if destructive {
        (Role::ActionUnstageFg, Role::ActionUnstageBg)
    } else {
        (Role::ActionStageFg, Role::ActionStageBg)
    };
    sized(button, size).custom(
        ButtonCustomVariant::new(cx)
            .foreground(rgb(semantic(fg)).into())
            .color(rgb(semantic(bg)).into())
            .border(rgb(semantic(fg)).into())
            .hover(rgb(semantic(bg)).into())
            .active(rgb(semantic(bg)).into()),
    )
}

/// The focus scope contains the stock Button; opacity never removes its target.
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
            .w(rems(if persistent { 28. / 13. } else { 18. / 13. })),
        )
}

pub fn toolbar_command(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    glyph: &str,
) -> Button {
    let label = label.into();
    Button::new(id)
        .ghost()
        .tab_stop(true)
        .h(rems(42. / 13.))
        .min_w(rems(46. / 13.))
        .px_1()
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .child(icon(glyph).size(px(16.)))
                .child(div().text_size(rems(11. / 13.)).child(label)),
        )
}
pub fn selector(
    id: impl Into<ElementId>,
    kicker: &str,
    value: impl Into<SharedString>,
    glyph: &str,
) -> Button {
    Button::new(id)
        .ghost()
        .tab_stop(true)
        .h(rems(42. / 13.))
        .w(rems(176. / 13.))
        .min_w_0()
        .px(rems(10. / 13.))
        .child(
            div()
                // Button wraps children in an intrinsic-width label. Give this
                // row a definite width so the text and caret can spread out.
                .w(rems(154. / 13.))
                .min_w_0()
                .flex()
                .items_center()
                .justify_between()
                .gap(rems(16. / 13.))
                .child(icon(glyph).size(px(16.)).flex_none())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .items_start()
                        .child(
                            div()
                                .text_size(rems(11. / 13.))
                                .text_color(rgb(semantic(Role::TextMuted)))
                                .child(kicker.to_owned()),
                        )
                        .child(
                            div()
                                .w_full()
                                .truncate()
                                .text_size(rems(13. / 13.))
                                .child(value.into()),
                        ),
                )
                .child(icon("down").size(px(12.)).flex_none()),
        )
}
