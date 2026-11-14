//! The popup list behind every picker in the toolbar.
//!
//! A [`Picker`] opens from a trigger button and shows a list of entries, with
//! an optional search field. Branches, repositories and tabs all use it, so
//! rows, keyboard navigation, scrolling and the scrollbar gutter behave the
//! same in each.
//!
//! Use it for choosing one thing from a list. Short command menus (fetch,
//! pull, column settings) stay library menus.
use super::layout::{overflows_vertically, scroll_area};
use super::list_row::ListRow;
use crate::theme::Role;
use crate::theme::*;
use gpui::component::{
    input::{Enter, Escape, Input, InputState, MoveDown, MoveUp},
    popover::{Popover, PopoverState},
    v_flex, Selectable,
};
use gpui::{prelude::*, *};
use std::rc::Rc;

/// Height of the scrolling list before it scrolls.
const LIST_MAX_HEIGHT: f32 = 320.;
/// Row heights: one line, and a label with a detail line under it.
const ROW_HEIGHT: f32 = 28.;
const DETAIL_ROW_HEIGHT: f32 = 44.;
/// How far the scrollbar reaches into the popup's own padding, so it does not
/// add a second gutter inside the list.
const BAR_BLEED: f32 = 6.;

/// One choosable row.
#[derive(Clone)]
pub struct PickerItem {
    label: SharedString,
    detail: Option<SharedString>,
    icon: Option<&'static str>,
    note: Option<SharedString>,
    checked: bool,
    disabled: bool,
    tooltip: Option<SharedString>,
}

impl PickerItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            detail: None,
            icon: None,
            note: None,
            checked: false,
            disabled: false,
            tooltip: None,
        }
    }

    /// A muted second line under the label.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// The icon shown at the start of the row.
    pub fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A muted note at the end of the row, such as a shortcut or a state.
    pub fn note(mut self, note: impl Into<SharedString>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Marks the current choice with a check.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// A disabled row is shown dimmed and cannot be chosen or highlighted.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }
}

/// A row, a section heading, or a divider.
#[derive(Clone)]
pub enum PickerEntry {
    Item(PickerItem),
    Heading(SharedString),
    Rule,
}

impl From<PickerItem> for PickerEntry {
    fn from(item: PickerItem) -> Self {
        Self::Item(item)
    }
}

/// Called with the position of the chosen row among the picker's items, in the
/// order they were given (headings and rules do not count).
type Choose = dyn Fn(usize, &mut Window, &mut App);

/// A keyboard or pointer action that needs no argument.
type Act = dyn Fn(&mut Window, &mut App);

/// Moves the highlight by a number of rows.
type Step = dyn Fn(isize, &mut Window, &mut App);

/// Keyboard highlight, as a position among the choosable rows. It returns to
/// the first row whenever the search text changes.
struct Nav {
    query: String,
    index: usize,
    scroll: ScrollHandle,
}

/// What is on screen, in order.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shown {
    /// A heading or rule, by position among the entries.
    Entry(usize),
    /// A choosable row, by position among the items.
    Item(usize),
}

/// What an entry is, for deciding what is shown and what the keyboard reaches.
#[derive(Clone, Copy)]
enum Kind {
    Item { disabled: bool },
    Other,
}

/// The rows on screen, and the ones the keyboard can land on as
/// `(item, position on screen)`. `matched` lists the items a search kept, best
/// first; with it only those items show and headings and rules are dropped.
fn plan(kinds: &[Kind], matched: Option<Vec<usize>>) -> (Vec<Shown>, Vec<(usize, usize)>) {
    let disabled: Vec<bool> = kinds
        .iter()
        .filter_map(|k| match k {
            Kind::Item { disabled } => Some(*disabled),
            Kind::Other => None,
        })
        .collect();
    let shown: Vec<Shown> = match matched {
        Some(items) => items.into_iter().map(Shown::Item).collect(),
        None => {
            let mut next = 0;
            kinds
                .iter()
                .enumerate()
                .map(|(i, k)| match k {
                    Kind::Item { .. } => {
                        next += 1;
                        Shown::Item(next - 1)
                    }
                    Kind::Other => Shown::Entry(i),
                })
                .collect()
        }
    };
    let selectable = shown
        .iter()
        .enumerate()
        .filter_map(|(at, s)| match s {
            Shown::Item(i) if !disabled[*i] => Some((*i, at)),
            _ => None,
        })
        .collect();
    (shown, selectable)
}

