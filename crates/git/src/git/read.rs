//! Reads: repository snapshots, history, commit details, diffs and status.
use super::*;

/// Largest operation-state file read when fingerprinting history state.
const MAX_HISTORY_METADATA_BYTES: u64 = 1024 * 1024;

impl Git {
    /// Reads repository status, refs and history with a fresh Git query.
    ///
    /// The history is capped at `limit.max(1)` commits and ordered with each
    /// child before its parents.
    ///
    /// # Errors
    ///
    /// Returns an error if repository data cannot be read, or if HEAD, the index
    /// or refs change during the snapshot's consistency checks.
    pub fn snapshot(&self, limit: usize) -> GitResult<RepoSnapshot> {
        self.snapshot_cached(limit, HistoryRead::Full, &mut None)
            .map(|(snapshot, _, _)| snapshot)
    }

    /// Lists refs as `object NUL peeled-object NUL full-name NUL
    /// upstream-remote NUL upstream-ref` lines.
    ///
    /// Short names are not requested from Git: `%(refname:short)` looks up
    /// every candidate name of every ref inside Git, which grows with the
    /// number of refs. [`Self::short_ref_names`] derives them from this list.
    fn read_refs(&self) -> GitResult<String> {
        self.run(&[
            "for-each-ref",
            "--format=%(objectname)%00%(*objectname)%00%(refname)%00%(upstream:remotename)%00%(upstream:remoteref)",
        ])
    }

    /// Shortens each full ref name the way Git's `%(refname:short)` does with
    /// `core.warnAmbiguousRefs` at its default: the shortest name that no
    /// other revision-parsing rule resolves to an existing ref.
    pub(super) fn short_ref_names(&self, full_names: &[&str]) -> Vec<String> {
        let listed: std::collections::HashSet<&str> = full_names.iter().copied().collect();
        let root_dirs: Vec<&Path> = [self.git_dir.as_deref(), self.common_dir.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        let exists = |name: &str| {
            if name.starts_with("refs/") {
                listed.contains(name)
            } else {
                // Outside refs/ only root refs such as HEAD or FETCH_HEAD resolve.
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b == b'_' || b == b'-')
                    && root_dirs.iter().any(|dir| dir.join(name).is_file())
            }
        };
        full_names
            .iter()
            .map(|full| shorten_ref(full, &exists))
            .collect()
    }

