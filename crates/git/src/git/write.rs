//! Writes: staging, hunks, commits, branches, stashes and resets.
use super::*;

impl Git {
    /// Stages or unstages a file after checking its reviewed status entry.
    ///
    /// A rename updates both its source and destination. A directory can be
    /// selected only when it represents a single indexed submodule entry.
    ///
    /// # Errors
    ///
    /// Returns an error if the entry changed or is unresolved, its path cannot
    /// be addressed, or Git cannot update the index.
    pub fn stage_reviewed(&self, reviewed: &FileChange, unstage: bool) -> Result<(), WriteError> {
        if reviewed.is_unmerged() {
            return Err(Self::unmerged_write_error());
        }
        if let Some(issue) = reviewed.path_issue() {
            return Err(issue.into());
        }
        if !reviewed.on_side(unstage) {
            return Err("File status changed since review. Refresh and review it again.".into());
        }
        let mut paths = vec![reviewed.path.as_str()];
        if (unstage && reviewed.index == 'R') || (!unstage && reviewed.worktree == 'R') {
            paths.push(
                reviewed
                    .original_path
                    .as_deref()
                    .ok_or("Rename source is missing.")?,
            );
        }
        // The status check, the conflict check and choosing the command are
        // independent reads, so their Git processes run side by side.
        let (status, unmerged, args) = std::thread::scope(|scope| {
            let status = scope.spawn(|| self.status());
            let unmerged = scope.spawn(|| self.has_unmerged_paths(&paths));
            let args = self.stage_command(unstage, false);
            (
                status
                    .join()
                    .map_err(|_| "Git status read failed".to_owned()),
                unmerged
                    .join()
                    .map_err(|_| "Git conflict check failed".to_owned()),
                args,
            )
        });
        if !status??.contains(reviewed) {
            return Err("File status changed since review. Refresh and review it again.".into());
        }
        for path in &paths {
            if path.ends_with('/') || self.path.join(path).is_dir() {
                // An indexed gitlink is a single entry even though it has a worktree directory.
                let entries = self.run(&["ls-files", "--stage", "--", path])?;
                let is_gitlink = entries.lines().count() == 1 && entries.starts_with("160000 ");
                if !is_gitlink {
                    return Err(
                        "The reviewed path now names a directory. Refresh and review its files."
                            .into(),
                    );
                }
            }
        }
        if unmerged?? {
            return Err(Self::unmerged_write_error());
        }
        let mut args = args?;
        args.extend_from_slice(&paths);
        self.write(&args).map(|_| ())
    }

