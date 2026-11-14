//! Shared geometry and semantic colors for the native workspace.
pub use gitcrab_core::design_tokens::{geometry, Role};
use gpui::component::Theme;
use gpui::{prelude::*, *};
use std::sync::atomic::{AtomicBool, Ordering};
static VALIDATION_LIGHT: AtomicBool = AtomicBool::new(false);
/// Returns the packed RGB color for a role in the active native palette.
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
/// Returns the color for a role in the active native palette.
pub fn role_color(role: Role) -> Rgba {
    rgb(semantic(role))
}

/// Styles an element with palette roles instead of raw colors.
pub trait RoleStyled: Styled + Sized {
    /// Sets the text color to a role.
    fn text_role(self, role: Role) -> Self {
        self.text_color(role_color(role))
    }
    /// Sets the background to a role.
    fn bg_role(self, role: Role) -> Self {
        self.bg(role_color(role))
    }
    /// Sets the border color to a role.
    fn border_role(self, role: Role) -> Self {
        self.border_color(role_color(role))
    }
}
impl<T: Styled> RoleStyled for T {}

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
/// Scales a size given in pixels at the default 13-pixel font to the current
/// font size.
///
/// The root view sets the rem to the font size, so every length built with
/// this grows and shrinks with the font-size preference. Use it for padding,
/// heights and gaps; keep `px` for hairlines, radii and measured widths.
pub fn scaled(base: f32) -> Rems {
    rems(base / 13.)
}
/// The pixel value of [`scaled`], for layout arithmetic.
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
/// Installs GitCrab's colors into the component theme while preserving the font size.
pub fn install(cx: &mut App) {
    let font_size = Theme::global(cx).font_size;
    gpui::component::Theme::change(
        if VALIDATION_LIGHT.load(Ordering::Relaxed) {
            gpui::component::ThemeMode::Light
        } else {
            gpui::component::ThemeMode::Dark
        },
        None,
        cx,
    );
    // `update` keeps the component tokens and the Base layer in step with
    // these colors; `global_mut` would leave them on the previous palette.
    Theme::update(cx, |t| {
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
        t.colors.background = role_color(Role::SurfacePanel).into();
        t.colors.foreground = role_color(Role::TextDefault).into();
        // Resting control outlines share the quiet pane boundary. Focus and
        // semantic actions supply their own stronger, meaningful outlines.
        t.colors.border = role_color(Role::BorderDivider).into();
        t.colors.primary = role_color(Role::ActionPrimary).into();
        t.colors.input = role_color(Role::BorderDivider).into();
        t.colors.popover = role_color(Role::SurfaceOverlay).into();
        t.colors.popover_foreground = role_color(Role::TextDefault).into();
        t.colors.secondary = role_color(Role::SurfaceOverlay).into();
        t.colors.secondary_hover = role_color(Role::InteractionHover).into();
        t.colors.list = role_color(Role::SurfacePanel).into();
        t.colors.list_hover = role_color(Role::InteractionHover).into();
        t.colors.primary_foreground = role_color(Role::TextOnPrimary).into();
        t.colors.primary_hover = role_color(Role::ActionPrimaryHover).into();
        t.colors.muted_foreground = role_color(Role::TextMuted).into();
        t.colors.ring = role_color(Role::InteractionFocus).into();
        t.colors.accent = role_color(Role::InteractionHover).into();
        t.colors.accent_foreground = role_color(Role::TextDefault).into();
        t.colors.caret = role_color(Role::ActionPrimary).into();
        t.colors.selection = role_color(Role::InteractionSelected).into();
        t.colors.secondary_active = role_color(Role::InteractionSelected).into();
        t.colors.secondary_foreground = role_color(Role::TextDefault).into();
        t.colors.primary_active = role_color(Role::ActionPrimaryPressed).into();
        t.colors.danger = role_color(Role::StatusErrorFg).into();
        t.colors.danger_foreground = role_color(Role::SurfaceCanvas).into();
        t.colors.danger_hover = role_color(Role::StatusErrorFg).into();
        t.colors.danger_active = role_color(Role::StatusErrorFg).into();
        t.colors.list_active = role_color(Role::InteractionSelected).into();
        t.colors.list_active_border = role_color(Role::ActionPrimary).into();
        t.colors.success = role_color(Role::StatusSuccessFg).into();
        t.colors.success_foreground = role_color(Role::StatusSuccessBg).into();
        t.colors.warning = role_color(Role::StatusWarningFg).into();
        t.colors.warning_foreground = role_color(Role::StatusWarningBg).into();
        t.colors.overlay = rgba(semantic(Role::OverlayScrim)).into();
        t.colors.scrollbar_thumb = role_color(Role::BorderControl).into();
        t.colors.scrollbar_thumb_hover = role_color(Role::TextSecondary).into();
        t.colors.accordion = role_color(Role::SurfacePanel).into();
        t.colors.group_box = role_color(Role::SurfacePanel).into();
        t.colors.group_box_foreground = role_color(Role::TextDefault).into();
        t.colors.chart_1 = role_color(Role::GraphLane1).into();
        t.colors.chart_2 = role_color(Role::GraphLane2).into();
        t.colors.chart_3 = role_color(Role::GraphLane3).into();
        t.colors.chart_4 = role_color(Role::GraphLane4).into();
        t.colors.chart_5 = role_color(Role::GraphLane5).into();
        t.colors.description_list_label = role_color(Role::SurfacePanel).into();
        t.colors.description_list_label_foreground = role_color(Role::TextDefault).into();
        t.colors.drag_border = role_color(Role::BorderDivider).into();
        t.colors.drop_target = role_color(Role::SurfacePanel).into();
        t.colors.info = role_color(Role::StatusInfoFg).into();
        t.colors.info_active = role_color(Role::StatusInfoFg).into();
        t.colors.info_foreground = role_color(Role::StatusInfoBg).into();
        t.colors.info_hover = role_color(Role::StatusInfoFg).into();
        t.colors.link = role_color(Role::ActionPrimary).into();
        t.colors.link_active = role_color(Role::InteractionSelected).into();
        t.colors.link_hover = role_color(Role::InteractionHover).into();
        t.colors.list_even = role_color(Role::SurfacePanel).into();
        t.colors.list_head = role_color(Role::SurfacePanel).into();
        t.colors.muted = role_color(Role::SurfaceChrome).into();
        t.colors.progress_bar = role_color(Role::ActionPrimary).into();
        t.colors.scrollbar = role_color(Role::SurfacePanel).into();
        t.colors.sidebar = role_color(Role::SurfaceNav).into();
        t.colors.sidebar_accent = role_color(Role::InteractionSelected).into();
        t.colors.sidebar_accent_foreground = role_color(Role::TextDefault).into();
        t.colors.sidebar_border = role_color(Role::BorderDivider).into();
        t.colors.sidebar_foreground = role_color(Role::TextDefault).into();
        t.colors.sidebar_primary = role_color(Role::ActionPrimary).into();
        t.colors.sidebar_primary_foreground = role_color(Role::TextDefault).into();
        t.colors.skeleton = role_color(Role::SurfaceChrome).into();
        t.colors.slider_bar = role_color(Role::ActionPrimary).into();
        t.colors.slider_thumb = role_color(Role::ActionPrimary).into();
        t.colors.success_hover = role_color(Role::StatusSuccessFg).into();
        t.colors.success_active = role_color(Role::StatusSuccessFg).into();
        t.colors.chart_bullish = role_color(Role::StatusSuccessFg).into();
        t.colors.chart_bearish = role_color(Role::StatusErrorFg).into();
        t.colors.switch = role_color(Role::SurfaceChrome).into();
        t.colors.switch_thumb = role_color(Role::TextSecondary).into();
        t.colors.tab = role_color(Role::SurfacePanel).into();
        t.colors.tab_active = role_color(Role::InteractionSelected).into();
        t.colors.tab_active_foreground = role_color(Role::TextDefault).into();
        t.colors.tab_bar = role_color(Role::SurfaceChrome).into();
        t.colors.tab_bar_segmented = role_color(Role::SurfaceChrome).into();
        t.colors.tab_foreground = role_color(Role::TextDefault).into();
        t.colors.table = role_color(Role::SurfacePanel).into();
        t.colors.table_active = role_color(Role::InteractionSelected).into();
        t.colors.table_active_border = role_color(Role::BorderDivider).into();
        t.colors.table_even = role_color(Role::SurfacePanel).into();
        t.colors.table_head = role_color(Role::SurfacePanel).into();
        t.colors.table_head_foreground = role_color(Role::TextDefault).into();
        t.colors.table_hover = role_color(Role::InteractionHover).into();
        t.colors.table_row_border = role_color(Role::BorderDivider).into();
        t.colors.title_bar = role_color(Role::SurfacePanel).into();
        t.colors.title_bar_border = role_color(Role::BorderDivider).into();
        t.colors.warning_active = role_color(Role::StatusWarningFg).into();
        t.colors.warning_hover = role_color(Role::StatusWarningFg).into();
        t.colors.window_border = role_color(Role::BorderDivider).into();
        t.colors.red = role_color(Role::StatusErrorFg).into();
        t.colors.red_light = role_color(Role::StatusErrorBg).into();
        t.colors.green = role_color(Role::StatusSuccessFg).into();
        t.colors.green_light = role_color(Role::StatusSuccessBg).into();
        t.colors.blue = role_color(Role::StatusInfoFg).into();
        t.colors.blue_light = role_color(Role::StatusInfoBg).into();
        t.colors.yellow = role_color(Role::StatusWarningFg).into();
        t.colors.yellow_light = role_color(Role::StatusWarningBg).into();
        t.colors.magenta = role_color(Role::StatusConflictFg).into();
        t.colors.magenta_light = role_color(Role::StatusConflictBg).into();
        t.colors.cyan = role_color(Role::StatusInfoFg).into();
        t.colors.cyan_light = role_color(Role::StatusInfoBg).into();
        t.colors.chart_grid = role_color(Role::BorderDivider).into();
        t.colors.status_bar = role_color(Role::SurfaceChrome).into();
        t.colors.status_bar_border = role_color(Role::BorderDivider).into();
        t.colors.table_foot = role_color(Role::SurfacePanel).into();
        t.colors.table_foot_foreground = role_color(Role::TextDefault).into();
        // Library buttons read their own colors; they no longer fall back to
        // primary, secondary and the status colors.
        t.colors.button = role_color(Role::SurfaceOverlay).into();
        t.colors.button_hover = role_color(Role::InteractionHover).into();
        t.colors.button_active = role_color(Role::InteractionSelected).into();
        t.colors.button_foreground = role_color(Role::TextDefault).into();
        t.colors.button_secondary = role_color(Role::SurfaceOverlay).into();
        t.colors.button_secondary_hover = role_color(Role::InteractionHover).into();
        t.colors.button_secondary_active = role_color(Role::InteractionSelected).into();
        t.colors.button_secondary_foreground = role_color(Role::TextDefault).into();
        t.colors.button_primary = role_color(Role::ActionPrimary).into();
        t.colors.button_primary_hover = role_color(Role::ActionPrimaryHover).into();
        t.colors.button_primary_active = role_color(Role::ActionPrimaryPressed).into();
        t.colors.button_primary_foreground = role_color(Role::TextOnPrimary).into();
        t.colors.button_danger = role_color(Role::StatusErrorFg).into();
        t.colors.button_danger_hover = role_color(Role::StatusErrorFg).into();
        t.colors.button_danger_active = role_color(Role::StatusErrorFg).into();
        t.colors.button_danger_foreground = role_color(Role::SurfaceCanvas).into();
        t.colors.button_success = role_color(Role::StatusSuccessFg).into();
        t.colors.button_success_hover = role_color(Role::StatusSuccessFg).into();
        t.colors.button_success_active = role_color(Role::StatusSuccessFg).into();
        t.colors.button_success_foreground = role_color(Role::SurfaceCanvas).into();
        t.colors.button_warning = role_color(Role::StatusWarningFg).into();
        t.colors.button_warning_hover = role_color(Role::StatusWarningFg).into();
        t.colors.button_warning_active = role_color(Role::StatusWarningFg).into();
        t.colors.button_warning_foreground = role_color(Role::SurfaceCanvas).into();
        t.colors.button_info = role_color(Role::StatusInfoFg).into();
        t.colors.button_info_hover = role_color(Role::StatusInfoFg).into();
        t.colors.button_info_active = role_color(Role::StatusInfoFg).into();
        t.colors.button_info_foreground = role_color(Role::SurfaceCanvas).into();
    });
}

/// Loads one of GitCrab's bundled icons by name.
pub fn icon(name: &str) -> gpui::component::Icon {
    gpui::component::Icon::default().path(SharedString::from(format!("icons/{name}.svg")))
}
