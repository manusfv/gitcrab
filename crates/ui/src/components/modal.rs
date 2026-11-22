//! Modal dialogs: the shared frame every dialog is built from.
//!
//! A dialog is a [`ModalHeader`] and a width applied with [`ModalFrame`], a
//! [`modal_body`] passed to [`ModalFrame::body`] and a [`ModalFooter`]. The
//! body pieces ([`subject`], [`paragraph`], [`callout`], [`section`],
//! [`setting_row`], [`shortcut_row`]) keep content consistent between
//! dialogs.
use super::buttons::{basic, ButtonKind, ControlSize};
use super::layout::scroll_area;
use crate::theme::Role;
use crate::theme::*;
use gpui::component::{
    button::Button,
    dialog::{AlertDialog, Dialog, DialogContent},
    WindowExt,
};
use gpui::{prelude::*, *};

/// The space between a dialog's edge and its content.
const INSET: f32 = 20.;
/// Matches the library surface's 10px corner inside its 1px border.
const INNER_RADIUS: f32 = 9.;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModalWidth {
    /// Confirmations and short messages.
    Small,
    /// Reference content and settings.
    Medium,
    /// Lists of records.
    Large,
}

impl ModalWidth {
    /// The width at the default text size. It grows with the text-size
    /// preference; the library keeps the surface inside the window.
    pub fn base(self) -> f32 {
        match self {
            Self::Small => 440.,
            Self::Medium => 520.,
            Self::Large => 640.,
        }
    }
}

/// What a dialog's icon tile signals.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    /// Consequences worth reading, such as trusting a repository.
    Warning,
    /// Permanent loss of work.
    Danger,
}

impl Tone {
    fn roles(self) -> (Role, Role) {
        match self {
            Self::Warning => (Role::StatusWarningFg, Role::StatusWarningBg),
            Self::Danger => (Role::ActionDiscardFg, Role::ActionDiscardBg),
        }
    }
    fn glyph(self) -> &'static str {
        "triangle-alert"
    }
}

/// A dialog's title row: an optional tone tile, the title, an optional
/// one-line description and, for dialogs that ask nothing, a close button.
#[derive(IntoElement)]
pub struct ModalHeader {
    title: SharedString,
    description: Option<SharedString>,
    tone: Option<(Tone, &'static str)>,
    closable: bool,
}

impl ModalHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            description: None,
            tone: None,
            closable: false,
        }
    }
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }
    /// Shows the tone's tile with its standard glyph.
    pub fn tone(self, tone: Tone) -> Self {
        self.tone_glyph(tone, tone.glyph())
    }
    /// Shows the tone's tile with a specific glyph.
    pub fn tone_glyph(mut self, tone: Tone, glyph: &'static str) -> Self {
        self.tone = Some((tone, glyph));
        self
    }
    /// Adds a close button, for dialogs that ask nothing.
    pub fn closable(mut self) -> Self {
        self.closable = true;
        self
    }
}

