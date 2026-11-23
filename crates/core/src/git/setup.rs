//! Rules for creating and cloning repositories: what a clone's folder is
//! called and which names and URLs GitCrab accepts.

/// The folder a clone of `url` gets by default, as `git clone` names it:
/// the last path segment without a trailing `.git`.
///
/// Handles URLs (`https://host/owner/repo.git`), scp-like addresses
/// (`git@host:owner/repo.git`) and local paths. `None` when the URL has no
/// usable last segment.
pub fn clone_folder_name(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches(['/', '\\']);
    let last = trimmed.rsplit(['/', '\\', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    validate_folder_name(name).ok().map(|_| name.to_owned())
}

/// Checks a clone URL before Git sees it.
///
/// # Errors
///
/// Returns why the URL cannot be used.
pub fn validate_clone_url(url: &str) -> Result<(), &'static str> {
    let url = url.trim();
    if url.is_empty() {
        Err("Enter the repository's URL.")
    } else if url.starts_with('-') {
        Err("A repository URL cannot start with a dash.")
    } else if url.contains(char::is_control) {
        Err("The URL contains control characters.")
    } else {
        Ok(())
    }
}

/// The branch a new repository starts on unless the user picks another.
pub const DEFAULT_BRANCH: &str = "main";

/// Checks a new repository's branch name for what can be told without Git.
/// Git's own `check-ref-format` has the final say.
///
/// # Errors
///
/// Returns why the name cannot be used.
pub fn validate_branch_name(name: &str) -> Result<(), &'static str> {
    if name.trim().is_empty() {
        Err("Enter a branch name, such as main.")
    } else if name.starts_with('-') {
        Err("A branch name cannot start with a dash.")
    } else if name.chars().any(|c| c.is_whitespace() || c.is_control()) {
        Err("A branch name cannot contain spaces.")
    } else {
        Ok(())
    }
}

/// Checks the name of a folder GitCrab creates for a new or cloned
/// repository. Names that are invalid on any desktop platform are refused so
/// that a repository can move between them.
///
/// # Errors
///
/// Returns why the name cannot be used.
pub fn validate_folder_name(name: &str) -> Result<(), &'static str> {
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or_default();
    if name.trim().is_empty() {
        Err("Enter a folder name.")
    } else if name == "." || name == ".." {
        Err("Choose a folder name other than . or ..")
    } else if name
        .chars()
        .any(|c| c.is_control() || r#"<>:"/\|?*"#.contains(c))
    {
        Err(r#"Folder names cannot contain < > : " / \ | ? or *."#)
    } else if name.ends_with(['.', ' ']) || name.starts_with(' ') {
        Err("Folder names cannot start with a space or end with a space or dot.")
    } else if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        Err("That name is reserved on Windows. Choose another.")
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_folder_names_follow_the_last_segment() {
        for (url, name) in [
            ("https://github.com/owner/repo.git", "repo"),
            ("https://github.com/owner/repo", "repo"),
            ("https://github.com/owner/repo/", "repo"),
            ("git@github.com:owner/repo.git", "repo"),
            ("git@host:repo.git", "repo"),
            ("ssh://git@host:22/owner/my.project.git", "my.project"),
            (r"C:\Users\me\code\tool", "tool"),
            ("  https://host/a/b.git  ", "b"),
        ] {
            assert_eq!(clone_folder_name(url).as_deref(), Some(name), "{url}");
        }
        for url in ["", "https://", ".git", "host:"] {
            assert_eq!(clone_folder_name(url), None, "{url}");
        }
    }

    #[test]
    fn clone_urls_cannot_pass_options_to_git() {
        assert!(validate_clone_url("https://host/repo.git").is_ok());
        assert!(validate_clone_url("  ").is_err());
        assert!(validate_clone_url("--upload-pack=touch x").is_err());
        assert!(validate_clone_url("https://host/\nrepo").is_err());
    }

    #[test]
    fn branch_names_are_checked_before_git_runs() {
        for name in [DEFAULT_BRANCH, "trunk", "feature/x"] {
            assert!(validate_branch_name(name).is_ok(), "{name}");
        }
        for name in ["", " ", "-main", "my branch", "a\tb"] {
            assert!(validate_branch_name(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn folder_names_must_work_on_every_platform() {
        for name in ["repo", "my.project", "Repo 2", ".dotfiles"] {
            assert!(validate_folder_name(name).is_ok(), "{name}");
        }
        for name in [
            "",
            " ",
            ".",
            "..",
            "a/b",
            r"a\b",
            "a:b",
            "what?",
            "trailing.",
            "trailing ",
            " leading",
            "con",
            "COM1.txt",
        ] {
            assert!(validate_folder_name(name).is_err(), "{name:?}");
        }
    }
}