    /// Stages or unstages all changes across the worktree when Git runs.
    ///
    /// Unstaging preserves working files, including in a repository with no
    /// commits. Unmerged paths keep their conflict entries. Git's own `add` and
    /// `restore` would resolve them even when excluded by a pathspec, so with
    /// conflicts present the other changed paths are read when this runs and
    /// passed to Git explicitly.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot determine HEAD or update the index. An
    /// [`FailureKind::Inspect`] error means the conflict entries changed
    /// while Git ran.
    pub fn stage_all(&self, unstage: bool) -> Result<BulkStage, WriteError> {
        let (before, conflicts, args) = std::thread::scope(|scope| {
            let before = scope.spawn(|| self.status());
            let conflicts = scope.spawn(|| self.unmerged_entries());
            let args = self.stage_command(unstage, false);
            (
                before
                    .join()
                    .map_err(|_| "Git status read failed".to_owned()),
                conflicts
                    .join()
                    .map_err(|_| "Git conflict check failed".to_owned()),
                args,
            )
        });
        let (before, conflicts) = (before??, conflicts??);
        if conflicts.is_empty() {
            // The whole index was just checked for conflicts, so the write
            // needs no second check of its own pathspec.
            let mut args = args?;
            args.push(".");
            self.write(&args)?;
        } else {
            let mut paths = std::collections::BTreeSet::new();
            for file in before
                .iter()
                .filter(|file| file.on_side(unstage) && !file.unsupported_path)
            {
                paths.insert(file.path.as_str());
                if unstage && file.index == 'R' {
                    paths.extend(file.original_path.as_deref());
                }
            }
            if !paths.is_empty() {
                let mut spec = Vec::new();
                for path in paths {
                    spec.extend_from_slice(path.as_bytes());
                    spec.push(0);
                }
                let args = self.stage_command(unstage, true)?;
                let result = capture(self.command(&args)?, Some(spec.as_slice()), 0, None)
                    .map_err(WriteError::inspect)?;
                if !result.status.success() {
                    return Err(WriteError::git(Self::failed_output(&result)));
                }
            }
            if self.unmerged_entries().map_err(WriteError::inspect)? != conflicts {
                return Err(WriteError::inspect(
                    "Conflict entries changed during the bulk index update. Refresh and review the conflicts.",
                ));
            }
        }
        let after = self.status().map_err(WriteError::inspect)?;
        let changed = before
            .iter()
            .chain(&after)
            .filter(|file| !before.contains(file) || !after.contains(file))
            .map(|file| file.path.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        Ok(BulkStage {
            changed,
            conflicts: after.iter().filter(|file| file.is_unmerged()).count(),
        })
    }

    /// Reads raw unmerged index entries from `git ls-files --unmerged`.
    fn unmerged_entries(&self) -> GitResult<Vec<u8>> {
        self.bytes(&["ls-files", "--unmerged", "-z"])
    }

    fn stage_command(&self, unstage: bool, from_stdin: bool) -> GitResult<Vec<&'static str>> {
        let mut args = if !unstage {
            vec!["add", "-A"]
        } else if self.head_is_born()? {
            vec!["restore", "--staged"]
        } else {
            // An unborn index has no HEAD to restore. --cached guarantees that
            // even a partially staged file keeps its working-tree contents.
            vec!["rm", "--cached", "-r", "--force", "--quiet"]
        };
        if from_stdin {
            args.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
        } else {
            args.push("--");
        }
        Ok(args)
    }

    #[cfg(test)]
    fn stage_paths(&self, paths: &[&str], unstage: bool) -> Result<(), WriteError> {
        let (args, unmerged) = std::thread::scope(|scope| {
            let unmerged = scope.spawn(|| self.reject_unmerged_paths(paths));
            let args = self.stage_command(unstage, false);
            (args, unmerged.join())
        });
        unmerged.map_err(|_| WriteError::from("Git conflict check failed"))??;
        let mut args: Vec<&str> = args?;
        args.extend_from_slice(paths);
        self.write(&args).map(|_| ())
    }

    /// Whether HEAD names a commit. One Git call unless the branch is unborn.
    fn head_is_born(&self) -> GitResult<bool> {
        let output = self.head_commit_output()?;
        if output.status.success() {
            return Ok(true);
        }
        Ok(!self
            .head_id_from(output, &self.symbolic_head()?)?
            .is_empty())
    }

