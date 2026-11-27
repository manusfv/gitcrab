//! Worktree reads and writes: the list, each worktree's uncommitted changes,
//! and add, remove, lock, unlock and prune.
use super::*;
use gitcrab_core::worktree::WorktreeChange;

/// What decides the worktree rows in history, to tell when the cached
/// layout still fits.
pub(super) fn wip_rows(worktrees: &[Worktree]) -> Vec<(&Path, &str, Option<WorktreeChanges>)> {
    worktrees
        .iter()
        .filter(|w| !w.current)
        .map(|w| (w.path.as_path(), w.head.as_str(), w.changes))
        .collect()
}

impl Git {
    /// Lists the repository's worktrees, marks the one at [`Git::path`], and
    /// counts the uncommitted changes of the others side by side.
    pub(super) fn worktrees(&self, known: Option<&[Worktree]>) -> GitResult<Vec<Worktree>> {
        let mut worktrees =
            parse_worktrees(&self.run(&["worktree", "list", "--porcelain", "-z"])?);
        let here = self.path.canonicalize().ok();
        for worktree in &mut worktrees {
            worktree.current = here.is_some() && worktree.path.canonicalize().ok() == here;
        }
        let earlier = |worktree: &Worktree| {
            known?
                .iter()
                .find(|w| w.path == worktree.path && w.head == worktree.head)?
                .changes
        };
        for worktree in &mut worktrees {
            worktree.changes = earlier(worktree);
        }
        std::thread::scope(|scope| {
            for worktree in worktrees
                .iter_mut()
                .filter(|w| !w.current && !w.prunable && !w.head.is_empty() && w.changes.is_none())
            {
                scope.spawn(|| {
                    worktree.changes = self
                        .sibling(&worktree.path)
                        .and_then(|git| git.status().ok())
                        .map(|status| WorktreeChanges::count(&status));
                });
            }
        });
        Ok(worktrees)
    }

    /// The same repository opened at another of its worktrees. `None` unless
    /// that worktree's `.git` file points inside this repository's metadata,
    /// so a planted file cannot direct Git at another repository.
    fn sibling(&self, path: &Path) -> Option<Git> {
        let common = self.common_dir.as_ref().or(self.git_dir.as_ref())?;
        let common = common.canonicalize().ok()?;
        let dot_git = path.join(".git");
        let git_dir = if dot_git.is_dir() {
            dot_git
        } else {
            let text = std::fs::read_to_string(&dot_git).ok()?;
            let pointer = Path::new(text.strip_prefix("gitdir:")?.trim());
            path.join(pointer)
        }
        .canonicalize()
        .ok()?;
        (git_dir == common || git_dir.starts_with(common.join("worktrees"))).then(|| Git {
            path: path.to_owned(),
            read_scope: self.read_scope.clone(),
            git_dir: Some(git_dir),
            common_dir: Some(common),
            index_cache: Default::default(),
            #[cfg(test)]
            observe_command: self.observe_command,
        })
    }

    /// Runs a worktree change. Deleting a branch on a remote runs under
    /// `scope`, which can cancel it.
    ///
    /// # Errors
    ///
    /// Returns Git's error. A removal that fails leaves the branch alone.
    pub(crate) fn change_worktree(
        &self,
        change: &WorktreeChange,
        scope: Option<&ReadScope>,
    ) -> Result<String, WriteError> {
        let path = change
            .path()
            .map(|path| {
                crate::platform::git_path(path)
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_default();
        let path = path.as_str();
        let shown = change
            .path()
            .map(gitcrab_core::presentation::display_location)
            .unwrap_or_default();
        match change {
            WorktreeChange::Add { start, branch, .. } => {
                validate_revision(start)?;
                let mut args = vec!["worktree", "add"];
                if let Some(branch) = branch {
                    validate_revision(branch)?;
                    self.run(&["check-ref-format", "--branch", branch])?;
                    args.extend(["-b", branch]);
                }
                args.extend(["--", path, start]);
                self.write(&args)?;
                Ok(format!("Created worktree {shown}"))
            }
            WorktreeChange::Remove {
                force,
                delete_branch,
                delete_remote,
                ..
            } => {
                for name in delete_branch.iter().chain(
                    delete_remote
                        .iter()
                        .flat_map(|(remote, branch)| [remote, branch]),
                ) {
                    validate_revision(name)?;
                }
                let mut args = vec!["worktree", "remove"];
                if *force {
                    // Twice also removes a locked worktree, as Git documents.
                    args.extend(["--force", "--force"]);
                }
                args.extend(["--", path]);
                self.write(&args)?;
                let mut message = format!("Removed worktree {shown}");
                if let Some(branch) = delete_branch {
                    self.write(&["branch", "-D", "--", branch])
                        .map_err(|error| {
                            WriteError::git(format!(
                                "Worktree was removed but deleting branch {branch} failed. {}",
                                error.message
                            ))
                        })?;
                    message.push_str(&format!(" and deleted branch {branch}"));
                }
                if let Some((remote, branch)) = delete_remote {
                    let refspec = format!(":refs/heads/{branch}");
                    let output =
                        self.network(&["push", remote, &refspec], scope, "Delete remote branch")?;
                    if !output.status.success() {
                        return Err(WriteError::git(format!(
                            "{message}, but deleting {remote}/{branch} failed. {}",
                            network_diagnostics(&output.stderr)
                        )));
                    }
                    message.push_str(&format!(" and {remote}/{branch}"));
                }
                Ok(message)
            }
            WorktreeChange::Lock { reason, .. } => {
                let mut args = vec!["worktree", "lock"];
                if !reason.is_empty() {
                    args.extend(["--reason", reason]);
                }
                args.extend(["--", path]);
                self.write(&args)?;
                Ok(format!("Locked worktree {shown}"))
            }
            WorktreeChange::Unlock { .. } => {
                self.write(&["worktree", "unlock", "--", path])?;
                Ok(format!("Unlocked worktree {shown}"))
            }
            WorktreeChange::Prune => {
                self.write(&["worktree", "prune"])?;
                Ok("Pruned worktrees whose folders are gone".into())
            }
        }
    }
}
