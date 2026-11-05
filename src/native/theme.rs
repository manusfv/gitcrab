//! Shared geometry and semantic colors for the native workspace.
pub use gitcrab_core::design_tokens::{geometry, Role};
use gpui::{prelude::*, *};
use gpui_component::Theme;
use std::sync::atomic::{AtomicBool, Ordering};
static VALIDATION_LIGHT: AtomicBool = AtomicBool::new(false);
pub fn semantic(role: Role) -> u32 {
    gitcrab_core::appearance::resolve(
        if VALIDATION_LIGHT.load(Ordering::Relaxed) {
            "light"
        } else {
            "dark"
        },
        role,
    )
}
pub fn lanes() -> [u32; 8] {
    [
        Role::GraphLane1,
        Role::GraphLane2,
        Role::GraphLane3,
        Role::GraphLane4,
        Role::GraphLane5,
        Role::GraphLane6,
        Role::GraphLane7,
        Role::GraphLane8,
    ]
    .map(semantic)
}
pub fn file_status_color(status: char) -> u32 {
    match status {
        'A' | '?' => semantic(Role::StatusSuccessFg),
        'D' | 'U' => semantic(Role::StatusErrorFg),
        'M' | 'T' => semantic(Role::StatusWarningFg),
        'R' | 'C' => semantic(Role::StatusInfoFg),
        _ => semantic(Role::TextMuted),
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
pub const TOOLBAR_HEIGHT: f32 = geometry::SIZE_SHELL_TOOLBAR;
pub const SECTION_HEIGHT: f32 = 36.;
pub const FILE_ROW_HEIGHT: f32 = geometry::SIZE_FILE_ACTION_ROW;
pub const INSPECTOR_INSET: f32 = geometry::SIZE_BULK_STAGE_INSPECTOR_INSET;
pub const CONTROL_RADIUS: f32 = 4.;
pub const GLYPH_SIZE: f32 = 16.;
pub const SECTION_INSET: f32 = 12.;
pub const SEAM: f32 = 1.;
pub fn type_size(cx: &App, base: f32) -> Pixels {
    px(f32::from(Theme::global(cx).font_size) * base / 13.)
}
pub const REF_BADGE_HEIGHT: f32 = 20.;
pub const REF_TINT: u8 = 0x1f;

pub fn branch_tint(color: usize, opacity: u8) -> Rgba {
    tint(lanes()[color % lanes().len()], opacity)
}

pub fn initialize(cx: &mut App) {
    #[cfg(feature = "native-evidence")]
    VALIDATION_LIGHT.store(
        std::env::var("GITCRAB_TEST_PALETTE").as_deref() == Ok("light"),
        Ordering::Relaxed,
    );
    install(cx);
}
#[cfg(feature = "native-evidence")]
pub fn toggle_validation_palette(cx: &mut App) {
    VALIDATION_LIGHT.fetch_xor(true, Ordering::Relaxed);
    install(cx);
    cx.refresh_windows();
}
pub fn install(cx: &mut App) {
    let font_size = Theme::global(cx).font_size;
    gpui_component::Theme::change(
        if VALIDATION_LIGHT.load(Ordering::Relaxed) {
            gpui_component::ThemeMode::Light
        } else {
            gpui_component::ThemeMode::Dark
        },
        None,
        cx,
    );
    let t = Theme::global_mut(cx);
    t.font_family = if cfg!(windows) {
        "Segoe UI"
    } else if cfg!(target_os = "macos") {
        ".AppleSystemUIFont"
    } else {
        "DejaVu Sans"
    }
    .into();
    t.font_size = font_size;
    t.radius = px(CONTROL_RADIUS);
    t.radius_lg = px(10.);
    t.colors.background = rgb(semantic(Role::SurfacePanel)).into();
    t.colors.foreground = rgb(semantic(Role::TextDefault)).into();
    // Resting control outlines share the quiet pane boundary. Focus and
    // semantic actions supply their own stronger, meaningful outlines.
    t.colors.border = rgb(semantic(Role::BorderDivider)).into();
    t.colors.primary = rgb(semantic(Role::ActionPrimary)).into();
    t.colors.input = rgb(semantic(Role::BorderDivider)).into();
    t.colors.popover = rgb(semantic(Role::SurfaceOverlay)).into();
    t.colors.popover_foreground = rgb(semantic(Role::TextDefault)).into();
    t.colors.secondary = rgb(semantic(Role::SurfaceOverlay)).into();
    t.colors.secondary_hover = rgb(semantic(Role::InteractionHover)).into();
    t.colors.list = rgb(semantic(Role::SurfacePanel)).into();
    t.colors.list_hover = rgb(semantic(Role::InteractionHover)).into();
    t.colors.primary_foreground = rgb(semantic(Role::TextOnPrimary)).into();
    t.colors.primary_hover = rgb(semantic(Role::ActionPrimaryHover)).into();
    t.colors.muted_foreground = rgb(semantic(Role::TextMuted)).into();
    t.colors.ring = rgb(semantic(Role::InteractionFocus)).into();
    t.colors.accent = rgb(semantic(Role::InteractionHover)).into();
    t.colors.accent_foreground = rgb(semantic(Role::TextDefault)).into();
    t.colors.caret = rgb(semantic(Role::ActionPrimary)).into();
    t.colors.selection = rgb(semantic(Role::InteractionSelected)).into();
    t.colors.secondary_active = rgb(semantic(Role::InteractionSelected)).into();
    t.colors.secondary_foreground = rgb(semantic(Role::TextDefault)).into();
    t.colors.primary_active = rgb(semantic(Role::ActionPrimaryPressed)).into();
    t.colors.danger = rgb(semantic(Role::StatusErrorFg)).into();
    t.colors.danger_foreground = rgb(semantic(Role::SurfaceCanvas)).into();
    t.colors.danger_hover = rgb(semantic(Role::StatusErrorFg)).into();
    t.colors.danger_active = rgb(semantic(Role::StatusErrorFg)).into();
    t.colors.list_active = rgb(semantic(Role::InteractionSelected)).into();
    t.colors.list_active_border = rgb(semantic(Role::ActionPrimary)).into();
    t.colors.success = rgb(semantic(Role::StatusSuccessFg)).into();
    t.colors.success_foreground = rgb(semantic(Role::StatusSuccessBg)).into();
    t.colors.warning = rgb(semantic(Role::StatusWarningFg)).into();
    t.colors.warning_foreground = rgb(semantic(Role::StatusWarningBg)).into();
    t.colors.overlay = rgba(semantic(Role::OverlayScrim)).into();
    t.colors.scrollbar_thumb = rgb(semantic(Role::BorderControl)).into();
    t.colors.scrollbar_thumb_hover = rgb(semantic(Role::TextSecondary)).into();
    t.colors.accordion = rgb(semantic(Role::SurfacePanel)).into();
    t.colors.accordion_hover = rgb(semantic(Role::InteractionHover)).into();
    t.colors.group_box = rgb(semantic(Role::SurfacePanel)).into();
    t.colors.group_box_foreground = rgb(semantic(Role::TextDefault)).into();
    t.colors.chart_1 = rgb(semantic(Role::GraphLane1)).into();
    t.colors.chart_2 = rgb(semantic(Role::GraphLane2)).into();
    t.colors.chart_3 = rgb(semantic(Role::GraphLane3)).into();
    t.colors.chart_4 = rgb(semantic(Role::GraphLane4)).into();
    t.colors.chart_5 = rgb(semantic(Role::GraphLane5)).into();
    t.colors.description_list_label = rgb(semantic(Role::SurfacePanel)).into();
    t.colors.description_list_label_foreground = rgb(semantic(Role::TextDefault)).into();
    t.colors.drag_border = rgb(semantic(Role::BorderDivider)).into();
    t.colors.drop_target = rgb(semantic(Role::SurfacePanel)).into();
    t.colors.info = rgb(semantic(Role::StatusInfoFg)).into();
    t.colors.info_active = rgb(semantic(Role::StatusInfoFg)).into();
    t.colors.info_foreground = rgb(semantic(Role::StatusInfoBg)).into();
    t.colors.info_hover = rgb(semantic(Role::StatusInfoFg)).into();
    t.colors.link = rgb(semantic(Role::ActionPrimary)).into();
    t.colors.link_active = rgb(semantic(Role::InteractionSelected)).into();
    t.colors.link_hover = rgb(semantic(Role::InteractionHover)).into();
    t.colors.list_even = rgb(semantic(Role::SurfacePanel)).into();
    t.colors.list_head = rgb(semantic(Role::SurfacePanel)).into();
    t.colors.muted = rgb(semantic(Role::SurfaceChrome)).into();
    t.colors.progress_bar = rgb(semantic(Role::ActionPrimary)).into();
    t.colors.scrollbar = rgb(semantic(Role::SurfacePanel)).into();
    t.colors.sidebar = rgb(semantic(Role::SurfaceNav)).into();
    t.colors.sidebar_accent = rgb(semantic(Role::InteractionSelected)).into();
    t.colors.sidebar_accent_foreground = rgb(semantic(Role::TextDefault)).into();
    t.colors.sidebar_border = rgb(semantic(Role::BorderDivider)).into();
    t.colors.sidebar_foreground = rgb(semantic(Role::TextDefault)).into();
    t.colors.sidebar_primary = rgb(semantic(Role::ActionPrimary)).into();
    t.colors.sidebar_primary_foreground = rgb(semantic(Role::TextDefault)).into();
    t.colors.skeleton = rgb(semantic(Role::SurfaceChrome)).into();
    t.colors.slider_bar = rgb(semantic(Role::ActionPrimary)).into();
    t.colors.slider_thumb = rgb(semantic(Role::ActionPrimary)).into();
    t.colors.success_hover = rgb(semantic(Role::StatusSuccessFg)).into();
    t.colors.success_active = rgb(semantic(Role::StatusSuccessFg)).into();
    t.colors.bullish = rgb(semantic(Role::StatusSuccessFg)).into();
    t.colors.bearish = rgb(semantic(Role::StatusErrorFg)).into();
    t.colors.switch = rgb(semantic(Role::SurfaceChrome)).into();
    t.colors.switch_thumb = rgb(semantic(Role::TextSecondary)).into();
    t.colors.tab = rgb(semantic(Role::SurfacePanel)).into();
    t.colors.tab_active = rgb(semantic(Role::InteractionSelected)).into();
    t.colors.tab_active_foreground = rgb(semantic(Role::TextDefault)).into();
    t.colors.tab_bar = rgb(semantic(Role::SurfaceChrome)).into();
    t.colors.tab_bar_segmented = rgb(semantic(Role::SurfaceChrome)).into();
    t.colors.tab_foreground = rgb(semantic(Role::TextDefault)).into();
    t.colors.table = rgb(semantic(Role::SurfacePanel)).into();
    t.colors.table_active = rgb(semantic(Role::InteractionSelected)).into();
    t.colors.table_active_border = rgb(semantic(Role::BorderDivider)).into();
    t.colors.table_even = rgb(semantic(Role::SurfacePanel)).into();
    t.colors.table_head = rgb(semantic(Role::SurfacePanel)).into();
    t.colors.table_head_foreground = rgb(semantic(Role::TextDefault)).into();
    t.colors.table_hover = rgb(semantic(Role::InteractionHover)).into();
    t.colors.table_row_border = rgb(semantic(Role::BorderDivider)).into();
    t.colors.title_bar = rgb(semantic(Role::SurfacePanel)).into();
    t.colors.title_bar_border = rgb(semantic(Role::BorderDivider)).into();
    t.colors.tiles = rgb(semantic(Role::SurfacePanel)).into();
    t.colors.warning_active = rgb(semantic(Role::StatusWarningFg)).into();
    t.colors.warning_hover = rgb(semantic(Role::StatusWarningFg)).into();
    t.colors.window_border = rgb(semantic(Role::BorderDivider)).into();
    t.colors.red = rgb(semantic(Role::StatusErrorFg)).into();
    t.colors.red_light = rgb(semantic(Role::StatusErrorBg)).into();
    t.colors.green = rgb(semantic(Role::StatusSuccessFg)).into();
    t.colors.green_light = rgb(semantic(Role::StatusSuccessBg)).into();
    t.colors.blue = rgb(semantic(Role::StatusInfoFg)).into();
    t.colors.blue_light = rgb(semantic(Role::StatusInfoBg)).into();
    t.colors.yellow = rgb(semantic(Role::StatusWarningFg)).into();
    t.colors.yellow_light = rgb(semantic(Role::StatusWarningBg)).into();
    t.colors.magenta = rgb(semantic(Role::StatusConflictFg)).into();
    t.colors.magenta_light = rgb(semantic(Role::StatusConflictBg)).into();
    t.colors.cyan = rgb(semantic(Role::StatusInfoFg)).into();
    t.colors.cyan_light = rgb(semantic(Role::StatusInfoBg)).into();
}
pub fn panel() -> Div {
    div()
        .flex()
        .flex_col()
        .size_full()
        .min_w_0()
        .min_h_0()
        .bg(rgb(semantic(Role::SurfacePanel)))
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
        .bg(rgb(semantic(Role::SurfaceChrome)))
        .border_b_1()
        .border_color(rgb(semantic(Role::BorderDivider)))
}

pub fn segmented_group() -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .rounded(px(CONTROL_RADIUS))
        .border_1()
        .border_color(rgb(semantic(Role::BorderDivider)))
        .bg(rgb(semantic(Role::SurfaceChrome)))
}

pub fn heading(s: impl Into<SharedString>) -> Div {
    div()
        .h(px(30.))
        .flex_none()
        .px_3()
        .flex()
        .items_center()
        .text_color(rgb(semantic(Role::TextSecondary)))
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
        .bg(rgb(semantic(Role::BorderDivider)))
}
