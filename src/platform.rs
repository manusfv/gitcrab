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
