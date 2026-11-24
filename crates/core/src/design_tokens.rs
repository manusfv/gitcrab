//! Semantic color palettes and layout geometry for the native interface.
//!
//! [`Role`] provides the shared index into the palette arrays and [`ROLE_NAMES`].
//! Geometry tokens use logical pixels for lengths and sizes, with separate
//! scalar values for font weights, counts and layer ordering.

/// Semantic color role whose discriminant indexes the palette arrays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Role {
    SurfaceCanvas,
    SurfaceChrome,
    SurfacePanel,
    SurfaceNav,
    SurfaceOverlay,
    BorderDivider,
    BorderControl,
    TextDefault,
    TextSecondary,
    TextMuted,
    ActionPrimary,
    ActionPrimaryHover,
    ActionPrimaryPressed,
    TextOnPrimary,
    ActionAccentOutline,
    ActionAccentTint,
    ActionAccentTintHover,
    ActionAccentTintPressed,
    ActionStageFg,
    ActionStageBg,
    ActionUnstageFg,
    ActionUnstageBg,
    ActionDiscardFg,
    ActionDiscardBg,
    InteractionHover,
    InteractionPressed,
    InteractionSelected,
    InteractionFocus,
    InteractionSelectionRail,
    SurfaceWipSelected,
    SurfaceWipInput,
    TextLink,
    TextDisabled,
    SurfaceDisabled,
    StatusInfoFg,
    StatusInfoBg,
    StatusSuccessFg,
    StatusSuccessBg,
    StatusWarningFg,
    StatusWarningBg,
    StatusErrorFg,
    StatusErrorBg,
    StatusConflictFg,
    StatusConflictBg,
    DiffAddedBg,
    DiffAddedGutter,
    DiffRemovedBg,
    DiffRemovedGutter,
    DiffContextBg,
    DiffHunkBg,
    DiffWhitespace,
    SyntaxString,
    SyntaxKeyword,
    SyntaxNumber,
    SyntaxComment,
    OverlayScrim,
    OverlayShadow,
    GraphLane1,
    GraphLane2,
    GraphLane3,
    GraphLane4,
    GraphLane5,
    GraphLane6,
    GraphLane7,
    GraphLane8,
    GraphLane9,
    GraphLane10,
}
/// Design token names in [`Role`] order.
pub const ROLE_NAMES: &[&str] = &["surface.canvas","surface.chrome","surface.panel","surface.nav","surface.overlay","border.divider","border.control","text.default","text.secondary","text.muted","action.primary","action.primaryHover","action.primaryPressed","text.onPrimary","action.accentOutline","action.accentTint","action.accentTintHover","action.accentTintPressed","action.stage.fg","action.stage.bg","action.unstage.fg","action.unstage.bg","action.discard.fg","action.discard.bg","interaction.hover","interaction.pressed","interaction.selected","interaction.focus","interaction.selectionRail","surface.wipSelected","surface.wipInput","text.link","text.disabled","surface.disabled","status.info.fg","status.info.bg","status.success.fg","status.success.bg","status.warning.fg","status.warning.bg","status.error.fg","status.error.bg","status.conflict.fg","status.conflict.bg","diff.added.bg","diff.added.gutter","diff.removed.bg","diff.removed.gutter","diff.context.bg","diff.hunk.bg","diff.whitespace","syntax.string","syntax.keyword","syntax.number","syntax.comment","overlay.scrim","overlay.shadow","graph.lane.1","graph.lane.2","graph.lane.3","graph.lane.4","graph.lane.5","graph.lane.6","graph.lane.7","graph.lane.8","graph.lane.9","graph.lane.10"];
/// Dark palette as packed `0xRRGGBB` colors in [`Role`] order.
pub const DARK: [u32; 67] = [
    0x0d0d0d, // surface.canvas
    0x161616, // surface.chrome
    0x202020, // surface.panel
    0x191919, // surface.nav
    0x292929, // surface.overlay
    0x414141, // border.divider
    0x929292, // border.control
    0xeeeeee, // text.default
    0xb7b7b7, // text.secondary
    0xababab, // text.muted
    0x668bff, // action.primary
    0x80a0ff, // action.primaryHover
    0x5478ea, // action.primaryPressed
    0x0a1024, // text.onPrimary
    0x91adff, // action.accentOutline
    0x192341, // action.accentTint
    0x202c51, // action.accentTintHover
    0x283762, // action.accentTintPressed
    0xa8db9c, // action.stage.fg
    0x23382d, // action.stage.bg
    0xffa4ad, // action.unstage.fg
    0x422c35, // action.unstage.bg
    0xffa4ad, // action.discard.fg
    0x422c35, // action.discard.bg
    0x2c2c2c, // interaction.hover
    0x343434, // interaction.pressed
    0x1f2d52, // interaction.selected
    0xa9bfff, // interaction.focus
    0x668bff, // interaction.selectionRail
    0x202a43, // surface.wipSelected
    0x1a2233, // surface.wipInput
    0x668bff, // text.link
    0xababab, // text.disabled
    0x161616, // surface.disabled
    0xa7cdff, // status.info.fg
    0x26384f, // status.info.bg
    0xa8db9c, // status.success.fg
    0x23382d, // status.success.bg
    0xf4ca84, // status.warning.fg
    0x42341f, // status.warning.bg
    0xffa4ad, // status.error.fg
    0x422c35, // status.error.bg
    0xf0bbff, // status.conflict.fg
    0x3b2946, // status.conflict.bg
    0x23382d, // diff.added.bg
    0xa8db9c, // diff.added.gutter
    0x422c35, // diff.removed.bg
    0xffa4ad, // diff.removed.gutter
    0x0d0d0d, // diff.context.bg
    0x252525, // diff.hunk.bg
    0xababab, // diff.whitespace
    0xb4dfba, // syntax.string
    0xdcb8f5, // syntax.keyword
    0xf1c19e, // syntax.number
    0xababab, // syntax.comment
    0x00000099, // overlay.scrim
    0x00000066, // overlay.shadow
    0x15a0bf, // graph.lane.1
    0x0669f7, // graph.lane.2
    0x8e00c2, // graph.lane.3
    0xc517b6, // graph.lane.4
    0xd90171, // graph.lane.5
    0xcd0101, // graph.lane.6
    0xf25d2e, // graph.lane.7
    0xf2ca33, // graph.lane.8
    0x7bd938, // graph.lane.9
    0x2ece9d, // graph.lane.10
];
/// Light palette as packed `0xRRGGBB` colors in [`Role`] order.
pub const LIGHT: [u32; 67] = [
    0xfafbfc, // surface.canvas
    0xf0f3f6, // surface.chrome
    0xffffff, // surface.panel
    0xf5f7fa, // surface.nav
    0xffffff, // surface.overlay
    0xcfd7e1, // border.divider
    0x677689, // border.control
    0x182430, // text.default
    0x374a5b, // text.secondary
    0x516476, // text.muted
    0x006478, // action.primary
    0x00566a, // action.primaryHover
    0x00485b, // action.primaryPressed
    0xffffff, // text.onPrimary
    0x006478, // action.accentOutline
    0xe8f3f5, // action.accentTint
    0xdeeef1, // action.accentTintHover
    0xd7ebf0, // action.accentTintPressed
    0x215e38, // action.stage.fg
    0xe4f4e8, // action.stage.bg
    0x882c43, // action.unstage.fg
    0xfbe9ed, // action.unstage.bg
    0x882c43, // action.discard.fg
    0xfbe9ed, // action.discard.bg
    0xe7eef4, // interaction.hover
    0xdde7ee, // interaction.pressed
    0xd7ebf0, // interaction.selected
    0x005d76, // interaction.focus
    0x006478, // interaction.selectionRail
    0xe3ebfb, // surface.wipSelected
    0xf1f5ff, // surface.wipInput
    0x006478, // text.link
    0x516476, // text.disabled
    0xf0f3f6, // surface.disabled
    0x254c83, // status.info.fg
    0xe9f2ff, // status.info.bg
    0x215e38, // status.success.fg
    0xe4f4e8, // status.success.bg
    0x70460b, // status.warning.fg
    0xfff1d6, // status.warning.bg
    0x882c43, // status.error.fg
    0xfbe9ed, // status.error.bg
    0x6d328a, // status.conflict.fg
    0xf5e9fa, // status.conflict.bg
    0xe4f4e8, // diff.added.bg
    0x215e38, // diff.added.gutter
    0xfbe9ed, // diff.removed.bg
    0x882c43, // diff.removed.gutter
    0xfafbfc, // diff.context.bg
    0xe8eff6, // diff.hunk.bg
    0x516476, // diff.whitespace
    0x265c43, // syntax.string
    0x633782, // syntax.keyword
    0x75451f, // syntax.number
    0x516476, // syntax.comment
    0x16202e66, // overlay.scrim
    0x16202e33, // overlay.shadow
    0x0e7d96, // graph.lane.1
    0x0550c2, // graph.lane.2
    0x7600a3, // graph.lane.3
    0x9c1290, // graph.lane.4
    0xb3005d, // graph.lane.5
    0xad0000, // graph.lane.6
    0xbd4218, // graph.lane.7
    0x8a6a00, // graph.lane.8
    0x467f1c, // graph.lane.9
    0x13806a, // graph.lane.10
];
/// Layout tokens for spacing, control sizes, typography and layer order.
pub mod geometry {
pub const SIZE_ICON_SM: f32 = 12.0;
pub const SIZE_ICON_MD: f32 = 16.0;
pub const SIZE_ICON_LG: f32 = 20.0;
pub const SIZE_AVATAR: f32 = 26.0;
pub const SIZE_TARGET_INLINE: f32 = 24.0;
pub const SIZE_TARGET_COMPACT: f32 = 28.0;
pub const SIZE_TARGET_REGULAR: f32 = 32.0;
pub const SIZE_TARGET_COARSE: f32 = 44.0;
pub const SIZE_ROW_COMPACT: f32 = 24.0;
pub const SIZE_ROW_REGULAR: f32 = 30.0;
pub const SIZE_ROW_SECTION: f32 = 36.0;
pub const SIZE_ROW_DIFF: f32 = 20.0;
pub const SIZE_SHELL_TAB_STRIP: f32 = 36.0;
pub const SIZE_SHELL_TOOLBAR: f32 = 60.0;
pub const SIZE_SHELL_CONTEXT: f32 = 28.0;
pub const SIZE_SHELL_STATUS: f32 = 28.0;
pub const SIZE_SHELL_MIN_WIDTH: f32 = 960.0;
pub const SIZE_SHELL_MIN_HEIGHT: f32 = 600.0;
pub const SIZE_SHELL_NAV_MIN: f32 = 176.0;
pub const SIZE_SHELL_NAV_DEFAULT: f32 = 224.0;
pub const SIZE_SHELL_NAV_MAX: f32 = 320.0;
pub const SIZE_SHELL_INSPECTOR_MIN: f32 = 288.0;
pub const SIZE_SHELL_INSPECTOR_DEFAULT: f32 = 352.0;
pub const SIZE_SHELL_INSPECTOR_MAX: f32 = 520.0;
pub const SIZE_SHELL_HISTORY_MIN: f32 = 360.0;
pub const SIZE_SHELL_REF_DEFAULT: f32 = 136.0;
pub const SIZE_SHELL_REF_MIN: f32 = 88.0;
pub const SIZE_SHELL_REF_MAX: f32 = 240.0;
pub const SIZE_SHELL_SHA: f32 = 88.0;
pub const SIZE_SHELL_MESSAGE_MIN: f32 = 180.0;
pub const SIZE_SHELL_RAIL: f32 = 44.0;
pub const SIZE_SHELL_SPLIT_SIDE_MIN: f32 = 280.0;
pub const SIZE_SHELL_DRAWER_MIN: f32 = 288.0;
pub const SIZE_SHELL_DRAWER_MAX: f32 = 400.0;
pub const SIZE_SHELL_OVERLAY_MODE_BELOW: f32 = 960.0;
pub const SIZE_SHELL_WIDE_THRESHOLD: f32 = 1200.0;
pub const SIZE_FIELD_REGULAR: f32 = 32.0;
pub const SIZE_FIELD_SEARCH_WIDTH: f32 = 224.0;
pub const SIZE_FIELD_SUMMARY: f32 = 36.0;
pub const SIZE_FIELD_DESCRIPTION_MIN: f32 = 72.0;
pub const SIZE_FIELD_DESCRIPTION_MAX: f32 = 176.0;
pub const SIZE_REPOSITORY_TOOLBAR_SELECTOR_GROUP_WIDTH: f32 = 340.0;
pub const SIZE_REPOSITORY_TOOLBAR_SELECTOR_KICKER: f32 = 11.0;
pub const SIZE_REPOSITORY_TOOLBAR_SELECTOR_VALUE: f32 = 13.0;
pub const SIZE_REPOSITORY_TOOLBAR_COMMAND_WIDTH: f32 = 46.0;
pub const SIZE_REPOSITORY_TOOLBAR_COMMAND_HEIGHT: f32 = 42.0;
pub const SIZE_REPOSITORY_TOOLBAR_COMMAND_ICON: f32 = 16.0;
pub const SIZE_REPOSITORY_TOOLBAR_COMMAND_LABEL: f32 = 11.0;
pub const SIZE_REPOSITORY_TOOLBAR_GROUP_GAP: f32 = 12.0;
pub const SIZE_REPOSITORY_TOOLBAR_OVERFLOW_BELOW: f32 = 940.0;
pub const SIZE_REPOSITORY_TAB_LABEL_MIN_WIDTH: f32 = 96.0;
pub const SIZE_REPOSITORY_TAB_LABEL_MAX_WIDTH: f32 = 224.0;
pub const SIZE_REPOSITORY_TAB_BODY_HEIGHT: f32 = 32.0;
pub const SIZE_REPOSITORY_TAB_CLOSE_TARGET: f32 = 24.0;
pub const SIZE_REPOSITORY_TAB_NEW_TARGET: f32 = 28.0;
pub const SIZE_BULK_STAGE_HEIGHT: f32 = 24.0;
pub const SIZE_BULK_STAGE_MIN_WIDTH: f32 = 0.0;
pub const SIZE_BULK_STAGE_PADDING_X: f32 = 6.0;
pub const SIZE_BULK_STAGE_FONT_SIZE: f32 = 11.0;
pub const SIZE_BULK_STAGE_LINE_HEIGHT: f32 = 14.0;
pub const SIZE_BULK_STAGE_RADIUS: f32 = 3.0;
pub const SIZE_BULK_STAGE_HEADER_MIN_HEIGHT: f32 = 32.0;
pub const SIZE_BULK_STAGE_HEADER_GAP: f32 = 8.0;
pub const SIZE_BULK_STAGE_INSPECTOR_INSET: f32 = 12.0;
pub const SIZE_BULK_STAGE_WRAP_BELOW: f32 = 260.0;
pub const SIZE_FILE_ACTION_GLYPH: f32 = 13.0;
pub const SIZE_FILE_ACTION_TARGET: f32 = 24.0;
pub const SIZE_FILE_ACTION_COARSE_TARGET: f32 = 28.0;
pub const SIZE_FILE_ACTION_ROW: f32 = 30.0;
pub const SIZE_REFERENCE_SIDEBAR_INSET: f32 = 10.0;
pub const SIZE_REFERENCE_SIDEBAR_HEADING: f32 = 30.0;
pub const SIZE_REFERENCE_SIDEBAR_ROW: f32 = 26.0;
pub const SIZE_REFERENCE_SIDEBAR_CHILD_INSET: f32 = 28.0;
pub const SIZE_REFERENCE_SIDEBAR_HEADING_ICON: f32 = 14.0;
pub const SIZE_REFERENCE_SIDEBAR_ROW_ICON: f32 = 13.0;
pub const SIZE_REFERENCE_SIDEBAR_INLINE_GAP: f32 = 6.0;
pub const SIZE_COMMIT_EDITOR_SUMMARY: f32 = 32.0;
pub const SIZE_COMMIT_EDITOR_BODY_MIN: f32 = 72.0;
pub const SIZE_COMMIT_EDITOR_BODY_MAX: f32 = 160.0;
pub const SIZE_COMMIT_EDITOR_INSET_X: f32 = 8.0;
pub const SIZE_COMMIT_EDITOR_BORDER: f32 = 1.0;
pub const SIZE_COMMIT_EDITOR_RADIUS: f32 = 4.0;
pub const SIZE_OVERLAY_MENU_MIN: f32 = 200.0;
pub const SIZE_OVERLAY_MENU_MAX: f32 = 384.0;
pub const SIZE_OVERLAY_DIALOG_DEFAULT: f32 = 480.0;
pub const SIZE_OVERLAY_DIALOG_MAX: f32 = 640.0;
pub const SIZE_OVERLAY_DIALOG_MARGIN: f32 = 24.0;
pub const SIZE_OVERLAY_DIALOG_MARGIN_COMPACT: f32 = 12.0;
pub const SIZE_OVERLAY_DIALOG_INSET: f32 = 16.0;
pub const SIZE_OVERLAY_TOOLTIP_MAX: f32 = 320.0;
pub const SIZE_GRAPH_LANE_PITCH: f32 = 20.0;
pub const SIZE_GRAPH_NODE: f32 = 8.0;
pub const SIZE_GRAPH_MERGE_NODE: f32 = 10.0;
pub const SIZE_GRAPH_WIP_NODE: f32 = 18.0;
pub const SIZE_GRAPH_STROKE: f32 = 2.0;
pub const SIZE_GRAPH_SELECTED_STROKE: f32 = 3.0;
pub const SIZE_GRAPH_RIBBON_INSET: f32 = 3.0;
pub const SIZE_GRAPH_RIBBON_EDGE: f32 = 1.0;
pub const SIZE_GRAPH_REF_HEIGHT: f32 = 20.0;
pub const SIZE_GRAPH_MAX_VISIBLE_LANES: f32 = 8.0;
pub const SIZE_HUNK_ACTION_HEIGHT: f32 = 20.0;
pub const SIZE_HUNK_ACTION_PADDING_X: f32 = 5.0;
pub const SIZE_HUNK_ACTION_FONT_SIZE: f32 = 11.0;
pub const SIZE_HUNK_ACTION_LINE_HEIGHT: f32 = 16.0;
pub const SIZE_HUNK_ACTION_HEADER_MIN_HEIGHT: f32 = 24.0;
pub const SIZE_WIP_EDITOR_HEIGHT: f32 = 24.0;
pub const SIZE_WIP_EDITOR_MIN_WIDTH: f32 = 96.0;
pub const SIZE_WIP_EDITOR_MAX_WIDTH: f32 = 180.0;
pub const SIZE_WIP_EDITOR_FONT_SIZE: f32 = 12.0;
pub const SIZE_WIP_EDITOR_PADDING_X: f32 = 8.0;
pub const SIZE_WIP_EDITOR_RADIUS: f32 = 3.0;
pub const SIZE_WIP_EDITOR_COUNTER_FONT_SIZE: f32 = 11.0;
pub const SIZE_WIP_EDITOR_COUNTER_ICON: f32 = 11.0;
pub const SIZE_WIP_EDITOR_COUNTER_GAP: f32 = 7.0;
pub const SIZE_RESIZE_HIT: f32 = 8.0;
pub const SIZE_RESIZE_KEY_STEP: f32 = 16.0;
pub const SIZE_RESIZE_KEY_LARGE_STEP: f32 = 64.0;
pub const SIZE_GUTTER_MIN: f32 = 44.0;
pub const SIZE_GUTTER_PADDING: f32 = 8.0;
pub const LAYER_CONTENT: f32 = 0.0;
pub const LAYER_LOCAL_MARK: f32 = 1.0;
pub const LAYER_POPOVER: f32 = 20.0;
pub const LAYER_MODAL: f32 = 40.0;
pub const LAYER_TOOLTIP: f32 = 50.0;
}
