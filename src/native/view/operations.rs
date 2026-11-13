use super::*;

type OperationRequest = (u32, u32);

/// Dialog state keyed by repository epoch and sequence while a request is active.
#[expect(
    clippy::large_enum_variant,
    reason = "a single dialog per window; boxing would only add indirection"
)]
pub(super) enum OperationDialog {
    Editing { error: Option<String> },
    Resolving(OperationRequest),
    Ready(ReviewedOperation),
    Executing(OperationRequest),
    Finished(String),
}

impl OperationDialog {
    fn resolving(&self) -> bool {
        matches!(self, Self::Resolving(_))
    }

    fn edit(&mut self) -> bool {
        if matches!(self, Self::Executing(_) | Self::Finished(_)) {
            return false;
        }
        *self = Self::Editing { error: None };
        true
    }

    fn resolved(&mut self, request: OperationRequest, result: Result<ReviewedOperation, String>) {
        if matches!(self, Self::Resolving(owner) if *owner == request) {
            *self = match result {
                Ok(reviewed) => Self::Ready(reviewed),
                Err(error) => Self::Editing { error: Some(error) },
            };
        }
    }

    fn finished(&mut self, request: OperationRequest, result: Result<String, String>) {
        if matches!(self, Self::Executing(owner) if *owner == request) {
            *self = match result {
                Ok(message) => Self::Finished(message),
                Err(error) => Self::Editing { error: Some(error) },
            };
        }
    }
}

