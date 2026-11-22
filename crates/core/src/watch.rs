//! Which filesystem changes matter to an open repository, and how a burst of
//! them collapses into one refresh.
//!
//! The UI's watcher reports raw events; these rules decide whether a refresh
//! is needed and whether it must reread metadata and history.
use crate::git::OPERATION_MARKERS;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The directories a repository's watcher covers.
#[derive(Clone, Debug)]
pub struct WatchScope {
    /// Worktree root.
    pub root: PathBuf,
    /// Git directory for this worktree.
    pub git: PathBuf,
    /// Git directory shared by linked worktrees.
    pub common: PathBuf,
}

/// Most worktree paths a change remembers. A burst touching more refreshes
/// without asking Git which of them it ignores.
pub const MAX_WORKTREE_PATHS: usize = 4096;

/// A pending change, or several coalesced into one.
#[derive(Clone, Debug)]
pub struct Change {
    pub error: Option<String>,
    /// Time of the most recent event in this group.
    pub event_time: Instant,
    /// Whether any event requires rechecking repository metadata and history.
    pub metadata_dirty: bool,
    /// The worktree paths the events touched, or `None` once there were too
    /// many to keep or an event could not name them.
    pub worktree_paths: Option<BTreeSet<PathBuf>>,
}

impl Change {
    /// Folds a newer change into this one. Metadata stays dirty once any
    /// event made it so, and an error is kept until a newer one replaces it.
    pub fn absorb(&mut self, newer: Change) {
        self.metadata_dirty |= newer.metadata_dirty;
        self.event_time = newer.event_time;
        if newer.error.is_some() {
            self.error = newer.error;
        }
        self.worktree_paths = match (self.worktree_paths.take(), newer.worktree_paths) {
            (Some(mut paths), Some(newer)) => {
                paths.extend(newer);
                (paths.len() <= MAX_WORKTREE_PATHS).then_some(paths)
            }
            _ => None,
        };
    }

    /// A change that rereads everything: a watcher error, or a rescan that
    /// may have missed events.
    pub fn interrupted(error: Option<String>) -> Self {
        Self {
            error,
            event_time: Instant::now(),
            metadata_dirty: true,
            worktree_paths: None,
        }
    }

    /// The worktree paths to test against Git's ignore rules before
    /// refreshing, or `None` when the change needs a refresh regardless:
    /// an error, a metadata change, or paths that were not kept.
    ///
    /// When Git ignores every one of these paths, the change cannot alter
    /// what GitCrab shows and needs no refresh.
    pub fn ignorable_paths(&self) -> Option<&BTreeSet<PathBuf>> {
        if self.error.is_some() || self.metadata_dirty {
            return None;
        }
        self.worktree_paths
            .as_ref()
            .filter(|paths| !paths.is_empty())
    }
}

impl WatchScope {
    /// Whether a change at `path` can affect what GitCrab shows. Lock files in
    /// Git metadata and objects are ignored; worktree files always count.
    pub fn is_relevant(&self, path: &Path) -> bool {
        let (relative, local) = if let Ok(relative) = path.strip_prefix(&self.git) {
            (relative, true)
        } else if let Ok(relative) = path.strip_prefix(&self.common) {
            (relative, false)
        } else {
            return path.starts_with(&self.root);
        };
        if relative
            .extension()
            .is_some_and(|extension| extension == "lock")
        {
            return false;
        }
        if local
            && OPERATION_MARKERS
                .iter()
                .any(|(marker, _)| relative.starts_with(marker))
        {
            return true;
        }
        relative.as_os_str().is_empty()
            || matches!(relative.to_str(), Some("HEAD" | "index" | "config.worktree") if local)
            || matches!(
                relative.to_str(),
                Some("packed-refs" | "config" | "shallow")
            )
            || relative.starts_with("refs")
            || relative == Path::new("info/grafts")
            || relative == Path::new("logs/refs/stash")
    }

    /// Whether `path` is inside Git metadata rather than the worktree.
    pub fn is_metadata(&self, path: &Path) -> bool {
        path.starts_with(&self.git) || path.starts_with(&self.common)
    }

