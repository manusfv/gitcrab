//! Git subprocess boundary. Every argument is passed directly, never through a shell.
use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub type GitResult<T> = Result<T, String>;

#[derive(Clone, Debug, Default)]
pub struct Commit {
    pub id: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub author: String,
    pub date: String,
    pub refs: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct FileChange {
    /// Destination path for renames; relative to the repository root.
    pub path: String,
    /// Source retained from porcelain's second NUL record for renames/copies.
    pub original_path: Option<String>,
    pub index: char,
    pub worktree: char,
}

#[derive(Clone, Debug, Default)]
pub struct RepoSnapshot {
    pub commits: Vec<Commit>,
    pub branches: Vec<String>,
    pub head: String,
    /// Full HEAD identity, including when outside the loaded history page.
    pub head_id: String,
    pub status: Vec<FileChange>,
}

#[derive(Clone, Debug)]
pub struct Hunk {
    pub title: String,
    /// Self-contained patch with original file headers and one hunk.
    pub patch: String,
}

#[derive(Clone, Debug)]
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
        let mut cmd = Command::new("git");
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
        let output = self
            .command(args)
            .output()
            .map_err(|e| format!("Could not start Git: {e}"))?;
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
        let head = self
            .run(&["symbolic-ref", "--short", "HEAD"])
            .or_else(|_| self.run(&["rev-parse", "--short", "HEAD"]))?
            .trim()
            .to_string();
        let refs = self.run(&[
            "for-each-ref",
            "--format=%(objectname)%00%(refname:short)%00%(*objectname)",
        ])?;
        let mut labels: HashMap<String, Vec<String>> = HashMap::new();
        for line in refs.lines() {
            let fields: Vec<_> = line.split('\0').collect();
            if fields.len() == 3 {
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
        let head_exists = self.run(&["rev-parse", "--verify", "HEAD"]);
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
        let branches = self
            .run(&["for-each-ref", "--format=%(refname:short)", "refs/heads"])?
            .lines()
            .map(String::from)
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
        Ok(RepoSnapshot {
            commits,
            branches,
            head,
            head_id,
            status,
        })
    }

    pub fn changed_files(&self, commit: &str) -> GitResult<Vec<String>> {
        validate_revision(commit)?;
        let parent = format!("{commit}^1");
        let data = if self.run(&["rev-parse", "--verify", &parent]).is_ok() {
            self.run(&["diff", "--name-only", "-z", &parent, commit, "--"])?
        } else {
            self.run(&[
                "diff-tree",
                "--root",
                "--no-commit-id",
                "--name-only",
                "-r",
                "-z",
                commit,
            ])?
        };
        Ok(data
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect())
    }

    /// A selected commit is diffed against its first parent. None selects index/worktree.
    pub fn diff(&self, commit: Option<&str>, path: &str, staged: bool) -> GitResult<String> {
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
        args.extend(["--", path]);
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
            vec!["rm", "--cached", "--"]
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
    struct Fixture {
        git: Git,
    }
    impl Fixture {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("gitcrab-backend-{}-{nonce}", std::process::id()));
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
        assert_eq!(git.changed_files(&root).unwrap(), vec!["root.txt"]);
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
            git.changed_files(&state.commits[0].id).unwrap(),
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
