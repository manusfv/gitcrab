//! Worktree writes and the Create worktree form's suggestions and checks,
//! following GitKraken's worktree panel.
use crate::git::{RepoSnapshot, Worktree};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A change to the repository's worktrees.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeChange {
    /// `git worktree add`: checks out `start` in a new folder. With `branch`
    /// it first creates that branch at `start`.
    Add {
        path: PathBuf,
        start: String,
        branch: Option<String>,
    },
    /// `git worktree remove`, then optionally deletes the branch it had
    /// checked out, locally and on a remote.
    Remove {
        path: PathBuf,
        force: bool,
        delete_branch: Option<String>,
        /// Remote name and branch name on that remote.
        delete_remote: Option<(String, String)>,
    },
    Lock {
        path: PathBuf,
        reason: String,
    },
    Unlock {
        path: PathBuf,
    },
    /// `git worktree prune`: forgets worktrees whose folders are gone.
    Prune,
}

impl WorktreeChange {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Add { .. } => "Create worktree",
            Self::Remove { .. } => "Remove worktree",
            Self::Lock { .. } => "Lock worktree",
            Self::Unlock { .. } => "Unlock worktree",
            Self::Prune => "Prune worktrees",
        }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Add { path, .. }
            | Self::Remove { path, .. }
            | Self::Lock { path, .. }
            | Self::Unlock { path } => Some(path),
            Self::Prune => None,
        }
    }

    /// Deleting a branch on a remote needs the network.
    pub fn is_network(&self) -> bool {
        matches!(
            self,
            Self::Remove {
                delete_remote: Some(_),
                ..
            }
        )
    }
}

/// What the form proposes once a reference is chosen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Suggestion {
    /// The branch the worktree checks out. Empty for a commit, which needs
    /// a typed name.
    pub branch: String,
    pub path: Option<PathBuf>,
}

/// GitKraken's suggestions: a local branch checks itself out; a remote
/// branch gets a local branch of the same name, or `<remote>-<name>` when
/// that name is taken. The folder is `<main worktree>.worktrees/<branch>`.
pub fn suggest(snapshot: &RepoSnapshot, start: &str, typed_branch: &str) -> Suggestion {
    let local = |name: &str| snapshot.branches.iter().any(|b| b.name == name);
    let suggested = if local(start) {
        start.to_owned()
    } else if let Some(remote) = snapshot.remote_refs.iter().find(|r| r.label == start) {
        let (remote_name, name) = remote.label.split_once('/').unwrap_or(("", &remote.label));
        if local(name) {
            format!("{remote_name}-{name}")
        } else {
            name.to_owned()
        }
    } else {
        String::new()
    };
    let branch = if typed_branch.is_empty() {
        suggested
    } else {
        typed_branch.to_owned()
    };
    let path = (!branch.is_empty())
        .then(|| main_path(snapshot))
        .flatten()
        .map(|main| native(&format!("{}.worktrees/{branch}", main.display())));
    Suggestion { branch, path }
}

/// Git prints paths with `/`; folders people see use the platform's separator.
fn native(path: &str) -> PathBuf {
    PathBuf::from(path.replace('/', std::path::MAIN_SEPARATOR_STR))
}

fn main_path(snapshot: &RepoSnapshot) -> Option<PathBuf> {
    snapshot
        .worktrees
        .iter()
        .find(|w| w.main)
        .map(|w| w.path.clone())
}

/// How serious a form message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
}

