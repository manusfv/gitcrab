//! Git subprocess boundary. Every argument is passed directly, never through a shell.
use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub type GitResult<T> = Result<T, String>;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct Commit {
    pub id: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub author: String,
    pub date: String,
    pub refs: Vec<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct FileChange {
    /// Destination path for renames; relative to the repository root.
    pub path: String,
    /// Source retained from porcelain's second NUL record for renames/copies.
    pub original_path: Option<String>,
    pub index: char,
    pub worktree: char,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct WriteIdentity {
    pub head: String,
    pub symbolic_head: String,
    pub index: Vec<u8>,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct RepoSnapshot {
    pub commits: Vec<Commit>,
    pub branches: Vec<String>,
    #[serde(default)]
    pub remote_refs: Vec<String>,
    #[serde(default)]
    pub stash_count: usize,
    #[serde(default)]
    pub tags: Vec<String>,
    pub head: String,
    /// Full HEAD identity, including when outside the loaded history page.
    pub head_id: String,
    pub status: Vec<FileChange>,
    #[serde(default)]
    pub operations: Vec<String>,
    #[serde(default)]
    pub reviewed: Option<WriteIdentity>,
    #[serde(default)]
    pub upstream_context: String,
}

impl RepoSnapshot {
    /// Porcelain XY entries share a destination path, including renames.
    pub fn unique_changes(&self) -> std::collections::BTreeSet<&str> {
        self.status.iter().map(|f| f.path.as_str()).collect()
    }
    pub fn change_counts(&self) -> [usize; 4] {
        let mut paths: std::collections::BTreeMap<&str, Vec<char>> = Default::default();
        for f in &self.status {
            paths
                .entry(&f.path)
                .or_default()
                .extend([f.index, f.worktree]);
        }
        let mut counts = [0; 4];
        for status in paths.values() {
            let category = if status.contains(&'R') || status.contains(&'C') {
                3
            } else if status.contains(&'U') {
                0
            } else if status.contains(&'?') || status.contains(&'A') {
                1
            } else if status.contains(&'D') {
                2
            } else {
                0
            };
            counts[category] += 1;
        }
        counts
    }
    pub fn commit_blocked(&self) -> bool {
        !self.operations.is_empty()
            || self.status.iter().any(|f| {
                f.index == 'U'
                    || f.worktree == 'U'
                    || matches!((f.index, f.worktree), ('A', 'A') | ('D', 'D'))
            })
    }

    pub fn head_label(&self) -> String {
        if self.head_id.is_empty() {
            format!("{} · unborn branch", self.head)
        } else if !self.branches.contains(&self.head) {
            format!("Detached HEAD · {}", self.head)
        } else {
            self.head.clone()
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Hunk {
    pub title: String,
    /// Self-contained patch with original file headers and one hunk.
    pub patch: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Git {
    pub path: PathBuf,
}

impl Git {
    /// Accepts a repository root or any directory within a worktree.
    pub fn open(path: impl AsRef<Path>) -> GitResult<Self> {
        let initial = Self {
            path: path.as_ref().to_path_buf(),
        };
        let root = initial.run(&["rev-parse", "--show-toplevel"])?;
        Ok(Self {
            path: PathBuf::from(root.trim_end_matches(['\r', '\n'])),
        })
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(crate::platform::git_executable());
        cmd.current_dir(&self.path)
            .args(["--no-pager", "--literal-pathspecs"])
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        cmd
    }

    fn bytes(&self, args: &[&str]) -> GitResult<Vec<u8>> {
        let output = self.command(args).output().map_err(|e| {
            format!(
                "Could not start Git: {e}. Install Git or set GITCRAB_GIT to its executable path."
            )
        })?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    }

    pub fn run(&self, args: &[&str]) -> GitResult<String> {
        String::from_utf8(self.bytes(args)?)
            .map_err(|_| "Git returned text that is not UTF-8".into())
    }

    pub fn snapshot(&self, limit: usize) -> GitResult<RepoSnapshot> {
        let reviewed = self.write_identity()?;
        let head = if let Some(branch) = reviewed.symbolic_head.strip_prefix("refs/heads/") {
            branch.to_string()
        } else {
            self.run(&["rev-parse", "--short", "HEAD"])?
                .trim()
                .to_string()
        };
        let refs = self.run(&[
            "for-each-ref",
            "--format=%(objectname)%00%(refname:short)%00%(*objectname)%00%(refname)",
        ])?;
        let mut labels: HashMap<String, Vec<String>> = HashMap::new();
        let mut branches = Vec::new();
        let mut remote_refs = Vec::new();
        let mut tags = Vec::new();
        for line in refs.lines() {
            let fields: Vec<_> = line.split('\0').collect();
            if fields.len() == 4 {
                let list = if fields[3].starts_with("refs/heads/") {
                    Some(&mut branches)
                } else if fields[3].starts_with("refs/remotes/") {
                    Some(&mut remote_refs)
                } else if fields[3].starts_with("refs/tags/") {
                    Some(&mut tags)
                } else {
                    None
                };
                if let Some(list) = list {
                    list.push(fields[1].to_string());
                }
                let target = if fields[2].is_empty() {
                    fields[0]
                } else {
                    fields[2]
                };
                labels
                    .entry(target.into())
                    .or_default()
                    .push(fields[1].into());
            }
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
        let count = format!("--max-count={}", limit.max(1));
        // Date order retains child-before-parent safety while interleaving
        // active branches by commit date instead of grouping each lane.
        let mut log_args = vec![
            "log",
            "--all",
            "--date-order",
            &count,
            "-z",
            "--format=%H%x00%P%x00%s%x00%an%x00%aI",
        ];
        if head_exists.is_ok() {
            log_args.push("HEAD");
        }
        let log = self.run(&log_args);
        let log = match log {
            Ok(s) => s,
            Err(_)
                if self.run(&["rev-parse", "--verify", "HEAD"]).is_err() && labels.is_empty() =>
            {
                String::new()
            }
            Err(e) => return Err(e),
        };
        let fields: Vec<_> = log.split('\0').collect();
        let commits = fields
            .chunks_exact(5)
            .filter(|f| !f[0].is_empty())
            .map(|f| Commit {
                id: f[0].into(),
                parents: f[1].split_whitespace().map(String::from).collect(),
                subject: f[2].into(),
                author: f[3].into(),
                date: f[4].into(),
                refs: labels.remove(f[0]).unwrap_or_default(),
            })
            .collect();
        let status = parse_status(&self.bytes(&[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ])?);
        let head_id = head_exists
            .as_ref()
            .map(|oid| oid.trim().to_string())
            .unwrap_or_default();
        if self.write_identity()? != reviewed {
            return Err("Repository changed during refresh. Refresh again before writing.".into());
        }
        Ok(RepoSnapshot {
            commits,
            branches,
            stash_count: self
                .run(&["stash", "list", "--format=%gd"])?
                .lines()
                .count(),
            remote_refs,
            tags,
            head,
            head_id,
            status,
            operations: self.operation_markers()?,
            upstream_context: self.upstream_context(&reviewed)?,
            reviewed: Some(reviewed),
        })
    }

    /// Git resolves linked-worktree marker paths; unresolved counts are independent.
    pub fn operation_markers(&self) -> GitResult<Vec<String>> {
        let markers = [
            ("MERGE_HEAD", "merge"),
            ("CHERRY_PICK_HEAD", "cherry-pick"),
            ("REVERT_HEAD", "revert"),
            ("rebase-merge", "rebase"),
            ("rebase-apply", "rebase"),
            ("sequencer", "sequencer"),
        ];
        let mut args = vec!["rev-parse", "--path-format=absolute"];
        for (marker, _) in markers {
            args.extend(["--git-path", marker]);
        }
        let paths = self.run(&args)?;
        let mut active = paths
            .lines()
            .zip(markers)
            .filter(|(p, _)| PathBuf::from(p).exists())
            .map(|(_, (_, name))| name.to_string())
            .collect::<Vec<_>>();
        active.sort();
        active.dedup();
        Ok(active)
    }
    pub fn upstream_context(&self, identity: &WriteIdentity) -> GitResult<String> {
        if identity.symbolic_head.is_empty() {
            return Ok("Detached HEAD · no upstream".into());
        }
        let upstream = self.run(&[
            "for-each-ref",
            "--format=%(upstream:short)",
            &identity.symbolic_head,
        ])?;
        let upstream = upstream.trim();
        if upstream.is_empty() {
            return Ok("No upstream configured".into());
        }
        match self.run(&[
            "rev-list",
            "--left-right",
            "--count",
            "HEAD...@{upstream}",
            "--",
        ]) {
            Ok(counts) => {
                let counts = counts.split_whitespace().collect::<Vec<_>>();
                if counts.len() != 2 {
                    return Err("Invalid upstream comparison".into());
                }
                Ok(format!(
                    "{} ahead · {} behind · {upstream} · network freshness unknown",
                    counts[0], counts[1]
                ))
            }
            Err(_) => Ok(format!(
                "Upstream {upstream} unavailable · network freshness unknown"
            )),
        }
    }
    pub fn changed_files(&self, commit: &str) -> GitResult<Vec<FileChange>> {
        validate_revision(commit)?;
        let parent = format!("{commit}^1");
        let data = if self.run(&["rev-parse", "--verify", &parent]).is_ok() {
            self.run(&[
                "diff",
                "--find-renames",
                "--name-status",
                "-z",
                &parent,
                commit,
                "--",
            ])?
        } else {
            self.run(&[
                "diff-tree",
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
        let mut fields = data.split('\0').filter(|s| !s.is_empty());
        let mut files = Vec::new();
        while let Some(status) = fields.next() {
            let kind = status.chars().next().unwrap_or(' ');
            let first = fields.next().ok_or("Missing changed-file path")?;
            let (original_path, path) = if matches!(kind, 'R' | 'C') {
                (
                    Some(first.to_string()),
                    fields.next().ok_or("Missing rename destination")?,
                )
            } else {
                (None, first)
            };
            files.push(FileChange {
                path: path.into(),
                original_path,
                index: kind,
                worktree: ' ',
            });
        }
        Ok(files)
    }

    /// Resolve identity in this comparison's namespace. An index rename's old
    /// path belongs to HEAD, not to the index/worktree comparison.
    pub fn diff_identity(
        &self,
        commit: Option<&str>,
        path: &str,
        staged: bool,
    ) -> GitResult<Option<String>> {
        let files = if let Some(id) = commit {
            self.changed_files(id)?
        } else {
            parse_status(&self.bytes(&[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
            ])?)
        };
        Ok(files.into_iter().find(|f| f.path == path).and_then(|f| {
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

    /// A selected commit is diffed against its first parent. None selects index/worktree.
    pub fn diff(&self, commit: Option<&str>, path: &str, staged: bool) -> GitResult<String> {
        let original = self.diff_identity(commit, path, staged)?;
        let mut args = vec![
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--find-renames",
        ];
        let parent;
        if let Some(id) = commit {
            validate_revision(id)?;
            parent = format!("{id}^1");
            if self.run(&["rev-parse", "--verify", &parent]).is_ok() {
                args.extend([&parent, id]);
            } else {
                return self.run(&[
                    "show",
                    "--format=",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-color",
                    id,
                    "--",
                    path,
                ]);
            }
        } else if staged {
            args.push("--cached");
        }
        args.push("--");
        if let Some(old) = &original {
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
            let output = self
                .command(&[
                    "diff",
                    "--no-index",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-color",
                    "--",
                    "/dev/null",
                    path,
                ])
                .output()
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

    fn index_paths(&self, path: &str, unstage: bool) -> GitResult<Vec<String>> {
        let status = parse_status(&self.bytes(&[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ])?);
        let renamed = status.iter().find(|change| {
            (change.index == 'R' || change.worktree == 'R')
                && (change.path == path || change.original_path.as_deref() == Some(path))
        });
        let Some(change) = renamed else {
            return Ok(vec![path.into()]);
        };
        let mut paths = vec![change.path.clone()];
        // Restore both sides of an index rename atomically. Staging a modified
        // rename destination needs only its existing index entry; including its
        // absent old index path would cause Git's pathspec validation to fail.
        if (unstage && change.index == 'R') || (!unstage && change.worktree == 'R') {
            if let Some(source) = &change.original_path {
                paths.push(source.clone());
            }
        }
        Ok(paths)
    }

    pub fn stage(&self, path: &str) -> GitResult<()> {
        let paths = self.index_paths(path, false)?;
        let mut args = vec!["add", "--"];
        args.extend(paths.iter().map(String::as_str));
        self.run(&args).map(|_| ())
    }
    pub fn unstage(&self, path: &str) -> GitResult<()> {
        let paths = self.index_paths(path, true)?;
        let mut args = if self.run(&["rev-parse", "--verify", "HEAD"]).is_ok() {
            vec!["restore", "--staged", "--"]
        } else {
            // An unborn index has no HEAD to restore. --cached guarantees that
            // even a partially staged file keeps its working-tree contents.
            vec!["rm", "--cached", "-r", "--force", "--"]
        };
        args.extend(paths.iter().map(String::as_str));
        self.run(&args).map(|_| ())
    }

    pub fn hunks(&self, path: &str, staged: bool) -> GitResult<Vec<Hunk>> {
        Ok(split_hunks(&self.diff(None, path, staged)?))
    }

    pub fn apply_hunk(&self, hunk: &Hunk, unstage: bool) -> GitResult<()> {
        let mut args = vec!["apply", "--cached", "--whitespace=nowarn"];
        if unstage {
            args.push("--reverse");
        }
        let mut child = self
            .command(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        child
            .stdin
            .take()
            .ok_or("Could not open Git input")?
            .write_all(hunk.patch.as_bytes())
            .map_err(|e| e.to_string())?;
        let result = child.wait_with_output().map_err(|e| e.to_string())?;
        if result.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&result.stderr).trim().into())
        }
    }

    /// Validate both Git's patch and the exact worktree preimage; verify actual bytes.
    /// External processes can still race between preflight and Git's write.
    pub fn discard_hunk(&self, path: &str, hunk: &Hunk) -> GitResult<()> {
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
                "Discard with automatic CRLF conversion is not qualified. Use Git for this file."
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
        for fields in attrs.split('\0').collect::<Vec<_>>().chunks_exact(3) {
            if fields[2] != "unspecified"
                && fields[2] != "unset"
                && !(fields[1] == "eol" && fields[2] == "lf")
            {
                return Err("Discard with content filters or non-LF encoding is not qualified. Use Git for this file.".into());
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
        let index = self.run(&["ls-files", "--stage", "-z"])?;
        let head = self
            .run(&["rev-parse", "--verify", "HEAD"])
            .unwrap_or_default();
        if std::fs::read(&file).map_err(|e| e.to_string())? != before {
            return Err("Working file changed before discard".into());
        }
        let mut child = self
            .command(&["apply", "--reverse", "--whitespace=nowarn"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        child
            .stdin
            .take()
            .ok_or("Could not open Git input")?
            .write_all(hunk.patch.as_bytes())
            .map_err(|e| e.to_string())?;
        let result = child.wait_with_output().map_err(|e| e.to_string())?;
        if !result.status.success() {
            return Err(String::from_utf8_lossy(&result.stderr).trim().into());
        }
        if std::fs::read(&file).map_err(|e| e.to_string())? != expected
            || self.run(&["ls-files", "--stage", "-z"])? != index
            || self
                .run(&["rev-parse", "--verify", "HEAD"])
                .unwrap_or_default()
                != head
        {
            return Err("Discard outcome uncertain: Git state differs from the reviewed effect. Refresh and inspect before retrying.".into());
        }
        Ok(())
    }
    pub fn write_identity(&self) -> GitResult<WriteIdentity> {
        // Three bounded read tasks join inside the serialized service worker.
        // No mutation can enter this session while the identity is collected.
        std::thread::scope(|scope| {
            let head = scope.spawn(|| {
                self.run(&["rev-parse", "--verify", "HEAD"])
                    .unwrap_or_default()
            });
            let symbolic = scope.spawn(|| {
                self.run(&["symbolic-ref", "--quiet", "HEAD"])
                    .unwrap_or_default()
            });
            let index = self.bytes(&["ls-files", "--stage", "-z"])?;
            Ok(WriteIdentity {
                head: head.join().map_err(|_| "HEAD read failed")?.trim().into(),
                symbolic_head: symbolic
                    .join()
                    .map_err(|_| "Symbolic HEAD read failed")?
                    .trim()
                    .into(),
                index,
            })
        })
    }
    pub fn commit_reviewed(&self, message: &str, reviewed: &WriteIdentity) -> GitResult<String> {
        if message.lines().next().is_none_or(|s| s.trim().is_empty()) {
            return Err("Enter a commit summary".into());
        }
        if !self.operation_markers()?.is_empty() {
            return Err("An active Git operation blocks ordinary commit. Complete or abort it in a terminal, then refresh.".into());
        }
        if self.write_identity()? != *reviewed {
            return Err("HEAD or index changed since review. Refresh and review staged changes; your message is saved.".into());
        }
        if self
            .run(&["diff", "--cached", "--name-only", "-z"])?
            .is_empty()
        {
            return Err("No staged changes to commit".into());
        }
        let tree = self.run(&["write-tree"])?;
        if self.write_identity()? != *reviewed {
            return Err("Index changed before commit. Refresh and review again.".into());
        }
        self.commit(message)?;
        let sha = self.run(&["rev-parse", "--verify", "HEAD"])?;
        let state = self.run(&["show", "-s", "--format=%P%x00%T", "HEAD"])?;
        let (parents, actual_tree) = state
            .trim()
            .split_once('\0')
            .ok_or("Commit outcome uncertain. Refresh and inspect HEAD.")?;
        if parents != reviewed.head
            || actual_tree != tree.trim()
            || self.write_identity()?.symbolic_head != reviewed.symbolic_head
        {
            return Err("Commit outcome uncertain: resulting parent/tree differs from the reviewed commit. Refresh and inspect HEAD before retrying. Your message is saved.".into());
        }
        Ok(sha.trim().into())
    }
    pub fn commit(&self, message: &str) -> GitResult<()> {
        if message.trim().is_empty() {
            return Err("Enter a commit message".into());
        }
        self.run(&["commit", "-m", message]).map(|_| ())
    }
    pub fn create_branch(&self, name: &str) -> GitResult<()> {
        validate_revision(name)?;
        self.run(&["check-ref-format", "--branch", name])?;
        self.run(&["branch", "--", name]).map(|_| ())
    }
    pub fn create_branch_at(&self, name: &str, start: &str) -> GitResult<()> {
        validate_revision(name)?;
        validate_revision(start)?;
        self.run(&["check-ref-format", "--branch", name])?;
        self.run(&["branch", "--", name, start]).map(|_| ())
    }
    pub fn checkout(&self, branch: &str) -> GitResult<()> {
        validate_revision(branch)?;
        let local_ref = format!("refs/heads/{branch}");
        if self
            .run(&["show-ref", "--verify", "--quiet", &local_ref])
            .is_ok()
        {
            self.run(&["switch", branch]).map(|_| ())
        } else {
            // A commit/tag selected in history is intentionally detached; a
            // local branch keeps its normal symbolic HEAD relationship.
            let commit_ref = format!("{branch}^{{commit}}");
            self.run(&["rev-parse", "--verify", &commit_ref])?;
            self.run(&["switch", "--detach", branch]).map(|_| ())
        }
    }
    pub fn merge(&self, target: &str) -> GitResult<()> {
        validate_revision(target)?;
        self.run(&["merge", "--no-edit", "--", target]).map(|_| ())
    }
    pub fn cherry_pick(&self, target: &str) -> GitResult<()> {
        validate_revision(target)?;
        self.run(&["cherry-pick", target]).map(|_| ())
    }
    pub fn revert(&self, target: &str) -> GitResult<()> {
        validate_revision(target)?;
        self.run(&["revert", "--no-edit", target]).map(|_| ())
    }
    pub fn stash(&self) -> GitResult<()> {
        self.stash_with_message("GitCrab stash")
    }

    pub fn stash_with_message(&self, message: &str) -> GitResult<()> {
        self.run(&["stash", "push", "-m", message]).map(|_| ())
    }
    pub fn stash_pop(&self) -> GitResult<()> {
        self.run(&["stash", "pop"]).map(|_| ())
    }
    pub fn rebase(&self, target: &str) -> GitResult<()> {
        validate_revision(target)?;
        self.run(&["rebase", target]).map(|_| ())
    }
    /// Supports preserving worktree content only; no hard reset is exposed.
    pub fn reset(&self, target: &str, soft: bool) -> GitResult<()> {
        validate_revision(target)?;
        self.run(&["reset", if soft { "--soft" } else { "--mixed" }, target])
            .map(|_| ())
    }
}

fn validate_revision(value: &str) -> GitResult<()> {
    if value.is_empty() || value.starts_with('-') || value.contains('\0') {
        Err("Invalid branch or revision".into())
    } else {
        Ok(())
    }
}

fn parse_status(data: &[u8]) -> Vec<FileChange> {
    let mut out = Vec::new();
    let mut records = data.split(|b| *b == 0);
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let index = record[0] as char;
        let worktree = record[1] as char;
        let original_path = if index == 'R' || index == 'C' || worktree == 'R' || worktree == 'C' {
            records
                .next()
                .map(|source| String::from_utf8_lossy(source).into_owned())
        } else {
            None
        };
        out.push(FileChange {
            path: String::from_utf8_lossy(&record[3..]).into_owned(),
            original_path,
            index,
            worktree,
        });
    }
    out
}

pub fn split_hunks(diff: &str) -> Vec<Hunk> {
    // Binary changes and renames have no textual hunks. Preserve headers per file.
    let mut header = String::new();
    let mut body = String::new();
    let mut title = String::new();
    let mut out = Vec::new();
    let flush = |out: &mut Vec<Hunk>, header: &String, body: &mut String, title: &String| {
        if !body.is_empty() {
            out.push(Hunk {
                title: title.clone(),
                patch: format!("{header}{body}"),
            });
            body.clear();
        }
    };
    for line in diff.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            flush(&mut out, &header, &mut body, &title);
            header.clear();
            header.push_str(line);
        } else if line.starts_with("@@ ") {
            flush(&mut out, &header, &mut body, &title);
            title = line.trim_end().into();
            body.push_str(line);
        } else if !body.is_empty() {
            body.push_str(line);
        } else {
            header.push_str(line);
        }
    }
    flush(&mut out, &header, &mut body, &title);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    struct Fixture {
        git: Git,
    }
    impl Fixture {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let serial = NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "gitcrab-backend-{}-{nonce}-{serial}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            let git = Git { path };
            git.run(&["init", "--quiet", "--initial-branch=main"])
                .unwrap();
            git.run(&["config", "user.name", "GitCrab Test"]).unwrap();
            git.run(&["config", "user.email", "test@example.invalid"])
                .unwrap();
            git.run(&["config", "commit.gpgsign", "false"]).unwrap();
            git.run(&["config", "core.autocrlf", "false"]).unwrap();
            std::fs::create_dir(git.path.join("empty-hooks")).unwrap();
            git.run(&["config", "core.hooksPath", "empty-hooks"])
                .unwrap();
            Self { git }
        }
        fn write(&self, name: &str, text: &str) {
            std::fs::write(self.git.path.join(name), text).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // This directory was exclusively created by Fixture::new above.
            let _ = std::fs::remove_dir_all(&self.git.path);
        }
    }
    #[test]
    fn reviewed_commit_rejects_index_head_hooks_and_active_markers() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("one.txt", "one\n");
        git.stage("one.txt").unwrap();
        let unborn = git.write_identity().unwrap();
        let sha = git.commit_reviewed("first", &unborn).unwrap();
        assert_eq!(sha, git.run(&["rev-parse", "HEAD"]).unwrap().trim());
        fixture.write("one.txt", "two\n");
        git.stage("one.txt").unwrap();
        let reviewed = git.write_identity().unwrap();
        fixture.write("extra.txt", "extra\n");
        git.stage("extra.txt").unwrap();
        assert!(git
            .commit_reviewed("stale index", &reviewed)
            .unwrap_err()
            .contains("changed"));
        let reviewed = git.write_identity().unwrap();
        git.run(&["commit", "--allow-empty", "-m", "external"])
            .unwrap();
        assert!(git.commit_reviewed("stale HEAD", &reviewed).is_err());
        fixture.write("one.txt", "three\n");
        git.stage("one.txt").unwrap();
        let reviewed = git.write_identity().unwrap();
        fixture.write(
            "empty-hooks/pre-commit",
            "#!/bin/sh\necho controlled-hook-failure >&2\nexit 1\n",
        );
        let failed = git.commit_reviewed("hook fails", &reviewed).unwrap_err();
        assert!(failed.contains("controlled-hook-failure"));
        assert_eq!(git.write_identity().unwrap(), reviewed);
        fixture.write("empty-hooks/pre-commit", "#!/bin/sh\nexit 0\n");
        let marker = git.run(&["rev-parse", "--git-path", "MERGE_HEAD"]).unwrap();
        std::fs::write(git.path.join(marker.trim()), format!("{}\n", reviewed.head)).unwrap();
        let snapshot = git.snapshot(100).unwrap();
        assert!(snapshot.commit_blocked());
        assert!(!snapshot.status.iter().any(|f| f.index == 'U'));
        assert!(git
            .commit_reviewed("ordinary commit", &reviewed)
            .unwrap_err()
            .contains("active Git operation"));
    }
    #[test]
    fn discard_only_reviewed_hunk_preserves_index_head_and_other_hunk() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let original = (0..40).map(|i| format!("line {i}\n")).collect::<String>();
        fixture.write("日本語.txt", &original);
        git.stage(".").unwrap();
        git.commit("base").unwrap();
        let staged = original.replace("line 1\n", "staged one\n");
        fixture.write("日本語.txt", &staged);
        git.stage(".").unwrap();
        let working = staged
            .replace("line 10\n", "work ten\n")
            .replace("line 30\n", "work thirty\n");
        fixture.write("日本語.txt", &working);
        let identity = git.write_identity().unwrap();
        let hunks = git.hunks("日本語.txt", false).unwrap();
        assert_eq!(hunks.len(), 2);
        git.discard_hunk("日本語.txt", &hunks[0]).unwrap();
        assert_eq!(git.write_identity().unwrap(), identity);
        let expected = staged.replace("line 30\n", "work thirty\n");
        assert_eq!(
            std::fs::read(git.path.join("日本語.txt")).unwrap(),
            expected.as_bytes()
        );
        assert!(git.discard_hunk("日本語.txt", &hunks[0]).is_err());
        fixture.write("new.txt", "untracked\n");
        let added = git.hunks("new.txt", false).unwrap();
        assert!(git.discard_hunk("new.txt", &added[0]).is_err());
    }
    #[test]
    fn unique_path_counts_and_linked_worktree_markers_stay_local() {
        let snapshot = RepoSnapshot {
            status: vec![
                FileChange {
                    path: "new".into(),
                    index: 'R',
                    worktree: 'M',
                    original_path: Some("old".into()),
                },
                FileChange {
                    path: "new".into(),
                    index: 'M',
                    worktree: 'M',
                    original_path: None,
                },
            ],
            ..Default::default()
        };
        assert_eq!(snapshot.unique_changes().len(), 1);
        assert_eq!(snapshot.change_counts(), [0, 0, 0, 1]);
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("a", "a");
        git.stage(".").unwrap();
        git.commit("root").unwrap();
        let linked_path = git.path.join("linked");
        git.run(&[
            "worktree",
            "add",
            "-b",
            "linked",
            linked_path.to_str().unwrap(),
        ])
        .unwrap();
        let linked = Git::open(&linked_path).unwrap();
        let marker = linked
            .run(&["rev-parse", "--git-path", "CHERRY_PICK_HEAD"])
            .unwrap();
        let marker = PathBuf::from(marker.trim());
        std::fs::write(
            if marker.is_absolute() {
                marker
            } else {
                linked.path.join(marker)
            },
            git.run(&["rev-parse", "HEAD"]).unwrap(),
        )
        .unwrap();
        assert_eq!(linked.operation_markers().unwrap(), ["cherry-pick"]);
        assert!(git.operation_markers().unwrap().is_empty());
    }
    #[test]
    fn rename_comparisons_keep_paths_and_blobs_in_the_correct_namespace() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let old = "legacy 日本語 [1].txt";
        let new = "archive 日本語 [2].txt";
        let text = (0..30)
            .map(|n| format!("stable line {n}\n"))
            .collect::<String>();
        fixture.write(old, &text);
        git.stage(old).unwrap();
        git.commit("root").unwrap();
        let before = git.run(&["rev-parse", &format!("HEAD:{old}")]).unwrap();
        git.run(&["mv", "--", old, new]).unwrap();
        let staged = git.diff(None, new, true).unwrap();
        assert!(staged.contains("rename from "));
        assert!(!staged.contains("@@"));
        assert_eq!(before, git.run(&["rev-parse", &format!(":{new}")]).unwrap());
        git.commit("pure rename").unwrap();
        let id = git.run(&["rev-parse", "HEAD"]).unwrap();
        let files = git.changed_files(id.trim()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].original_path.as_deref(), Some(old));
        assert_eq!(files[0].path, new);
        let committed = git.diff(Some(id.trim()), new, false).unwrap();
        assert!(committed.contains("similarity index 100%"));
        assert!(!committed.contains("@@"));
        // An unstaged edit after a staged rename compares against the new
        // index path, while the staged comparison still uses HEAD's old path.
        git.run(&["mv", "--", new, old]).unwrap();
        fixture.write(old, &text.replace("stable line 15", "edited line 15"));
        let working = git.diff(None, old, false).unwrap();
        assert!(working.contains("-stable line 15"));
        assert!(working.contains("+edited line 15"));
        assert!(!working.contains("rename from"));
        git.stage(old).unwrap();
        let edited = git.diff(None, old, true).unwrap();
        assert!(edited.contains("rename from"));
        assert!(edited.contains("-stable line 15"));
        git.commit("rename with edit").unwrap();
        let id = git.run(&["rev-parse", "HEAD"]).unwrap();
        assert_eq!(
            git.diff(Some(id.trim()), old, false).unwrap(),
            git.run(&[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--find-renames",
                "HEAD^1",
                "HEAD",
                "--",
                new,
                old
            ])
            .unwrap()
        );
        assert_eq!(
            git.run(&["show", &format!("HEAD:{old}")]).unwrap(),
            text.replace("stable line 15", "edited line 15")
        );
    }

    #[test]
    fn real_git_create_branch_at_keeps_head_and_index() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("file", "one");
        git.stage("file").unwrap();
        git.commit("first").unwrap();
        let first = git.run(&["rev-parse", "HEAD"]).unwrap();
        fixture.write("file", "two");
        git.stage("file").unwrap();
        git.commit("second").unwrap();
        let head = git.run(&["rev-parse", "HEAD"]).unwrap();
        let tree = git.run(&["write-tree"]).unwrap();
        git.create_branch_at("feature/日本語", first.trim())
            .unwrap();
        assert_eq!(git.run(&["rev-parse", "feature/日本語"]).unwrap(), first);
        assert_eq!(git.run(&["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(git.run(&["write-tree"]).unwrap(), tree);
        assert!(git.create_branch_at("--bad", first.trim()).is_err());
    }
    #[test]
    fn real_git_unicode_paths_and_branch_names() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let name = "notes 日本語 [draft].txt";
        fixture.write(name, "First version\n");
        git.stage(name).unwrap();
        git.commit("Unicode path").unwrap();
        git.create_branch("feature/日本語").unwrap();
        git.checkout("feature/日本語").unwrap();
        fixture.write(name, "Second version\n");
        let state = git.snapshot(10).unwrap();
        assert_eq!(state.head, "feature/日本語");
        assert_eq!(state.status[0].path, name);
        assert!(git
            .diff(None, name, false)
            .unwrap()
            .contains("+Second version"));
        let nested = git.path.join("folder with spaces 日本語");
        std::fs::create_dir(&nested).unwrap();
        assert_eq!(
            Git::open(nested).unwrap().path.canonicalize().unwrap(),
            git.path.canonicalize().unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn untracked_symlink_preview_cannot_leave_repository() {
        let fixture = Fixture::new();
        let outside = Fixture::new();
        outside.write("outside.txt", "outside repository\n");
        std::os::unix::fs::symlink(
            outside.git.path.join("outside.txt"),
            fixture.git.path.join("link.txt"),
        )
        .unwrap();
        let error = fixture.git.diff(None, "link.txt", false).unwrap_err();
        assert!(error.contains("leaves the repository"));
        assert_eq!(
            std::fs::read_to_string(outside.git.path.join("outside.txt")).unwrap(),
            "outside repository\n"
        );
    }

    #[test]
    fn status_keeps_rename_destination_and_literal_names() {
        let rows = parse_status(b"R  new name\0old name\0 M [literal]*.rs\0?? line\nbreak\0");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].path, "new name");
        assert_eq!(rows[0].original_path.as_deref(), Some("old name"));
        assert_eq!(rows[1].path, "[literal]*.rs");
        assert_eq!(rows[2].path, "line\nbreak");
    }
    #[test]
    fn split_patch_keeps_context_and_headers() {
        let patches = split_hunks("diff --git a/a b/a\nindex 1..2 100644\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n@@ -20 +20 @@\n-before\n+after\n");
        assert_eq!(patches.len(), 2);
        assert!(patches[1].patch.contains("--- a/a\n+++ b/a\n@@ -20"));
        assert!(!patches[1].patch.contains("+new"));
    }
    #[test]
    fn real_git_unborn_literal_paths_and_independent_hunks() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        assert!(git.snapshot(100).unwrap().commits.is_empty());
        let original: String = (0..40).map(|i| format!("line {i}\n")).collect();
        fixture.write("[literal].txt", &original);
        assert!(git
            .diff(None, "[literal].txt", false)
            .unwrap()
            .contains("+line 2"));
        git.stage("[literal].txt").unwrap();
        assert_eq!(git.snapshot(100).unwrap().status[0].index, 'A');
        git.unstage("[literal].txt").unwrap();
        assert_eq!(git.snapshot(100).unwrap().status[0].index, '?');
        git.stage("[literal].txt").unwrap();
        git.commit("Initial content").unwrap();
        fixture.write(
            "[literal].txt",
            &original
                .replace("line 2\n", "first edit\n")
                .replace("line 32\n", "second edit\n"),
        );
        let hunks = git.hunks("[literal].txt", false).unwrap();
        assert_eq!(hunks.len(), 2);
        git.apply_hunk(&hunks[0], false).unwrap();
        let cached = git.diff(None, "[literal].txt", true).unwrap();
        assert!(cached.contains("+first edit"));
        assert!(!cached.contains("+second edit"));
        let remaining = git.diff(None, "[literal].txt", false).unwrap();
        assert!(remaining.contains("+second edit"));
        assert!(!remaining.contains("+first edit"));
        git.apply_hunk(&git.hunks("[literal].txt", true).unwrap()[0], true)
            .unwrap();
        assert!(git.diff(None, "[literal].txt", true).unwrap().is_empty());
        assert_eq!(git.hunks("[literal].txt", false).unwrap().len(), 2);
    }
    #[test]
    fn real_git_merge_refs_root_diff_and_detached_head() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("root.txt", "root\n");
        git.stage("root.txt").unwrap();
        git.commit("Root").unwrap();
        let root = git.snapshot(100).unwrap().commits[0].id.clone();
        assert_eq!(
            git.changed_files(&root)
                .unwrap()
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec!["root.txt"]
        );
        assert!(git
            .diff(Some(&root), "root.txt", false)
            .unwrap()
            .contains("+root"));
        git.create_branch("feature").unwrap();
        git.checkout("feature").unwrap();
        fixture.write("feature.txt", "feature\n");
        git.stage("feature.txt").unwrap();
        git.commit("Feature").unwrap();
        git.checkout("main").unwrap();
        fixture.write("main.txt", "main\n");
        git.stage("main.txt").unwrap();
        git.commit("Main").unwrap();
        git.merge("feature").unwrap();
        git.run(&["tag", "-a", "v1", "-m", "Version one"]).unwrap();
        let state = git.snapshot(100).unwrap();
        assert_eq!(state.commits[0].parents.len(), 2);
        assert!(state.commits[0].refs.contains(&"v1".into()));
        assert!(state.commits[0].refs.contains(&"HEAD".into()));
        assert_eq!(
            git.changed_files(&state.commits[0].id)
                .unwrap()
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec!["feature.txt"]
        );
        git.checkout(&root).unwrap();
        assert!(git.run(&["symbolic-ref", "--quiet", "HEAD"]).is_err());
        let detached = git.snapshot(100).unwrap();
        assert!(detached
            .commits
            .iter()
            .any(|c| c.id == root && c.refs.contains(&"HEAD".into())));
    }
    #[test]
    fn real_git_cherry_pick_revert_stash_and_conflict_feedback() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        fixture.write("shared.txt", "original\n");
        git.stage("shared.txt").unwrap();
        git.commit("Root").unwrap();
        git.create_branch("feature").unwrap();
        git.checkout("feature").unwrap();
        fixture.write("feature.txt", "feature\n");
        git.stage("feature.txt").unwrap();
        git.commit("Feature addition").unwrap();
        let feature = git.run(&["rev-parse", "HEAD"]).unwrap().trim().to_string();
        git.checkout("main").unwrap();
        git.cherry_pick(&feature).unwrap();
        assert!(git.path.join("feature.txt").exists());
        git.revert("HEAD").unwrap();
        assert!(!git.path.join("feature.txt").exists());
        fixture.write("shared.txt", "work in progress\n");
        git.stash().unwrap();
        assert!(git.snapshot(100).unwrap().status.is_empty());
        git.stash_pop().unwrap();
        assert_eq!(
            std::fs::read_to_string(git.path.join("shared.txt")).unwrap(),
            "work in progress\n"
        );
        git.stage("shared.txt").unwrap();
        git.commit("Main line changes").unwrap();
        git.checkout("feature").unwrap();
        fixture.write("shared.txt", "feature conflicting version\n");
        git.stage("shared.txt").unwrap();
        git.commit("Feature line changes").unwrap();
        git.checkout("main").unwrap();
        assert!(git.merge("feature").is_err());
        assert!(git
            .snapshot(100)
            .unwrap()
            .status
            .iter()
            .any(|f| f.index == 'U' && f.worktree == 'U'));
        assert!(git.path.join(".git/MERGE_HEAD").exists());
    }
    #[test]
    fn real_git_rename_unstage_restores_both_index_paths_and_stale_stage_all_is_safe() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let original="Original component\nKeep this content to identify the renamed file.\nMore stable content.\n";
        fixture.write("old [name].txt", original);
        git.stage("old [name].txt").unwrap();
        git.commit("Track original name").unwrap();
        git.run(&["mv", "--", "old [name].txt", "new [name].txt"])
            .unwrap();
        let changed = format!("{original}Additional unstaged line\n");
        fixture.write("new [name].txt", &changed);
        git.stage("new [name].txt").unwrap();
        let state = git.snapshot(100).unwrap();
        assert_eq!(state.status.len(), 1);
        assert_eq!(state.status[0].index, 'R');
        assert_eq!(state.status[0].worktree, ' ');
        assert_eq!(
            state.status[0].original_path.as_deref(),
            Some("old [name].txt")
        );
        git.unstage("new [name].txt").unwrap();
        assert!(git
            .run(&["diff", "--cached", "--name-only"])
            .unwrap()
            .is_empty());
        assert!(!git.path.join("old [name].txt").exists());
        assert_eq!(
            std::fs::read_to_string(git.path.join("new [name].txt")).unwrap(),
            changed
        );
        let stale = git.snapshot(100).unwrap().status;
        // Stage-all queues paths from one snapshot. Both orders and repeated
        // snapshots must remain valid as Git combines deletion+addition into R.
        for _ in 0..2 {
            for file in stale.iter().rev() {
                git.stage(&file.path).unwrap();
            }
        }
        assert_eq!(git.snapshot(100).unwrap().status[0].index, 'R');
        git.unstage("new [name].txt").unwrap();
        assert!(git
            .run(&["diff", "--cached", "--name-only"])
            .unwrap()
            .is_empty());
    }
    #[test]
    fn real_git_new_file_hunk_can_be_staged_and_unstaged_without_touching_worktree() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let name = "new [file] with spaces.txt";
        let content = "First untracked line\nSecond untracked line\n";
        fixture.write(name, content);
        let hunks = git.hunks(name, false).unwrap();
        assert_eq!(hunks.len(), 1);
        git.apply_hunk(&hunks[0], false).unwrap();
        assert_eq!(git.snapshot(100).unwrap().status[0].index, 'A');
        assert_eq!(git.run(&["show", &format!(":{name}")]).unwrap(), content);
        let staged = git.hunks(name, true).unwrap();
        git.apply_hunk(&staged[0], true).unwrap();
        assert_eq!(git.snapshot(100).unwrap().status[0].index, '?');
        assert_eq!(
            std::fs::read_to_string(git.path.join(name)).unwrap(),
            content
        );
    }
}
