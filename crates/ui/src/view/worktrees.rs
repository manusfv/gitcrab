//! Worktree actions, following GitKraken: the worktree context menu, the
//! Create worktree dialog, and the prompts before removing, locking and
//! after creating a worktree.
use super::*;
use crate::workspace::OpenRecent;
use components::{modal::field, ButtonKind, ContextMenu, MenuTarget};
use gitcrab_core::git::{RepoSnapshot, Worktree};
use gitcrab_core::worktree::{self as plan, Level, WorktreeChange};
use std::path::PathBuf;

/// What runs once every confirmation is accepted.
type Then = std::rc::Rc<dyn Fn(&mut GitCrab, &mut Window, &mut Context<GitCrab>)>;

/// A removal of the worktree a tab showed, waiting for the tab to finish
/// opening the main worktree.
pub(super) struct PendingRemoval {
    main: PathBuf,
    change: WorktreeChange,
    /// The repository epoch that opening `main` starts.
    epoch: u32,
}

/// How much a removal deletes besides the worktree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Extent {
    Worktree,
    Branch,
    BranchAndRemote,
}

/// A worktree menu item.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum WorktreeCommand {
    Open(String),
    Remove(Box<Worktree>, Extent),
    Lock(PathBuf),
    Unlock(PathBuf),
    Prune,
    /// Opens the Create worktree dialog, starting from this reference.
    Create(Option<String>),
}

/// Builds the menu GitKraken shows for a worktree.
pub(super) fn worktree_menu(
    view: WeakEntity<GitCrab>,
    worktree: &Worktree,
    snapshot: &RepoSnapshot,
    window: &Window,
) -> ContextMenu {
    let target = MenuTarget::new(window, view);
    let run = |command: WorktreeCommand| target.command(MenuCommand::Worktree(command));
    let path = worktree.path.display().to_string();
    let mut menu = ContextMenu::new();
    if !worktree.prunable && !worktree.current {
        menu = menu
            .item(
                "Open this worktree",
                true,
                run(WorktreeCommand::Open(path.clone())),
            )
            .item("Open worktree in new tab", true, OpenRecent(path));
    }
    menu = menu.separator();
    if !worktree.main {
        let remove = |extent| run(WorktreeCommand::Remove(Box::new(worktree.clone()), extent));
        menu = menu.item("Remove this worktree", true, remove(Extent::Worktree));
        if let Some(branch) = &worktree.branch {
            menu = menu.item(
                "Remove worktree and delete branch",
                true,
                remove(Extent::Branch),
            );
            if upstream(snapshot, branch).is_some() {
                menu = menu.item(
                    "Remove worktree and delete branch (local & remote)",
                    true,
                    remove(Extent::BranchAndRemote),
                );
            }
        }
    }
    menu = menu.separator();
    if !worktree.main && !worktree.locked {
        menu = menu.item(
            "Lock this worktree",
            true,
            run(WorktreeCommand::Lock(worktree.path.clone())),
        );
    }
    if worktree.locked {
        menu = menu.item(
            "Unlock this worktree",
            true,
            run(WorktreeCommand::Unlock(worktree.path.clone())),
        );
    }
    if worktree.prunable {
        menu = menu
            .separator()
            .item("Prune worktrees", true, run(WorktreeCommand::Prune));
    }
    menu
}

fn upstream(snapshot: &RepoSnapshot, branch: &str) -> Option<(String, String)> {
    snapshot
        .branches
        .iter()
        .find(|b| b.name == branch)?
        .upstream
        .clone()
}

/// The Create worktree dialog's fields and its live check.
struct CreateForm {
    snapshot: RepoSnapshot,
    reference: Entity<InputState>,
    branch: Entity<InputState>,
    path: Entity<InputState>,
    suggestion: plan::Suggestion,
    message: Option<(Level, String)>,
    view: WeakEntity<GitCrab>,
    _subscriptions: Vec<Subscription>,
}

