//! Menu entries.
use gpui::component::menu::PopupMenuItem;
use gpui::{prelude::*, *};

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

/// Copies `text` to the clipboard.
pub fn copy_item(label: impl Into<SharedString>, text: impl Into<String>) -> PopupMenuItem {
    let text = text.into();
    menu_item(label, true)
        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())))
}

/// The platform's name for showing a file in its folder.
pub const REVEAL_LABEL: &str = if cfg!(target_os = "macos") {
    "Reveal in Finder"
} else if cfg!(target_os = "windows") {
    "Show in Explorer"
} else {
    "Show in file manager"
};

/// Shows `path` in the system file manager. Disabled when it does not exist.
pub fn reveal_item(path: std::path::PathBuf) -> PopupMenuItem {
    menu_item(REVEAL_LABEL, path.exists()).on_click(move |_, _, cx| cx.reveal_path(&path))
}

/// Opens `path` in its default application. Disabled when it is not a file.
pub fn open_item(path: std::path::PathBuf) -> PopupMenuItem {
    menu_item("Open file", path.is_file()).on_click(move |_, _, cx| cx.open_with_system(&path))
}

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

/// Shortens a name shown inside a menu label, so a long branch name cannot
/// widen the menu past the window.
pub fn clip(name: &str) -> String {
    const MAX: usize = 40;
    if name.chars().count() <= MAX {
        return name.to_owned();
    }
    let head: String = name.chars().take(MAX - 1).collect();
    format!("{head}…")
}

#[cfg(test)]
mod clip_tests {
    use super::clip;

    #[test]
    fn long_names_end_in_an_ellipsis() {
        assert_eq!(clip("main"), "main");
        let long = "x".repeat(60);
        assert_eq!(clip(&long).chars().count(), 40);
        assert!(clip(&long).ends_with('…'));
    }
}
