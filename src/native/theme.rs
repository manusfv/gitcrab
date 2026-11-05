//! Shared geometry and semantic colors for the native workspace.
use gpui::{prelude::*, *};
use gpui_component::Theme;
// Surfaces, from the recessed workspace to raised overlays.
pub const CANVAS: u32 = 0x181b20;
pub const CHROME: u32 = 0x21262d;
pub const PANEL: u32 = 0x242a32;
pub const SIDEBAR: u32 = 0x2b313a;
pub const RAISED: u32 = 0x343c47;
pub const BORDER: u32 = 0x424b57;
pub const CONTROL_BORDER: u32 = 0x838e9c;

pub const TEXT: u32 = 0xeeeeef;
pub const SUBTLE: u32 = 0xc0c9d4;
pub const MUTED: u32 = 0xb4bfcc;
pub const ACCENT: u32 = 0x72d6e5;
pub const ACCENT_HOVER: u32 = 0x9ce5ef;
pub const ACCENT_PRESSED: u32 = 0x58b7c8;
pub const HOVER: u32 = 0x39434f;
pub const GRAPH_HOVER: u32 = 0x252c35;
pub const SELECTED: u32 = 0x304b59;

pub const POSITIVE: u32 = 0xa8db9c;
pub const DANGER: u32 = 0xffa4ad;
pub const WARNING: u32 = 0xf4ca84;
pub const INFO: u32 = 0xa7cdff;
pub const DIFF_ADDED: u32 = 0x23382d;
pub const DIFF_REMOVED: u32 = 0x422c35;
pub const DIFF_HUNK: u32 = 0x293440;
pub const SYNTAX_STRING: u32 = 0xb4dfba;
pub const SYNTAX_KEYWORD: u32 = 0xdcb8f5;
pub const SYNTAX_NUMBER: u32 = 0xf1c19e;
pub const OVERLAY: u32 = 0x00000099;

pub fn file_status_color(status: char) -> u32 {
    match status {
        'A' | '?' => POSITIVE,
        'D' | 'U' => DANGER,
        'M' | 'T' => WARNING,
        'R' | 'C' => INFO,
        _ => MUTED,
    }
}

pub fn tint(color: u32, opacity: u8) -> Rgba {
    rgba((color << 8) | u32::from(opacity))
}

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
pub fn type_size(cx: &App, base: f32) -> Pixels {
    px(f32::from(Theme::global(cx).font_size) * base / 13.)
}
pub const PALETTE: [u32; 8] = [
    0x73cdff, 0xffff73, 0x9b91ff, 0xb9eba5, 0xf596b0, 0xc3cd41, 0x91ffff, 0xb99b5f,
];
pub const REF_BADGE_HEIGHT: f32 = 20.;
pub const REF_TINT: u8 = 0x2c;

pub fn branch_tint(color: usize, opacity: u8) -> Rgba {
    tint(PALETTE[color % PALETTE.len()], opacity)
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
    t.colors.border = rgb(CONTROL_BORDER).into();
    t.colors.primary = rgb(ACCENT).into();
    t.colors.input = rgb(CONTROL_BORDER).into();
    t.colors.popover = rgb(RAISED).into();
    t.colors.popover_foreground = rgb(TEXT).into();
    t.colors.secondary = rgb(RAISED).into();
    t.colors.secondary_hover = rgb(HOVER).into();
    t.colors.list = rgb(PANEL).into();
    t.colors.list_hover = rgb(HOVER).into();
    t.colors.primary_foreground = rgb(CHROME).into();
    t.colors.primary_hover = rgb(ACCENT_HOVER).into();
    t.colors.muted_foreground = rgb(MUTED).into();
    t.colors.ring = rgb(ACCENT).into();
    t.colors.accent = rgb(HOVER).into();
    t.colors.accent_foreground = rgb(TEXT).into();
    t.colors.caret = rgb(ACCENT).into();
    t.colors.selection = rgb(SELECTED).into();
    t.colors.secondary_active = rgb(SELECTED).into();
    t.colors.secondary_foreground = rgb(TEXT).into();
    t.colors.primary_active = rgb(ACCENT_PRESSED).into();
    t.colors.danger = rgb(DANGER).into();
    t.colors.danger_foreground = rgb(CANVAS).into();
    t.colors.danger_hover = rgb(DANGER).into();
    t.colors.danger_active = rgb(DANGER).into();
    t.colors.list_active = rgb(SELECTED).into();
    t.colors.list_active_border = rgb(ACCENT).into();
    t.colors.overlay = rgba(OVERLAY).into();
    t.colors.scrollbar_thumb = rgb(CONTROL_BORDER).into();
    t.colors.scrollbar_thumb_hover = rgb(SUBTLE).into();
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
        .bg(tint(color, 0x18))
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
