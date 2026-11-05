//! Host integration stays outside the Git and graph models.
use std::ffi::OsString;

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

/// Worktree identity for existing directories; missing paths retain their own key.
/// Linked worktrees have a .git file and remain distinct from their common repo.
pub fn worktree_identity(path: &std::path::Path) -> std::path::PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let identity = if canonical.is_dir() {
        canonical
            .ancestors()
            .find(|p| p.join(".git").exists())
            .unwrap_or(&canonical)
    } else {
        &canonical
    };
    #[cfg(windows)]
    {
        std::path::PathBuf::from(identity.to_string_lossy().to_lowercase())
    }
    #[cfg(not(windows))]
    {
        identity.to_path_buf()
    }
}