impl CreateForm {
    fn new(
        snapshot: RepoSnapshot,
        start: Option<String>,
        view: WeakEntity<GitCrab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let reference = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Branch, remote branch or commit")
                .default_value(start.unwrap_or_default())
        });
        let branch = cx.new(|cx| InputState::new(window, cx));
        let path = cx.new(|cx| InputState::new(window, cx));
        let subscriptions =
            [&reference, &branch, &path]
                .into_iter()
                .map(|input| {
                    cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| {
                        match event {
                            InputEvent::Change => this.update(window, cx),
                            InputEvent::PressEnter {
                                secondary: false, ..
                            } => this.submit(window, cx),
                            _ => {}
                        }
                    })
                })
                .collect();
        let mut form = Self {
            snapshot,
            reference,
            branch,
            path,
            suggestion: Default::default(),
            message: None,
            view,
            _subscriptions: subscriptions,
        };
        form.update(window, cx);
        form
    }

    fn values(&self, cx: &App) -> (String, String, PathBuf) {
        let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let branch = Some(text(&self.branch))
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| self.suggestion.branch.clone());
        let path = Some(text(&self.path))
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .or_else(|| self.suggestion.path.clone())
            .unwrap_or_default();
        (text(&self.reference), branch, path)
    }

    fn planned(&self, cx: &App) -> Result<(WorktreeChange, Option<String>), (Level, String)> {
        let (start, branch, path) = self.values(cx);
        let exists = !path.as_os_str().is_empty() && path.exists();
        plan::plan(&self.snapshot, &start, &branch, &path, exists)
    }

    /// Refreshes the suggested branch and folder, shown as placeholders, and
    /// the message under the fields.
    fn update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let start = self.reference.read(cx).value().trim().to_owned();
        let typed = self.branch.read(cx).value().trim().to_owned();
        self.suggestion = plan::suggest(&self.snapshot, &start, &typed);
        let branch_hint = plan::suggest(&self.snapshot, &start, "").branch;
        let path_hint = self
            .suggestion
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.branch.update(cx, |input, cx| {
            input.set_placeholder(branch_hint, window, cx)
        });
        self.path
            .update(cx, |input, cx| input.set_placeholder(path_hint, window, cx));
        self.message = match self.planned(cx) {
            Ok((_, warning)) => warning.map(|w| (Level::Warning, w)),
            Err(_) if start.is_empty() => None,
            Err(error) => Some(error),
        };
        cx.notify();
    }

    fn choose_reference(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.reference
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.update(window, cx);
    }

    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose folder".into()),
        });
        cx.spawn_in(window, async move |form, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = form.update_in(cx, |this, window, cx| {
                // The chosen folder holds the new worktree, named like the branch.
                let (_, branch, _) = this.values(cx);
                let text = path.join(branch).display().to_string();
                this.path
                    .update(cx, |input, cx| input.set_value(text, window, cx));
                this.update(window, cx);
            });
        })
        .detach();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let change = match self.planned(cx) {
            Ok((change, _)) => change,
            Err(error) => {
                self.message = Some(error);
                cx.notify();
                return;
            }
        };
        window.close_dialog(cx);
        let _ = self.view.update(cx, |v, cx| {
            if let WorktreeChange::Add { path, .. } = &change {
                v.worktree_created = Some(path.clone());
            }
            v.mutate(Action::Worktree(change), window, cx);
        });
    }
}

