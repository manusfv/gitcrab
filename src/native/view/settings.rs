use super::*;

impl GitCrab {
    pub(super) fn settings_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut content = div()
            .id("appearance-settings")
            .max_h(relative(0.94))
            .overflow_y_scroll()
            .w(px(430.))
            .rounded(px(10.))
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(BORDER))
            .p_5()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_lg().child("Appearance"))
            .child("Atom One Dark · native system fonts")
            .child(
                div()
                    .text_size(type_size(cx, 12.))
                    .text_color(rgb(MUTED))
                    .child(format!(
                        "GitCrab {} · {}",
                        env!("CARGO_PKG_VERSION"),
                        env!("GITCRAB_BUILD")
                    )),
            )
            .child(
                self.action_button(
                    "settings-density",
                    if self.compact {
                        "Density: Compact"
                    } else {
                        "Density: Comfortable"
                    },
                )
                .selected(self.compact)
                .tab_stop(true)
                .on_click(cx.listener(|v, _, _, cx| {
                    v.compact = !v.compact;
                    cx.notify();
                })),
            )
            .child(
                self.action_button(
                    "diff-density",
                    if self.preferences.compact_diff {
                        "Code spacing: Compact"
                    } else {
                        "Code spacing: Comfortable"
                    },
                )
                .selected(self.preferences.compact_diff)
                .tab_stop(true)
                .on_click(cx.listener(|v, _, _, cx| {
                    v.preferences.compact_diff = !v.preferences.compact_diff;
                    cx.notify();
                })),
            )
            .child(
                self.action_button("reset-appearance", "Reset appearance and layout")
                    .tab_stop(true)
                    .on_click(cx.listener(|v, _, _, cx| {
                        let recent = v.preferences.recent.clone();
                        v.preferences = Preferences::default();
                        v.preferences.recent = recent;
                        v.compact = false;
                        v.split = false;
                        v.layout_revision += 1;
                        v.reset_panels(cx);
                        gpui_component::Theme::global_mut(cx).font_size =
                            px(v.preferences.font_size);
                        cx.notify();
                    })),
            );
        for (index, label, value) in [
            (0, "Text size", self.preferences.font_size),
            (1, "Navigation width", self.preferences.nav_width),
            (2, "Inspector width", self.preferences.inspector_width),
            (3, "Code text size", self.preferences.code_size),
        ] {
            let mut row = div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().min_w_0().child(label))
                .child(div().w(px(36.)).flex_none().child(format!("{value:.0}")));
            for direction in [-1., 1.] {
                row = row.child(
                    self.icon_button(
                        ("appearance", index * 2 + usize::from(direction > 0.)),
                        if direction > 0. { "plus" } else { "minus" },
                        format!(
                            "{} {label}",
                            if direction > 0. {
                                "Increase"
                            } else {
                                "Decrease"
                            }
                        ),
                    )
                    .disabled(if direction < 0. {
                        value <= [11., 150., 260., 11.][index]
                    } else {
                        value >= [18., 330., 560., 20.][index]
                    })
                    .tab_stop(true)
                    .on_click(cx.listener(move |v, _, _, cx| {
                        match index {
                            0 => {
                                v.preferences.font_size =
                                    (v.preferences.font_size + direction).clamp(11., 18.);
                                gpui_component::Theme::global_mut(cx).font_size =
                                    px(v.preferences.font_size);
                            }
                            1 => {
                                v.preferences.nav_width =
                                    (v.preferences.nav_width + direction * 10.).clamp(150., 330.)
                            }
                            2 => {
                                v.preferences.inspector_width = (v.preferences.inspector_width
                                    + direction * 10.)
                                    .clamp(260., 560.)
                            }
                            _ => {
                                v.preferences.code_size =
                                    (v.preferences.code_size + direction).clamp(11., 20.);
                            }
                        };
                        if index == 1 || index == 2 {
                            v.layout_revision += 1;
                            v.reset_panels(cx);
                        }
                        cx.notify();
                    })),
                );
            }
            content = content.child(row);
        }
        content = content
            .child(
                "Panel widths can also be dragged. These controls provide keyboard alternatives.",
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        self.action_button("settings-close", "Close")
                            .tab_stop(true)
                            .on_click(cx.listener(|v, _, w, cx| v.back(&Back, w, cx))),
                    )
                    .child(
                        self.primary_button("save-appearance", "Save appearance")
                            .tab_stop(true)
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.preference_save = None;
                                match v.save_preferences(cx) {
                                    Ok(()) => {
                                        v.state.notice = "Appearance saved".into();
                                        v.settings_open = false;
                                    }
                                    Err(error) => {
                                        v.state.error = true;
                                        v.state.notice = error;
                                    }
                                }
                                cx.notify();
                            })),
                    ),
            );
        div()
            .absolute()
            .inset_0()
            .occlude()
            .bg(rgba(OVERLAY))
            .flex()
            .items_center()
            .justify_center()
            .child(content)
            .into_any_element()
    }
}
