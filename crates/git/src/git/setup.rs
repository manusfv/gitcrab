//! Creating and cloning repositories, before there is a worktree to open.
//!
//! Like the network commands, a clone runs under a cancellable scope with no
//! time budget and `GIT_TERMINAL_PROMPT=0`, so credential helpers and SSH
//! agents work but Git never waits on a hidden terminal.
use super::untranslated_messages;
use crate::process::{capture, ReadScope};
use gitcrab_core::git::{
    network_diagnostics, network_explanation, validate_branch_name, validate_clone_url,
};
use std::{
    fs,
    io::ErrorKind,
    path::Path,
    process::Command,
    sync::{atomic::AtomicBool, Arc},
};

/// A Git command that runs in `directory` with the user's normal
/// configuration and none of the caller's `GIT_*` overrides.
fn command(directory: &Path) -> Result<Command, String> {
    let mut cmd = Command::new(crate::platform::git_executable()?);
    for (key, _) in std::env::vars_os() {
        if key
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            cmd.env_remove(key);
        }
    }
    untranslated_messages(&mut cmd);
    #[cfg(test)]
    {
        crate::test_repo::isolate_config(&mut cmd, directory);
        cmd.env("GIT_AUTHOR_NAME", "GitCrab Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "GitCrab Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid");
    }
    cmd.current_dir(directory)
        .arg("--no-pager")
        .env("GIT_TERMINAL_PROMPT", "0");
    Ok(cmd)
}

fn is_empty_dir(path: &Path) -> bool {
    fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
}

/// Removes what a failed clone left in `destination`: the folder itself if
/// the clone created it, otherwise only its contents, since it was empty.
fn discard(destination: &Path, existed: bool) {
    if !existed {
        let _ = fs::remove_dir_all(destination);
        return;
    }
    if let Ok(entries) = fs::read_dir(destination) {
        for entry in entries.flatten() {
            let path = entry.path();
            let _ = if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
        }
    }
}

fn capitalized(text: String) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => text,
    }
}

/// Runs Git in `directory`. A failure is explained by `failure`, which gets
/// Git's diagnostics without its advice lines.
fn run(directory: &Path, args: &[&str], failure: fn(&str) -> String) -> Result<(), String> {
    let mut cmd = command(directory)?;
    cmd.args(args);
    match capture(cmd, None, 0, None) {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(failure(&network_diagnostics(&output.stderr))),
        Err(error) => Err(format!("Could not run Git: {error}")),
    }
}

fn first_commit_failure(diagnostics: &str) -> String {
    let text = diagnostics.to_ascii_lowercase();
    if text.contains("please tell me who you are") || text.contains("identity unknown") {
        "Git needs your name and email for the first commit. Set them from a terminal with `git config --global user.name \"Your Name\"` and `git config --global user.email you@example.com`, then try again.".into()
    } else {
        format!("Git could not make the first commit. {diagnostics}")
    }
}

/// Creates a repository in `path` on `branch`, creating the folder if
/// needed, and makes a first commit so the branch exists: it adds a
/// `README.md` titled with the folder's name, or commits nothing when the
/// folder already has one.
///
/// # Errors
///
/// Returns a message if `path` is a file or already holds a repository, the
/// branch name is not valid, or Git fails. Whatever was created is removed
/// again, so a failure leaves the folder as it was.
pub fn init(path: &Path, branch: &str) -> Result<(), String> {
    validate_branch_name(branch)?;
    if path.is_file() {
        return Err(format!("{} is a file, not a folder.", path.display()));
    }
    if path.join(".git").exists() {
        return Err(format!("{} is already a Git repository.", path.display()));
    }
    let existed = path.exists();
    fs::create_dir_all(path)
        .map_err(|error| format!("Could not create {}: {error}", path.display()))?;
    let readme = path.join("README.md");
    let write_readme = !readme.exists();
    let result = (|| {
        run(path, &["check-ref-format", "--branch", branch], |_| {
            "That branch name is not valid in Git. Choose another, such as main.".into()
        })?;
        run(
            path,
            &["init", "--quiet", &format!("--initial-branch={branch}")],
            |diagnostics| format!("Git could not create the repository. {diagnostics}"),
        )?;
        let mut commit = vec!["commit", "--quiet", "--message", "Initial commit"];
        if write_readme {
            let title = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            fs::write(&readme, format!("# {title}\n"))
                .map_err(|error| format!("Could not write README.md: {error}"))?;
            run(path, &["add", "--", "README.md"], |diagnostics| {
                format!("Git could not add README.md. {diagnostics}")
            })?;
        } else {
            commit.push("--allow-empty");
        }
        run(path, &commit, first_commit_failure)
    })();
    if result.is_err() {
        if existed {
            let _ = fs::remove_dir_all(path.join(".git"));
            if write_readme {
                let _ = fs::remove_file(&readme);
            }
        } else {
            let _ = fs::remove_dir_all(path);
        }
    }
    result
}