    pub(crate) fn snapshot_cached(
        &self,
        limit: usize,
        mode: HistoryRead,
        cache: &mut Option<HistoryCache>,
    ) -> GitResult<(RepoSnapshot, crate::graph::Graph, bool)> {
        let limit = limit.max(1);
        // Each Git call costs a process start, so independent reads run side
        // by side. The identity and references are read again at the end to
        // detect a change during the refresh.
        // Other worktrees' changes are counted on a full refresh and for
        // worktrees not seen before; other reads reuse the last counts.
        let known = match (mode, cache.as_ref()) {
            (HistoryRead::Full, _) | (_, None) => None,
            (_, Some(cached)) => Some(cached.worktrees.as_slice()),
        };
        let (reviewed, metadata, refs, stashes, worktrees) = std::thread::scope(|scope| {
            let metadata = scope.spawn(|| self.read_metadata());
            let refs = scope.spawn(|| self.read_refs());
            let stashes = scope.spawn(|| {
                self.run(&[
                    "stash",
                    "list",
                    "--format=%H%x00%P%x00%an%x00%ae%x00%aI%x00%ci%x00%gs",
                ])
            });
            let worktrees = scope.spawn(|| self.worktrees(known));
            let reviewed = self.write_identity();
            let metadata = metadata.join().map_err(|_| "Git metadata read failed")?;
            let refs = refs.join().map_err(|_| "Git reference read failed")?;
            let stashes = stashes.join().map_err(|_| "Git stash read failed")?;
            let worktrees = worktrees.join().map_err(|_| "Git worktree read failed")?;
            Ok::<_, String>((reviewed?, metadata?, refs?, stashes?, worktrees?))
        })?;
        let head = if let Some(branch) = reviewed.symbolic_head.strip_prefix("refs/heads/") {
            branch.to_string()
        } else {
            self.run(&["rev-parse", "--short", "HEAD"])?
                .trim()
                .to_string()
        };
        let mut labels: HashMap<String, Vec<String>> = HashMap::new();
        let mut branches = Vec::new();
        let mut remote_refs = Vec::new();
        let mut tags = Vec::new();
        let listed: Vec<[&str; 5]> = refs
            .lines()
            .filter_map(|line| {
                let mut fields = line.split('\0');
                let mut next = || fields.next();
                Some([next()?, next()?, next()?, next()?, next()?])
            })
            .collect();
        let short = self.short_ref_names(&listed.iter().map(|f| f[2]).collect::<Vec<_>>());
        for (&[object, peeled, full, remote, remote_ref], short) in listed.iter().zip(short) {
            if full == "refs/stash" {
                continue;
            }
            let target = if peeled.is_empty() { object } else { peeled };
            if let Some(name) = full.strip_prefix("refs/heads/") {
                branches.push(LocalBranch {
                    name: name.into(),
                    tip: object.into(),
                    upstream: remote_ref
                        .strip_prefix("refs/heads/")
                        .filter(|_| !remote.is_empty())
                        .map(|branch| (remote.into(), branch.into())),
                });
            } else if let Some(list) = if full.starts_with("refs/remotes/") {
                Some(&mut remote_refs)
            } else if full.starts_with("refs/tags/") {
                Some(&mut tags)
            } else {
                None
            } {
                list.push(ReferenceTip {
                    full_name: full.into(),
                    label: short.clone(),
                    tip: target.into(),
                });
            }
            labels.entry(target.into()).or_default().push(short);
        }
        let head_exists: GitResult<String> = if reviewed.head.is_empty() {
            Err("Unborn HEAD".into())
        } else {
            Ok(reviewed.head.clone())
        };
        if let Ok(oid) = &head_exists {
            labels
                .entry(oid.trim().into())
                .or_default()
                .insert(0, "HEAD".into());
        }
        let previous = cache.as_ref().filter(|cached| {
            cached.refs == refs
                && cached.head == reviewed.head
                && cached.metadata == metadata.history_stamp
                && cached.stashes == stashes
                && super::worktree::wip_rows(&cached.worktrees)
                    == super::worktree::wip_rows(&worktrees)
        });
        let reuse = previous.is_some_and(|cached| match mode {
            HistoryRead::Full => false,
            HistoryRead::Worktree => cached.limit == limit,
            HistoryRead::More => cached.limit <= limit && cached.commits.len() < cached.limit,
        });
        let append = previous
            .is_some_and(|cached| matches!(mode, HistoryRead::More) && cached.limit < limit);
        let skip = if append {
            previous.unwrap().commits.len()
        } else {
            0
        };
        let count = format!("--max-count={}", limit.saturating_sub(skip).max(1));
        let skip_arg = format!("--skip={skip}");
        // Date order retains child-before-parent safety while interleaving
        // active branches by commit date instead of grouping each lane.
        let mut log_args = vec![
            "log",
            "--exclude=refs/stash",
            "--all",
            "--date-order",
            "--no-decorate",
            "--no-show-signature",
            "--no-notes",
            "--no-use-mailmap",
            &count,
            &skip_arg,
            "-z",
            "--format=%H%x00%P%x00%s%x00%an%x00%ae%x00%aI%x00%b",
        ];
        if head_exists.is_ok() {
            log_args.push("HEAD");
        }
        // A stash's base can be reachable from no ref, as when its branch was
        // deleted, so the base commits are walked too.
        let stash_list = parse_stashes(&stashes);
        log_args.extend(stash_list.iter().map(|(row, _)| row.parents[0].as_str()));
        let (log, status, remote) = std::thread::scope(|scope| {
            let status = scope.spawn(|| {
                self.bytes(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])
                    .map(|bytes| parse_status(&bytes))
            });
            let remote = scope.spawn(|| self.remote_state(&reviewed));
            let log = if reuse {
                Ok(String::new())
            } else {
                self.run(&log_args)
            };
            let status = status.join().map_err(|_| "Git status read failed")?;
            let remote = remote.join().map_err(|_| "Git remote read failed")?;
            Ok::<_, String>((log?, status?, remote?))
        })?;
        let fields: Vec<_> = log.split('\0').collect();
        let mut commits: Vec<_> = fields
            .as_chunks::<7>()
            .0
            .iter()
            .filter(|f| !f[0].is_empty())
            .map(|f| Commit {
                id: f[0].into(),
                parents: f[1].split_whitespace().map(String::from).collect(),
                subject: f[2].into(),
                body_preview: body_preview(f[6]),
                author: f[3].into(),
                author_email: f[4].into(),
                date: f[5].into(),
                refs: labels.remove(f[0]).unwrap_or_default(),
                search_key: String::new(),
                stash: false,
                worktree: None,
            })
            .map(Commit::index_search)
            .collect();
        if reuse {
            commits = previous.unwrap().commits.clone();
        } else if append {
            let mut loaded = previous.unwrap().commits.clone();
            loaded.extend(commits);
            commits = loaded;
        }
        let head_id = head_exists
            .as_ref()
            .map(|oid| oid.trim().to_string())
            .unwrap_or_default();
        let (identity_now, refs_now) = std::thread::scope(|scope| {
            let refs_now = scope.spawn(|| self.read_refs());
            let identity_now = self.write_identity();
            let refs_now = refs_now.join().map_err(|_| "Git reference read failed")?;
            Ok::<_, String>((identity_now?, refs_now?))
        })?;
        if identity_now != reviewed {
            return Err("Repository changed during refresh. Refresh again before writing.".into());
        }
        if refs_now != refs {
            return Err("References changed during refresh. Refresh again.".into());
        }
        let conflict_markers = self.conflict_marker_paths(&status);
        let stash_count = stash_list.len();
        let latest_stash = stash_list.first().map(|(_, entry)| entry.clone());
        let upstream_context = remote.context(&reviewed);
        let next = (!reuse).then(|| {
            let rows = with_worktree_rows(
                with_stashes(&commits, stash_list.into_iter().map(|(row, _)| row)),
                &worktrees,
            );
            HistoryCache {
                refs,
                head: reviewed.head.clone(),
                limit,
                graph: crate::graph::layout_with_head(
                    &rows,
                    (!head_id.is_empty()).then_some(head_id.as_str()),
                ),
                commits,
                stashes,
                rows,
                metadata: metadata.history_stamp,
                worktrees: worktrees.clone(),
            }
        });
        if let Some(scope) = &self.read_scope {
            scope.check().map_err(|error| error.to_string())?;
        }
        if let Some(next) = next {
            *cache = Some(next);
        }
        let history = cache.as_mut().expect("successful snapshot has history");
        history.limit = limit;
        history.worktrees = worktrees.clone();
        Ok((
            RepoSnapshot {
                worktree_id: self.path.clone(),
                commits: history.rows.clone(),
                branches,
                stash_count,
                latest_stash,
                worktrees,
                remote_refs,
                tags,
                head,
                head_id,
                status,
                conflict_markers,
                operations: metadata.operations,
                operation_message: metadata.message,
                upstream_context,
                remote,
                reviewed: Some(reviewed),
                journal: Default::default(),
            },
            history.graph.clone(),
            !reuse,
        ))
    }

    /// Lists the distinct active operation names indicated by Git metadata.
    ///
    /// Git resolves marker paths for the current worktree. To check for
    /// unmerged files, inspect the repository status separately.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot resolve the paths or history metadata cannot be read.
    pub fn operation_markers(&self) -> GitResult<Vec<String>> {
        Ok(self.read_metadata()?.operations)
    }

    fn read_metadata(&self) -> GitResult<RepositoryMetadata> {
        use std::hash::{Hash, Hasher};
        let mut args = vec!["rev-parse", "--path-format=absolute"];
        for (marker, _) in OPERATION_MARKERS {
            args.extend(["--git-path", marker]);
        }
        let history_files = ["shallow", "info/grafts", "config", "config.worktree"];
        for path in history_files {
            args.extend(["--git-path", path]);
        }
        args.extend(["--git-path", "MERGE_MSG"]);
        let paths = self.run(&args)?;
        let paths: Vec<_> = paths.lines().collect();
        if paths.len() != OPERATION_MARKERS.len() + history_files.len() + 1 {
            return Err("Git metadata paths could not be represented.".into());
        }
        let (markers, rest) = paths.split_at(OPERATION_MARKERS.len());
        let (history, merge_message) = rest.split_at(history_files.len());
        let mut active = vec![];
        let mut sequencer = None;
        for (path, (_, name)) in markers.iter().zip(OPERATION_MARKERS) {
            if Path::new(path).exists() {
                active.push(name.to_string());
                if *name == "sequencer" {
                    sequencer = Some(Path::new(path));
                }
            }
        }
        // Between the picks of a multi-commit cherry-pick or revert, only the
        // sequencer directory says which of the two is in progress.
        if let Some(sequencer) = sequencer {
            if !active
                .iter()
                .any(|op| op == "cherry-pick" || op == "revert")
            {
                active.push(sequencer_kind(sequencer).name().to_owned());
            }
        }
        active.sort();
        active.dedup();
        let message = match Sequence::from_operations(&active) {
            Some(sequence) if sequence.takes_message() => {
                prepared_message(Path::new(merge_message[0]))?
            }
            _ => None,
        };
        let mut stamp = std::collections::hash_map::DefaultHasher::new();
        for path in history {
            path.hash(&mut stamp);
            match std::fs::metadata(path) {
                Ok(metadata) => {
                    if metadata.len() > MAX_HISTORY_METADATA_BYTES {
                        return Err("Git history metadata exceeds 1 MiB.".into());
                    }
                    std::fs::read(path)
                        .map_err(|e| e.to_string())?
                        .hash(&mut stamp);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(RepositoryMetadata {
            operations: active,
            message,
            history_stamp: stamp.finish(),
        })
    }
    /// Lists paths changed by `commit` relative to its first parent.
    /// A root commit is compared with the empty tree.
    ///
    /// # Errors
    ///
    /// Returns an error if the revision is invalid, Git fails, or the file list is malformed.
    pub fn changed_files(&self, commit: &str) -> GitResult<Vec<FileChange>> {
        validate_revision(commit)?;
        let parent = format!("{commit}^1");
        let data = if self.run(&["rev-parse", "--verify", &parent]).is_ok() {
            self.bytes(&[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--find-renames",
                "--name-status",
                "-z",
                &parent,
                commit,
                "--",
            ])?
        } else {
            self.bytes(&[
                "diff-tree",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--root",
                "--no-commit-id",
                "--find-renames",
                "--name-status",
                "-r",
                "-z",
                commit,
                "--",
            ])?
        };
        let mut fields = data.split(|b| *b == 0).filter(|s| !s.is_empty());
        let mut files = Vec::new();
        while let Some(status) = fields.next() {
            let kind = status[0] as char;
            let first = fields.next().ok_or("Missing changed-file path")?;
            let (original_path, path) = if matches!(kind, 'R' | 'C') {
                (
                    Some(first),
                    fields.next().ok_or("Missing rename destination")?,
                )
            } else {
                (None, first)
            };
            files.push(file_change(path, original_path, kind, ' '));
        }
        Ok(files)
    }

    /// Reads the author email and message body for the selected commit.
    pub(crate) fn commit_message_details(&self, commit: &str) -> GitResult<(String, String)> {
        validate_revision(commit)?;
        let metadata = self.run(&[
            "show",
            "-s",
            "--no-notes",
            "--no-show-signature",
            "--no-use-mailmap",
            "--format=%ae%x00%b",
            commit,
            "--",
        ])?;
        let (email, body) = metadata
            .split_once('\0')
            .ok_or("Git returned incomplete commit metadata")?;
        Ok((email.to_owned(), body.trim_end_matches('\n').to_owned()))
    }

    /// Reads the commit `revision` names together with its summary, so a
    /// summary read beside a separate lookup can be matched to its commit.
    pub(crate) fn commit_summary_with_id(
        &self,
        revision: &str,
    ) -> GitResult<(String, CommitSummary)> {
        validate_revision(revision)?;
        let commit = format!("{revision}^{{commit}}");
        let output = self.run(&[
            "show",
            "-s",
            "--no-notes",
            "--no-show-signature",
            "--format=%H%x00%P%x00%s%x00%ci",
            &commit,
            "--",
        ])?;
        let mut fields = output.trim_end_matches('\n').splitn(4, '\0');
        let id = fields.next().unwrap_or_default().to_owned();
        let parents = fields.next().unwrap_or_default().split_whitespace().count();
        Ok((
            id,
            CommitSummary {
                subject: fields.next().unwrap_or_default().to_owned(),
                date: fields.next().unwrap_or_default().to_owned(),
                parents,
            },
        ))
    }

    /// Reads the subject, committer date and parent count of `commit`.
    pub(crate) fn commit_summary(&self, commit: &str) -> GitResult<CommitSummary> {
        validate_revision(commit)?;
        let output = self.run(&[
            "show",
            "-s",
            "--no-notes",
            "--no-show-signature",
            "--format=%P%x00%s%x00%ci",
            commit,
            "--",
        ])?;
        let mut fields = output.trim_end_matches('\n').splitn(3, '\0');
        let parents = fields.next().unwrap_or_default().split_whitespace().count();
        Ok(CommitSummary {
            subject: fields.next().unwrap_or_default().to_owned(),
            date: fields.next().unwrap_or_default().to_owned(),
            parents,
        })
    }

    /// Finds the old path for a rename or copy in the requested comparison.
    ///
    /// `path` is the repository-relative destination. In working changes,
    /// `staged` selects the comparison that can contribute the old path.
    /// For example, a staged rename followed by an unstaged edit uses the
    /// new path on both sides of the unstaged comparison.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot read status or the commit's changed files.
    pub fn diff_identity(
        &self,
        commit: Option<&str>,
        path: &str,
        staged: bool,
    ) -> GitResult<Option<String>> {
        let files = if let Some(id) = commit {
            self.changed_files(id)?
        } else {
            // Untracked files never carry a rename source, so skipping the
            // untracked scan gives the same answer for less work.
            parse_status(&self.bytes(&[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=no",
            ])?)
        };
        Ok(files
            .into_iter()
            .find(|f| !f.unsupported_path && f.path == path)
            .and_then(|f| {
                if commit.is_some()
                    || (staged && matches!(f.index, 'R' | 'C'))
                    || (!staged && matches!(f.worktree, 'R' | 'C'))
                {
                    f.original_path
                } else {
                    None
                }
            }))
    }

    /// Returns the raw Git patch for a file comparison.
    ///
    /// See [`Self::compare`] for comparison rules and error conditions.
    pub fn diff(&self, commit: Option<&str>, path: &str, staged: bool) -> GitResult<String> {
        Ok(self.compare(commit, path, staged)?.patch)
    }

    /// Returns a Git patch and the old path if this comparison detects a rename or copy.
    ///
    /// Supply the repository-relative destination as `path`. A selected commit
    /// is compared with its first parent, or the empty tree for a root commit.
    /// With no commit, `staged` selects HEAD versus index when `true` and index
    /// versus working tree otherwise. Untracked files are compared against an
    /// empty file if they resolve inside the worktree.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot produce a UTF-8 patch or an untracked path
    /// resolves outside the worktree.
    pub fn compare(
        &self,
        commit: Option<&str>,
        path: &str,
        staged: bool,
    ) -> GitResult<DiffComparison> {
        // Most comparisons have no rename, so the patch without a source path
        // is read beside the rename lookup and replaced only when one is found.
        let (original, plain) = std::thread::scope(|scope| {
            let plain = scope.spawn(|| self.patch_for_paths(commit, path, staged, None));
            let original = self.diff_identity(commit, path, staged);
            (original, plain.join())
        });
        let original = original?;
        let patch = match original.as_deref() {
            None => plain.map_err(|_| "Git diff read failed")??,
            Some(old) => self.patch_for_paths(commit, path, staged, Some(old))?,
        };
        Ok(DiffComparison {
            patch,
            original_path: original,
        })
    }

    fn patch_for_paths(
        &self,
        commit: Option<&str>,
        path: &str,
        staged: bool,
        original: Option<&str>,
    ) -> GitResult<String> {
        let mut args = patch_args("diff");
        args.push("--find-renames");
        let parent;
        if let Some(id) = commit {
            validate_revision(id)?;
            parent = format!("{id}^1");
            if self.run(&["rev-parse", "--verify", &parent]).is_ok() {
                args.extend([&parent, id]);
            } else {
                let mut args = patch_args("show");
                args.extend(["--format=", "--no-notes", id, "--", path]);
                return self.run(&args);
            }
        } else if staged {
            args.push("--cached");
        }
        args.push("--");
        if let Some(old) = original {
            args.push(old);
        }
        args.push(path);
        let diff = self.run(&args)?;
        if diff.is_empty()
            && commit.is_none()
            && !staged
            && self
                .run(&["ls-files", "--error-unmatch", "--", path])
                .is_err()
        {
            let candidate = self.path.join(path);
            if !candidate.is_file() {
                return Ok(diff);
            }
            let canonical = candidate.canonicalize().map_err(|e| e.to_string())?;
            let root = self.path.canonicalize().map_err(|e| e.to_string())?;
            if !canonical.starts_with(root) {
                return Err("Untracked preview path leaves the repository".into());
            }
            let mut args = patch_args("diff");
            args.extend(["--no-index", "--", "/dev/null", path]);
            let scope = self.read_scope.clone().unwrap_or_default();
            let output = capture_read(self.read_command(&args)?, 1, Some(&scope))
                .map_err(|e| e.to_string())?;
            // For --no-index, exit 1 means a successful comparison with differences.
            if output.status.success() || output.status.code() == Some(1) {
                return String::from_utf8(output.stdout)
                    .map_err(|_| "Untracked diff is not UTF-8".into());
            }
            return Err(String::from_utf8_lossy(&output.stderr).trim().into());
        }
        Ok(diff)
    }

    /// Reads working changes together with the HEAD and index identity they
    /// belong to, for refreshing the file lists after an index write.
    ///
    /// Like a snapshot, the identity is read again afterwards: if HEAD or the
    /// index moved while status ran, the pair would not match and this fails.
    pub(crate) fn worktree_state(&self) -> GitResult<(Vec<FileChange>, WriteIdentity)> {
        let (identity, status) = std::thread::scope(|scope| {
            let status = scope.spawn(|| self.status());
            (self.write_identity(), status.join())
        });
        let identity = identity?;
        let status = status.map_err(|_| "Git status read failed")??;
        if self.write_identity()? != identity {
            return Err("Repository changed during refresh.".into());
        }
        Ok((status, identity))
    }

    /// The unmerged paths in `status` whose working file still has a
    /// conflict marker line, as [`Self::mark_resolved`] would refuse them.
    /// A file that cannot be read is left out.
    pub(crate) fn conflict_marker_paths(
        &self,
        status: &[FileChange],
    ) -> std::collections::BTreeSet<String> {
        status
            .iter()
            .filter(|file| file.is_unmerged() && !file.unsupported_path)
            .filter(|file| {
                let path = self.path.join(&file.path);
                std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file())
                    && std::fs::read(&path)
                        .is_ok_and(|content| conflict_marker_line(&content).is_some())
            })
            .map(|file| file.path.clone())
            .collect()
    }

    pub(super) fn status(&self) -> GitResult<Vec<FileChange>> {
        Ok(parse_status(&self.bytes(&[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ])?))
    }
}

/// Whether the multi-commit sequence saved in the `sequencer` directory is a
/// cherry-pick or a revert, from the first instruction of its todo list.
fn sequencer_kind(sequencer: &Path) -> Sequence {
    let todo = std::fs::read(sequencer.join("todo")).unwrap_or_default();
    let todo = String::from_utf8_lossy(&todo);
    let first = todo
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .and_then(|line| line.split_whitespace().next());
    if first == Some("revert") {
        Sequence::Revert
    } else {
        Sequence::CherryPick
    }
}

/// Git's prepared commit message at `path` without its comment lines, or
/// `None` when the file is missing or holds nothing else.
fn prepared_message(path: &Path) -> GitResult<Option<String>> {
    let bytes = match std::fs::metadata(path) {
        Ok(metadata) if metadata.len() > MAX_HISTORY_METADATA_BYTES => return Ok(None),
        Ok(_) => std::fs::read(path).map_err(|e| e.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let message = String::from_utf8_lossy(&bytes)
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    let message = message.trim_matches('\n');
    Ok((!message.is_empty()).then(|| message.to_owned()))
}