    /// Marks an unmerged path resolved by staging its working-tree version.
    ///
    /// A missing working file stages the deletion. Unless `allow_markers` is
    /// set, a regular file that still contains a conflict marker line is
    /// refused with [`FailureKind::ConflictMarkers`].
    ///
    /// # Errors
    ///
    /// Returns an error if the path is invalid, no longer unmerged, names a
    /// directory, still has markers, or Git cannot update the index.
    pub fn mark_resolved(&self, path: &str, allow_markers: bool) -> Result<(), WriteError> {
        if path.is_empty()
            || path == "."
            || Path::new(path).is_absolute()
            || path.split(['/', '\\']).any(|s| s == "..")
        {
            return Err("Invalid conflict path".into());
        }
        if !self.has_unmerged_paths(&[path])? {
            return Err(
                "This path no longer has unresolved conflicts. Refresh and review it again.".into(),
            );
        }
        let file = self.path.join(path);
        match std::fs::symlink_metadata(&file) {
            Ok(metadata) if metadata.is_dir() => {
                return Err(
                    "The conflicted path now names a directory. Resolve it with Git.".into(),
                )
            }
            Ok(metadata) if metadata.is_file() && !allow_markers => {
                let content = std::fs::read(&file).map_err(|e| e.to_string())?;
                if let Some(line) = conflict_marker_line(&content) {
                    return Err(WriteError {
                        kind: FailureKind::ConflictMarkers,
                        message: format!(
                            "{} still contains conflict markers (<<<<<<<, =======, >>>>>>>) at line {line}. Edit the file to resolve them, or confirm that the markers are intended.",
                            display_path(path)
                        ),
                    });
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string().into()),
        }
        self.write(&["add", "-A", "--", path])?;
        if self
            .has_unmerged_paths(&[path])
            .map_err(WriteError::inspect)?
        {
            return Err(WriteError::inspect(
                "Git kept unresolved entries for this path. Refresh and inspect it.",
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn stage(&self, path: &str) -> GitResult<()> {
        self.stage_paths(&[path], false)
            .map_err(|error| error.message)
    }

    #[cfg(test)]
    pub(crate) fn unstage(&self, path: &str) -> GitResult<()> {
        if path == "." {
            return self
                .stage_all(true)
                .map(|_| ())
                .map_err(|error| error.message);
        }
        let file = self
            .status()?
            .into_iter()
            .find(|file| file.path == path && file.on_side(true))
            .ok_or("Expected a staged file")?;
        self.stage_reviewed(&file, true)
            .map_err(|error| error.message)
    }

    /// Reads a working changes patch and splits it into individual hunks.
    ///
    /// See [`Self::compare`] for how `path` and `staged` select the comparison.
    /// Check [`Hunk::blocked_reason`] before using a hunk for a write.
    pub fn hunks(&self, path: &str, staged: bool) -> GitResult<Vec<Hunk>> {
        Ok(split_hunks(&self.diff(None, path, staged)?))
    }

    /// Applies a hunk to the index, reversing it when `unstage` is `true`.
    ///
    /// The caller should ensure the hunk is from the current comparison.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported patch headers or if Git cannot apply the patch.
    pub fn apply_hunk(&self, hunk: &Hunk, unstage: bool) -> Result<(), WriteError> {
        if let Some(reason) = hunk.blocked_reason() {
            return Err(reason.into());
        }
        let mut args = vec!["apply", "--cached", "-p1", "--whitespace=nowarn"];
        if unstage {
            args.push("--reverse");
        }
        self.apply_patch(&args, hunk)
    }

    fn apply_patch(&self, args: &[&str], hunk: &Hunk) -> Result<(), WriteError> {
        let result = capture(self.command(args)?, Some(hunk.patch.as_bytes()), 0, None)
            .map_err(WriteError::inspect)?;
        if result.status.success() {
            Ok(())
        } else {
            Err(WriteError::git(Self::failed_output(&result)))
        }
    }

    /// Reverses a hunk in a working file and verifies the resulting bytes.
    ///
    /// Use a repository-relative `path` and a hunk from its current unstaged
    /// patch. Only ordinary UTF-8 files with LF endings and no content filters
    /// are supported. Verification also checks that HEAD and the index stayed
    /// the same. External processes can still change the file during the write.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsupported path or patch, a changed working
    /// file, or a Git failure. [`FailureKind::Inspect`] means the caller should
    /// refresh and inspect the resulting state before retrying.
    pub fn discard_hunk(&self, path: &str, hunk: &Hunk) -> Result<(), WriteError> {
        if let Some(reason) = hunk.blocked_reason() {
            return Err(reason.into());
        }
        if path.is_empty()
            || path == "."
            || PathBuf::from(path).is_absolute()
            || path.split(['/', '\\']).any(|s| s == "..")
        {
            return Err("Invalid discard path".into());
        }
        if self
            .run(&["config", "--get", "core.autocrlf"])
            .is_ok_and(|s| s.trim().eq_ignore_ascii_case("true"))
        {
            return Err(
                "Discard doesn't support automatic CRLF conversion yet. Use Git for this file."
                    .into(),
            );
        }
        let attrs = self.run(&[
            "check-attr",
            "-z",
            "filter",
            "working-tree-encoding",
            "eol",
            "--",
            path,
        ])?;
        for fields in attrs.split('\0').collect::<Vec<_>>().as_chunks::<3>().0 {
            if fields[2] != "unspecified"
                && fields[2] != "unset"
                && !(fields[1] == "eol" && fields[2] == "lf")
            {
                return Err("Discard doesn't support content filters or non-LF line endings yet. Use Git for this file.".into());
            }
        }
        let file = self.path.join(path);
        if std::fs::symlink_metadata(&file)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("Discard of symlinks is not supported".into());
        }
        let before = std::fs::read(&file).map_err(|e| e.to_string())?;
        let expected = crate::hunk_edit::reversed_worktree(hunk, &before)?;
        let identity = self.write_identity()?;
        if std::fs::read(&file).map_err(|e| e.to_string())? != before {
            return Err("Working file changed before discard".into());
        }
        self.reject_unmerged_paths(&[path])?;
        self.apply_patch(&["apply", "--reverse", "-p1", "--whitespace=nowarn"], hunk)?;
        if std::fs::read(&file).map_err(WriteError::inspect)? != expected
            || self.write_identity().map_err(WriteError::inspect)? != identity
        {
            return Err(WriteError::inspect(
                "Discard outcome uncertain. Git state differs from the reviewed effect.",
            ));
        }
        Ok(())
    }
    /// Reads HEAD and the index entries for use in subsequent write review.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot read HEAD or the index.
    pub fn write_identity(&self) -> GitResult<WriteIdentity> {
        // The three reads are independent, so each Git process starts at once.
        std::thread::scope(|scope| {
            let symbolic = scope.spawn(|| self.symbolic_head());
            let commit = scope.spawn(|| self.head_commit_output());
            let index = self.index_listing();
            let symbolic_head = symbolic.join().map_err(|_| "HEAD read failed")??;
            let commit = commit.join().map_err(|_| "HEAD read failed")??;
            let index = index?;
            let head = self.head_id_from(commit, &symbolic_head)?;
            Ok(WriteIdentity {
                head,
                symbolic_head,
                index,
            })
        })
    }
    /// Commits the reviewed staged changes and returns the verified commit object ID.
    ///
    /// Checks the reviewed HEAD and index before committing, then verifies the
    /// resulting parent, tree and symbolic HEAD. Normal Git commit hooks can run.
    ///
    /// # Errors
    ///
    /// Rejects a missing summary, active Git operation, stale review or empty
    /// staged diff. Git failures and uncertain outcomes require inspecting the
    /// repository before retrying.
    pub fn commit_reviewed(
        &self,
        message: &str,
        reviewed: &WriteIdentity,
    ) -> Result<String, WriteError> {
        if message.lines().next().is_none_or(|s| s.trim().is_empty()) {
            return Err("Enter a commit summary".into());
        }
        // These checks only read, so their Git processes run side by side.
        // Failures are reported in the order the checks were written.
        // write-tree can rewrite the index, which Windows refuses while
        // another process has it open, so it runs on its own afterwards.
        let (markers, identity, staged) = std::thread::scope(|scope| {
            let markers = scope.spawn(|| self.operation_markers());
            let identity = scope.spawn(|| self.write_identity());
            let staged = self.run(&["diff", "--cached", "--name-only", "-z"]);
            let failed = |_| "Git read failed".to_owned();
            (
                markers.join().map_err(failed),
                identity.join().map_err(failed),
                Ok::<_, String>(staged),
            )
        });
        if !markers??.is_empty() {
            return Err("An active Git operation blocks ordinary commit. Complete or abort it in a terminal, then refresh.".into());
        }
        if identity?? != *reviewed {
            return Err("HEAD or index changed since review. Refresh and review staged changes. Your draft is kept in this tab.".into());
        }
        if staged??.is_empty() {
            return Err("No staged changes to commit".into());
        }
        let tree = self.write(&["write-tree"])?;
        if self.write_identity()? != *reviewed {
            return Err("Index changed before commit. Refresh and review again.".into());
        }
        self.commit(message)?;
        // The three checks of the result are independent reads.
        let (state, symbolic, resolved) = std::thread::scope(|scope| {
            let symbolic = scope.spawn(|| self.symbolic_head());
            let resolved = scope.spawn(|| self.resolve_commit("HEAD"));
            let state = self.run(&[
                "show",
                "-s",
                "--no-show-signature",
                "--no-notes",
                "--format=%H%x00%P%x00%T",
                "HEAD",
            ]);
            (
                state,
                symbolic.join().map_err(|_| "HEAD read failed".to_owned()),
                resolved.join().map_err(|_| "HEAD read failed".to_owned()),
            )
        });
        let state = state.map_err(WriteError::inspect)?;
        let mut fields = state.trim().splitn(3, '\0');
        let (Some(sha), Some(parents), Some(actual_tree)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return Err(WriteError::inspect(
                "Could not verify the commit parent and tree.",
            ));
        };
        let sha = sha.to_owned();
        if parents != reviewed.head
            || actual_tree != tree.trim()
            || symbolic.and_then(|s| s).map_err(WriteError::inspect)? != reviewed.symbolic_head
            || resolved.and_then(|r| r).map_err(WriteError::inspect)? != sha
        {
            return Err(WriteError::inspect("Commit outcome uncertain. The resulting parent, tree, or branch differs from the reviewed commit. Your draft is kept in this tab."));
        }
        Ok(sha)
    }
    /// Runs `git commit` with the supplied message and currently staged changes.
    ///
    /// Use [`Self::commit_reviewed`] when the user confirmed a specific index state.
    ///
    /// # Errors
    ///
    /// Returns an error for a blank message or a Git commit failure.
    pub fn commit(&self, message: &str) -> Result<(), WriteError> {
        if message.trim().is_empty() {
            return Err("Enter a commit message".into());
        }
        self.write(&["commit", "-m", message]).map(|_| ())
    }
    /// Creates a local branch at HEAD without switching to it.
    ///
    /// # Errors
    ///
    /// Returns an error if the branch name is invalid or Git cannot create the branch.
    pub fn create_branch(&self, name: &str) -> Result<(), WriteError> {
        validate_revision(name)?;
        self.run(&["check-ref-format", "--branch", name])?;
        self.write(&["branch", "--", name]).map(|_| ())
    }
    /// Creates a local branch at `start` without switching to it.
    ///
    /// # Errors
    ///
    /// Returns an error if the branch name or revision is invalid or Git cannot create the branch.
    pub fn create_branch_at(&self, name: &str, start: &str) -> Result<(), WriteError> {
        validate_revision(name)?;
        validate_revision(start)?;
        self.run(&["check-ref-format", "--branch", name])?;
        self.write(&["branch", "--", name, start]).map(|_| ())
    }
    /// Switches to a named local branch or checks out the revision with detached HEAD.
    ///
    /// # Errors
    ///
    /// Returns an error if the target is invalid or Git cannot switch to it.
    pub fn checkout(&self, branch: &str) -> Result<(), WriteError> {
        validate_revision(branch)?;
        let local_ref = format!("refs/heads/{branch}");
        if self
            .run(&["show-ref", "--verify", "--quiet", &local_ref])
            .is_ok()
        {
            self.switch_to(Some(&local_ref), branch)
        } else {
            let commit_ref = format!("{branch}^{{commit}}");
            self.run(&["rev-parse", "--verify", &commit_ref])?;
            self.switch_to(None, branch)
        }
    }

    /// Switches to the local branch named by `full_ref` (`refs/heads/...`).
    /// Any other ref, or none, detaches HEAD at commit `id`.
    pub(crate) fn switch_to(&self, full_ref: Option<&str>, id: &str) -> Result<(), WriteError> {
        match full_ref.and_then(|name| name.strip_prefix("refs/heads/")) {
            Some(branch) => {
                validate_revision(branch)?;
                self.write(&["switch", "--", branch])
            }
            None => {
                validate_revision(id)?;
                self.write(&["switch", "--detach", id])
            }
        }
        .map(|_| ())
    }
    /// Merges the target into the current checkout using Git's default message.
    /// A Git failure may leave an in-progress merge or conflicts.
    pub fn merge(&self, target: &str) -> Result<(), WriteError> {
        validate_revision(target)?;
        self.write(&["merge", "--no-edit", "--", target])
            .map(|_| ())
    }
    /// Cherry-picks the target onto the current checkout.
    /// A Git failure may leave an in-progress operation or conflicts.
    pub fn cherry_pick(&self, target: &str) -> Result<(), WriteError> {
        validate_revision(target)?;
        self.write(&["cherry-pick", target]).map(|_| ())
    }
    /// Reverts the target using Git's default commit message.
    /// A Git failure may leave an in-progress operation or conflicts.
    pub fn revert(&self, target: &str) -> Result<(), WriteError> {
        validate_revision(target)?;
        self.write(&["revert", "--no-edit", target]).map(|_| ())
    }
    /// Stashes tracked changes with the default GitCrab message.
    pub fn stash(&self) -> Result<(), WriteError> {
        self.stash_with_message("GitCrab stash")
    }

    /// Stashes tracked changes with `message`. Untracked files stay in the worktree.
    ///
    /// # Errors
    ///
    /// Besides Git failures, returns [`NOTHING_TO_STASH`] when Git saved no
    /// stash because no tracked file differs from HEAD. Git itself exits
    /// successfully in that case.
    pub fn stash_with_message(&self, message: &str) -> Result<(), WriteError> {
        let before = self.ref_commit("refs/stash").map_err(WriteError::inspect)?;
        self.write(&["stash", "push", "-m", message])?;
        if self.ref_commit("refs/stash").map_err(WriteError::inspect)? == before {
            return Err(NOTHING_TO_STASH.into());
        }
        Ok(())
    }
    /// Applies the latest stash and drops it if Git applies it successfully.
    pub fn stash_pop(&self) -> Result<(), WriteError> {
        self.write(&["stash", "pop"]).map(|_| ())
    }

    /// Drops the newest stash entry whose commit is `id`.
    ///
    /// Git addresses stash entries only by position, and no Git command drops
    /// an entry by object ID. Another client can change the list between
    /// reading the position and dropping it, so the list is compared before
    /// and after the drop. An entry removed by mistake is stored again, so no
    /// stash is ever lost. If no entry holds `id` any more, nothing is dropped
    /// and the call succeeds.
    pub fn drop_stash_object(&self, id: &str) -> Result<(), WriteError> {
        let before = self.stash_entries().map_err(WriteError::inspect)?;
        let Some(index) = before.iter().position(|(entry, _)| entry == id) else {
            return Ok(());
        };
        self.drop_stash_entry(&before, index)
    }

    /// Drops `stash@{index}`, which `before` says holds the entry to remove,
    /// then restores any other entry the drop removed instead.
    pub(super) fn drop_stash_entry(
        &self,
        before: &[(String, String)],
        index: usize,
    ) -> Result<(), WriteError> {
        let id = &before[index].0;
        let entry = format!("stash@{{{index}}}");
        let dropped = self.write(&["stash", "drop", "--quiet", &entry]);
        let after = self.stash_entries().map_err(WriteError::inspect)?;
        let count = |list: &[(String, String)], wanted: &(String, String)| {
            list.iter().filter(|entry| *entry == wanted).count()
        };
        let mut restored = 0;
        let mut seen = std::collections::BTreeSet::new();
        for other in before.iter().filter(|(other, _)| other != id) {
            if !seen.insert(other) {
                continue;
            }
            for _ in count(&after, other)..count(before, other) {
                self.stash_store(&other.1, &other.0)
                    .map_err(|error| WriteError::inspect(format!("The stash list changed while GitCrab removed the applied stash, and another entry ({}) could not be put back. Its commit is {}; restore it with `git stash store {}`. {}", other.1, other.0, other.0, error.message)))?;
                restored += 1;
            }
        }
        dropped?;
        let removed = after.iter().filter(|(entry, _)| entry == id).count()
            < before.iter().filter(|(entry, _)| entry == id).count();
        match (restored, removed) {
            (0, true) => Ok(()),
            (0, false) => Err(WriteError::inspect(
                "The changes were applied, but the stash list changed at the same time and the stash is still listed. Check the stash list and drop it yourself if you no longer need it.",
            )),
            (_, true) => Err(WriteError::inspect(
                "The changes were applied. The stash list changed at the same time, so Git also removed another entry; GitCrab put it back at the top of the stash list. Check the stash list.",
            )),
            (_, false) => Err(WriteError::inspect(
                "The changes were applied, but the stash list changed at the same time and Git removed a different entry. GitCrab put that entry back at the top of the stash list, and the applied stash is still listed. Check the stash list.",
            )),
        }
    }
    /// Rebases the current branch onto `target`.
    /// A Git failure may leave an in-progress rebase or conflicts.
    pub fn rebase(&self, target: &str) -> Result<(), WriteError> {
        validate_revision(target)?;
        self.write(&["rebase", target]).map(|_| ())
    }
    /// Resets HEAD to `target` while preserving working-tree contents.
    /// With `soft`, the index stays as it is. A mixed reset updates the index
    /// to match `target`.
    pub fn reset(&self, target: &str, soft: bool) -> Result<(), WriteError> {
        validate_revision(target)?;
        self.write(&["reset", if soft { "--soft" } else { "--mixed" }, target])
            .map(|_| ())
    }

    // Undo journal support (`crate::journal`). Reads report what the journal's
    // preconditions need; writes are the reversing commands it reviews.

    /// Full symbolic ref of HEAD (empty when detached) and its commit ID
    /// (empty for an unborn branch).
    pub(crate) fn head_state(&self) -> GitResult<(String, String)> {
        // Both reads are independent, as in `write_identity`.
        let (symbolic, commit) = std::thread::scope(|scope| {
            let commit = scope.spawn(|| self.head_commit_output());
            (self.symbolic_head(), commit.join())
        });
        let symbolic = symbolic?;
        let id = self.head_id_from(commit.map_err(|_| "HEAD read failed")??, &symbolic)?;
        Ok((symbolic, id))
    }

    /// The commit a full ref points to, or `None` when the ref does not exist.
    pub(crate) fn ref_commit(&self, full_ref: &str) -> GitResult<Option<String>> {
        validate_revision(full_ref)?;
        let commit = format!("{full_ref}^{{commit}}");
        let output = self.output(&[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &commit,
        ])?;
        match output.status.code() {
            Some(0) => String::from_utf8(output.stdout)
                .map(|s| Some(s.trim().to_owned()))
                .map_err(|e| e.to_string()),
            Some(1) if output.stderr.is_empty() => Ok(None),
            _ => Err(Self::failed_output(&output)),
        }
    }

    /// Parent IDs of commit `id`, or `None` when no such commit exists.
    pub(crate) fn commit_parents(&self, id: &str) -> GitResult<Option<Vec<String>>> {
        let Some(id) = self.ref_commit(id)? else {
            return Ok(None);
        };
        let parents = self.run(&["show", "-s", "--no-notes", "--format=%P", &id])?;
        Ok(Some(
            parents.split_whitespace().map(str::to_owned).collect(),
        ))
    }

    /// Stash entries as object ID and reflog subject, newest first.
    pub(crate) fn stash_entries(&self) -> GitResult<Vec<(String, String)>> {
        Ok(self
            .run(&["stash", "list", "--format=%H%x00%gs"])?
            .lines()
            .filter_map(|line| {
                let (id, subject) = line.split_once('\0')?;
                Some((id.trim().to_owned(), subject.to_owned()))
            })
            .collect())
    }

    /// Returns tracked files in the index and working tree to HEAD with
    /// `reset --hard`. Untracked files stay. Callers verify beforehand that
    /// every discarded change is saved elsewhere.
    pub(crate) fn reset_tracked_to_head(&self) -> Result<(), WriteError> {
        self.write(&["reset", "--hard", "--quiet", "HEAD"])
            .map(|_| ())
    }

    /// Whether tracked files differ from HEAD in the index or working tree.
    /// Untracked files are not counted.
    pub(crate) fn has_tracked_changes(&self) -> GitResult<bool> {
        Ok(!self
            .bytes(&["status", "--porcelain=v1", "-z", "--untracked-files=no"])?
            .is_empty())
    }

    /// Whether the working tree and index hold exactly the tracked state that
    /// the stash-shaped commit `state` records: its tree for the working tree
    /// and its second parent's tree for the index.
    pub(crate) fn tracked_state_matches(&self, state: &str) -> GitResult<bool> {
        validate_revision(state)?;
        let index = format!("{state}^2");
        let worktree_args: &[&str] = &[
            "diff",
            "--quiet",
            "--no-ext-diff",
            "--no-textconv",
            state,
            "--",
        ];
        let index_args: &[&str] = &[
            "diff",
            "--quiet",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            &index,
            "--",
        ];
        // Both comparisons run at once; their results are read in the
        // original order, so the first difference or error still decides.
        let (worktree, index) = std::thread::scope(|scope| {
            let index = scope.spawn(|| self.output(index_args));
            (self.output(worktree_args), index.join())
        });
        for output in [worktree, index.map_err(|_| "Git diff read failed")?] {
            let output = output?;
            match output.status.code() {
                Some(0) => {}
                Some(1) => return Ok(false),
                _ => return Err(Self::failed_output(&output)),
            }
        }
        Ok(true)
    }

    /// Records the tracked index and working tree as a dangling stash-shaped
    /// commit without changing refs or files. `None` when nothing differs from HEAD.
    pub(crate) fn capture_tracked_state(&self) -> Result<Option<String>, WriteError> {
        let id = self.write(&["stash", "create"])?;
        Ok(Some(id.trim().to_owned()).filter(|id| !id.is_empty()))
    }

    /// Applies stash commit `id`, restoring its index state too when `index` is set.
    pub(crate) fn stash_apply(&self, id: &str, index: bool) -> Result<(), WriteError> {
        validate_revision(id)?;
        let mut args = vec!["stash", "apply"];
        if index {
            args.push("--index");
        }
        args.push(id);
        self.write(&args).map(|_| ())
    }

    /// Adds stash commit `id` back to the top of the stash list.
    pub(crate) fn stash_store(&self, message: &str, id: &str) -> Result<(), WriteError> {
        validate_revision(id)?;
        self.write(&["stash", "store", "-m", message, id])
            .map(|_| ())
    }

    /// Moves HEAD (and its branch) to `id` with `reset --keep`, which refuses
    /// to overwrite local changes or untracked files.
    pub(crate) fn reset_keep(&self, id: &str) -> Result<(), WriteError> {
        validate_revision(id)?;
        self.write(&["reset", "--keep", id]).map(|_| ())
    }

    /// Deletes local branch `name`. Git refuses a branch checked out in any worktree.
    pub(crate) fn delete_branch(&self, name: &str) -> Result<(), WriteError> {
        validate_revision(name)?;
        self.write(&["branch", "-D", "--", name]).map(|_| ())
    }
}
