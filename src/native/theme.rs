//! Semantic styling for the native GitCrab workspace.
use gpui::{prelude::*, *};
use gpui_component::Theme;
pub const CANVAS: u32 = 0x1c1f24;
pub const CHROME: u32 = 0x16191d;
pub const PANEL: u32 = 0x21252b;
pub const BORDER: u32 = 0x36393f;
pub const HOVER: u32 = 0x2a2f37;
pub const RAISED: u32 = 0x292e36;
pub const SUBTLE: u32 = 0x737c8c;
pub const TEXT: u32 = 0xeeeeef;
pub const MUTED: u32 = 0x9ca0a4;
pub const ACCENT: u32 = 0x61afef;
pub const SELECTED: u32 = 0x27394b;
pub const GREEN: u32 = 0x98c379;
pub const RED: u32 = 0xe06c75;
pub const PALETTE: [u32; 8] = [
    0x56b6c2, ACCENT, 0xc678dd, RED, 0xe5c07b, GREEN, 0xd19a66, 0xbe82d9,
];
pub fn install(cx: &mut App) {
    let t = Theme::global_mut(cx);
    t.font_family = if cfg!(windows) {
        "Segoe UI"
    } else if cfg!(target_os = "macos") {
        ".AppleSystemUIFont"
    } else {
        "DejaVu Sans"
    }
    .into();
    t.font_size = px(13.);
    t.radius = px(6.);
    t.radius_lg = px(10.);
    t.colors.background = rgb(PANEL).into();
    t.colors.foreground = rgb(TEXT).into();
    t.colors.border = rgb(BORDER).into();
    t.colors.primary = rgb(ACCENT).into();
    t.colors.input = rgb(CHROME).into();
    t.colors.popover = rgb(0x2e3238).into();
    t.colors.popover_foreground = rgb(TEXT).into();
    t.colors.secondary = rgb(0x35393e).into();
    t.colors.secondary_hover = rgb(HOVER).into();
    t.colors.list = rgb(PANEL).into();
    t.colors.list_hover = rgb(HOVER).into();
    t.colors.primary_foreground = rgb(CHROME).into();
    t.colors.primary_hover = rgb(0x76baf3).into();
    t.colors.muted_foreground = rgb(MUTED).into();
    t.colors.ring = rgb(ACCENT).into();
}
pub fn panel() -> Div {
    div()
        .flex()
        .flex_col()
        .size_full()
        .min_w_0()
        .min_h_0()
        .bg(rgb(PANEL))
        .rounded(px(10.))
        .border_1()
        .border_color(rgb(BORDER))
        .overflow_hidden()
}
pub fn heading(s: impl Into<SharedString>) -> Div {
    div()
        .h(px(32.))
        .flex_none()
        .px_3()
        .flex()
        .items_center()
        .text_color(rgb(SUBTLE))
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(s.into())
}

pub fn icon(name: &str) -> gpui_component::Icon {
    gpui_component::Icon::default().path(SharedString::from(format!("icons/{name}.svg")))
}

pub fn badge(label: impl Into<SharedString>, color: u32) -> Div {
    div()
        .flex_none()
        .px_2()
        .py(px(2.))
        .rounded(px(4.))
        .bg(rgba((color << 8) | 0x18))
        .text_color(rgb(color))
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .child(label.into())
}

pub fn separator() -> Div {
    div()
        .w(px(1.))
        .h(px(22.))
        .mx_2()
        .flex_none()
        .bg(rgb(BORDER))
}