impl GitCrab {
    /// Opens target review against the current snapshot's HEAD and index.
    pub(super) fn review(
        &mut self,
        op: Operation,
        target: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.state.snapshot.as_ref() else {
            return;
        };
        if self.state.busy || self.operation_dialog.is_some() {
            return;
        }
        let Some(destination) = snapshot.reviewed.clone() else {
            self.state.notice = "Refresh the repository before reviewing an operation.".into();
            cx.notify();
            return;
        };
        let head = snapshot.head_label();
        self.target.update(cx, |s, cx| {
            s.set_placeholder(op.placeholder(), window, cx);
            s.set_value(target, window, cx);
        });
        let target = self.target.clone();
        let model = cx.new(|_| OperationDialog::Editing { error: None });
        self.operation_dialog = Some(model.clone());
        let view = cx.weak_entity();
        let cancel = view.clone();
        let epoch = self.state.epoch;
        let repo = self.state.path.clone();
        let pop_stash = matches!(op, Operation::PopStash);
        // Undo and Redo act on the journal's top entry, so the review starts at once.
        let journal = op.journal_direction().is_some();
        // Pull and Push act on the checked-out branch, so they also start at once.
        let network = op.is_network();
        let journal_review = (journal || network).then(|| (op.clone(), destination.clone()));
        // Before the review resolves, name the refs from the snapshot.
        let snapshot_refs = snapshot.remote.network_upstream().map(|upstream| {
            let branch = snapshot.checked_out_branch().unwrap_or_default();
            match op {
                Operation::Push => format!("{branch} → {}", upstream.name),
                _ => format!("{} → {branch}", upstream.name),
            }
        });
        let label = op.label();
        // Values already visible in the app, offered so the user need not type them.
        let current = snapshot.checked_out_branch().map(str::to_owned);
        let choices: Vec<(String, String)> = if op.takes_revision() {
            let mut choices: Vec<(String, String)> = snapshot
                .branches
                .iter()
                .filter(|branch| current.as_deref() != Some(branch.name.as_str()))
                .map(|branch| (branch.name.clone(), branch.name.clone()))
                .collect();
            if let Some(id) = self.state.selected.clone() {
                let subject = snapshot
                    .commits
                    .iter()
                    .find(|c| c.id == id)
                    .map(|c| c.subject.clone())
                    .unwrap_or_default();
                choices.insert(
                    0,
                    (
                        format!("Selected commit {} · {subject}", &id[..id.len().min(8)]),
                        id,
                    ),
                );
            }
            choices
        } else {
            vec![]
        };
        let latest_stash = snapshot.latest_stash.clone();
        let head_name = head.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let view = view.clone();
            let cancel = cancel.clone();
            let target = target.clone();
            let op = op.clone();
            let target_value = target.read(cx).value();
            let validation = op.validate_target(target_value.as_ref()).err();
            let phase = model.read(cx);
            // Blank input is explained when the user tries to continue, not on open.
            let typed = !target_value.trim().is_empty();
            let issue = match phase {
                OperationDialog::Editing { error } => {
                    error.clone().or(validation.filter(|_| typed))
                }
                _ => None,
            };
            let executing = matches!(phase, OperationDialog::Executing(_));
            let finished = matches!(phase, OperationDialog::Finished(_));
            let resolving = phase.resolving();
            let reviewed = match phase {
                OperationDialog::Ready(reviewed) if reviewed.target == target_value.as_ref() => {
                    Some(reviewed)
                }
                _ => None,
            };
            let status = match phase {
                OperationDialog::Resolving(_) => Some("Resolving the target…".into()),
                OperationDialog::Executing(_) => Some(
                    "Git is running. If you dismiss this dialog, see Activity for the result."
                        .into(),
                ),
                OperationDialog::Finished(message) => Some(message.clone()),
                _ => None,
            };
            let submit_label = match reviewed {
                Some(reviewed) => reviewed.title(),
                None if journal || network => "Review",
                None => "Review target",
            };
            let title = reviewed.map_or(label, |reviewed| reviewed.title());
            let consequence = reviewed.map(|reviewed| reviewed.describe(&head_name));
            let refs = reviewed
                .and_then(ReviewedOperation::refs)
                .or_else(|| snapshot_refs.clone());
            let target_id = reviewed.and_then(|reviewed| reviewed.target_id.clone());
            let resolved = reviewed.and_then(|reviewed| {
                let summary = reviewed.target_summary.as_ref()?;
                Some(format!(
                    "{} · {} · {}",
                    reviewed.target_id.as_deref().unwrap_or_default(),
                    summary.subject,
                    display_date(&summary.date)
                ))
            });
            let choices = choices.clone();
            let stash_identity = latest_stash
                .as_ref()
                .filter(|_| pop_stash)
                .map(|stash| format!(
                    "stash@{{0}} · {} · {}",
                    stash.message,
                    display_date(&stash.date)
                ));
            let submit = view.clone();
            let submit_op = op.clone();
            let submit_destination = destination.clone();
            let ok_destination = destination.clone();
            let submit_model = model.clone();
            let ok_model = model.clone();
            let cancel_model = model.clone();
            dialog
                .title(title)
                .confirm()
                .footer(move |_, cancel, w, cx| {
                    let mut buttons = vec![cancel(w, cx)];
                    if executing && network {
                        // Dismiss leaves Git running; this stops it.
                        let stop = submit.clone();
                        buttons.push(
                            components::basic(
                                Button::new("operation-cancel-network").label(format!(
                                    "Cancel {}",
                                    if matches!(submit_op, Operation::Push) {
                                        "push"
                                    } else {
                                        "pull"
                                    }
                                )),
                                components::ButtonKind::Secondary,
                                ControlSize::Regular,
                            )
                            .on_click(move |_, _, cx| {
                                let _ = stop.update(cx, |v, cx| v.cancel_network(cx));
                            })
                            .into_any_element(),
                        );
                    }
                    if !executing && !finished {
                        buttons.push(
                            components::basic(
                                Button::new("operation-submit").label(submit_label),
                                components::ButtonKind::Primary,
                                ControlSize::Regular,
                            )
                            .disabled_when(resolving)
                                .on_click({
                                    let submit = submit.clone();
                                    let op = submit_op.clone();
                                    let destination = submit_destination.clone();
                                    let model = submit_model.clone();
                                    move |_, w, cx| {
                                        let _ = submit.update(cx, |v, cx| {
                                            v.confirm_operation(
                                                &model,
                                                op.clone(),
                                                epoch,
                                                destination.clone(),
                                                w,
                                                cx,
                                            );
                                        });
                                    }
                                })
                                .into_any_element(),
                        );
                    }
                    buttons
                })
                .button_props(
                    gpui_component::dialog::DialogButtonProps::default()
                        .ok_text(submit_label)
                        .cancel_text(if executing {
                            "Dismiss"
                        } else if finished {
                            "Close"
                        } else {
                            "Cancel"
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .p_3()
                                .rounded(px(6.))
                                .bg(rgb(semantic(Role::SurfaceCanvas)))
                                .border_1()
                                .border_color(rgb(semantic(Role::BorderDivider)))
                                .child(
                                    div()
                                        .text_size(type_size(cx, 12.))
                                        .text_color(rgb(semantic(Role::TextSecondary)))
                                        .child("REPOSITORY"),
                                )
                                .child(
                                    div()
                                        .mt_1()
                                        .text_size(type_size(cx, 12.))
                                        .child(repo.clone()),
                                )
                                .child(div().mt_2().child(badge(
                                    format!(
                                        "Current HEAD · {head} · {}",
                                        &destination.head[..destination.head.len().min(12)]
                                    ),
                                    semantic(Role::ActionPrimary),
                                ))),
                        )
                        .child(div().font_weight(FontWeight::MEDIUM).child(match op {
                            Operation::CreateBranch | Operation::CreateBranchAt { .. } => {
                                "New branch name"
                            }
                            Operation::Stash => "Stash message (optional)",
                            Operation::PopStash => "Latest stash",
                            Operation::Undo | Operation::Redo => "Most recent action in this tab",
                            Operation::Pull { .. } | Operation::Push => "Source → destination",
                            _ => "Target branch or commit",
                        }))
                        .when_some(refs, |e, refs| {
                            e.child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(refs),
                            )
                        })
                        .when_some(stash_identity, |e, identity| {
                            e.child(div().text_size(type_size(cx, 12.)).child(identity))
                        })
                        .when(!matches!(op, Operation::PopStash | Operation::Undo | Operation::Redo) && !network, |e| {
                            e.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .child(Input::new(&target).disabled(executing || finished)),
                                    )
                                    .when(!choices.is_empty() && !executing && !finished, |row| {
                                        let target = target.clone();
                                        let choices = choices.clone();
                                        row.child(
                                            Button::new("operation-pick")
                                                .label("Choose…")
                                                .outline()
                                                .small()
                                                .h(px(CONTROL_HEIGHT))
                                                .tooltip("Pick a branch or the selected commit, or type any revision")
                                                .dropdown_menu(move |mut menu, _, _| {
                                                    for (label, value) in &choices {
                                                        let target = target.clone();
                                                        let value = value.clone();
                                                        menu = menu.item(
                                                            PopupMenuItem::new(label.clone())
                                                                .on_click(move |_, w, cx| {
                                                                    target.update(cx, |s, cx| {
                                                                        s.set_value(
                                                                            value.clone(),
                                                                            w,
                                                                            cx,
                                                                        )
                                                                    });
                                                                }),
                                                        );
                                                    }
                                                    menu
                                                }),
                                        )
                                    }),
                            )
                        })
                        .when_some(consequence, |e, consequence| {
                            e.child(
                                div()
                                    .p_2()
                                    .rounded(px(6.))
                                    .bg(rgb(semantic(Role::StatusInfoBg)))
                                    .text_color(rgb(semantic(Role::TextDefault)))
                                    .child(consequence),
                            )
                        })
                        .when_some(resolved.or(target_id.map(|id| id.to_string())), |e, id| {
                            e.child(
                                div()
                                    .text_size(type_size(cx, 12.))
                                    .text_color(rgb(semantic(Role::TextSecondary)))
                                    .child(format!("Reviewed object · {id}")),
                            )
                        })
                        .when_some(
                            match &op {
                                Operation::CreateBranchAt { start } => Some(start.clone()),
                                _ => None,
                            },
                            |e, start| e.child(format!("Starting at {}", start)),
                        )
                        .child(
                            div()
                                .text_size(type_size(cx, 12.))
                                .text_color(rgb(semantic(Role::TextMuted)))
                                .child(op.help()),
                        )
                        .when_some(issue, |e, issue| {
                            e.child(
                                div()
                                    .text_color(rgb(semantic(Role::StatusErrorFg)))
                                    .child(issue),
                            )
                        })
                        .when_some(status, |e, status| e.child(status)),
                )
                .on_ok(move |_, w, cx| {
                    view.update(cx, |v, cx| {
                        if finished {
                            return v.dismiss_operation(&ok_model, cx);
                        }
                        v.confirm_operation(
                            &ok_model,
                            op.clone(),
                            epoch,
                            ok_destination.clone(),
                            w,
                            cx,
                        );
                        false
                    })
                    .unwrap_or(true)
                })
                .on_cancel(move |_, _, cx| {
                    cancel
                        .update(cx, |v, cx| v.dismiss_operation(&cancel_model, cx))
                        .unwrap_or(true)
                })
        });
        // Popping a stash has no editable target, so keep focus in the dialog.
        if !pop_stash && !journal && !network {
            self.target.update(cx, |s, cx| s.focus(window, cx));
        }
        if let (Some((op, destination)), Some(model)) =
            (journal_review, self.operation_dialog.clone())
        {
            self.confirm_operation(&model, op, epoch, destination, window, cx);
        }
        cx.notify();
    }
    fn cancel_resolution(&mut self) {
        self.state.cancel_operation_review();
        self.service.cancel_read(ReadKind::OperationReview);
    }

    fn dismiss_operation(
        &mut self,
        model: &Entity<OperationDialog>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.operation_dialog.as_ref() != Some(model) {
            return false;
        }
        // Escape and the footer share this dismissal path. Only a read
        // resolution is cancelled. The tab still records executing writes.
        if model.read(cx).resolving() {
            self.cancel_resolution();
        }
        self.operation_dialog = None;
        cx.notify();
        true
    }

    pub(super) fn operation_target_changed(&mut self, cx: &mut Context<Self>) {
        if let Some(model) = self.operation_dialog.clone() {
            let resolving = model.read(cx).resolving();
            if model.update(cx, |state, cx| {
                let edited = state.edit();
                if edited {
                    cx.notify();
                }
                edited
            }) {
                if resolving {
                    self.cancel_resolution();
                }
                cx.notify();
            }
        }
    }

    pub(super) fn operation_resolved(
        &mut self,
        request: OperationRequest,
        result: Result<ReviewedOperation, String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(model) = &self.operation_dialog {
            model.update(cx, |state, cx| {
                state.resolved(request, result);
                cx.notify();
            });
        }
    }

    pub(super) fn operation_finished(
        &mut self,
        request: OperationRequest,
        result: Result<String, String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(model) = &self.operation_dialog {
            model.update(cx, |state, cx| {
                state.finished(request, result);
                cx.notify();
            });
        }
    }
    fn confirm_operation(
        &mut self,
        model: &Entity<OperationDialog>,
        op: Operation,
        epoch: u32,
        destination: gitcrab_core::git::WriteIdentity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.operation_dialog.as_ref() != Some(model) || self.state.busy {
            return;
        }
        if self.state.epoch != epoch {
            model.update(cx, |state, cx| {
                *state = OperationDialog::Editing {
                    error: Some("Repository changed. Open a new review.".into()),
                };
                cx.notify();
            });
            return;
        }
        let target = self.target.read(cx).value().to_string();
        if let Err(error) = op.validate_target(&target) {
            // Explain the problem now that the user tried to continue.
            model.update(cx, |state, cx| {
                if matches!(state, OperationDialog::Editing { .. }) {
                    *state = OperationDialog::Editing { error: Some(error) };
                    cx.notify();
                }
            });
            return;
        }
        let reviewed = match model.read(cx) {
            OperationDialog::Ready(reviewed) if reviewed.target == target => Some(reviewed.clone()),
            OperationDialog::Ready(_) | OperationDialog::Editing { .. } => None,
            _ => return,
        };
        let (phase, job) = if let Some(reviewed) = reviewed {
            let Some(job) = self.state.mutate(Action::Operation { reviewed }) else {
                return;
            };
            (
                OperationDialog::Executing((job.1.epoch, job.1.sequence)),
                Some(job),
            )
        } else {
            let (sequence, job) = self.state.review_operation(op, target, destination);
            (OperationDialog::Resolving((epoch, sequence)), job)
        };
        let executing = matches!(phase, OperationDialog::Executing(_));
        model.update(cx, |state, cx| {
            *state = phase;
            cx.notify();
        });
        if executing {
            self.dispatch_mutation(job, window, cx);
        } else {
            self.dispatch(job, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OperationDialog;

    #[test]
    fn editing_during_resolution_rejects_its_late_feedback() {
        let mut dialog = OperationDialog::Resolving((1, 1));
        assert!(dialog.resolving());
        assert!(dialog.edit());
        dialog.resolved((1, 1), Err("obsolete target".into()));
        assert!(matches!(dialog, OperationDialog::Editing { error: None }));

        dialog = OperationDialog::Resolving((1, 2));
        dialog.resolved((1, 1), Err("obsolete target".into()));
        assert!(matches!(dialog, OperationDialog::Resolving((1, 2))));
        dialog.resolved((1, 2), Err("current target failed".into()));
        assert!(
            matches!(dialog, OperationDialog::Editing { error: Some(error) } if error == "current target failed")
        );
    }

    #[test]
    fn executing_ignores_edits_and_only_its_owner_can_finish_it() {
        let mut dialog = OperationDialog::Executing((2, 1));
        assert!(!dialog.edit());
        dialog.finished((1, 1), Ok("obsolete write".into()));
        assert!(matches!(dialog, OperationDialog::Executing((2, 1))));
        dialog.finished((2, 1), Ok("current write finished".into()));
        assert!(!dialog.edit());
        assert!(
            matches!(dialog, OperationDialog::Finished(message) if message == "current write finished")
        );
    }

    #[test]
    fn dismissing_a_write_leaves_a_later_dialog_unaffected() {
        let executing = OperationDialog::Executing((1, 3));
        assert!(!executing.resolving());
        let mut next_dialog = OperationDialog::Resolving((1, 4));
        next_dialog.finished((1, 3), Err("previous write failed".into()));
        assert!(matches!(next_dialog, OperationDialog::Resolving((1, 4))));
    }
}
