//! Repository data parsed from Git output: status, history, refs and patches.
use super::*;
use std::path::PathBuf;

/// A commit and its decorations in the loaded history.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct Commit {
    /// Full Git object ID.
    pub id: String,
    /// Parent object IDs in Git's order, with the first parent first.
    pub parents: Vec<String>,
    pub subject: String,
    /// The message body on one line, whitespace collapsed and capped, shown
    /// dimmed after the subject in the history list. Empty when there is none.
    #[serde(default)]
    pub body_preview: String,
    pub author: String,
    #[serde(default)]
    pub author_email: String,
    /// Author date in strict ISO 8601 format.
    pub date: String,
    /// Display names of refs pointing at this commit, including the `HEAD` label.
    pub refs: Vec<String>,
    /// Lowercase search text built from the subject, author, object ID and refs.
    #[serde(default)]
    pub search_key: String,
    /// A stash entry. It lists only its base commit as a parent; the commits
    /// Git keeps for the index and untracked files are not part of history.
    #[serde(default)]
    pub stash: bool,
    /// The folder of another worktree whose uncommitted changes this row
    /// stands for. Its only parent is that worktree's HEAD.
    #[serde(default)]
    pub worktree: Option<PathBuf>,
}

impl Commit {
    pub fn index_search(mut self) -> Self {
        self.search_key = format!(
            "{} {} {} {}",
            self.subject,
            self.author,
            self.id,
            self.refs.join(" ")
        )
        .to_lowercase();
        self
    }

    /// Searches the cached text for a substring prepared by [`normalize_search`].
    pub fn matches_search(&self, normalized_query: &str) -> bool {
        self.search_key.contains(normalized_query)
    }
}

/// Longest body preview kept per commit, in characters. A history row never
/// shows more than this, and the cap keeps long messages out of memory.
const BODY_PREVIEW_CHARS: usize = 200;

/// Joins a commit body onto one line for [`Commit::body_preview`].
pub fn body_preview(body: &str) -> String {
    let mut preview = String::new();
    for word in body.split_whitespace() {
        if !preview.is_empty() {
            preview.push(' ');
        }
        preview.push_str(word);
        if preview.len() > BODY_PREVIEW_CHARS * 4 {
            break;
        }
    }
    if let Some((end, _)) = preview.char_indices().nth(BODY_PREVIEW_CHARS) {
        preview.truncate(end);
        preview.push('…');
    }
    preview
}

/// Trims and lowercases a query for [`Commit::matches_search`].
pub fn normalize_search(query: &str) -> String {
    query.trim().to_lowercase()
}

/// A changed path from working-tree status or a commit comparison.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct FileChange {
    /// Repository-relative path, using the destination for renames and copies.
    pub path: String,
    /// Repository-relative source path for a rename or copy.
    pub original_path: Option<String>,
    /// Porcelain index status, or the change status for a commit comparison.
    pub index: char,
    /// Porcelain working-tree status, or a space for a commit comparison.
    pub worktree: char,
    /// The paths include non-UTF-8 bytes and are only suitable for display.
    #[serde(default)]
    pub unsupported_path: bool,
}

