use super::*;
use gpui::component::switch::Switch;

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
        // Update choices are app-wide: show what another tab may have set.
        let shared = crate::preferences::current(cx);
        for preferences in [&mut self.preferences, &mut self.saved_preferences] {
            preferences.check_for_updates = shared.check_for_updates;
            preferences.update_channel = shared.update_channel;
        }
        let weak = cx.weak_entity();
        let close = weak.clone();
        let scroll = self.settings_scroll.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let content = weak.update(cx, |v, cx| v.settings_dialog(cx)).ok();
            let status = weak
                .read_with(cx, |v, _| v.appearance_status.clone())
                .unwrap_or_default();
            let close = close.clone();
            let footer_close = close.clone();
            let (text, role) = match &status {
                AppearanceStatus::Idle => (String::new(), Role::TextMuted),
                AppearanceStatus::Saving(_) => ("Saving…".to_owned(), Role::TextMuted),
                AppearanceStatus::Saved => ("Saved".to_owned(), Role::StatusSuccessFg),
                AppearanceStatus::Failed(error) => {
                    (format!("Could not save: {error}"), Role::StatusErrorFg)
                }
            };
            let footer = ModalFooter::new()
                .leading(
                    div()
                        .id("appearance-save-status")
                        .min_w_0()
                        .truncate()
                        .text_size(scaled(12.))
                        .text_role(role)
                        .child(text),
                )
                .action(
                    components::style(
                        Button::new("settings-close").label("Done").tab_stop(true),
                        components::ButtonKind::Primary,
                        ControlSize::Regular,
                        cx,
                    )
                    .on_click(move |_, w, cx| {
                        // The same persistence path as Escape and dismissal.
                        let _ = footer_close.update(cx, |v, cx| v.finish_settings(w, cx));
                        w.close_dialog(cx);
                    }),
                );
            dialog
                .frame(
                    ModalHeader::new("Appearance")
                        .description("Changes apply immediately and save automatically.")
                        .closable(),
                    ModalWidth::Medium,
                    cx,
                )
                .body(&scroll, modal_body().children(content))
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

    /// A two-way choice drawn as a segmented control. `select` applies the
    /// chosen option's index and reports whether anything changed, which
    /// saves the preferences.
    fn settings_choice(
        &self,
        id: &'static str,
        options: [(&'static str, bool); 2],
        select: impl Fn(&mut Self, usize, &mut Window, &mut Context<Self>) -> bool + Clone + 'static,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut group = segmented_group();
        for (index, (label, selected)) in options.into_iter().enumerate() {
            let select = select.clone();
            group = group.child(
                self.segment_button((id, index), label, selected)
                    .tab_stop(true)
                    .on_click(cx.listener(move |v, _, w, cx| {
                        if select(v, index, w, cx) {
                            v.persist_preferences(w, cx);
                            cx.notify();
                        }
                    })),
            );
        }
        group
    }

    /// A value with keyboard-reachable decrease and increase buttons.
    /// `label` names the value in the buttons' tooltips.
    fn settings_stepper(
        &self,
        index: usize,
        label: &'static str,
        value: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut row = segmented_group();
        for direction in [-1., 1.] {
            if direction > 0. {
                row = row.child(
                    div()
                        .min_w(scaled(40.))
                        .h(scaled(CONTROL_HEIGHT))
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_x_1()
                        .border_role(Role::BorderDivider)
                        .text_size(scaled(12.))
                        .text_role(Role::TextDefault)
                        .child(format!("{value:.0}")),
                );
            }
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

    /// Automatic checks, the release channel and a manual check with its result.
    fn update_settings(&self, cx: &mut Context<Self>) -> Div {
        use crate::updates::{InstallSupport, Status, UpdateChannel, CURRENT_VERSION};
        let automatic = self.preferences.check_for_updates;
        let channel = UpdateChannel::effective(self.preferences.update_channel, CURRENT_VERSION);
        let status = crate::updates::status(cx);
        let channels = self.settings_choice(
            "update-channel",
            [
                (
                    UpdateChannel::Stable.label(),
                    channel == UpdateChannel::Stable,
                ),
                (
                    UpdateChannel::Prerelease.label(),
                    channel == UpdateChannel::Prerelease,
                ),
            ],
            |v, index, _, cx| {
                let option = [UpdateChannel::Stable, UpdateChannel::Prerelease][index];
                if v.preferences.update_channel == Some(option) {
                    return false;
                }
                let previous =
                    UpdateChannel::effective(v.preferences.update_channel, CURRENT_VERSION);
                v.preferences.update_channel = Some(option);
                if previous != option {
                    crate::updates::channel_changed(cx);
                }
                true
            },
            cx,
        );
        let installing = crate::updates::install_state(cx).in_progress();
        let mut install = None;
        let (text, role) = match &status {
            Status::Idle => (String::new(), Role::TextMuted),
            Status::Checking => ("Checking…".to_owned(), Role::TextMuted),
            Status::UpToDate => ("GitCrab is up to date.".to_owned(), Role::StatusSuccessFg),
            Status::Available(update) => {
                let mut text = if crate::updates::is_skipped(&update.version, cx) {
                    format!("GitCrab {} is available. You skipped it.", update.version)
                } else {
                    format!("GitCrab {} is available.", update.version)
                };
                let support = crate::updates::install_support(cx);
                if let Some(InstallSupport::Unsupported(reason)) = &support {
                    text.push(' ');
                    text.push_str(reason);
                }
                install = Some(self.update_install(update, support, cx));
                (text, Role::StatusInfoFg)
            }
            Status::Failed(error) => (
                format!("Could not check for updates: {error}"),
                Role::StatusErrorFg,
            ),
        };
        let checking = status == Status::Checking || installing;
        let check = self
            .action_button("check-updates-now", "Check now")
            .tab_stop(!checking)
            .disabled_when(checking)
            .on_click(cx.listener(move |_, _, _, cx| {
                crate::updates::check_now(channel, cx);
                cx.notify();
            }));
        let automatic_switch = Switch::new("automatic-update-checks")
            .checked(automatic)
            .tab_stop(true)
            .accessibility_label("Check for updates automatically")
            .on_click(cx.listener(|v, checked: &bool, w, cx| {
                v.preferences.check_for_updates = *checked;
                v.persist_preferences(w, cx);
                cx.notify();
            }));
        let mut rows = vec![
            modal::setting_row("Check for updates automatically", None, automatic_switch)
                .into_any_element(),
            modal::setting_row(
                "Update channel",
                Some(
                    if self.preferences.update_channel.is_some() {
                        "Prerelease includes stable releases."
                    } else {
                        "Following this build's channel. Prerelease includes stable releases."
                    }
                    .into(),
                ),
                channels,
            )
            .into_any_element(),
            modal::setting_row(
                format!("GitCrab {CURRENT_VERSION}"),
                Some(crate::BUILD.into()),
                check,
            )
            .into_any_element(),
        ];
        if !text.is_empty() || install.is_some() {
            rows.push(
                div()
                    .py(scaled(4.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .when(!text.is_empty(), |el| {
                        el.child(
                            div()
                                .id("update-check-status")
                                .text_size(scaled(12.))
                                .line_height(relative(1.45))
                                .text_role(role)
                                .child(text),
                        )
                    })
                    .children(install)
                    .into_any_element(),
            );
        }
        modal::settings_card("Updates", rows)
    }

    /// Installing the available release from here: the install button, its
    /// progress or error, and the release page. The workspace closes this
    /// dialog just before installing.
    fn update_install(
        &self,
        update: &crate::updates::AvailableUpdate,
        support: Option<crate::updates::InstallSupport>,
        cx: &mut Context<Self>,
    ) -> Div {
        use crate::updates::{Install, InstallSupport};
        let install = crate::updates::install_state(cx);
        let url = update.release_url.clone();
        let notes = self
            .action_button("settings-release-notes", "Release notes")
            .tab_stop(true)
            .tooltip(url.clone())
            .on_click(move |_, _, cx| cx.open_url(&url));
        let mut row = div().flex().items_center().gap_2();
        if support == Some(InstallSupport::Supported) {
            let progress = match &install {
                Install::Downloading(Some(percent)) => {
                    Some((format!("Downloading {percent}%"), Some(f32::from(*percent))))
                }
                Install::Downloading(None) => Some(("Downloading…".to_owned(), None)),
                Install::Installing => Some(("Installing…".to_owned(), None)),
                Install::Idle | Install::Failed(_) => None,
            };
            row = match progress {
                Some((label, value)) => row.child(
                    div()
                        .id("settings-update-progress")
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(scaled(12.))
                        .text_role(Role::TextMuted)
                        .child(label.clone())
                        .child(update_progress_bar(
                            "settings-update-progress-bar",
                            value,
                            label,
                        )),
                ),
                None => {
                    let version = update.version.clone();
                    let focus = self.focus.clone();
                    row.child(
                        components::basic(
                            Button::new("settings-install-update").label(
                                if matches!(install, Install::Failed(_)) {
                                    "Try again"
                                } else {
                                    "Install and restart"
                                },
                            ),
                            components::ButtonKind::Primary,
                            ControlSize::Compact,
                        )
                        .tab_stop(true)
                        .tooltip("Download, verify and install this version, then restart GitCrab")
                        // Dispatched from the tab's own focus node so the
                        // workspace, which owns restarting, handles it.
                        .on_click(move |_, w, cx| {
                            focus.dispatch_action(
                                &crate::workspace::InstallUpdateFromSettings(version.clone()),
                                w,
                                cx,
                            )
                        }),
                    )
                }
            };
        }
        let failed = match install {
            Install::Failed(error) => Some(error),
            _ => None,
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .when_some(failed, |el, error| {
                el.child(
                    div()
                        .id("settings-update-error")
                        .text_size(scaled(12.))
                        .text_role(Role::StatusErrorFg)
                        .child(error),
                )
            })
            .child(row.child(notes))
    }

    /// Closes Settings the way its Close button does, for an update that
    /// installs and restarts next.
    pub fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            self.finish_settings(window, cx);
            window.close_dialog(cx);
        }
    }

    pub(super) fn settings_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        // Every preference resolves to the dark palette, so state it plainly
        // instead of offering a choice with no effect.
        let theme = modal::setting_row(
            "Theme",
            Some("GitCrab currently has one theme.".into()),
            div().text_role(Role::TextSecondary).child("Dark"),
        );
        #[cfg(feature = "native-evidence")]
        let theme = theme.child(
            self.action_button("validation-palette", "Validation: swap palette")
                .on_click(|_, _, cx| super::super::theme::toggle_validation_palette(cx)),
        );
        let appearance = modal::settings_card(
            "Theme and text",
            vec![
                theme.into_any_element(),
                modal::setting_row(
                    "Text size",
                    None,
                    self.settings_stepper(0, "Text size", self.preferences.font_size, cx),
                )
                .into_any_element(),
            ],
        );
        let compact_diff = self.preferences.compact_diff;
        let code = modal::settings_card(
            "Code",
            vec![
                modal::setting_row(
                    "Code text size",
                    None,
                    self.settings_stepper(3, "Code text size", self.preferences.code_size, cx),
                )
                .into_any_element(),
                modal::setting_row(
                    "Code spacing",
                    None,
                    self.settings_choice(
                        "diff-density",
                        [("Comfortable", !compact_diff), ("Compact", compact_diff)],
                        |v, index, _, _| {
                            let compact = index == 1;
                            let changed = v.preferences.compact_diff != compact;
                            v.preferences.compact_diff = compact;
                            changed
                        },
                        cx,
                    ),
                )
                .into_any_element(),
            ],
        );
        let always = self.preferences.always_show_file_actions;
        let layout = modal::settings_card(
            "Layout",
            vec![
                modal::setting_row(
                    "Density",
                    None,
                    self.settings_choice(
                        "settings-density",
                        [("Comfortable", !self.compact), ("Compact", self.compact)],
                        |v, index, _, _| {
                            let compact = index == 1;
                            let changed = v.compact != compact;
                            v.compact = compact;
                            changed
                        },
                        cx,
                    ),
                )
                .into_any_element(),
                modal::setting_row(
                    "File actions",
                    Some("Show stage and unstage buttons on every row, or only on hover.".into()),
                    self.settings_choice(
                        "file-actions-visibility",
                        [("On hover", !always), ("Always", always)],
                        |v, index, _, _| {
                            let always = index == 1;
                            let changed = v.preferences.always_show_file_actions != always;
                            v.preferences.always_show_file_actions = always;
                            changed
                        },
                        cx,
                    ),
                )
                .into_any_element(),
                modal::setting_row(
                    "Navigation width",
                    None,
                    self.settings_stepper(1, "Navigation width", self.preferences.nav_width, cx),
                )
                .into_any_element(),
                modal::setting_row(
                    "Inspector width",
                    None,
                    self.settings_stepper(
                        2,
                        "Inspector width",
                        self.preferences.inspector_width,
                        cx,
                    ),
                )
                .into_any_element(),
                modal::setting_row(
                    "Reset appearance and layout",
                    Some("Restores text sizes, density and panel widths.".into()),
                    self.action_button("reset-appearance", "Reset")
                        .tab_stop(true)
                        .on_click(cx.listener(|v, _, w, cx| {
                            let kept = v.preferences.clone();
                            v.preferences = Preferences {
                                recent: kept.recent,
                                check_for_updates: kept.check_for_updates,
                                update_channel: kept.update_channel,
                                ..Preferences::default()
                            };
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
                )
                .into_any_element(),
            ],
        )
        .child(modal::fine_print(
            "You can also drag panel edges to resize them.",
        ));
        let updates = self.update_settings(cx);
        div()
            .id("appearance-settings")
            .w_full()
            .flex()
            .flex_col()
            .gap(scaled(20.))
            .child(appearance)
            .child(code)
            .child(layout)
            .child(updates)
            .into_any_element()
    }
}
