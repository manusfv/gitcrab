//! Small tinted labels: counts, file status letters and reference markers.
use crate::theme::Role;
use crate::theme::*;
use gpui::component::tooltip::Tooltip;
use gpui::{prelude::*, *};

/// A label on a translucent fill of its own color.
///
/// The color is a packed RGB value so palette roles, lane colors and file
/// status colors all fit. Style methods adjust the defaults, for example
/// `.text_size(..)` or `.px_1()`.
#[derive(IntoElement)]
pub struct Badge {
    label: SharedString,
    color: u32,
    width: Option<f32>,
    tooltip: Option<(ElementId, SharedString)>,
    style: StyleRefinement,
}

impl Badge {
    pub fn new(label: impl Into<SharedString>, color: u32) -> Self {
        Self {
            label: label.into(),
            color,
            width: None,
            tooltip: None,
            style: StyleRefinement::default(),
        }
    }

    /// A badge in the color of a palette role.
    pub fn role(label: impl Into<SharedString>, role: Role) -> Self {
        Self::new(label, semantic(role))
    }

    /// Fixes the width, given in pixels at the default font size, so a column
    /// of badges lines up whatever their labels.
    pub fn fixed_width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// Explains the badge on hover. The id must be unique among its siblings.
    pub fn tooltip(mut self, id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some((id.into(), text.into()));
        self
    }
}

impl Styled for Badge {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Badge {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let badge = div()
            .flex_none()
            .px_2()
            .py(px(2.))
            .rounded(px(4.))
            .bg(tint(self.color, 0x18))
            .text_color(rgb(self.color))
            .text_size(scaled(12.))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .when_some(self.width, |badge, width| {
                badge
                    .w(scaled(width))
                    .flex()
                    .justify_center()
                    .overflow_hidden()
            })
            .child(self.label);
        let badge = super::apply_style(badge, &self.style);
        match self.tooltip {
            Some((id, text)) => badge
                .id(id)
                .tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
                .into_any_element(),
            None => badge.into_any_element(),
        }
    }
}