impl FileChange {
    /// Escapes a supported path for display or returns the stored label for an unsupported path.
    pub fn display_path(&self) -> String {
        if self.unsupported_path {
            self.path.clone()
        } else {
            display_path(&self.path)
        }
    }
    /// Returns the reason file actions are unavailable for a non-UTF-8 path.
    pub fn path_issue(&self) -> Option<&'static str> {
        self.unsupported_path
            .then_some("GitCrab can't act on this path because it isn't valid UTF-8.")
    }
    /// Reports whether the porcelain status describes unresolved index entries.
    pub fn is_unmerged(&self) -> bool {
        matches!(
            (self.index, self.worktree),
            ('D', 'D')
                | ('A', 'U')
                | ('U', 'D')
                | ('U', 'A')
                | ('D', 'U')
                | ('A', 'A')
                | ('U', 'U')
        )
    }

    /// Names the change on one side of working changes, or a commit comparison's change.
    ///
    /// Unmerged entries are named by their conflict type.
    pub fn status_name(&self, staged: bool) -> &'static str {
        if self.is_unmerged() {
            return match (self.index, self.worktree) {
                ('U', 'U') => "Both modified",
                ('A', 'A') => "Both added",
                ('D', 'D') => "Both deleted",
                ('A', 'U') => "Added by us",
                ('U', 'A') => "Added by them",
                ('D', 'U') => "Deleted by us",
                _ => "Deleted by them",
            };
        }
        let status = if self.index == '?' {
            '?'
        } else if staged {
            self.index
        } else {
            self.worktree
        };
        match status {
            '?' => "Untracked",
            'A' => "Added",
            'D' => "Deleted",
            'R' => "Renamed",
            'C' => "Copied",
            'T' => "Type changed",
            'M' => "Modified",
            _ => "Changed",
        }
    }

    /// Describes the path for a tooltip, naming the status and any rename or copy source.
    pub fn describe(&self, staged: bool) -> String {
        let name = self.status_name(staged);
        if let Some(issue) = self.path_issue() {
            return format!("{name}: {}. {issue}", self.display_path());
        }
        match &self.original_path {
            Some(old) if matches!(self.index, 'R' | 'C') => {
                format!("{name}: {} → {}", display_path(old), self.display_path())
            }
            _ => format!("{name}: {}", self.display_path()),
        }
    }

    /// Reports whether this path belongs in an ordinary staged or unstaged list.
    /// Unmerged paths belong in the conflict list, and untracked paths are unstaged.
    pub fn on_side(&self, staged: bool) -> bool {
        if self.is_unmerged() {
            return false;
        }
        if staged {
            self.index != ' ' && self.index != '?'
        } else {
            self.worktree != ' ' || self.index == '?'
        }
    }

    /// Predicts this entry after staging it, or unstaging it with `unstage`,
    /// for showing the result before Git confirms it.
    ///
    /// `Some(None)` means the entry disappears. `None` means the outcome is
    /// not obvious from the status alone, as for renames, conflicts and
    /// unusual combinations; the entry is then left as it is. A prediction
    /// can still be wrong, for example when a file was edited back to its
    /// staged content; the status Git reports afterwards replaces it.
    pub fn predicted(&self, unstage: bool) -> Option<Option<Self>> {
        if !self.on_side(unstage) {
            return Some(Some(self.clone()));
        }
        if self.is_unmerged() || self.unsupported_path || self.original_path.is_some() {
            return None;
        }
        let edited = |c: char| matches!(c, 'M' | 'T' | 'D');
        let (index, worktree) = match (self.index, self.worktree, unstage) {
            ('?', '?', false) => ('A', ' '),
            ('A', 'D', false) => return Some(None),
            ('A', 'M' | 'T', false) => ('A', ' '),
            (' ' | 'M' | 'T', w, false) if edited(w) => (w, ' '),
            ('A', ' ' | 'M', true) => ('?', '?'),
            ('A', 'D', true) => return Some(None),
            (i, ' ', true) if edited(i) => (' ', i),
            (i, w, true) if edited(i) && edited(w) => (' ', w),
            _ => return None,
        };
        Some(Some(Self {
            index,
            worktree,
            ..self.clone()
        }))
    }

    /// Predicts this entry once its staged change is committed: the staged
    /// side empties and only working-tree changes remain. `Some(None)` means
    /// the entry disappears, `None` that it cannot be predicted (conflicts).
    pub fn after_commit(&self) -> Option<Option<Self>> {
        if self.is_unmerged() {
            return None;
        }
        if !self.on_side(true) {
            return Some(Some(self.clone()));
        }
        Some((self.worktree != ' ').then(|| Self {
            index: ' ',
            original_path: None,
            ..self.clone()
        }))
    }
}

/// Escapes a path for display using Rust's debug escaping without quotes.
pub fn display_path(path: &str) -> String {
    path.escape_debug().to_string()
}