    /// The change an event touching `paths` causes, or `None` when none of
    /// them is relevant.
    pub fn change_for<'a>(&self, paths: impl IntoIterator<Item = &'a PathBuf>) -> Option<Change> {
        let mut metadata_dirty = false;
        let mut worktree_paths = BTreeSet::new();
        let mut relevant = false;
        for path in paths.into_iter().filter(|path| self.is_relevant(path)) {
            relevant = true;
            if self.is_metadata(path) {
                metadata_dirty = true;
            } else {
                worktree_paths.insert(path.clone());
            }
        }
        relevant.then(|| Change {
            error: None,
            event_time: Instant::now(),
            metadata_dirty,
            worktree_paths: (worktree_paths.len() <= MAX_WORKTREE_PATHS).then_some(worktree_paths),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linked() -> WatchScope {
        let common = PathBuf::from("main/.git");
        WatchScope {
            root: "linked".into(),
            git: common.join("worktrees/linked"),
            common,
        }
    }

    #[test]
    fn linked_watch_scope_tracks_markers_and_only_ignores_metadata_locks() {
        let scope = linked();
        let (root, git, common) = (&scope.root, &scope.git, &scope.common);
        for path in [
            root.join("notes.lock"),
            git.join("MERGE_HEAD"),
            git.join("rebase-merge/head-name"),
            git.join("HEAD"),
            git.join("index"),
            common.join("refs/heads/main"),
            common.join("packed-refs"),
        ] {
            assert!(scope.is_relevant(&path), "{}", path.display());
        }
        for path in [
            git.join("index.lock"),
            common.join("refs/heads/main.lock"),
            common.join("objects/pack/object"),
            common.join("MERGE_HEAD"),
            common.join("worktrees/other/CHERRY_PICK_HEAD"),
        ] {
            assert!(!scope.is_relevant(&path), "{}", path.display());
        }
    }

    #[test]
    fn events_mark_metadata_dirty_only_when_metadata_changed() {
        let scope = linked();
        let file = scope.root.join("file");
        let head = scope.git.join("HEAD");
        let object = scope.common.join("objects/ab");
        assert!(!scope.change_for([&file]).unwrap().metadata_dirty);
        assert!(scope.change_for([&file, &head]).unwrap().metadata_dirty);
        assert!(scope.change_for([&object]).is_none());
        assert!(scope.change_for([]).is_none());
    }

    #[test]
    fn coalesced_changes_keep_dirty_metadata_and_the_latest_error() {
        let scope = linked();
        let file = scope.root.join("file");
        let mut pending = scope.change_for([&file]).unwrap();
        pending.absorb(Change::interrupted(None));
        assert!(pending.metadata_dirty);
        assert!(pending.error.is_none());
        pending.absorb(Change::interrupted(Some("interrupted".into())));
        pending.absorb(scope.change_for([&file]).unwrap());
        assert!(pending.metadata_dirty);
        assert_eq!(pending.error.as_deref(), Some("interrupted"));
    }

    #[test]
    fn only_worktree_changes_can_be_skipped_as_ignored() {
        let scope = linked();
        let build = scope.root.join("target/out.o");
        let other = scope.root.join("target/out.d");
        let head = scope.git.join("HEAD");
        let mut pending = scope.change_for([&build]).unwrap();
        pending.absorb(scope.change_for([&other, &build]).unwrap());
        let paths: Vec<_> = pending.ignorable_paths().unwrap().iter().collect();
        assert_eq!(paths, [&other, &build]);
        // Metadata needs a refresh whatever the worktree paths are.
        pending.absorb(scope.change_for([&head]).unwrap());
        assert!(pending.ignorable_paths().is_none());
        // So does an interruption, which cannot name what changed.
        let mut interrupted = scope.change_for([&build]).unwrap();
        interrupted.absorb(Change::interrupted(None));
        assert!(interrupted.ignorable_paths().is_none());
    }

    #[test]
    fn too_many_worktree_paths_refresh_without_an_ignore_check() {
        let scope = linked();
        let paths: Vec<_> = (0..=MAX_WORKTREE_PATHS)
            .map(|i| scope.root.join(format!("f{i}")))
            .collect();
        let mut pending = scope.change_for(&paths[..MAX_WORKTREE_PATHS]).unwrap();
        assert!(pending.ignorable_paths().is_some());
        pending.absorb(scope.change_for(&paths[MAX_WORKTREE_PATHS..]).unwrap());
        assert!(pending.ignorable_paths().is_none());
    }
}
