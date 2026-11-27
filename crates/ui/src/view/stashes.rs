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
        }
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
                stash.message
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
                    message: stash.message.clone(),
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