/// Returns the 1-based line of the first conflict marker in a merged file.
///
/// Recognizes `<<<<<<<`, `|||||||`, `=======` and `>>>>>>>` lines, but only
/// when an opening or closing marker is present so a lone `=======`
/// underline is not reported.
pub fn conflict_marker_line(content: &[u8]) -> Option<usize> {
    let is_marker = |line: &[u8], marker: &[u8], bare: bool| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        line.strip_prefix(marker).is_some_and(|rest| {
            if bare {
                rest.is_empty()
            } else {
                rest.is_empty() || rest[0] == b' '
            }
        })
    };
    let lines = || content.split(|&byte| byte == b'\n');
    let bounded = lines()
        .any(|line| is_marker(line, b"<<<<<<<", false) || is_marker(line, b">>>>>>>", false));
    if !bounded {
        return None;
    }
    lines()
        .position(|line| {
            is_marker(line, b"<<<<<<<", false)
                || is_marker(line, b"|||||||", false)
                || is_marker(line, b"=======", true)
                || is_marker(line, b">>>>>>>", false)
        })
        .map(|index| index + 1)
}

/// Result of a whole-worktree index action.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BulkStage {
    /// Distinct paths whose status entry changed.
    pub changed: usize,
    /// Unmerged paths left for explicit resolution.
    pub conflicts: usize,
}
/// HEAD and index state used to detect changes since a write was reviewed.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct WriteIdentity {
    /// Full HEAD commit object ID, or an empty string for an unborn branch.
    pub head: String,
    /// Full symbolic ref of HEAD, or an empty string for detached HEAD.
    pub symbolic_head: String,
    /// Raw NUL-delimited index entries from `git ls-files --stage -z`.
    pub index: Vec<u8>,
}

impl WriteIdentity {
    /// The index entries at a conflict stage (1 to 3), each as
    /// `git ls-files --unmerged -z` prints it, without the terminating NUL.
    pub fn unmerged_entries(&self) -> Vec<&[u8]> {
        self.index
            .split(|&byte| byte == 0)
            .filter(|entry| {
                let meta = entry
                    .split(|&byte| byte == b'\t')
                    .next()
                    .unwrap_or_default();
                meta.rsplit(|&byte| byte == b' ')
                    .next()
                    .is_some_and(|stage| !stage.is_empty() && stage != b"0")
            })
            .collect()
    }
}
/// Human-readable identity of a resolved commit or stash.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitSummary {
    pub subject: String,
    /// Committer date in Git's ISO-like format.
    pub date: String,
    pub parents: usize,
}
/// Local branch and the commit it points to.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct LocalBranch {
    /// Branch name without the `refs/heads/` prefix.
    pub name: String,
    pub tip: String,
    /// The remote and the branch name there that it tracks.
    #[serde(default)]
    pub upstream: Option<(String, String)>,
}
/// Remote-tracking ref or tag and its target object.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct ReferenceTip {
    /// Full ref name, including `refs/remotes/` or `refs/tags/`.
    pub full_name: String,
    /// Short ref name for display.
    pub label: String,
    /// Target object ID, dereferenced for an annotated tag.
    pub tip: String,
}

/// Reference namespace used for history previews.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceKind {
    LocalBranch,
    Remote,
    Tag,
}

/// A stash list entry as people identify it.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct StashEntry {
    /// The stash commit, which also names its history row.
    #[serde(default)]
    pub id: String,
    /// Reflog subject, such as `On main: message`.
    pub message: String,
    /// Creation date in Git's ISO-like format.
    pub date: String,
}

/// A working tree attached to the repository.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Worktree {
    /// Root folder as Git reports it.
    pub path: PathBuf,
    /// Checked-out branch without `refs/heads/`. `None` when detached or bare.
    pub branch: Option<String>,
    /// Whether this is the worktree the tab shows.
    pub current: bool,
    /// Whether the folder is gone and `git worktree prune` would remove it.
    pub prunable: bool,
    /// The repository's first worktree, which cannot be removed.
    pub main: bool,
    /// Whether `git worktree lock` protects it from removal and pruning.
    pub locked: bool,
    /// Checked-out commit. Empty for a bare repository.
    pub head: String,
    /// Uncommitted changes, read for worktrees other than the current one.
    pub changes: Option<WorktreeChanges>,
}

/// Counts of uncommitted paths in a worktree, as GitKraken shows them.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorktreeChanges {
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
}

