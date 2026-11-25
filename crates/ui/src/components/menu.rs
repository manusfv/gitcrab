//! Context menus shown by the operating system, and drawn menu entries.
use gpui::component::menu::{PopupMenu, PopupMenuItem};
use gpui::component::native_menu::NativeMenu;
use gpui::{prelude::*, *};
use std::any::TypeId;
use std::collections::HashSet;
use std::path::PathBuf;

/// A context menu, built when it opens and shown by the operating system.
///
/// Items dispatch an action. Actions that act on a view carry a
/// [`MenuTarget`], so they reach it wherever keyboard focus is.
#[derive(Default)]
pub struct ContextMenu {
    entries: Vec<Entry>,
}

enum Entry {
    Label(SharedString),
    Item {
        label: SharedString,
        enabled: bool,
        checked: bool,
        action: Box<dyn Action>,
    },
    Separator,
}

impl ContextMenu {
    pub fn new() -> Self {
        Self::default()
    }

    /// A heading that cannot be chosen.
    pub fn label(mut self, text: impl Into<SharedString>) -> Self {
        self.entries.push(Entry::Label(text.into()));
        self
    }

    pub fn item(self, label: impl Into<SharedString>, enabled: bool, action: impl Action) -> Self {
        self.checked_item(label, enabled, false, action)
    }

    pub fn checked_item(
        mut self,
        label: impl Into<SharedString>,
        enabled: bool,
        checked: bool,
        action: impl Action,
    ) -> Self {
        self.entries.push(Entry::Item {
            label: label.into(),
            enabled,
            checked,
            action: Box::new(action),
        });
        self
    }

    /// A separator. Leading, trailing and repeated separators are dropped.
    pub fn separator(mut self) -> Self {
        self.entries.push(Entry::Separator);
        self
    }

    /// Copies `text` to the clipboard.
    pub fn copy(self, label: impl Into<SharedString>, text: impl Into<String>) -> Self {
        self.item(label, true, CopyText(text.into()))
    }

    /// Shows `path` in the system file manager. Disabled when it does not exist.
    pub fn reveal(self, path: PathBuf) -> Self {
        self.item(REVEAL_LABEL, path.exists(), RevealPath(path))
    }

    /// Opens `path` in its default application. Disabled when it is not a file.
    pub fn open(self, path: PathBuf) -> Self {
        self.item("Open file", path.is_file(), OpenPath(path))
    }

    fn into_entries(self) -> Vec<Entry> {
        let mut entries: Vec<Entry> = Vec::with_capacity(self.entries.len());
        for entry in self.entries {
            if matches!(entry, Entry::Separator)
                && entries
                    .last()
                    .is_none_or(|last| matches!(last, Entry::Separator))
            {
                continue;
            }
            entries.push(entry);
        }
        if matches!(entries.last(), Some(Entry::Separator)) {
            entries.pop();
        }
        entries
    }

    /// Pops the menu up at `position`, in window coordinates. `on_close` runs
    /// once it closes, after the chosen item's action is dispatched.
    pub fn show(
        self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
        on_close: impl FnOnce(&mut App) + 'static,
    ) {
        register(cx, |action: &CopyText, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(action.0.clone()))
        });
        register(cx, |action: &RevealPath, cx| cx.reveal_path(&action.0));
        register(cx, |action: &OpenPath, cx| cx.open_with_system(&action.0));

        let items = self
            .into_entries()
            .into_iter()
            .map(|entry| match entry {
                Entry::Separator => MenuItem::Separator,
                Entry::Label(label) => native_item(label, Box::new(NoAction), false, false),
                Entry::Item {
                    label,
                    enabled,
                    checked,
                    action,
                } => native_item(label, action, enabled, checked),
            })
            .collect();
        let menu = NativeMenu::from(Menu {
            name: SharedString::default(),
            items,
            disabled: false,
        });
        menu.show(position, window, cx);
        // The native menu runs from a task it has just queued, and queued
        // tasks run in order, so this one starts once the menu has closed.
        cx.spawn(async move |cx| cx.update(on_close)).detach();
    }

    /// Fills a drawn popup menu with the same entries, for a dropdown.
    pub fn into_popup(self, mut menu: PopupMenu) -> PopupMenu {
        for entry in self.into_entries() {
            menu = match entry {
                Entry::Separator => menu.separator(),
                Entry::Label(label) => menu.item(PopupMenuItem::label(label)),
                Entry::Item {
                    label,
                    enabled,
                    checked,
                    action,
                } => menu.item(menu_item(label, enabled).checked(checked).on_click(
                    move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx),
                )),
            };
        }
        menu
    }
}

fn native_item(
    label: SharedString,
    action: Box<dyn Action>,
    enabled: bool,
    checked: bool,
) -> MenuItem {
    // Win32 menus read `&` as a mnemonic marker.
    let name = if cfg!(target_os = "windows") {
        label.replace('&', "&&").into()
    } else {
        label
    };
    MenuItem::Action {
        name,
        action,
        os_action: None,
        checked,
        disabled: !enabled,
    }
}

