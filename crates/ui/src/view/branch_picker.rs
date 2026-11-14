//! The toolbar's branch control.
//!
//! It shows the checked-out branch, or a detached HEAD, and opens a searchable
//! list of local branches. Choosing a branch other than the checked-out one
//! reviews a checkout. Previewing a branch's history without switching stays
//! with the sidebar and the history graph.

use super::*;
use gitcrab_core::presentation::filter_branches;
use gpui::component::{
    input::{Enter, Escape, MoveDown, MoveUp},
    popover::{Popover, PopoverState},
    v_flex,
};
use std::rc::Rc;

/// Picks the branch at a filtered-list index.
type ChooseBranch = dyn Fn(usize, &mut Window, &mut App);

/// Keyboard highlight in the filtered list. It returns to the first match
/// whenever the query changes.
struct PickerNav {
    query: String,
    index: usize,
    scroll: ScrollHandle,
}

/// Height of the scrolling branch list before it scrolls.
const LIST_MAX_HEIGHT: f32 = 320.;
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

        let query = window.use_keyed_state("branch-picker-query", cx, |window, cx| {
            InputState::new(window, cx).placeholder("Search branches")
        });
        let nav = window.use_keyed_state("branch-picker-nav", cx, |_, _| PickerNav {
            query: String::new(),
            index: 0,
            scroll: ScrollHandle::new(),
        });
        let query_focus = query.read(cx).focus_handle(cx);
        let view_focus = self.focus.clone();
        let weak = cx.weak_entity();
        let popup_width = f32::from(width).max(MIN_POPUP_WIDTH);

        let on_open_change = {
            let query = query.clone();
            let nav = nav.clone();
            let query_focus = query_focus.clone();
            let view_focus = view_focus.clone();
            move |open: &bool, window: &mut Window, cx: &mut App| {
                if *open {
                    query.update(cx, |q, cx| q.set_value("", window, cx));
                    nav.update(cx, |nav, _| {
                        nav.query.clear();
                        nav.index = 0;
                        nav.scroll.scroll_to_item(0);
                    });
                } else if query_focus.is_focused(window) {
                    // Return keyboard focus to the tab so its shortcuts work.
                    view_focus.focus(window, cx);
                }
            }
        };

        let content =
            move |_: &mut PopoverState, _: &mut Window, cx: &mut Context<PopoverState>| {
                let popover = cx.entity();
                let text = query.read(cx).value().to_string();
                let matches = Rc::new(filter_branches(&branches, &text));
                let (index, scroll) = nav.update(cx, |nav, _| {
                    if nav.query != text {
                        nav.query = text.clone();
                        nav.index = 0;
                        nav.scroll.scroll_to_item(0);
                    }
                    nav.index = nav.index.min(matches.len().saturating_sub(1));
                    (nav.index, nav.scroll.clone())
                });

                // Closes the dropdown and reviews a checkout of `branch` unless it
                // is already checked out.
                let choose: Rc<ChooseBranch> = Rc::new({
                    let popover = popover.clone();
                    let branches = branches.clone();
                    let current = current.clone();
                    let weak = weak.clone();
                    let view_focus = view_focus.clone();
                    move |branch, window, cx| {
                        popover.update(cx, |state, cx| state.dismiss(window, cx));
                        view_focus.focus(window, cx);
                        let Some(name) = branches.get(branch).cloned() else {
                            return;
                        };
                        if current.as_deref() == Some(name.as_str()) {
                            return;
                        }
                        let _ = weak.update(cx, |v, cx| {
                            v.run_command(Command::Checkout, name, window, cx)
                        });
                    }
                });

                let rows = matches.iter().enumerate().map(|(row, &branch)| {
                    let name = branches[branch].clone();
                    let is_current = current.as_deref() == Some(name.as_str());
                    let choose = choose.clone();
                    // The search field keeps focus; arrow keys move the selection.
                    ListRow::new(("branch-picker-row", row))
                        .height(28.)
                        .focusable(false)
                        .rounded(px(3.))
                        .selected(row == index)
                        .leading(
                            icon(if is_current { "check" } else { "branch" })
                                .size(px(14.))
                                .flex_none()
                                .when(is_current, |i| i.text_role(Role::StatusSuccessFg)),
                        )
                        .content(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .when(is_current, |d| d.font_weight(FontWeight::MEDIUM))
                                .child(name),
                        )
                        .when(is_current, |r| {
                            r.trailing(
                                div()
                                    .flex_none()
                                    .text_size(scaled(11.))
                                    .text_role(Role::TextMuted)
                                    .child("checked out"),
                            )
                        })
                        .on_click(move |_, window, cx| choose(branch, window, cx))
                });

                let note = if branches.is_empty() {
                    Some("This repository has no local branches yet.".to_owned())
                } else if matches.is_empty() {
                    Some(format!("No branches match “{}”", text.trim()))
                } else {
                    None
                };

                v_flex()
                    .w(px(popup_width))
                    .gap_2()
                    .capture_action({
                        let nav = nav.clone();
                        move |_: &MoveUp, window, cx| {
                            nav.update(cx, |nav, _| {
                                nav.index = nav.index.saturating_sub(1);
                                nav.scroll.scroll_to_item(nav.index);
                            });
                            window.refresh();
                            cx.stop_propagation();
                        }
                    })
                    .capture_action({
                        let nav = nav.clone();
                        let last = matches.len().saturating_sub(1);
                        move |_: &MoveDown, window, cx| {
                            nav.update(cx, |nav, _| {
                                nav.index = (nav.index + 1).min(last);
                                nav.scroll.scroll_to_item(nav.index);
                            });
                            window.refresh();
                            cx.stop_propagation();
                        }
                    })
                    .capture_action({
                        let nav = nav.clone();
                        let matches = matches.clone();
                        let choose = choose.clone();
                        move |_: &Enter, window, cx| {
                            cx.stop_propagation();
                            if let Some(&branch) = matches.get(nav.read(cx).index) {
                                choose(branch, window, cx);
                            }
                        }
                    })
                    .capture_action({
                        let popover = popover.clone();
                        let view_focus = view_focus.clone();
                        move |_: &Escape, window, cx| {
                            cx.stop_propagation();
                            popover.update(cx, |state, cx| state.dismiss(window, cx));
                            view_focus.focus(window, cx);
                        }
                    })
                    .child(
                        Input::new(&query)
                            .h(scaled(32.))
                            .prefix(icon("search").size(px(14.)).text_role(Role::TextMuted)),
                    )
                    .child(
                        scroll_area(
                            "branch-picker-list",
                            &scroll,
                            div()
                                .id("branch-picker-list")
                                .max_h(px(LIST_MAX_HEIGHT))
                                .flex()
                                .flex_col()
                                .children(rows),
                        )
                        // Let the bar sit in the popover's own padding instead of
                        // adding a second gutter inside it.
                        .when(overflows_vertically(&scroll), |list| list.mr(px(-6.))),
                    )
                    .when_some(note, |el, note| {
                        el.child(
                            div()
                                .px_2()
                                .text_size(scaled(12.))
                                .text_role(Role::TextMuted)
                                .child(note),
                        )
                    })
                    .when_some(checkout_reason, |el, reason| {
                        el.child(
                            div()
                                .px_2()
                                .text_size(scaled(12.))
                                .text_role(Role::TextMuted)
                                .child(format!("Switching branches is unavailable. {reason}")),
                        )
                    })
            };

        Popover::new("branch-picker")
            .anchor(Anchor::TopLeft)
            .track_focus(&query_focus)
            .on_open_change(on_open_change)
            .trigger(
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
            )
            .content(content)
    }
}