impl WorktreeChanges {
    /// Counts the entries of `git status --porcelain=v1 -z`.
    pub fn count(status: &[FileChange]) -> Self {
        let mut changes = Self::default();
        for file in status {
            match (file.index, file.worktree) {
                ('?', _) | ('A', _) | (_, 'A') => changes.added += 1,
                ('D', _) | (_, 'D') => changes.deleted += 1,
                _ => changes.modified += 1,
            }
        }
        changes
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Such as `2 added, 1 deleted`, leaving out zero counts.
    pub fn summary(&self) -> String {
        [
            (self.added, "added"),
            (self.modified, "modified"),
            (self.deleted, "deleted"),
        ]
        .iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, what)| format!("{count} {what}"))
        .collect::<Vec<_>>()
        .join(", ")
    }
}

impl Worktree {
    /// The folder's last path segment.
    pub fn folder(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// The branch, or the folder when detached.
    pub fn label(&self) -> String {
        self.branch.clone().unwrap_or_else(|| self.folder())
    }

    /// The folder, shown beside the branch unless it only repeats the branch
    /// name: whole, its last segment, or with `/` written as `__` as
    /// GitKraken names worktree folders.
    pub fn detail(&self) -> Option<String> {
        let branch = self.branch.as_ref()?;
        let folder = self.folder();
        let repeats = folder == *branch
            || folder == branch.replace('/', "__")
            || branch.ends_with(&format!("/{folder}"));
        (!repeats).then_some(folder)
    }
}

/// One history row per other worktree with uncommitted changes, placed just
/// above the commit it has checked out. A worktree whose HEAD is not among
/// `rows` gets none.
pub fn with_worktree_rows(mut rows: Vec<Commit>, worktrees: &[Worktree]) -> Vec<Commit> {
    for worktree in worktrees {
        let Some(changes) = worktree.changes.filter(|c| !c.is_empty()) else {
            continue;
        };
        if worktree.current {
            continue;
        }
        let Some(at) = rows.iter().position(|row| row.id == worktree.head) else {
            continue;
        };
        let date = rows[at].date.clone();
        rows.insert(
            at,
            Commit {
                id: format!(
                    "{}{}",
                    crate::controller::WORKTREE_ROW,
                    worktree.path.display()
                ),
                parents: vec![worktree.head.clone()],
                subject: format!("WIP on {}", worktree.label()),
                body_preview: changes.summary(),
                date,
                worktree: Some(worktree.path.clone()),
                ..Default::default()
            },
        );
    }
    rows
}

/// Parses `git worktree list --porcelain -z`, main worktree first.
pub fn parse_worktrees(output: &str) -> Vec<Worktree> {
    let mut worktrees: Vec<Worktree> = vec![];
    for field in output.split('\0') {
        if let Some(path) = field.strip_prefix("worktree ") {
            worktrees.push(Worktree {
                path: path.into(),
                main: worktrees.is_empty(),
                ..Default::default()
            });
        } else if let Some(worktree) = worktrees.last_mut() {
            if let Some(branch) = field.strip_prefix("branch ") {
                worktree.branch = Some(branch.strip_prefix("refs/heads/").unwrap_or(branch).into());
            } else if let Some(head) = field.strip_prefix("HEAD ") {
                worktree.head = head.into();
            } else if field == "prunable" || field.starts_with("prunable ") {
                worktree.prunable = true;
            } else if field == "locked" || field.starts_with("locked ") {
                worktree.locked = true;
            }
        }
    }
    worktrees
}

impl StashEntry {
    /// The stash's own message and the branch it was made on, as GitKraken
    /// shows them. `On main: idea` is `("idea", Some("main"))`; Git's default
    /// `WIP on main: abc1234 subject` has no message of its own. Branch names
    /// cannot contain `:`, so the first `": "` ends the branch.
    pub fn parts(&self) -> (&str, Option<&str>) {
        stash_parts(&self.message)
    }
}

fn stash_parts(subject: &str) -> (&str, Option<&str>) {
    if let Some((branch, message)) = subject
        .strip_prefix("On ")
        .and_then(|rest| rest.split_once(": "))
    {
        return (message, Some(branch));
    }
    if let Some((branch, _)) = subject
        .strip_prefix("WIP on ")
        .and_then(|rest| rest.split_once(": "))
    {
        return ("", Some(branch));
    }
    (subject, None)
}

/// Parses `git stash list --format=%H%x00%P%x00%an%x00%ae%x00%aI%x00%ci%x00%gs`,
/// newest first, into history rows and list entries.
///
/// Each row keeps only the stash's base commit as its parent, so the commits
/// Git creates for the index and untracked files never reach the graph.
pub fn parse_stashes(output: &str) -> Vec<(Commit, StashEntry)> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(7, '\0');
            let [id, parents, author, author_email, date, created, message] =
                std::array::from_fn(|_| fields.next().unwrap_or_default());
            let base = parents.split_whitespace().next()?;
            let subject = match stash_parts(message) {
                ("", _) => message,
                (own, _) => own,
            };
            let row = Commit {
                id: id.to_owned(),
                parents: vec![base.to_owned()],
                subject: subject.to_owned(),
                author: author.to_owned(),
                author_email: author_email.to_owned(),
                date: date.to_owned(),
                stash: true,
                ..Default::default()
            }
            .index_search();
            let entry = StashEntry {
                id: id.to_owned(),
                message: message.to_owned(),
                date: created.to_owned(),
            };
            Some((row, entry))
        })
        .collect()
}