impl GitCrab {
    pub(super) fn run_worktree_command(
        &mut self,
        command: WorktreeCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            WorktreeCommand::Open(path) => self.open_path(path, window, cx),
            WorktreeCommand::Remove(worktree, extent) => {
                self.remove_worktree(*worktree, extent, window, cx)
            }
            WorktreeCommand::Lock(path) => self.prompt_lock(path, window, cx),
            WorktreeCommand::Unlock(path) => self.mutate(
                Action::Worktree(WorktreeChange::Unlock { path }),
                window,
                cx,
            ),
            WorktreeCommand::Prune => {
                self.mutate(Action::Worktree(WorktreeChange::Prune), window, cx)
            }
            WorktreeCommand::Create(start) => self.create_worktree(start, window, cx),
        }
    }

    /// Opens the Create worktree dialog with `start` as the reference.
    pub(super) fn create_worktree(
        &mut self,
        start: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.state.snapshot.clone() else {
            return;
        };
        if window.has_active_dialog(cx) {
            return;
        }
        let mut references: Vec<(String, &'static str)> = snapshot
            .branches
            .iter()
            .map(|b| (b.name.clone(), "branch"))
            .collect();
        references.extend(
            snapshot
                .remote_refs
                .iter()
                .filter(|r| !r.label.ends_with("/HEAD"))
                .map(|r| (r.label.clone(), "remote")),
        );
        let view = cx.weak_entity();
        let form = cx.new(|cx| CreateForm::new(snapshot, start, view, window, cx));
        let first = form.read(cx).reference.clone();
        let picker_focus = cx.focus_handle();
        let scroll = ScrollHandle::new();
        window.open_modal(cx, move |dialog, window, cx| {
            let state = form.read(cx);
            let (reference, branch, path) = (
                state.reference.clone(),
                state.branch.clone(),
                state.path.clone(),
            );
            let message = state.message.clone();
            let blocked = matches!(message, Some((Level::Error, _))) || state.planned(cx).is_err();
            let names: Vec<String> = references.iter().map(|(n, _)| n.clone()).collect();
            let picked = form.clone();
            let picker = Picker::new(
                "worktree-reference-picker",
                components::style(
                    Button::new("worktree-reference-choose")
                        .label("Choose…")
                        .tab_stop(true),
                    ButtonKind::Secondary,
                    ControlSize::Regular,
                    cx,
                ),
                px(320.),
                &picker_focus,
                move |index, window, cx| {
                    if let Some(name) = names.get(index).cloned() {
                        picked.update(cx, |form, cx| form.choose_reference(name, window, cx));
                    }
                },
            )
            .search("Search branches", "branches")
            .entries(
                references
                    .iter()
                    .map(|(name, glyph)| PickerItem::new(name.clone()).icon(glyph).into()),
            )
            .empty("This repository has no branches yet.")
            .build(window, cx);
            let choose = form.clone();
            let submit = form.clone();
            let confirm = form.clone();
            let body = modal_body()
                .child(field(
                    "Reference to checkout",
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1().min_w_0().child(Input::new(&reference)))
                        .child(picker),
                ))
                .child(field("Worktree branch to create", Input::new(&branch)))
                .child(field(
                    "Working directory",
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1().min_w_0().child(Input::new(&path)))
                        .child(
                            components::style(
                                Button::new("worktree-choose-folder")
                                    .label("Choose…")
                                    .tab_stop(true),
                                ButtonKind::Secondary,
                                ControlSize::Regular,
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                choose.update(cx, |form, cx| form.choose_folder(window, cx))
                            }),
                        ),
                ))
                .when_some(message, |body, (level, text)| {
                    body.child(modal::callout(
                        match level {
                            Level::Error => Tone::Danger,
                            Level::Warning => Tone::Warning,
                        },
                        text,
                    ))
                });
            let footer = ModalFooter::new()
                .action(components::cancel_button(cx).on_click(|_, window, cx| {
                    window.close_dialog(cx);
                }))
                .action(
                    components::style(
                        Button::new("worktree-create")
                            .label("Create Worktree")
                            .tab_stop(true),
                        ButtonKind::Primary,
                        ControlSize::Regular,
                        cx,
                    )
                    .set_disabled(blocked)
                    .on_click(move |_, window, cx| {
                        submit.update(cx, |form, cx| form.submit(window, cx))
                    }),
                );
            dialog
                .frame(
                    ModalHeader::new("Create Worktree").description(
                        "Check out a branch or commit in its own folder, beside this one.",
                    ),
                    ModalWidth::Medium,
                    cx,
                )
                .body(&scroll, body)
                .footer(footer)
                .on_ok(move |_, window, cx| {
                    confirm.update(cx, |form, cx| form.submit(window, cx));
                    false
                })
        });
        window
            .on_next_frame(move |window, cx| first.update(cx, |input, cx| input.focus(window, cx)));
    }

    /// Asks whether to open a worktree just created, as GitKraken does.
    pub(super) fn prompt_open_created(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let shown = display_path(&path.display().to_string());
        let text = path.display().to_string();
        let weak = cx.weak_entity();
        let scroll = ScrollHandle::new();
        window.open_alert(cx, move |dialog, _, cx| {
            let here = weak.clone();
            let open_here = text.clone();
            let open_tab = text.clone();
            dialog
                .frame(
                    ModalHeader::new(format!("Do you want to open Worktree \"{name}\"?")),
                    ModalWidth::Small,
                    cx,
                )
                .body(
                    &scroll,
                    modal_body().child(modal::subject(shown.clone(), None)),
                )
                .on_ok(|_, _, _| true)
                .footer(
                    ModalFooter::new()
                        .action(components::cancel_button(cx).on_click(|_, w, cx| {
                            w.close_dialog(cx);
                        }))
                        .action(
                            components::style(
                                Button::new("worktree-open-tab")
                                    .label("Open in New Tab")
                                    .tab_stop(true),
                                ButtonKind::Secondary,
                                ControlSize::Regular,
                                cx,
                            )
                            .on_click(move |_, w, cx| {
                                w.close_dialog(cx);
                                w.dispatch_action(Box::new(OpenRecent(open_tab.clone())), cx);
                            }),
                        )
                        .action(
                            components::style(
                                Button::new("worktree-open-now")
                                    .label("Open Now")
                                    .tab_stop(true),
                                ButtonKind::Primary,
                                ControlSize::Regular,
                                cx,
                            )
                            .on_click(move |_, w, cx| {
                                w.close_dialog(cx);
                                let path = open_here.clone();
                                let _ = here.update(cx, |v, cx| v.open_path(path, w, cx));
                            }),
                        ),
                )
        });
    }

    /// Asks for an optional lock message, then locks the worktree.
    fn prompt_lock(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Lock message"));
        let first = input.clone();
        let weak = cx.weak_entity();
        let scroll = ScrollHandle::new();
        let shown = display_path(&path.display().to_string());
        window.open_alert(cx, move |dialog, _, cx| {
            let lock = {
                let weak = weak.clone();
                let input = input.clone();
                let path = path.clone();
                move |window: &mut Window, cx: &mut App| {
                    let reason = input.read(cx).value().trim().to_owned();
                    let change = WorktreeChange::Lock {
                        path: path.clone(),
                        reason,
                    };
                    let _ = weak.update(cx, |v, cx| v.mutate(Action::Worktree(change), window, cx));
                }
            };
            let click = lock.clone();
            dialog
                .frame(
                    ModalHeader::new("Lock this worktree")
                        .description("Git will not remove or prune a locked worktree."),
                    ModalWidth::Small,
                    cx,
                )
                .body(
                    &scroll,
                    modal_body()
                        .child(modal::subject(shown.clone(), None))
                        .child(field("Message (optional):", Input::new(&input))),
                )
                .on_ok(move |_, window, cx| {
                    lock(window, cx);
                    true
                })
                .footer(
                    ModalFooter::new()
                        .action(components::cancel_button(cx).on_click(|_, w, cx| {
                            w.close_dialog(cx);
                        }))
                        .action(
                            components::style(
                                Button::new("worktree-lock").label("Lock").tab_stop(true),
                                ButtonKind::Primary,
                                ControlSize::Regular,
                                cx,
                            )
                            .on_click(move |_, w, cx| {
                                click(w, cx);
                                w.close_dialog(cx);
                            }),
                        ),
                )
        });
        window
            .on_next_frame(move |window, cx| first.update(cx, |input, cx| input.focus(window, cx)));
    }

    /// Removes a worktree, first confirming a branch deletion, then asking
    /// to force when Git would refuse: the worktree is locked or has
    /// uncommitted changes.
    fn remove_worktree(
        &mut self,
        worktree: Worktree,
        extent: Extent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = &self.state.snapshot else {
            return;
        };
        let branch = worktree
            .branch
            .clone()
            .filter(|_| extent != Extent::Worktree);
        let remote = branch
            .as_deref()
            .filter(|_| extent == Extent::BranchAndRemote)
            .and_then(|b| upstream(snapshot, b));
        let dirty = if worktree.current {
            !snapshot.status.is_empty()
        } else {
            worktree.changes.is_some_and(|c| !c.is_empty())
        };
        let refusal = if worktree.locked {
            Some("is locked.")
        } else if dirty {
            Some("contains modified or untracked files.")
        } else {
            None
        };
        let change = WorktreeChange::Remove {
            path: worktree.path.clone(),
            force: refusal.is_some(),
            delete_branch: branch.clone(),
            delete_remote: remote.clone(),
        };
        let main = snapshot
            .worktrees
            .iter()
            .find(|w| w.main)
            .map(|w| w.path.clone());
        let current = worktree.current;
        let shown = display_path(&worktree.path.display().to_string());
        let run = move |v: &mut GitCrab, window: &mut Window, cx: &mut Context<GitCrab>| {
            // A worktree cannot be removed while this tab has it open, so the
            // tab moves to the main worktree first and removes it from there.
            match (current, &main) {
                (true, Some(main)) => {
                    v.pending_worktree_removal = Some(PendingRemoval {
                        main: main.clone(),
                        change: change.clone(),
                        epoch: v.state.epoch + 1,
                    });
                    v.open_path(main.display().to_string(), window, cx);
                }
                _ => v.mutate(Action::Worktree(change.clone()), window, cx),
            }
        };
        let force = refusal.map(|refusal| {
            (
                format!(
                    "'{shown}' {refusal} Removing this worktree requires force, which is destructive and cannot be undone. Do you want to complete the removal?"
                ),
                "Force Remove Worktree",
            )
        });
        let confirm_delete = branch.map(|branch| {
            (
                if remote.is_some() {
                    format!("This will permanently remove the worktree at '{shown}' and delete the local branch '{branch}' and its remotes. This action cannot be undone.")
                } else {
                    format!("This will permanently remove the worktree at '{shown}' and delete the local branch '{branch}'. This action cannot be undone.")
                },
                "Remove and Delete",
            )
        });
        let steps: Vec<(String, &'static str)> = confirm_delete.into_iter().chain(force).collect();
        self.confirm_steps(steps, std::rc::Rc::new(run), window, cx);
    }

    /// Shows each confirmation in turn and runs `then` after the last.
    fn confirm_steps(
        &mut self,
        mut steps: Vec<(String, &'static str)>,
        then: Then,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if steps.is_empty() {
            then(self, window, cx);
            return;
        }
        let (message, label) = steps.remove(0);
        let weak = cx.weak_entity();
        let cancel_focus = cx.focus_handle();
        let initial_focus = cancel_focus.clone();
        let scroll = ScrollHandle::new();
        window.open_alert(cx, move |dialog, _, cx| {
            let weak = weak.clone();
            let steps = steps.clone();
            let then = then.clone();
            let go = components::style(
                Button::new("worktree-confirm").label(label).tab_stop(true),
                ButtonKind::Discard,
                ControlSize::Regular,
                cx,
            )
            .on_click(move |_, w, cx| {
                w.close_dialog(cx);
                let (steps, then) = (steps.clone(), then.clone());
                let _ = weak.update(cx, |v, cx| v.confirm_steps(steps, then, w, cx));
            });
            dialog
                .frame(
                    ModalHeader::new("Remove worktree").tone(Tone::Danger),
                    ModalWidth::Small,
                    cx,
                )
                .confirm()
                .body(
                    &scroll,
                    modal_body().child(modal::paragraph(message.clone())),
                )
                .on_ok(|_, _, _| true)
                .footer(
                    ModalFooter::new()
                        .action(components::dialog_cancel_focused(&cancel_focus, cx))
                        .action(go),
                )
        });
        window.on_next_frame(move |w, cx| initial_focus.focus(w, cx));
    }

    /// Runs a removal that waited for this tab to reach the main worktree.
    /// It runs only after the very open it started: if that open was
    /// cancelled or failed, the removal is dropped so a later open of the
    /// main worktree cannot run it without confirmation.
    pub(super) fn resume_worktree_removal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.awaiting_trust {
            return;
        }
        let Some(pending) = self.pending_worktree_removal.take() else {
            return;
        };
        let here = std::path::Path::new(&self.state.path).canonicalize().ok();
        if self.state.epoch == pending.epoch
            && here.is_some()
            && pending.main.canonicalize().ok() == here
        {
            let change = pending.change;
            self.close_if_duplicate = true;
            self.mutate(Action::Worktree(change), window, cx);
        }
    }
}

