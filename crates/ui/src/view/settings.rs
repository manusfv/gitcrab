use super::*;

impl GitCrab {
    /// Opens appearance settings when no other dialog is active.
    ///
    /// Changes apply as they are made and save automatically. Close, Escape
    /// and the dialog's own dismissal all finish through [`Self::finish_settings`].
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open || window.has_active_dialog(cx) {
            return;
        }
        self.settings_open = true;
        self.appearance_status = AppearanceStatus::Idle;
        let weak = cx.weak_entity();
        let close = weak.clone();
        window.open_dialog(cx, move |dialog, w, cx| {
            let height = (f32::from(w.viewport_size().height) - 220.).clamp(200., 560.);
            let content = weak.update(cx, |v, cx| v.settings_dialog(height, cx)).ok();
            let status = weak
                .read_with(cx, |v, _| v.appearance_status.clone())
                .unwrap_or_default();
            let close = close.clone();
            let footer_close = close.clone();
            let (text, role) = match &status {
                AppearanceStatus::Idle => (
                    "Changes apply immediately and save automatically.".to_owned(),
                    Role::TextMuted,
                ),
                AppearanceStatus::Saving(_) => ("Saving…".to_owned(), Role::TextMuted),
                AppearanceStatus::Saved => ("Saved".to_owned(), Role::StatusSuccessFg),
                AppearanceStatus::Failed(error) => {
                    (format!("Could not save: {error}"), Role::StatusErrorFg)
                }
            };
            let footer = components::dialog_footer([
                div()
                    .id("appearance-save-status")
                    .flex_1()
                    .min_w_0()
                    .text_size(scaled(12.))
                    .text_role(role)
                    .child(text)
                    .into_any_element(),
                Button::new("settings-close")
                    .label("Close")
                    .cursor_pointer()
                    .primary()
                    .small()
                    .h(scaled(CONTROL_HEIGHT))
                    .tab_stop(true)
                    .on_click(move |_, w, cx| {
                        // The same persistence path as Escape and dismissal.
                        let _ = footer_close.update(cx, |v, cx| v.finish_settings(w, cx));
                        w.close_dialog(cx);
                    })
                    .into_any_element(),
            ])
            .items_center();
            dialog
                .title("Appearance")
                .w(px(480.))
                .p_4()
                .child(div().children(content))
                .footer(footer)
                .on_close(move |_, w, cx| {
                    let _ = close.update(cx, |v, cx| v.finish_settings(w, cx));
                })
        });
        cx.notify();
    }

    /// Ends an Appearance session: saves pending edits now and reports the
    /// result in the status bar once the settings writer acknowledges it.
    pub(super) fn finish_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings_open {
            return;
        }
        self.settings_open = false;
        self.preference_save = None;
        if let Err(error) = self.save_preferences(true, cx) {
            self.persistence_error("appearance", error);
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn settings_group(&self, title: &'static str) -> Div {
        div().flex().flex_col().gap_2().child(
            div()
                .text_size(scaled(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_role(Role::TextSecondary)
                .child(title),
        )
    }

    /// A labelled value with keyboard-reachable decrease and increase buttons.
    fn settings_stepper(
        &self,
        index: usize,
        label: &'static str,
        value: f32,
        cx: &mut Context<Self>,
    ) -> Div {
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
                .disabled_when(if direction < 0. {
                    value <= [11., 150., 260., 11.][index]
                } else {
                    value >= [26., 330., 560., 26.][index]
                })
                .tab_stop(true)
                .on_click(cx.listener(move |v, _, w, cx| {
                    match index {
                        0 => {
                            v.preferences.font_size =
                                (v.preferences.font_size + direction).clamp(11., 26.);
                            crate::preferences::set_app_appearance(
                                v.preferences.font_size,
                                v.preferences.theme,
                                cx,
                            );
                        }
                        1 => {
                            v.preferences.nav_width =
                                (v.preferences.nav_width + direction * 10.).clamp(150., 330.)
                        }
                        2 => {
                            v.preferences.inspector_width =
                                (v.preferences.inspector_width + direction * 10.).clamp(260., 560.)
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
                    v.persist_preferences(w, cx);
                    cx.notify();
                })),
            );
        }
        row
    }

    pub(super) fn settings_dialog(&self, height: f32, cx: &mut Context<Self>) -> AnyElement {
        let small = scaled(12.);
        let muted = |text: &'static str| {
            div()
                .text_size(small)
                .text_role(Role::TextMuted)
                .child(text)
        };
        // Every preference resolves to the dark palette, so state it plainly
        // instead of offering a choice with no effect.
        let theme = self
            .settings_group("Theme")
            .child("Dark")
            .child(muted("GitCrab currently has one theme."));
        #[cfg(feature = "native-evidence")]
        let theme = theme.child(
            self.action_button(
                "validation-palette",
                "Validation: swap dark / proposed light",
            )
            .on_click(|_, _, cx| super::super::theme::toggle_validation_palette(cx)),
        );
        let text = self.settings_group("Text").child(self.settings_stepper(
            0,
            "Text size",
            self.preferences.font_size,
            cx,
        ));
        let code = self
            .settings_group("Code")
            .child(self.settings_stepper(3, "Code text size", self.preferences.code_size, cx))
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
                .on_click(cx.listener(|v, _, w, cx| {
                    v.preferences.compact_diff = !v.preferences.compact_diff;
                    v.persist_preferences(w, cx);
                    cx.notify();
                })),
            );
        let layout = self
            .settings_group("Panel layout")
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
                .on_click(cx.listener(|v, _, w, cx| {
                    v.compact = !v.compact;
                    v.persist_preferences(w, cx);
                    cx.notify();
                })),
            )
            .child(
                self.action_button(
                    "file-actions-visibility",
                    if self.preferences.always_show_file_actions {
                        "File actions: Always visible · larger targets"
                    } else {
                        "File actions: Reveal on hover or focus"
                    },
                )
                .tab_stop(true)
                .on_click(cx.listener(|v, _, w, cx| {
                    v.preferences.always_show_file_actions =
                        !v.preferences.always_show_file_actions;
                    v.persist_preferences(w, cx);
                    cx.notify();
                })),
            )
            .child(self.settings_stepper(1, "Navigation width", self.preferences.nav_width, cx))
            .child(self.settings_stepper(
                2,
                "Inspector width",
                self.preferences.inspector_width,
                cx,
            ))
            .child(muted(
                "Panel widths can also be dragged. These controls provide keyboard alternatives.",
            ))
            .child(
                self.action_button("reset-appearance", "Reset appearance and layout")
                    .tab_stop(true)
                    .on_click(cx.listener(|v, _, w, cx| {
                        let recent = v.preferences.recent.clone();
                        v.preferences = Preferences::default();
                        v.preferences.recent = recent;
                        v.compact = false;
                        v.split = false;
                        v.layout_revision += 1;
                        v.reset_panels(cx);
                        crate::preferences::set_app_appearance(
                            v.preferences.font_size,
                            v.preferences.theme,
                            cx,
                        );
                        v.persist_preferences(w, cx);
                        cx.notify();
                    })),
            );
        let about = self.settings_group("About").child(muted(concat!(
            "GitCrab ",
            env!("CARGO_PKG_VERSION"),
            " · ",
            env!("GITCRAB_BUILD")
        )));
        div()
            .id("appearance-settings")
            .max_h(px(height))
            .overflow_y_scroll()
            .w(px(430.))
            .rounded(px(10.))
            .bg_role(Role::SurfaceOverlay)
            .border_1()
            .border_role(Role::BorderDivider)
            .p_4()
            .flex()
            .flex_col()
            .gap_4()
            .child(theme)
            .child(text)
            .child(code)
            .child(layout)
            .child(about)
            .into_any_element()
    }
}
