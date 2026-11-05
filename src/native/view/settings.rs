use super::*;

impl GitCrab {
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open || window.has_active_dialog(cx) {
            return;
        }
        self.settings_open = true;
        let weak = cx.weak_entity();
        let close = weak.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let content = weak.update(cx, |v, cx| v.settings_dialog(cx)).ok();
            let close = close.clone();
            dialog
                .title("Appearance")
                .w(px(480.))
                .p_4()
                .child(div().children(content))
                .on_close(move |_, w, cx| {
                    let _ = close.update(cx, |v, cx| {
                        v.settings_open = false;
                        v.persist_preferences(w, cx);
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }

    pub(super) fn settings_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut content = div()
            .id("appearance-settings")
            .max_h(px(440.))
            .overflow_y_scroll()
            .w(px(430.))
            .rounded(px(10.))
            .bg(rgb(semantic(Role::SurfaceOverlay)))
            .border_1()
            .border_color(rgb(semantic(Role::BorderDivider)))
            .p_4()
            .flex()
            .flex_col()
            .gap_3()

            .child("App appearance · native system fonts")
            .child(self.action_button("theme-preference", if self.preferences.theme == gitcrab_core::appearance::ThemePreference::System { "Theme: Follow system" } else { "Theme: Neutral Black + Electric Blue" })
                .tab_stop(true)
                .on_click(cx.listener(|v, _, _, cx| {
                    use gitcrab_core::appearance::ThemePreference;
                    v.preferences.theme = if v.preferences.theme == ThemePreference::System { ThemePreference::Dark } else { ThemePreference::System };
                    super::super::theme::install(cx);
                    cx.refresh_windows();
                })))
            .child(div().text_color(rgb(semantic(Role::TextMuted))).child("Follow system uses Neutral Black until the proposed light palette passes native qualification."))
            .child(
                div()
                    .text_size(type_size(cx, 12.))
                    .text_color(rgb(semantic(Role::TextMuted)))
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
            .child(self.action_button("file-actions-visibility",if self.preferences.always_show_file_actions {"File actions: Always visible · larger targets"}else{"File actions: Reveal on hover or focus"})
                .tab_stop(true).on_click(cx.listener(|v,_,_,cx|{v.preferences.always_show_file_actions=!v.preferences.always_show_file_actions;cx.notify();})))
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
        #[cfg(feature = "native-evidence")]
        {
            content = content.child(
                self.action_button(
                    "validation-palette",
                    "Validation: swap dark / proposed light",
                )
                .on_click(|_, _, cx| super::super::theme::toggle_validation_palette(cx)),
            );
        }
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
                        value >= [26., 330., 560., 26.][index]
                    })
                    .tab_stop(true)
                    .on_click(cx.listener(move |v, _, _, cx| {
                        match index {
                            0 => {
                                v.preferences.font_size =
                                    (v.preferences.font_size + direction).clamp(11., 26.);
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
                                    (v.preferences.code_size + direction).clamp(11., 26.);
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
                            .on_click(cx.listener(|v, _, w, cx| {
                                v.back(&Back, w, cx);
                                w.close_dialog(cx);
                            })),
                    )
                    .child(
                        self.primary_button("save-appearance", "Save appearance", false)
                            .tab_stop(true)
                            .on_click(cx.listener(|v, _, w, cx| {
                                v.preference_save = None;
                                match v.save_preferences(cx) {
                                    Ok(()) => {
                                        v.state.notice = "Appearance saved".into();
                                        v.settings_open = false;
                                        w.close_dialog(cx);
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
        content.into_any_element()
    }
}