impl GitCrab {
    /// The inspector for another worktree's changes row: where it is, what
    /// changed, and buttons to open it, as GitKraken's WIP card offers.
    pub(super) fn worktree_details(
        &self,
        el: Stateful<Div>,
        path: &std::path::Path,
        row: &gitcrab_core::git::Commit,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let text = path.display().to_string();
        let here = text.clone();
        let tab = text.clone();
        let button = |id: &'static str, label: &'static str, kind| {
            components::style(
                Button::new(id).label(label).tab_stop(true),
                kind,
                ControlSize::Regular,
                cx,
            )
        };
        el.child(
            div()
                .p(scaled(INSPECTOR_INSET))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(row.subject.clone()),
                )
                .child(
                    div()
                        .text_size(scaled(TEXT_SIZE_SMALL))
                        .text_role(Role::TextMuted)
                        .child(display_path(&text)),
                )
                .child(div().child(format!("Uncommitted: {}", row.body_preview)))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .child(
                            button("worktree-row-open", "Open Worktree", ButtonKind::Primary)
                                .on_click(
                                    cx.listener(move |v, _, w, cx| {
                                        v.open_path(here.clone(), w, cx)
                                    }),
                                ),
                        )
                        .child(
                            button("worktree-row-tab", "Open in New Tab", ButtonKind::Secondary)
                                .on_click(move |_, w, cx| {
                                    w.dispatch_action(Box::new(OpenRecent(tab.clone())), cx)
                                }),
                        ),
                ),
        )
    }
}