/// Moves a highlight position by `delta`, staying within `len` rows.
fn step_index(current: usize, delta: isize, len: usize) -> usize {
    let last = len.saturating_sub(1) as isize;
    (current as isize + delta).clamp(0, last) as usize
}

/// The labels containing `query`, best first: those starting with it, then the
/// rest, each in their given order. Case is ignored.
fn rank(labels: &[&str], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let (mut prefix, mut inner) = (vec![], vec![]);
    for (i, label) in labels.iter().enumerate() {
        let label = label.to_lowercase();
        if label.starts_with(&query) {
            prefix.push(i);
        } else if label.contains(&query) {
            inner.push(i);
        }
    }
    prefix.extend(inner);
    prefix
}

pub struct Picker<T> {
    id: &'static str,
    trigger: T,
    width: Pixels,
    search: Option<&'static str>,
    noun: &'static str,
    entries: Vec<PickerEntry>,
    empty: Option<SharedString>,
    footer: Option<SharedString>,
    focus: FocusHandle,
    anchor: Anchor,
    on_choose: Rc<Choose>,
}

impl<T: Selectable + IntoElement + 'static> Picker<T> {
    /// `trigger` opens the list, `focus` is where keyboard focus returns when
    /// it closes, and `on_choose` runs after the list has closed.
    pub fn new(
        id: &'static str,
        trigger: T,
        width: Pixels,
        focus: &FocusHandle,
        on_choose: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id,
            trigger,
            width,
            search: None,
            noun: "items",
            entries: Vec::new(),
            empty: None,
            footer: None,
            focus: focus.clone(),
            anchor: Anchor::TopLeft,
            on_choose: Rc::new(on_choose),
        }
    }

    /// Which corner of the trigger the list opens from. Defaults to top left.
    pub fn anchor(mut self, anchor: Anchor) -> Self {
        self.anchor = anchor;
        self
    }

    /// Adds a search field above the list.
    pub fn search(mut self, placeholder: &'static str, noun: &'static str) -> Self {
        self.search = Some(placeholder);
        self.noun = noun;
        self
    }

    pub fn entries(mut self, entries: impl IntoIterator<Item = PickerEntry>) -> Self {
        self.entries = entries.into_iter().collect();
        self
    }

    /// Shown instead of the list when there is nothing to pick.
    pub fn empty(mut self, note: impl Into<SharedString>) -> Self {
        self.empty = Some(note.into());
        self
    }

    /// A muted note under the list.
    pub fn footer(mut self, note: Option<impl Into<SharedString>>) -> Self {
        self.footer = note.map(Into::into);
        self
    }

    /// Builds the popover. Call it every render: keyboard and search state
    /// live in the window, keyed by `id`.
    pub fn build(self, window: &mut Window, cx: &mut App) -> Popover {
        let Self {
            id,
            trigger,
            width,
            search,
            noun,
            entries,
            empty,
            footer,
            focus,
            anchor,
            on_choose,
        } = self;
        let key = |suffix: &str| SharedString::from(format!("{id}-{suffix}"));
        let query = search.map(|placeholder| {
            window.use_keyed_state(key("query"), cx, move |window, cx| {
                InputState::new(window, cx).placeholder(placeholder)
            })
        });
        let nav = window.use_keyed_state(key("nav"), cx, |_, _| Nav {
            query: String::new(),
            index: 0,
            scroll: ScrollHandle::new(),
        });
        // Without a search field the list itself takes keyboard focus.
        let key_focus = match &query {
            Some(query) => query.read(cx).focus_handle(cx),
            None => window
                .use_keyed_state(key("focus"), cx, |_, cx| cx.focus_handle())
                .read(cx)
                .clone(),
        };
        let entries = Rc::new(entries);
        let has_search = query.is_some();

        let on_open_change = {
            let query = query.clone();
            let nav = nav.clone();
            let key_focus = key_focus.clone();
            let focus = focus.clone();
            move |open: &bool, window: &mut Window, cx: &mut App| {
                if *open {
                    if let Some(query) = &query {
                        query.update(cx, |q, cx| q.set_value("", window, cx));
                    }
                    nav.update(cx, |nav, _| {
                        nav.query.clear();
                        nav.index = 0;
                        nav.scroll.scroll_to_item(0);
                    });
                } else if key_focus.is_focused(window) {
                    // Return keyboard focus to the tab so its shortcuts work.
                    focus.focus(window, cx);
                }
            }
        };

        let content_focus = key_focus.clone();
        let content =
            move |_: &mut PopoverState, _: &mut Window, cx: &mut Context<PopoverState>| {
                let popover = cx.entity();
                let text = query
                    .as_ref()
                    .map(|q| q.read(cx).value().to_string())
                    .unwrap_or_default();

                // Items in the order they were given.
                let items: Vec<&PickerItem> = entries
                    .iter()
                    .filter_map(|e| match e {
                        PickerEntry::Item(item) => Some(item),
                        _ => None,
                    })
                    .collect();
                let item = |index: usize| items[index];
                let filtering = has_search && !text.trim().is_empty();
                let kinds: Vec<Kind> = entries
                    .iter()
                    .map(|e| match e {
                        PickerEntry::Item(item) => Kind::Item {
                            disabled: item.disabled,
                        },
                        _ => Kind::Other,
                    })
                    .collect();
                let matched = filtering.then(|| {
                    let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
                    rank(&labels, &text)
                });
                let (shown, selectable) = plan(&kinds, matched);
                let selectable = Rc::new(selectable);

                let (index, scroll) = nav.update(cx, |nav, _| {
                    if nav.query != text {
                        nav.query = text.clone();
                        nav.index = 0;
                        nav.scroll.scroll_to_item(0);
                    }
                    nav.index = nav.index.min(selectable.len().saturating_sub(1));
                    (nav.index, nav.scroll.clone())
                });
                let highlighted = selectable.get(index).map(|&(i, _)| i);

                // Closes the list, then reports the choice.
                let choose: Rc<Choose> = Rc::new({
                    let popover = popover.clone();
                    let focus = focus.clone();
                    let on_choose = on_choose.clone();
                    move |item, window, cx| {
                        popover.update(cx, |state, cx| state.dismiss(window, cx));
                        focus.focus(window, cx);
                        on_choose(item, window, cx);
                    }
                });
                let dismiss: Rc<Act> = Rc::new({
                    let popover = popover.clone();
                    let focus = focus.clone();
                    move |window, cx| {
                        popover.update(cx, |state, cx| state.dismiss(window, cx));
                        focus.focus(window, cx);
                    }
                });
                let step: Rc<Step> = Rc::new({
                    let nav = nav.clone();
                    let selectable = selectable.clone();
                    move |delta, window, cx| {
                        nav.update(cx, |nav, _| {
                            nav.index = step_index(nav.index, delta, selectable.len());
                            if let Some(&(_, child)) = selectable.get(nav.index) {
                                nav.scroll.scroll_to_item(child);
                            }
                        });
                        window.refresh();
                    }
                });
                let confirm: Rc<Act> = Rc::new({
                    let nav = nav.clone();
                    let selectable = selectable.clone();
                    let choose = choose.clone();
                    move |window, cx| {
                        if let Some(&(i, _)) = selectable.get(nav.read(cx).index) {
                            choose(i, window, cx);
                        }
                    }
                });

                let rows = shown.iter().enumerate().map(|(child, s)| match *s {
                    Shown::Entry(i) => match &entries[i] {
                        PickerEntry::Heading(text) => div()
                            .px_2()
                            .pt_2()
                            .pb_1()
                            .text_size(scaled(11.))
                            .text_role(Role::TextMuted)
                            .child(text.clone())
                            .into_any_element(),
                        _ => div()
                            .mx_2()
                            .my_1()
                            .h(px(1.))
                            .bg_role(Role::BorderDivider)
                            .into_any_element(),
                    },
                    Shown::Item(i) => {
                        let it = item(i);
                        let choose = choose.clone();
                        let two_lines = it.detail.is_some();
                        // The search field keeps focus; arrow keys move the highlight.
                        ListRow::new((SharedString::from(format!("{id}-row")), child))
                            .height(if two_lines {
                                DETAIL_ROW_HEIGHT
                            } else {
                                ROW_HEIGHT
                            })
                            .focusable(false)
                            .rounded(px(3.))
                            .selected(highlighted == Some(i))
                            .disabled(it.disabled)
                            .leading(if it.checked {
                                icon("check")
                                    .size(px(14.))
                                    .flex_none()
                                    .text_role(Role::StatusSuccessFg)
                                    .into_any_element()
                            } else if let Some(name) = it.icon {
                                icon(name).size(px(14.)).flex_none().into_any_element()
                            } else {
                                // Keeps labels aligned with rows that have an icon.
                                div().size(px(14.)).flex_none().into_any_element()
                            })
                            .content(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .truncate()
                                            .when(it.checked, |d| d.font_weight(FontWeight::MEDIUM))
                                            .child(it.label.clone()),
                                    )
                                    .when_some(it.detail.clone(), |col, detail| {
                                        col.child(
                                            div()
                                                .truncate()
                                                .text_size(scaled(11.))
                                                .text_role(Role::TextMuted)
                                                .child(detail),
                                        )
                                    }),
                            )
                            .when_some(it.note.clone(), |row, note| {
                                row.trailing(
                                    div()
                                        .flex_none()
                                        .text_size(scaled(11.))
                                        .text_role(Role::TextMuted)
                                        .child(note),
                                )
                            })
                            .when_some(it.tooltip.clone(), |row, tip| row.tooltip(tip))
                            .on_click(move |_, window, cx| choose(i, window, cx))
                            .into_any_element()
                    }
                });

                let note = if items.is_empty() {
                    empty.clone()
                } else if filtering && shown.is_empty() {
                    Some(format!("No {noun} match “{}”", text.trim()).into())
                } else {
                    None
                };

                v_flex()
                    .w(width)
                    .gap_2()
                    .when(has_search, |el| {
                        // The search field has focus, so its own actions arrive here.
                        el.capture_action({
                            let step = step.clone();
                            move |_: &MoveUp, window, cx| {
                                step(-1, window, cx);
                                cx.stop_propagation();
                            }
                        })
                        .capture_action({
                            let step = step.clone();
                            move |_: &MoveDown, window, cx| {
                                step(1, window, cx);
                                cx.stop_propagation();
                            }
                        })
                        .capture_action({
                            let confirm = confirm.clone();
                            move |_: &Enter, window, cx| {
                                cx.stop_propagation();
                                confirm(window, cx);
                            }
                        })
                        .capture_action({
                            let dismiss = dismiss.clone();
                            move |_: &Escape, window, cx| {
                                cx.stop_propagation();
                                dismiss(window, cx);
                            }
                        })
                    })
                    .when(!has_search, |el| {
                        el.track_focus(&content_focus).on_key_down({
                            let step = step.clone();
                            let confirm = confirm.clone();
                            let dismiss = dismiss.clone();
                            move |event: &KeyDownEvent, window, cx| {
                                match event.keystroke.key.as_str() {
                                    "up" => step(-1, window, cx),
                                    "down" => step(1, window, cx),
                                    "enter" => confirm(window, cx),
                                    "escape" => dismiss(window, cx),
                                    _ => return,
                                }
                                cx.stop_propagation();
                            }
                        })
                    })
                    .when_some(query.clone(), |el, query| {
                        el.child(
                            Input::new(&query)
                                .h(scaled(32.))
                                .prefix(icon("search").size(px(14.)).text_role(Role::TextMuted)),
                        )
                    })
                    .child(
                        scroll_area(
                            id,
                            &scroll,
                            div()
                                .id(SharedString::from(format!("{id}-list")))
                                .max_h(px(LIST_MAX_HEIGHT))
                                .flex()
                                .flex_col()
                                .children(rows),
                        )
                        .when(overflows_vertically(&scroll), |list| {
                            list.mr(px(-BAR_BLEED))
                        }),
                    )
                    .when_some(note, |el, note| el.child(muted(note)))
                    .when_some(footer.clone(), |el, note| el.child(muted(note)))
            };

        Popover::new(id)
            .anchor(anchor)
            .track_focus(&key_focus)
            .on_open_change(on_open_change)
            .trigger(trigger)
            .content(content)
    }
}