/// Handles menu actions of type `A` app-wide. Registering a type again does nothing.
pub fn register<A: Action>(cx: &mut App, handler: impl Fn(&A, &mut App) + 'static) {
    #[derive(Default)]
    struct Registered(HashSet<TypeId>);
    impl Global for Registered {}

    if cx
        .default_global::<Registered>()
        .0
        .insert(TypeId::of::<A>())
    {
        cx.on_action(handler);
    }
}

/// The view, and the window holding it, that a menu action acts on.
pub struct MenuTarget<V> {
    window: AnyWindowHandle,
    view: WeakEntity<V>,
}

impl<V: 'static> MenuTarget<V> {
    pub fn new(window: &Window, view: WeakEntity<V>) -> Self {
        Self {
            window: window.window_handle(),
            view,
        }
    }

    /// Runs `update` on the view. It waits for the action's dispatch to
    /// finish, since the window is in use while action handlers run.
    pub fn update(
        &self,
        cx: &mut App,
        update: impl FnOnce(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) {
        let (window, view) = (self.window, self.view.clone());
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                view.update(cx, |view, cx| update(view, window, cx))
            });
        });
    }
}

impl<V> Clone for MenuTarget<V> {
    fn clone(&self) -> Self {
        Self {
            window: self.window,
            view: self.view.clone(),
        }
    }
}

impl<V> PartialEq for MenuTarget<V> {
    fn eq(&self, other: &Self) -> bool {
        self.window == other.window && self.view == other.view
    }
}

impl<V> std::fmt::Debug for MenuTarget<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MenuTarget")
            .field("window", &self.window.window_id())
            .field("view", &self.view.entity_id())
            .finish()
    }
}

#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = gitcrab, no_json)]
struct CopyText(String);

#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = gitcrab, no_json)]
struct RevealPath(PathBuf);

#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = gitcrab, no_json)]
struct OpenPath(PathBuf);

/// A popup menu entry whose row shows the pointer cursor while it is enabled.
///
/// Library menu entries keep the arrow cursor and accept no styling, so the
/// label is rendered as a custom element. The library still handles clicks,
/// keyboard navigation, `checked`, `action` shortcuts and the disabled look.
pub fn menu_item(label: impl Into<SharedString>, enabled: bool) -> PopupMenuItem {
    let label = label.into();
    PopupMenuItem::element(move |_, _| {
        div()
            // Reach over the entry's own padding (`px_2`, `py_1`) so the
            // whole row shows the cursor, not just the text.
            .mx(rems(-0.5))
            .my(rems(-0.25))
            .px(rems(0.5))
            .py(rems(0.25))
            .flex_1()
            .min_w_0()
            .when(enabled, |row| row.cursor_pointer())
            .child(label.clone())
    })
    .disabled(!enabled)
}

/// The platform's name for showing a file in its folder.
pub const REVEAL_LABEL: &str = if cfg!(target_os = "macos") {
    "Reveal in Finder"
} else if cfg!(target_os = "windows") {
    "Show in Explorer"
} else {
    "Show in file manager"
};

/// Joins a slash-separated repository path to the worktree, in the
/// platform's separators, without a Windows verbatim prefix that file
/// managers reject.
pub fn worktree_file(worktree: &std::path::Path, relative: &str) -> std::path::PathBuf {
    let root = gitcrab_core::presentation::display_path(&worktree.to_string_lossy());
    let mut path = std::path::PathBuf::from(root);
    path.extend(relative.split('/').filter(|part| !part.is_empty()));
    path
}

#[cfg(test)]
mod tests {
    use super::worktree_file;
    use std::path::Path;

    #[test]
    fn worktree_files_use_platform_separators() {
        let path = worktree_file(Path::new("repo"), "src/main.rs");
        assert_eq!(path, Path::new("repo").join("src").join("main.rs"));
    }

    #[cfg(windows)]
    #[test]
    fn worktree_files_drop_the_verbatim_prefix() {
        let path = worktree_file(Path::new(r"\\?\C:\repo"), "a/b.txt");
        assert_eq!(path, Path::new(r"C:\repo\a\b.txt"));
    }
}

#[cfg(test)]
mod context_menu_tests {
    use super::{native_item, ContextMenu, Entry};
    use gpui::{MenuItem, NoAction};

    fn shape(menu: ContextMenu) -> Vec<&'static str> {
        menu.into_entries()
            .iter()
            .map(|entry| match entry {
                Entry::Separator => "-",
                Entry::Label(_) => "label",
                Entry::Item { .. } => "item",
            })
            .collect()
    }

    #[test]
    fn separators_never_lead_trail_or_repeat() {
        let menu = ContextMenu::new()
            .separator()
            .copy("A", "a")
            .separator()
            .separator()
            .copy("B", "b")
            .separator();
        assert_eq!(shape(menu), ["item", "-", "item"]);
        assert!(shape(ContextMenu::new().separator()).is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn ampersands_show_literally_on_windows() {
        let MenuItem::Action { name, .. } =
            native_item("Merge a&b".into(), Box::new(NoAction), true, false)
        else {
            panic!("expected an action item");
        };
        assert_eq!(name, "Merge a&&b");
    }
}
