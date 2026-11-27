//! Stash list actions, following GitKraken: each stash's context menu and
//! the confirmation before deleting one.
use super::*;
use components::{ButtonKind, ContextMenu, MenuTarget};
use gitcrab_core::git::StashEntry;

/// A stash menu item. Each names the stash by its commit.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum StashCommand {
    Apply(String),
    Pop(String),
    Delete(StashEntry),
    EditMessage(StashEntry),
}

/// Builds the menu GitKraken shows for a stash.
pub(super) fn stash_menu(
    view: WeakEntity<GitCrab>,
    stash: &StashEntry,
    window: &Window,
) -> ContextMenu {
    let target = MenuTarget::new(window, view);
    let run = |command: StashCommand| target.command(MenuCommand::Stash(command));
    ContextMenu::new()
        .item(
            "Apply Stash",
            true,
            run(StashCommand::Apply(stash.id.clone())),
        )
        .item("Pop Stash", true, run(StashCommand::Pop(stash.id.clone())))
        .item(
            "Delete Stash",
            true,
            run(StashCommand::Delete(stash.clone())),
        )
        .separator()
        .item(
            "Edit stash message",
            true,
            run(StashCommand::EditMessage(stash.clone())),
        )
}

impl GitCrab {
    pub(super) fn run_stash_command(
        &mut self,
        command: StashCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            StashCommand::Apply(id) => self.run_operation(Operation::ApplyStash, id, window, cx),
            StashCommand::Pop(id) => self.run_operation(Operation::PopStash, id, window, cx),
            StashCommand::Delete(stash) => self.confirm_drop_stash(stash, window, cx),
            StashCommand::EditMessage(stash) => self.prompt_stash_message(stash, window, cx),
        }
    }

    /// Asks for a stash's new message, keeping the branch Git recorded.
    fn prompt_stash_message(
        &mut self,
        stash: StashEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (own, branch) = stash.parts();
        let branch = branch.map(str::to_owned);
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Stash title")
                .default_value(own.to_owned())
        });
        let first = input.clone();
        let weak = cx.weak_entity();
        let scroll = ScrollHandle::new();
        window.open_alert(cx, move |dialog, _, cx| {
            let save = {
                let weak = weak.clone();
                let input = input.clone();
                let id = stash.id.clone();
                let branch = branch.clone();
                move |window: &mut Window, cx: &mut App| {
                    let message = input.read(cx).value().trim().to_owned();
                    if message.is_empty() {
                        return false;
                    }
                    let subject = match &branch {
                        Some(branch) => format!("On {branch}: {message}"),
                        None => message,
                    };
                    let action = Action::RenameStash {
                        id: id.clone(),
                        subject,
                    };
                    let _ = weak.update(cx, |v, cx| v.mutate(action, window, cx));
                    true
                }
            };
            let click = save.clone();
            dialog
                .frame(
                    ModalHeader::new("Edit stash message"),
                    ModalWidth::Small,
                    cx,
                )
                .body(
                    &scroll,
                    modal_body().child(modal::field("Message", Input::new(&input))),
                )
                .on_ok(move |_, window, cx| save(window, cx))
                .footer(
                    ModalFooter::new()
                        .action(components::cancel_button(cx).on_click(|_, w, cx| {
                            w.close_dialog(cx);
                        }))
                        .action(
                            components::style(
                                Button::new("stash-rename").label("Save").tab_stop(true),
                                ButtonKind::Primary,
                                ControlSize::Regular,
                                cx,
                            )
                            .on_click(move |_, w, cx| {
                                if click(w, cx) {
                                    w.close_dialog(cx);
                                }
                            }),
                        ),
                )
        });
        window
            .on_next_frame(move |window, cx| first.update(cx, |input, cx| input.focus(window, cx)));
    }

    /// Asks before deleting a stash, with GitKraken's wording.
    fn confirm_drop_stash(
        &mut self,
        stash: StashEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.weak_entity();
        let cancel_focus = cx.focus_handle();
        let initial_focus = cancel_focus.clone();
        let scroll = ScrollHandle::new();
        window.open_alert(cx, move |dialog, _, cx| {
            let weak = weak.clone();
            let stash = stash.clone();
            let message = format!(
                "This is a destructive operation, are you sure you want to delete stash with message \"{}\"?",
                stash.name()
            );
            let delete = components::style(
                Button::new("stash-delete").label("Delete").tab_stop(true),
                ButtonKind::Discard,
                ControlSize::Regular,
                cx,
            )
            .on_click(move |_, w, cx| {
                w.close_dialog(cx);
                let action = Action::DropStash {
                    id: stash.id.clone(),
                    message: stash.name().to_owned(),
                };
                let _ = weak.update(cx, |v, cx| v.mutate(action, w, cx));
            });
            dialog
                .frame(
                    ModalHeader::new("Delete stash").tone(Tone::Danger),
                    ModalWidth::Small,
                    cx,
                )
                .confirm()
                .body(&scroll, modal_body().child(modal::paragraph(message)))
                .on_ok(|_, _, _| true)
                .footer(
                    ModalFooter::new()
                        .action(components::dialog_cancel_focused(&cancel_focus, cx))
                        .action(delete),
                )
        });
        window.on_next_frame(move |w, cx| initial_focus.focus(w, cx));
    }
}