/// Places stash rows among `commits`, each above its base commit and among
/// the rows of its own date, the way `git log --date-order` would.
///
/// A stash whose base commit is not loaded is left out until a later page
/// brings the base in.
pub fn with_stashes(commits: &[Commit], stashes: impl IntoIterator<Item = Commit>) -> Vec<Commit> {
    let mut rows = commits.to_vec();
    for stash in stashes {
        let Some(base) = rows.iter().position(|row| row.id == stash.parents[0]) else {
            continue;
        };
        let created = iso_seconds(&stash.date);
        let at = rows[..base]
            .iter()
            .position(|row| {
                created.is_some_and(|created| iso_seconds(&row.date).is_some_and(|d| d < created))
            })
            .unwrap_or(base);
        rows.insert(at, stash);
    }
    rows
}

/// Seconds since the Unix epoch of a strict ISO 8601 date with an offset,
/// such as `2026-11-22T10:11:12+01:00`.
fn iso_seconds(date: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| date.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let offset = match date.get(19..)? {
        "Z" => 0,
        zone => {
            let sign = match zone.get(..1)? {
                "+" => 1,
                "-" => -1,
                _ => return None,
            };
            sign * (number(20..22)? * 3600 + number(23..25)? * 60)
        }
    };
    // Days from civil date (Howard Hinnant's algorithm), with March as the
    // first month so the leap day falls at the end of the year.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// Repository state read together for a tab refresh.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct RepoSnapshot {
    /// Canonical path used to identify this worktree.
    #[serde(default)]
    pub worktree_id: PathBuf,
    /// Loaded history in child-before-parent date order.
    pub commits: Vec<Commit>,
    pub branches: Vec<LocalBranch>,
    #[serde(default)]
    pub remote_refs: Vec<ReferenceTip>,
    #[serde(default)]
    pub stash_count: usize,
    /// The stash list, newest (`stash@{0}`) first.
    #[serde(default)]
    pub stashes: Vec<StashEntry>,
    #[serde(default)]
    pub worktrees: Vec<Worktree>,
    #[serde(default)]
    pub tags: Vec<ReferenceTip>,
    /// Branch name or abbreviated detached HEAD object ID.
    pub head: String,
    /// Full HEAD object ID, even outside the loaded page. Empty for unborn HEAD.
    pub head_id: String,
    pub status: Vec<FileChange>,
    /// Unmerged paths whose working file still had a conflict marker line
    /// when `status` was read.
    #[serde(default)]
    pub conflict_markers: std::collections::BTreeSet<String>,
    #[serde(default)]
    pub operations: Vec<String>,
    /// Git's prepared commit message (`MERGE_MSG`) for the merge,
    /// cherry-pick or revert in progress, without comment lines.
    #[serde(default)]
    pub operation_message: Option<String>,
    /// HEAD and index identity captured for subsequent write review.
    #[serde(default)]
    pub reviewed: Option<WriteIdentity>,
    /// Display text for the upstream relationship based on local refs.
    #[serde(default)]
    pub upstream_context: String,
    /// Remotes, the checked-out branch's upstream and the last fetch time.
    #[serde(default)]
    pub remote: RemoteState,
    /// Undo and Redo availability, filled in by the tab's worker.
    #[serde(default)]
    pub journal: crate::journal::JournalStatus,
}