/// Clones `url` into `destination`, which must not exist or be empty, until
/// Git finishes or `cancelled` is set.
///
/// # Errors
///
/// Returns a plain explanation for common authentication and connection
/// failures, followed by Git's diagnostics, or a cancellation. A failed or
/// cancelled clone leaves nothing behind in `destination`.
pub fn clone(url: &str, destination: &Path, cancelled: Arc<AtomicBool>) -> Result<(), String> {
    validate_clone_url(url)?;
    let url = url.trim();
    let parent = destination
        .parent()
        .filter(|parent| parent.is_dir())
        .ok_or_else(|| {
            format!(
                "The folder to clone into, {}, does not exist.",
                destination.parent().unwrap_or(destination).display()
            )
        })?;
    let existed = destination.exists();
    if existed && !is_empty_dir(destination) {
        return Err(format!(
            "{} already exists and is not empty. Choose another folder name.",
            destination.display()
        ));
    }
    let mut cmd = command(parent)?;
    cmd.arg("clone")
        .arg("--")
        .arg(url)
        .arg(crate::platform::git_path(destination).as_ref());
    let result = capture(cmd, None, 0, Some(&ReadScope::network(cancelled)));
    let error = match result {
        Ok(output) if output.status.success() => return Ok(()),
        Ok(output) => {
            let diagnostics = network_diagnostics(&output.stderr);
            match network_explanation(&diagnostics, None) {
                Some(explanation) => {
                    format!("{}\n\nGit said: {diagnostics}", capitalized(explanation))
                }
                None => format!("Clone failed. {diagnostics}"),
            }
        }
        Err(error) if error.kind() == ErrorKind::Interrupted => {
            "Clone was cancelled. Git was stopped.".to_owned()
        }
        Err(error) => format!("Could not run Git to clone: {error}"),
    };
    discard(destination, existed);
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_repo::Fixture;

    fn cancelled() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    fn git(directory: &Path, args: &[&str]) -> String {
        let mut cmd = command(directory).unwrap();
        cmd.args(args);
        let output = capture(cmd, None, 0, None).unwrap();
        assert!(output.status.success(), "git {args:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    fn init_makes_a_first_commit_with_a_readme_on_the_chosen_branch() {
        let fixture = Fixture::new();
        let target = fixture.root().join("created");
        init(&target, "trunk").unwrap();
        assert_eq!(git(&target, &["branch", "--show-current"]).trim(), "trunk");
        assert_eq!(
            git(&target, &["log", "--format=%s", "--name-only"]).trim(),
            "Initial commit\n\nREADME.md"
        );
        assert_eq!(
            fs::read_to_string(target.join("README.md")).unwrap(),
            "# created\n"
        );
        let again = init(&target, "trunk").unwrap_err();
        assert!(again.contains("already a Git repository"), "{again}");
    }

    #[test]
    fn init_keeps_an_existing_readme_out_of_the_first_commit() {
        let fixture = Fixture::new();
        let target = fixture.root().join("existing");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("README.md"), "mine\n").unwrap();
        init(&target, "main").unwrap();
        assert_eq!(
            git(&target, &["log", "--format=%s", "--name-only"]).trim(),
            "Initial commit"
        );
        assert_eq!(
            fs::read_to_string(target.join("README.md")).unwrap(),
            "mine\n"
        );
    }

    #[test]
    fn a_refused_init_leaves_the_folder_as_it_was() {
        let fixture = Fixture::new();
        let target = fixture.root().join("refused");
        let error = init(&target, "bad..name").unwrap_err();
        assert!(error.contains("branch name"), "{error}");
        assert!(!target.exists());

        fs::create_dir(&target).unwrap();
        init(&target, "bad..name").unwrap_err();
        assert!(target.is_dir() && is_empty_dir(&target));
    }

    #[test]
    fn clone_copies_a_repository_and_refuses_a_used_folder() {
        let source = Fixture::new();
        crate::git::network_tests::commit(&source, "a.txt", "a");
        let target = source.root().join("copy");
        let url = source.git.path.display().to_string();
        clone(&url, &target, cancelled()).unwrap();
        assert!(target.join("a.txt").is_file());

        let refused = clone(&url, &target, cancelled()).unwrap_err();
        assert!(refused.contains("not empty"), "{refused}");
        assert!(target.join("a.txt").is_file());
    }

    #[test]
    fn a_failed_clone_leaves_nothing_behind() {
        let fixture = Fixture::new();
        let target = fixture.root().join("missing-copy");
        let missing = fixture.root().join("no-such-repository");
        let error = clone(&missing.display().to_string(), &target, cancelled()).unwrap_err();
        assert!(!error.is_empty());
        assert!(!target.exists());

        let empty = fixture.root().join("empty");
        fs::create_dir(&empty).unwrap();
        clone(&missing.display().to_string(), &empty, cancelled()).unwrap_err();
        assert!(empty.is_dir() && is_empty_dir(&empty));
    }

    #[test]
    fn clone_refuses_option_like_urls() {
        let fixture = Fixture::new();
        let target = fixture.root().join("copy");
        let error = clone("--upload-pack=touch x", &target, cancelled()).unwrap_err();
        assert!(error.contains("dash"), "{error}");
        assert!(!target.exists());
    }
}