impl RenderOnce for ModalHeader {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let tile = scaled(32.);
        div()
            .w_full()
            .flex()
            .items_start()
            .gap(scaled(12.))
            .pt(scaled(INSET))
            .px(scaled(INSET))
            .pb(scaled(8.))
            .debug_selector(|| "modal-header".into())
            .when_some(self.tone, |row, (tone, glyph)| {
                let (fg, bg) = tone.roles();
                row.child(
                    div()
                        .size(tile)
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(8.))
                        .bg_role(bg)
                        .text_role(fg)
                        .child(icon(glyph).size(scaled(18.))),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(self.tone.is_some(), |column| column.min_h(tile))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(scaled(4.))
                    .child(
                        div()
                            .text_size(scaled(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_height(relative(1.3))
                            .text_role(Role::TextDefault)
                            .child(self.title),
                    )
                    .when_some(self.description, |column, description| {
                        column.child(
                            div()
                                .text_size(scaled(13.))
                                .font_weight(FontWeight::NORMAL)
                                .line_height(relative(1.45))
                                .text_role(Role::TextSecondary)
                                .child(description),
                        )
                    }),
            )
            .when(self.closable, |row| {
                row.child(
                    basic(
                        Button::new("modal-close").icon(icon("x").size(scaled(16.))),
                        ButtonKind::Ghost,
                        ControlSize::Compact,
                    )
                    .w(scaled(28.))
                    .px_0()
                    .flex_none()
                    // Optically aligns the glyph with the title's first line.
                    .mt(scaled(-4.))
                    .mr(scaled(-6.))
                    .tab_stop(true)
                    .tooltip("Close")
                    .on_click(|_, window, cx| window.close_dialog(cx)),
                )
            })
    }
}

/// Applies the shared frame to a library dialog: its header, its width, and
/// a body region between the header and the footer.
///
/// A dialog is as tall as its content until it reaches the window's limit;
/// then only the body gives way. Call [`Self::frame`] before the body.
pub trait ModalFrame: Sized {
    fn frame(self, header: ModalHeader, width: ModalWidth, cx: &App) -> Self;

    /// A body that scrolls when the dialog runs out of room. It reaches from
    /// the header to the footer's divider, its padding scrolls with the
    /// content, its scrollbar sits at the dialog's edge, and a divider under
    /// the header appears once content has scrolled beneath it. Build
    /// `content` with [`modal_body`] for the standard padding.
    fn body(self, scroll: &ScrollHandle, content: impl IntoElement) -> Self {
        self.region(scrolling_region(scroll, content.into_any_element()))
    }

    /// A body for content that scrolls itself, such as a list under a pinned
    /// search field. It prefers `height` (at the default text size) and gives
    /// way when the window is shorter; `content` should fill it.
    fn fill_body(self, height: f32, content: impl IntoElement) -> Self {
        self.region(filling_region(height, content.into_any_element()))
    }

    #[doc(hidden)]
    fn region(self, region: Div) -> Self;
}

impl ModalFrame for AlertDialog {
    fn frame(self, header: ModalHeader, width: ModalWidth, cx: &App) -> Self {
        self.title(header)
            .width(type_size(cx, width.base()))
            .p_0()
            .gap_0()
    }
    fn region(self, region: Div) -> Self {
        self.content(content_once(region))
    }
}

impl ModalFrame for Dialog {
    fn frame(self, header: ModalHeader, width: ModalWidth, cx: &App) -> Self {
        // The header draws its own close button, aligned with the title.
        self.title(header)
            .width(type_size(cx, width.base()))
            .close_button(false)
            .p_0()
            .gap_0()
    }
    fn region(self, region: Div) -> Self {
        self.content(content_once(region))
    }
}

/// Places `region` in the dialog's content slot. Unlike the library's child
/// slot, which wraps children in a scroller of its own, the content slot
/// leaves sizing to the region.
///
/// The library calls the builder once per render, and the dialog is rebuilt
/// every render, so the region is moved out on that call.
fn content_once(
    region: Div,
) -> impl Fn(DialogContent, &mut Window, &mut App) -> DialogContent + 'static {
    let region = std::cell::RefCell::new(Some(region.into_any_element()));
    move |content, _, _| {
        // Lets the body shrink; otherwise the footer is pushed off-window.
        content.min_h_0().children(region.borrow_mut().take())
    }
}

fn scrolling_region(scroll: &ScrollHandle, content: AnyElement) -> Div {
    let scrolled = scroll.offset().y < px(-0.5);
    scroll_area(
        "modal-body",
        scroll,
        div()
            .id("modal-body")
            .w_full()
            // The content's own padding keeps it clear of the scrollbar, so
            // skip the gutter `scroll_area` would add.
            .pr_0()
            .child(content),
    )
    .flex_1()
    .min_h_0()
    .border_t_1()
    .border_color(if scrolled {
        role_color(Role::BorderDivider).into()
    } else {
        gpui::transparent_black()
    })
    .debug_selector(|| "modal-body".into())
}

fn filling_region(height: f32, content: AnyElement) -> Div {
    div()
        .flex_1()
        .flex_basis(scaled(height))
        .min_h_0()
        .flex()
        .flex_col()
        .px(scaled(INSET))
        .pb(scaled(INSET))
        .child(div().flex_1().min_h_0().flex().flex_col().child(content))
        .debug_selector(|| "modal-body".into())
}

/// The dialog's content column, with the standard padding. Pass it to
/// [`ModalFrame::body`].
pub fn modal_body() -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(scaled(12.))
        .px(scaled(INSET))
        .pb(scaled(INSET))
}

/// The dialog's action row, on the chrome surface below a divider. Actions
/// sit at the right with the safe choice first; `leading` holds a status or
/// a secondary action at the left.
#[derive(IntoElement, Default)]
pub struct ModalFooter {
    leading: Option<AnyElement>,
    actions: Vec<AnyElement>,
}