impl RepoSnapshot {
    /// Returns distinct changed destination paths across staged and unstaged changes.
    pub fn unique_changes(&self) -> std::collections::BTreeSet<&str> {
        self.status.iter().map(|f| f.path.as_str()).collect()
    }
    /// Counts paths with unresolved conflicts separately from ordinary changes.
    pub fn conflict_count(&self) -> usize {
        self.status.iter().filter(|file| file.is_unmerged()).count()
    }

    /// Counts ordinary paths as `[modified, added, deleted, renamed_or_copied]`.
    /// Untracked paths count as added.
    pub fn change_counts(&self) -> [usize; 4] {
        let mut paths: std::collections::BTreeMap<&str, Vec<char>> = Default::default();
        for f in self.status.iter().filter(|file| !file.is_unmerged()) {
            paths
                .entry(&f.path)
                .or_default()
                .extend([f.index, f.worktree]);
        }
        let mut counts = [0; 4];
        for status in paths.values() {
            let category = if status.contains(&'R') || status.contains(&'C') {
                3
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
    /// Reports whether an active operation or unresolved status blocks an ordinary commit.
    pub fn commit_blocked(&self) -> bool {
        !self.operations.is_empty() || self.status.iter().any(FileChange::is_unmerged)
    }

    /// Lists status indices for an ordinary staged or unstaged list in status order.
    /// Unmerged paths are excluded. See [`Self::conflicts`].
    pub fn side(&self, staged: bool) -> Vec<usize> {
        (0..self.status.len())
            .filter(|&index| self.status[index].on_side(staged))
            .collect()
    }

    /// Lists status indices of unmerged paths in status order.
    pub fn conflicts(&self) -> Vec<usize> {
        (0..self.status.len())
            .filter(|&index| self.status[index].is_unmerged())
            .collect()
    }

    /// Reports whether HEAD points at a commit without a checked-out local branch.
    pub fn is_detached(&self) -> bool {
        !self.head_id.is_empty() && self.checked_out_branch().is_none()
    }

    /// Names the active operations for display, such as `merge` or `rebase`.
    pub fn operation_label(&self) -> Option<String> {
        (!self.operations.is_empty()).then(|| {
            self.operations
                .iter()
                .map(|op| match op.as_str() {
                    "sequencer" => "multi-commit sequence",
                    op => op,
                })
                .collect::<Vec<_>>()
                .join(" and ")
        })
    }

    /// The merge, rebase, cherry-pick or revert that Git stopped part-way,
    /// which Continue, Skip and Abort act on.
    pub fn sequence(&self) -> Option<Sequence> {
        Sequence::from_operations(&self.operations)
    }

    /// Describes how to finish or abandon the active operation.
    pub fn operation_recovery(&self) -> String {
        let Some(sequence) = self.sequence() else {
            if self.operations.is_empty() {
                return "Edit each conflicted file, mark it resolved, then commit the result."
                    .into();
            }
            return "Finish or abort the operation in a terminal, then refresh GitCrab.".into();
        };
        let name = sequence.name();
        let skip = if sequence.can_skip() {
            ", Skip drops this commit"
        } else {
            ""
        };
        format!(
            "Edit each conflicted file and mark it resolved. Then Continue {}{skip}, or Abort returns to the state before the {name}.",
            sequence.continue_effect()
        )
    }

    /// Explains why an ordinary commit is unavailable, without checking the summary.
    pub fn commit_blocker(&self) -> Option<String> {
        let conflicts = self.conflict_count();
        if conflicts > 0 {
            return Some(format!(
                "Resolve {conflicts} conflicted {} before committing.",
                if conflicts == 1 { "file" } else { "files" }
            ));
        }
        if let Some(op) = self.operation_label() {
            return Some(format!(
                "A {op} is in progress. Finish it with Continue, or Abort it."
            ));
        }
        if self.is_detached() {
            return Some(
                "HEAD is detached. Create a branch first so the new commit stays reachable.".into(),
            );
        }
        if self.side(true).is_empty() {
            return Some("Stage changes to create a commit.".into());
        }
        None
    }

    /// Explains why a new local branch called `name` cannot be created next to
    /// the loaded branches. Git refs are files, so `a` and `a/b` cannot both
    /// exist.
    pub fn branch_name_conflict(&self, name: &str) -> Option<String> {
        self.branches.iter().find_map(|b| {
            if b.name == name {
                Some(format!("A branch named {name} already exists."))
            } else if name.starts_with(&format!("{}/", b.name))
                || b.name.starts_with(&format!("{name}/"))
            {
                Some(format!(
                    "{name} cannot sit beside the existing branch {}. Choose another name.",
                    b.name
                ))
            } else {
                None
            }
        })
    }

    /// Returns the short local branch name from the reviewed symbolic HEAD, if present.
    pub fn checked_out_branch(&self) -> Option<&str> {
        self.reviewed
            .as_ref()?
            .symbolic_head
            .strip_prefix("refs/heads/")
    }

    /// The commit an operation target naming a branch points at: a local
    /// branch by name, else a remote-tracking branch by short or full name.
    /// `None` for other targets, such as commit IDs.
    pub fn branch_target_tip(&self, target: &str) -> Option<&str> {
        let local = target.strip_prefix("refs/heads/").unwrap_or(target);
        self.branches
            .iter()
            .find(|branch| branch.name == local)
            .map(|branch| branch.tip.as_str())
            .or_else(|| {
                self.remote_refs
                    .iter()
                    .find(|remote| remote.label == target || remote.full_name == target)
                    .map(|remote| remote.tip.as_str())
            })
    }

    /// Returns a ref's object ID if it points to a commit in the loaded history.
    /// Use the short name for a local branch and the full ref name for a remote
    /// or tag.
    pub fn loaded_ref_tip(&self, kind: ReferenceKind, name: &str) -> Option<&str> {
        let tip = match kind {
            ReferenceKind::LocalBranch => &self.branches.iter().find(|b| b.name == name)?.tip,
            ReferenceKind::Remote | ReferenceKind::Tag => {
                let refs = if kind == ReferenceKind::Remote {
                    &self.remote_refs
                } else {
                    &self.tags
                };
                &refs
                    .iter()
                    .find(|reference| reference.full_name == name)?
                    .tip
            }
        };
        self.commits
            .iter()
            .any(|c| &c.id == tip)
            .then_some(tip.as_str())
    }

    /// Formats HEAD for display, with a label for unborn or detached HEAD.
    pub fn head_label(&self) -> String {
        if self.head_id.is_empty() {
            format!("{} (no commits)", self.head)
        } else if self.checked_out_branch().is_none() {
            format!("Detached HEAD at {}", self.head)
        } else {
            self.head.clone()
        }
    }
}

/// One hunk from a Git patch, with the headers needed to apply it independently.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Hunk {
    /// Hunk header with trailing whitespace removed.
    pub title: String,
    /// Self-contained patch with original file headers and one hunk.
    pub patch: String,
}

/// A machine-format Git patch and the source identity for a rename or copy.
pub struct DiffComparison {
    pub patch: String,
    /// Source path in the before side of this comparison.
    pub original_path: Option<String>,
}

impl Hunk {
    /// Returns a reason to use a whole-file action based on the patch headers.
    pub fn blocked_reason(&self) -> Option<&'static str> {
        header_blocked_reason(&self.patch)
    }
}

fn header_blocked_reason(patch: &str) -> Option<&'static str> {
    let mut header = patch.lines();
    if !header
        .next()
        .is_some_and(|line| line.starts_with("diff --git "))
    {
        return Some("Use a whole-file action for unsupported patch formats.");
    }
    for line in header.take_while(|line| !line.starts_with("@@ ")) {
        if let Some(index) = line.strip_prefix("index ") {
            if let Some((_, mode)) = index.split_once(' ') {
                if !matches!(mode, "100644" | "100755") {
                    return Some("Use a whole-file action for symlinks and submodules.");
                }
            }
        } else if !line.starts_with("--- ")
            && !line.starts_with("+++ ")
            && !matches!(line, "new file mode 100644" | "new file mode 100755")
        {
            return Some("Use a whole-file action for changes to file metadata.");
        }
    }
    None
}

