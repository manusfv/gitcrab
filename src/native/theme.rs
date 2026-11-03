//! Shared geometry and semantic colors for the native workspace.
use gpui::{prelude::*, *};
use gpui_component::Theme;
pub const CANVAS: u32 = 0x1c1f24;
pub const CHROME: u32 = 0x16191d;
pub const PANEL: u32 = 0x21252b;
pub const BORDER: u32 = 0x36393f;
pub const HOVER: u32 = 0x2a2f37;
pub const RAISED: u32 = 0x292e36;
pub const SUBTLE: u32 = 0x9aa3b2;
pub const TEXT: u32 = 0xeeeeef;
pub const MUTED: u32 = 0x9ca0a4;
pub const ACCENT: u32 = 0x61afef;
pub const SELECTED: u32 = 0x303b49;
pub const GREEN: u32 = 0x98c379;
pub const RED: u32 = 0xe06c75;
pub const DIFF_ADDED: u32 = 0x293a30;
pub const DIFF_REMOVED: u32 = 0x402c34;
pub const CODE_FONT: &str = if cfg!(windows) {
    "Consolas"
} else if cfg!(target_os = "macos") {
    "Menlo"
} else {
    "DejaVu Sans Mono"
};

pub const CONTROL_HEIGHT: f32 = 28.;
pub const TOOLBAR_HEIGHT: f32 = 48.;
pub const SECTION_HEIGHT: f32 = 36.;
pub const FILE_ROW_HEIGHT: f32 = CONTROL_HEIGHT;
pub const INSPECTOR_INSET: f32 = 8.;
pub const CONTROL_RADIUS: f32 = 4.;
pub const GLYPH_SIZE: f32 = 16.;
pub const SECTION_INSET: f32 = 12.;
pub const SEAM: f32 = 1.;
pub const WARNING: u32 = 0xe5c07b;
pub const SYNTAX_KEYWORD: u32 = 0xc678dd;
pub const SYNTAX_NUMBER: u32 = 0xd19a66;
pub const OVERLAY: u32 = 0x00000099;
pub const DIFF_HUNK: u32 = 0x293440;
pub fn type_size(cx: &App, base: f32) -> Pixels {
    px(f32::from(Theme::global(cx).font_size) * base / 13.)
}
pub const PALETTE: [u32; 8] = [
    0x56b6c2, ACCENT, 0xc678dd, RED, 0xe5c07b, GREEN, 0xd19a66, 0xbe82d9,
];
pub const REF_BADGE_HEIGHT: f32 = 20.;
pub const REF_TINT: u8 = 0x2c;

pub fn branch_tint(color: usize, opacity: u8) -> Rgba {
    rgba((PALETTE[color % PALETTE.len()] << 8) | u32::from(opacity))
}

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
    t.radius = px(CONTROL_RADIUS);
    t.radius_lg = px(10.);
    t.colors.background = rgb(PANEL).into();
    t.colors.foreground = rgb(TEXT).into();
    t.colors.border = rgb(BORDER).into();
    t.colors.primary = rgb(ACCENT).into();
    t.colors.input = rgb(CHROME).into();
    t.colors.popover = rgb(0x2e3238).into();
    t.colors.popover_foreground = rgb(TEXT).into();
    t.colors.secondary = rgb(RAISED).into();
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
        .overflow_hidden()
}
pub fn section_bar() -> Div {
    div()
        .h(px(SECTION_HEIGHT))
        .flex_none()
        .flex()
        .items_center()
        .gap_2()
        .px(px(SECTION_INSET))
        .bg(rgb(CHROME))
        .border_b_1()
        .border_color(rgb(BORDER))
}

pub fn segmented_group() -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .rounded(px(CONTROL_RADIUS))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(CHROME))
}

pub fn heading(s: impl Into<SharedString>) -> Div {
    div()
        .h(px(30.))
        .flex_none()
        .px_3()
        .flex()
        .items_center()
        .text_color(rgb(SUBTLE))
        .text_size(rems(12. / 13.))
        .font_weight(FontWeight::MEDIUM)
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
        .text_size(rems(12. / 13.))
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