fn muted(text: SharedString) -> Div {
    div()
        .px_2()
        .text_size(scaled(12.))
        .text_role(Role::TextMuted)
        .child(text)
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring gpui's `test` macro in over `#[test]`.
    use super::{plan, rank, step_index, Kind, Shown};

    const ITEM: Kind = Kind::Item { disabled: false };
    const OFF: Kind = Kind::Item { disabled: true };

    #[test]
    fn headings_and_rules_are_shown_but_not_selectable() {
        // heading, item, rule, item
        let (shown, selectable) = plan(&[Kind::Other, ITEM, Kind::Other, ITEM], None);
        assert_eq!(
            shown,
            [
                Shown::Entry(0),
                Shown::Item(0),
                Shown::Entry(2),
                Shown::Item(1)
            ]
        );
        // (item, position on screen)
        assert_eq!(selectable, [(0, 1), (1, 3)]);
    }

    #[test]
    fn disabled_items_show_but_the_keyboard_skips_them() {
        let (shown, selectable) = plan(&[ITEM, OFF, ITEM], None);
        assert_eq!(shown.len(), 3);
        assert_eq!(selectable, [(0, 0), (2, 2)]);
    }

    #[test]
    fn a_search_keeps_only_matches_in_ranked_order_and_drops_headings() {
        let (shown, selectable) = plan(&[Kind::Other, ITEM, ITEM, ITEM], Some(vec![2, 0]));
        assert_eq!(shown, [Shown::Item(2), Shown::Item(0)]);
        assert_eq!(selectable, [(2, 0), (0, 1)]);
    }

    #[test]
    fn a_search_with_no_matches_shows_nothing() {
        let (shown, selectable) = plan(&[ITEM], Some(vec![]));
        assert!(shown.is_empty() && selectable.is_empty());
    }

    #[test]
    fn stepping_stays_inside_the_list() {
        assert_eq!(step_index(0, -1, 3), 0);
        assert_eq!(step_index(0, 1, 3), 1);
        assert_eq!(step_index(2, 1, 3), 2);
        assert_eq!(step_index(0, 1, 0), 0);
    }

    #[test]
    fn rank_puts_prefix_matches_first_and_ignores_case() {
        let labels = ["main", "feat/topic", "Topic-1", "other"];
        assert_eq!(rank(&labels, "topic"), [2, 1]);
        assert_eq!(rank(&labels, "  MAIN "), [0]);
        assert!(rank(&labels, "zzz").is_empty());
    }
}
