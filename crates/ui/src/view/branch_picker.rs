//! The toolbar's branch control.
//!
//! It shows the checked-out branch, or a detached HEAD, and opens a searchable
//! list of local branches. Choosing a branch other than the checked-out one
//! reviews a checkout. Previewing a branch's history without switching stays
//! with the sidebar and the history graph.

use super::*;
use std::rc::Rc;

/// Narrowest the dropdown gets, so branch names stay readable.
const MIN_POPUP_WIDTH: f32 = 260.;

impl GitCrab {
    /// Builds the branch control: kicker "branch", the checked-out branch as
    /// its value, and a dropdown with a search field over local branches.
    pub(super) fn branch_picker(
        &self,
        width: Pixels,
        height: Pixels,
        commands: &CommandContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let snapshot = self.state.snapshot.as_ref();
        let current = snapshot
            .and_then(|s| s.checked_out_branch())
            .map(str::to_owned);
        let value = snapshot.map_or_else(|| "No repository".to_owned(), |s| s.head_label());
        let branches: Rc<Vec<String>> = Rc::new(
            snapshot
                .map(|s| s.branches.iter().map(|b| b.name.clone()).collect())
                .unwrap_or_default(),
        );
        let checkout_reason = Command::Checkout.availability(commands).reason();
        let enabled = commands.repository && !commands.modal;
        let tooltip = match (&current, snapshot) {
            (_, None) => "Open a repository to choose a branch".to_owned(),
            (Some(name), Some(_)) => format!(
                "Checked out: {name}. New commits go here. Choose a branch to review switching to it."
            ),
            (None, Some(s)) if s.head_id.is_empty() => format!(
                "{value}. Choose a branch to review switching to it."
            ),
            (None, Some(_)) => format!(
                "{value}. HEAD is not on a branch. Choose a branch to review switching to it."
            ),
        };

        let focus = self.focus.clone();
        let weak = cx.weak_entity();
        let names = branches.clone();
        let current_name = current.clone();
        let entries = branches.iter().map(|name| {
            let is_current = current.as_deref() == Some(name.as_str());
            let item = PickerItem::new(name.clone())
                .icon("branch")
                .checked(is_current);
            if is_current {
                item.note("checked out")
            } else {
                item
            }
            .into()
        });

        Picker::new(
            "branch-picker",
            components::selector(
                "branch-selector",
                Some("branch"),
                value,
                "branch",
                width,
                height,
            )
            .set_disabled(!enabled)
            .tooltip(tooltip),
            width.max(px(MIN_POPUP_WIDTH)),
            &focus,
            // Reviews a checkout of the branch unless it is already checked out.
            move |index, window, cx| {
                let Some(name) = names.get(index).cloned() else {
                    return;
                };
                if current_name.as_deref() == Some(name.as_str()) {
                    return;
                }
                let _ = weak.update(cx, |v, cx| {
                    v.run_command(Command::Checkout, name, window, cx)
                });
            },
        )
        .search("Search branches", "branches")
        .entries(entries)
        .empty("This repository has no local branches yet.")
        .footer(
            checkout_reason.map(|reason| format!("Switching branches is unavailable. {reason}")),
        )
        .build(window, cx)
    }
}