/// Git's revision-parsing rules (`ref_rev_parse_rules`) as prefix and suffix.
const REV_PARSE_RULES: [(&str, &str); 6] = [
    ("", ""),
    ("refs/", ""),
    ("refs/tags/", ""),
    ("refs/heads/", ""),
    ("refs/remotes/", ""),
    ("refs/remotes/", "/HEAD"),
];

/// Git's `shorten_unambiguous_ref` in strict mode: try the most specific rule
/// first, and keep a short name only if no other rule turns it back into a
/// ref for which `exists` holds. Falls back to the full name.
pub fn shorten_ref(full: &str, exists: &dyn Fn(&str) -> bool) -> String {
    for (i, (prefix, suffix)) in REV_PARSE_RULES.iter().enumerate().skip(1).rev() {
        let Some(short) = full
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(suffix))
            .filter(|short| !short.is_empty())
        else {
            continue;
        };
        let ambiguous = REV_PARSE_RULES
            .iter()
            .enumerate()
            .any(|(j, (p, s))| j != i && exists(&format!("{p}{short}{s}")));
        if !ambiguous {
            return short.to_owned();
        }
    }
    full.to_owned()
}

pub fn parse_status(data: &[u8]) -> Vec<FileChange> {
    let mut out = Vec::new();
    let mut records = data.split(|b| *b == 0);
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let index = record[0] as char;
        let worktree = record[1] as char;
        let original_path = if index == 'R' || index == 'C' || worktree == 'R' || worktree == 'C' {
            records.next()
        } else {
            None
        };
        out.push(file_change(&record[3..], original_path, index, worktree));
    }
    out
}