/// The change the form submits: whether `branch` is created, and the
/// message to show instead when the form cannot be submitted.
pub fn plan(
    snapshot: &RepoSnapshot,
    start: &str,
    branch: &str,
    path: &Path,
    path_exists: bool,
) -> Result<(WorktreeChange, Option<String>), (Level, String)> {
    let error = |message: &str| Err((Level::Error, message.to_owned()));
    if start.trim().is_empty() {
        return error("Choose a branch or commit to check out.");
    }
    if branch.is_empty() {
        return error("Name the branch the worktree checks out.");
    }
    let creates = start != branch;
    if creates {
        if snapshot.branches.iter().any(|b| b.name == branch) {
            return error(
                "Cannot create worktree because a local branch with the same name already exists",
            );
        }
        if crate::service::Operation::CreateBranch
            .validate_target(branch)
            .is_err()
        {
            return error("Cannot create worktree because the specified reference name is invalid");
        }
    } else if snapshot
        .worktrees
        .iter()
        .any(|w| w.branch.as_deref() == Some(branch))
    {
        return error(
            "Cannot create worktree because a worktree with the same branch name already exists",
        );
    }
    if start.starts_with('-') || start.contains('\0') {
        return error("Cannot create worktree because the specified reference name is invalid");
    }
    if path.as_os_str().is_empty() {
        return error("Choose the worktree's folder.");
    }
    if path_exists {
        return error(
            "Cannot create worktree because the specified working directory already exists",
        );
    }
    let warning = snapshot
        .worktrees
        .iter()
        .any(|w| path.starts_with(&w.path))
        .then(|| {
            "It is not recommended to choose a location inside a working directory. If you wish to proceed, please make sure your .gitignore is properly configured.".to_owned()
        });
    Ok((
        WorktreeChange::Add {
            path: path.to_owned(),
            start: start.to_owned(),
            branch: creates.then(|| branch.to_owned()),
        },
        warning,
    ))
}

/// The worktree that has `branch` checked out, other than the current one.
pub fn holding<'a>(snapshot: &'a RepoSnapshot, branch: &str) -> Option<&'a Worktree> {
    snapshot
        .worktrees
        .iter()
        .find(|w| !w.current && w.branch.as_deref() == Some(branch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{LocalBranch, ReferenceTip};

    fn snapshot() -> RepoSnapshot {
        RepoSnapshot {
            branches: ["main", "topic"]
                .map(|name| LocalBranch {
                    name: name.into(),
                    tip: "a".into(),
                    upstream: None,
                })
                .into(),
            remote_refs: ["origin/main", "origin/new"]
                .map(|label| ReferenceTip {
                    full_name: format!("refs/remotes/{label}"),
                    label: label.into(),
                    tip: "a".into(),
                })
                .into(),
            worktrees: vec![Worktree {
                path: "/r/app".into(),
                branch: Some("main".into()),
                main: true,
                current: true,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn suggestions_follow_the_reference_kind() {
        let s = snapshot();
        let suggest = |start: &str| suggest(&s, start, "");
        assert_eq!(suggest("topic").branch, "topic");
        assert_eq!(
            suggest("topic").path,
            Some(native("/r/app.worktrees/topic"))
        );
        assert_eq!(suggest("origin/new").branch, "new");
        assert_eq!(suggest("origin/main").branch, "origin-main");
        assert_eq!(suggest("abc123"), Suggestion::default());
        assert_eq!(super::suggest(&s, "abc123", "fix").branch, "fix");
    }

    #[test]
    fn plan_creates_a_branch_only_when_the_name_differs_from_the_start() {
        let s = snapshot();
        let path = Path::new("/r/app.worktrees/x");
        let add = |start: &str, branch: &str| plan(&s, start, branch, path, false);
        assert!(matches!(
            add("topic", "topic"),
            Ok((WorktreeChange::Add { branch: None, .. }, None))
        ));
        assert!(matches!(
            add("origin/new", "new"),
            Ok((
                WorktreeChange::Add {
                    branch: Some(_),
                    ..
                },
                None
            ))
        ));
        assert!(add("main", "main")
            .unwrap_err()
            .1
            .contains("worktree with the same branch"));
        assert!(add("abc", "topic")
            .unwrap_err()
            .1
            .contains("local branch with the same name"));
        assert!(add("abc", "bad name").unwrap_err().1.contains("invalid"));
        assert!(plan(&s, "topic", "topic", path, true)
            .unwrap_err()
            .1
            .contains("already exists"));
        let inside = plan(&s, "topic", "topic", Path::new("/r/app/wt"), false).unwrap();
        assert!(inside.1.is_some());
    }
}