impl ModalFooter {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading = Some(element.into_any_element());
        self
    }
    pub fn action(mut self, element: impl IntoElement) -> Self {
        self.actions.push(element.into_any_element());
        self
    }
}

impl RenderOnce for ModalFooter {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .px(scaled(INSET))
            .py(scaled(12.))
            .border_t_1()
            .border_role(Role::BorderDivider)
            .bg_role(Role::SurfaceChrome)
            .rounded_b(px(INNER_RADIUS))
            .debug_selector(|| "modal-footer".into())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .children(self.leading),
            )
            .children(self.actions)
    }
}

/// What the dialog acts on, such as a file or repository path, in code type
/// so it can be checked before reading the rest. `detail` is a second,
/// quieter line such as the containing repository.
pub fn subject(title: impl Into<SharedString>, detail: Option<SharedString>) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(scaled(2.))
        .px(scaled(10.))
        .py(scaled(8.))
        .rounded(px(6.))
        .bg_role(Role::SurfaceChrome)
        .border_1()
        .border_role(Role::BorderDivider)
        .font_family(CODE_FONT)
        .child(
            div()
                .text_size(scaled(12.5))
                .text_role(Role::TextDefault)
                .child(title.into()),
        )
        .when_some(detail, |el, detail| {
            el.child(
                div()
                    .text_size(scaled(11.5))
                    .text_role(Role::TextMuted)
                    .child(detail),
            )
        })
}

/// Explanatory body text.
pub fn paragraph(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(scaled(13.))
        .line_height(relative(1.5))
        .text_role(Role::TextSecondary)
        .child(text.into())
}

/// Small print, such as what a choice remembers.
pub fn fine_print(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(scaled(12.))
        .line_height(relative(1.45))
        .text_role(Role::TextMuted)
        .child(text.into())
}

/// A tinted note that stands out from the explanation, for the consequence
/// people must not miss.
pub fn callout(tone: Tone, text: impl Into<SharedString>) -> Div {
    let (fg, bg) = tone.roles();
    div()
        .w_full()
        .flex()
        .items_start()
        .gap(scaled(8.))
        .px(scaled(10.))
        .py(scaled(8.))
        .rounded(px(6.))
        .bg_role(bg)
        .child(
            div()
                .flex_none()
                .text_role(fg)
                .pt(scaled(1.))
                .child(icon(tone.glyph()).size(scaled(14.))),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(scaled(12.5))
                .line_height(relative(1.45))
                .text_role(Role::TextDefault)
                .child(text.into()),
        )
}

