//! Host integration stays outside the Git and graph models.
use std::{ffi::OsString, path::PathBuf};

pub const OPEN_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "Cmd+O"
} else {
    "Ctrl+O"
};

pub fn font_files() -> Vec<(&'static str, PathBuf)> {
    #[cfg(windows)]
    {
        let windows = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:/Windows"));
        vec![
            ("system-ui", windows.join("Fonts/segoeui.ttf")),
            ("system-symbols", windows.join("Fonts/seguisym.ttf")),
        ]
    }
    #[cfg(target_os = "macos")]
    {
        vec![
            (
                "system-ui",
                "/System/Library/Fonts/Supplemental/Arial.ttf".into(),
            ),
            (
                "system-symbols",
                "/System/Library/Fonts/Apple Symbols.ttf".into(),
            ),
        ]
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        vec![(
            "system-ui",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into(),
        )]
    }
}

pub fn git_executable() -> OsString {
    if let Some(path) = std::env::var_os("GITCRAB_GIT").filter(|p| !p.is_empty()) {
        return path;
    }
    #[cfg(target_os = "macos")]
    {
        // Finder-launched apps often lack the shell's Homebrew PATH. Honor PATH
        // first, then try conventional installations without invoking a shell.
        let on_path = std::env::var_os("PATH")
            .is_some_and(|paths| std::env::split_paths(&paths).any(|p| p.join("git").is_file()));
        if !on_path {
            for path in [
                "/opt/homebrew/bin/git",
                "/usr/local/bin/git",
                "/usr/bin/git",
            ] {
                if std::path::Path::new(path).is_file() {
                    return path.into();
                }
            }
        }
    }
    "git".into()
}
