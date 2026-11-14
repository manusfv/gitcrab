//! Filesystem identity of a local worktree, confirmed by the user before Git runs.
//!
//! Finding a worktree on disk is the backend's job; see
//! [`crate::service::GitBackend::discover`].
use std::path::PathBuf;

/// Filesystem identity presented for trust before any Git command runs.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct RepositoryLocation {
    /// Canonical root of the selected worktree.
    pub worktree: PathBuf,
    /// Canonical Git directory containing metadata specific to this worktree.
    pub git_dir: PathBuf,
    /// Canonical Git directory shared by linked worktrees.
    pub common_dir: PathBuf,
}

impl RepositoryLocation {
    /// A location whose metadata sits in `<path>/.git`, for tests.
    #[doc(hidden)]
    pub fn test_path(path: impl Into<PathBuf>) -> Self {
        let worktree = path.into();
        let git_dir = worktree.join(".git");
        Self {
            worktree,
            common_dir: git_dir.clone(),
            git_dir,
        }
    }
}