/// A titled group of related content. Add rows as children.
pub fn section(title: impl Into<SharedString>) -> Div {
    div().w_full().flex().flex_col().gap(scaled(6.)).child(
        div()
            .text_size(scaled(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_role(Role::TextMuted)
            .child(title.into()),
    )
}

/// A labelled value, such as a path in a details list.
pub fn detail_row(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(scaled(2.))
        .text_size(scaled(12.))
        .child(div().text_role(Role::TextMuted).child(label.into()))
        .child(
            div()
                .font_family(CODE_FONT)
                .text_role(Role::TextSecondary)
                .child(value.into()),
        )
}

/// A setting: its label and an optional description at the left, its
/// control at the right.
pub fn setting_row(
    label: impl Into<SharedString>,
    description: Option<SharedString>,
    control: impl IntoElement,
) -> Div {
    div()
        .w_full()
        .min_h(scaled(36.))
        .flex()
        .items_center()
        .gap(scaled(16.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(scaled(2.))
                .child(
                    div()
                        .text_size(scaled(13.))
                        .text_role(Role::TextDefault)
                        .child(label.into()),
                )
                .when_some(description, |column, description| {
                    column.child(
                        div()
                            .text_size(scaled(12.))
                            .line_height(relative(1.4))
                            .text_role(Role::TextMuted)
                            .child(description),
                    )
                }),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .child(control),
        )
}

/// The settings groups' frame: rows separated by hairlines inside a bordered
/// card, under a section title.
pub fn settings_card(title: impl Into<SharedString>, rows: Vec<AnyElement>) -> Div {
    let count = rows.len();
    section(title).child(
        div()
            .w_full()
            .flex()
            .flex_col()
            .rounded(px(8.))
            .border_1()
            .border_role(Role::BorderDivider)
            .bg_role(Role::SurfaceChrome)
            .children(rows.into_iter().enumerate().map(move |(index, row)| {
                div()
                    .px(scaled(12.))
                    .py(scaled(6.))
                    .when(index + 1 < count, |row| {
                        row.border_b_1().border_role(Role::BorderDivider)
                    })
                    .child(row)
            })),
    )
}

/// The modifier GitCrab's shortcuts use on this platform.
pub const COMMAND_KEY: &str = if cfg!(target_os = "macos") {
    "Cmd"
} else {
    "Ctrl"
};

/// A keyboard shortcut and what it does. Each key is drawn as a keycap;
/// separate alternatives with "/" as their own entry.
pub fn shortcut_row(keys: &[&'static str], description: impl Into<SharedString>) -> Div {
    let mut caps = div().flex_none().flex().items_center().gap(scaled(4.));
    for key in keys {
        caps = caps.child(if *key == "/" || *key == "+" {
            div()
                .text_size(scaled(11.))
                .text_role(Role::TextMuted)
                .child(*key)
        } else {
            div()
                .px(scaled(6.))
                .min_w(scaled(20.))
                .h(scaled(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .border_1()
                .border_b_2()
                .border_role(Role::BorderDivider)
                .bg_role(Role::SurfaceOverlay)
                .text_size(scaled(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_role(Role::TextDefault)
                .child(*key)
        });
    }
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(scaled(16.))
        .py(scaled(4.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(scaled(13.))
                .line_height(relative(1.4))
                .text_role(Role::TextSecondary)
                .child(description.into()),
        )
        .child(caps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::component::Root;
    use gpui::{TestAppContext, VisualTestContext};
    // `gpui::*` brings in GPUI's `test` macro, which expands to `#[test]`.
    use std::prelude::rust_2021::test;

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn window(cx: &mut TestAppContext, height: f32) -> &mut VisualTestContext {
        cx.update(|cx| {
            gpui::component::init(cx);
            cx.set_reduce_motion(true);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| Host);
            Root::new(view, window, cx)
        });
        cx.simulate_resize(size(px(900.), px(height)));
        draw(cx);
        cx
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        // One frame mounts the dialog layer, the next lays it out at rest.
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
        cx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} was not painted"))
    }

    fn rows(count: usize) -> Div {
        modal_body().children((0..count).map(|i| {
            div()
                .h(px(30.))
                .flex_none()
                .when(i + 1 == count, |row| {
                    row.debug_selector(|| "last-row".into())
                })
                .child(format!("Row {i}"))
        }))
    }

    fn footer() -> ModalFooter {
        ModalFooter::new().action(div().w(px(80.)).h(px(32.)))
    }

    fn open_alert(cx: &mut VisualTestContext, scroll: &ScrollHandle, count: usize) {
        let scroll = scroll.clone();
        cx.update(move |window, cx| {
            window.open_alert_dialog(cx, move |dialog, _, cx| {
                dialog
                    .frame(
                        ModalHeader::new("Title").description("Description"),
                        ModalWidth::Small,
                        cx,
                    )
                    .body(&scroll, rows(count))
                    .footer(footer())
            })
        });
        draw(cx);
    }

    /// The header, body and footer stack without gaps, the body runs into
    /// the footer's divider, and the whole surface stays inside the window.
    fn assert_pinned(cx: &mut VisualTestContext, window_height: f32) {
        let surface = bounds(cx, "dialog-0");
        let header = bounds(cx, "modal-header");
        let body = bounds(cx, "modal-body");
        let footer = bounds(cx, "modal-footer");
        assert!(
            surface.bottom() <= px(window_height),
            "surface {surface:?} leaves a {window_height}px window"
        );
        assert!(
            header.bottom() <= body.top(),
            "body {body:?} overlaps header {header:?}"
        );
        assert!(
            (footer.top() - body.bottom()).abs() <= px(1.),
            "body {body:?} stops short of footer {footer:?}"
        );
        assert!(
            (surface.bottom() - footer.bottom()).abs() <= px(1.5),
            "footer {footer:?} is not at the bottom of {surface:?}"
        );
    }

    #[gpui::test]
    fn a_long_body_scrolls_between_the_header_and_footer(cx: &mut TestAppContext) {
        let cx = window(cx, 480.);
        let scroll = ScrollHandle::new();
        open_alert(cx, &scroll, 60);

        assert_pinned(cx, 480.);
        assert_viewport_fills_body(cx, &scroll);
        assert!(scroll.max_offset().y > px(0.));
        assert!(bounds(cx, "last-row").bottom() > scroll.bounds().bottom());

        scroll.set_offset(point(px(0.), -scroll.max_offset().y));
        draw(cx);

        assert_pinned(cx, 480.);
        assert_viewport_fills_body(cx, &scroll);
        let viewport = scroll.bounds();
        let last = bounds(cx, "last-row");
        assert!(
            last.bottom() < viewport.bottom() - px(10.),
            "the body's bottom padding did not scroll into view: {last:?} in {viewport:?}"
        );
    }

    /// The scrolling viewport itself, not just its wrapper, spans the dialog's
    /// width and reaches the footer, so content is never cut off above an
    /// empty strip and the scrollbar sits at the dialog's edge.
    fn assert_viewport_fills_body(cx: &mut VisualTestContext, scroll: &ScrollHandle) {
        let viewport = scroll.bounds();
        let surface = bounds(cx, "dialog-0");
        let footer = bounds(cx, "modal-footer");
        assert!(
            (footer.top() - viewport.bottom()).abs() <= px(1.),
            "viewport {viewport:?} stops short of footer {footer:?}"
        );
        assert!(
            (surface.right() - viewport.right()).abs() <= px(1.5)
                && (viewport.left() - surface.left()).abs() <= px(1.5),
            "viewport {viewport:?} does not span surface {surface:?}"
        );
    }

    #[gpui::test]
    fn a_short_body_sizes_the_dialog_to_its_content(cx: &mut TestAppContext) {
        let cx = window(cx, 800.);
        let scroll = ScrollHandle::new();
        open_alert(cx, &scroll, 3);

        assert_pinned(cx, 800.);
        assert_viewport_fills_body(cx, &scroll);
        assert_eq!(scroll.max_offset().y, px(0.));
        assert!(bounds(cx, "dialog-0").size.height < px(500.));
        assert!(bounds(cx, "last-row").bottom() < bounds(cx, "modal-body").bottom());
    }

    #[gpui::test]
    fn a_non_alert_dialog_scrolls_its_body_the_same_way(cx: &mut TestAppContext) {
        let cx = window(cx, 480.);
        let scroll = ScrollHandle::new();
        let handle = scroll.clone();
        cx.update(move |window, cx| {
            window.open_dialog(cx, move |dialog, _, cx| {
                dialog
                    .frame(ModalHeader::new("Title").closable(), ModalWidth::Medium, cx)
                    .body(&handle, rows(60))
                    .footer(footer())
            })
        });
        draw(cx);

        assert_pinned(cx, 480.);
        assert_viewport_fills_body(cx, &scroll);
        assert!(scroll.max_offset().y > px(0.));
    }

    fn open_filled(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            window.open_alert_dialog(cx, |dialog, _, cx| {
                dialog
                    .frame(ModalHeader::new("Title"), ModalWidth::Medium, cx)
                    .fill_body(
                        400.,
                        div()
                            .flex_1()
                            .min_h_0()
                            .debug_selector(|| "fill-content".into())
                            .child(div().size_full().debug_selector(|| "fill-inner".into())),
                    )
                    .footer(footer())
            })
        });
        draw(cx);
    }

    #[gpui::test]
    fn a_filled_body_takes_its_preferred_height_and_gives_way(cx: &mut TestAppContext) {
        let cx = window(cx, 1000.);
        open_filled(cx);
        assert_pinned(cx, 1000.);
        let roomy = bounds(cx, "fill-content").size.height;
        assert!(roomy >= px(380.), "preferred height not used: {roomy:?}");
        assert_eq!(bounds(cx, "fill-inner").size.height, roomy);

        cx.update(|window, cx| window.close_dialog(cx));
        cx.simulate_resize(size(px(900.), px(420.)));
        draw(cx);
        open_filled(cx);
        assert_pinned(cx, 420.);
        let tight = bounds(cx, "fill-content").size.height;
        assert!(
            tight < roomy && tight > px(0.),
            "did not give way: {tight:?}"
        );
        assert_eq!(bounds(cx, "fill-inner").size.height, tight);
    }
}