pub fn file_change(
    path: &[u8],
    original: Option<&[u8]>,
    index: char,
    worktree: char,
) -> FileChange {
    let display = |raw: &[u8]| match std::str::from_utf8(raw) {
        Ok(path) => path.to_owned(),
        Err(_) => format!(
            "b\"{}\"",
            raw.iter()
                .flat_map(|byte| std::ascii::escape_default(*byte))
                .map(char::from)
                .collect::<String>()
        ),
    };
    FileChange {
        path: display(path),
        original_path: original.map(display),
        index,
        worktree,
        unsupported_path: std::str::from_utf8(path).is_err()
            || original.is_some_and(|raw| std::str::from_utf8(raw).is_err()),
    }
}

/// Splits a unified Git patch into individual hunks in patch order.
/// Each hunk includes the original headers for its file.
pub fn split_hunks(diff: &str) -> Vec<Hunk> {
    hunk_ranges(diff)
        .iter()
        .map(|hunk| hunk.materialize(diff))
        .collect()
}

pub struct HunkRange {
    header: std::ops::Range<usize>,
    body: std::ops::Range<usize>,
}

impl HunkRange {
    pub fn title<'a>(&self, patch: &'a str) -> &'a str {
        patch[self.body.clone()]
            .lines()
            .next()
            .unwrap_or_default()
            .trim_end()
    }
    pub fn blocked_reason(&self, patch: &str) -> Option<&'static str> {
        header_blocked_reason(&patch[self.header.clone()])
    }
    pub fn materialize(&self, patch: &str) -> Hunk {
        Hunk {
            title: self.title(patch).to_owned(),
            patch: format!(
                "{}{}",
                &patch[self.header.clone()],
                &patch[self.body.clone()]
            ),
        }
    }
}

pub fn hunk_ranges(diff: &str) -> Vec<HunkRange> {
    let mut ranges = Vec::new();
    let (mut offset, mut file_start) = (0, 0);
    let mut header_end = None;
    let mut body_start = None;
    for line in diff.split_inclusive('\n') {
        let file = line.starts_with("diff --git ");
        let hunk = line.starts_with("@@ ");
        if file || hunk {
            if let (Some(header_end), Some(start)) = (header_end, body_start.take()) {
                ranges.push(HunkRange {
                    header: file_start..header_end,
                    body: start..offset,
                });
            }
            if file {
                file_start = offset;
                header_end = None;
            } else {
                header_end.get_or_insert(offset);
                body_start = Some(offset);
            }
        }
        offset += line.len();
    }
    if let (Some(header_end), Some(start)) = (header_end, body_start) {
        ranges.push(HunkRange {
            header: file_start..header_end,
            body: start..offset,
        });
    }
    ranges
}
