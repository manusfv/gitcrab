//! Display rules shared by the desktop views: paths, labels, search position and toolbar fit.
//!
//! These functions are independent of the UI toolkit so their policy is unit tested.

use crate::design_tokens::geometry;

/// Formats a filesystem path for people to read.
///
/// Windows verbatim prefixes (`\\?\C:\…` and `\\?\UNC\server\share\…`) are
/// replaced by their ordinary forms. Other paths are returned unchanged.
pub fn display_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        path.to_owned()
    }
}

/// [`display_path`] for a filesystem path that may not be valid Unicode.
pub fn display_location(path: &std::path::Path) -> String {
    display_path(&path.display().to_string())
}

/// Characters of an object id shown where a commit is named in passing.
pub const SHORT_ID_LEN: usize = 8;

/// The abbreviated form of an object id, as in `git log --oneline`.
pub fn short_id(id: &str) -> &str {
    id.char_indices()
        .nth(SHORT_ID_LEN)
        .map_or(id, |(end, _)| &id[..end])
}

fn components(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).filter(|c| !c.is_empty()).collect()
}

fn separator(path: &str) -> char {
    if path.contains('\\') {
        '\\'
    } else {
        '/'
    }
}

/// Whether a path follows Windows rules: a drive letter, a UNC prefix or
/// backslashes. Windows paths compare without regard to case.
fn is_windows_path(path: &str) -> bool {
    path.contains('\\') || path.as_bytes().get(1) == Some(&b':')
}

/// The folders of a readable path in the form used to compare it.
fn path_key(path: &str) -> Vec<String> {
    let windows = is_windows_path(path);
    components(path)
        .into_iter()
        .map(|c| {
            if windows {
                c.to_lowercase()
            } else {
                c.to_owned()
            }
        })
        .collect()
}

/// Whether two paths name the same folder, ignoring verbatim prefixes,
/// separator style, trailing separators and, for Windows paths, case.
pub fn same_path(a: &str, b: &str) -> bool {
    path_key(&display_path(a)) == path_key(&display_path(b))
}

/// Puts paths in readable form and drops later spellings of a path already listed.
pub fn unique_paths(paths: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen: Vec<Vec<String>> = vec![];
    paths
        .into_iter()
        .map(|path| display_path(&path))
        .filter(|path| {
            let key = path_key(path);
            let new = !seen.contains(&key);
            if new {
                seen.push(key);
            }
            new
        })
        .collect()
}

/// Names a repository path in a list where several entries may share a folder name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathLabel {
    /// The final folder name.
    pub name: String,
    /// The shortest trailing run of folders that no other listed path shares.
    pub suffix: String,
    /// The readable parent folder, for a secondary line or tooltip.
    pub location: String,
}

/// Labels each path with its shortest unique trailing suffix.
///
/// Paths are compared as [`same_path`] does, so spellings of one folder, such
/// as the same repository open in two tabs, share its folder name. A path that
/// is a trailing part of another, longer path keeps its full display path.
pub fn path_labels(paths: &[String]) -> Vec<PathLabel> {
    let shown: Vec<String> = paths.iter().map(|p| display_path(p)).collect();
    let parts: Vec<Vec<&str>> = shown.iter().map(|p| components(p)).collect();
    let keys: Vec<Vec<String>> = shown.iter().map(|p| path_key(p)).collect();
    shown
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let own = &parts[index];
            let key = &keys[index];
            let name = own.last().copied().unwrap_or(path.as_str()).to_owned();
            let start = |k: usize| own.len().saturating_sub(k);
            let suffix = (1..=own.len().max(1))
                .find(|&k| {
                    keys.iter()
                        .filter(|other| *other != key)
                        .all(|other| !other.ends_with(&key[start(k)..]))
                })
                .map(|k| own[start(k)..].join(&separator(path).to_string()))
                .unwrap_or_else(|| path.clone());
            let location = path
                .trim_end_matches(['/', '\\'])
                .rsplit_once(['/', '\\'])
                .map(|(parent, _)| {
                    if parent.is_empty() || parent.ends_with(':') {
                        format!("{parent}{}", separator(path))
                    } else {
                        parent.to_owned()
                    }
                })
                .unwrap_or_default();
            PathLabel {
                name,
                suffix,
                location,
            }
        })
        .collect()
}

/// Describes the history search result count and the selected match's position.
pub fn search_status(
    matches: &[String],
    selected: Option<&str>,
    loaded: usize,
    more_available: bool,
) -> String {
    let scope = format!(
        "{loaded} loaded commit{}",
        if loaded == 1 { "" } else { "s" }
    );
    if matches.is_empty() {
        return if more_available {
            format!("No matches in {scope}. Search older commits to keep looking.")
        } else {
            format!("No matches in {scope}")
        };
    }
    match selected.and_then(|id| matches.iter().position(|m| m == id)) {
        Some(index) => format!("{} of {} in {scope}", index + 1, matches.len()),
        None => format!(
            "{} match{} in {scope}",
            matches.len(),
            if matches.len() == 1 { "" } else { "es" }
        ),
    }
}

/// The kind of a decoration printed next to a commit in history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecorationKind {
    /// The checked-out branch, printed as `HEAD -> name`.
    HeadBranch,
    /// A detached `HEAD`.
    Head,
    Tag,
    /// A local or remote-tracking branch.
    Branch,
}

/// Splits a `git log --decorate` label into its kind and name.
pub fn classify_decoration(label: &str) -> (DecorationKind, &str) {
    if let Some(name) = label.strip_prefix("HEAD -> ") {
        (DecorationKind::HeadBranch, name)
    } else if label == "HEAD" {
        (DecorationKind::Head, label)
    } else if let Some(name) = label.strip_prefix("tag: ") {
        (DecorationKind::Tag, name)
    } else {
        (DecorationKind::Branch, label)
    }
}

/// Where the toolbar's action group sits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToolbarActions {
    /// Centred on the whole toolbar. `left` is the offset of a box `width`
    /// wide, measured from the toolbar's left edge, that holds the group.
    Centered { left: f32, width: f32 },
    /// After the branch control, with labels under the icons.
    Inline,
    /// After the branch control, icons only. Labels move into tooltips.
    Compact,
    /// On a row of their own below the selectors and search, with labels.
    Wrapped,
}

/// Widths for the repository toolbar at a viewport width and text scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolbarLayout {
    /// Width of the repository and branch selectors.
    pub selector_width: f32,
    pub search_width: f32,
    pub actions: ToolbarActions,
}

impl ToolbarLayout {
    /// Whether action buttons show their label under the icon.
    pub fn labels(&self) -> bool {
        !matches!(self.actions, ToolbarActions::Compact)
    }
}

/// Horizontal padding on each side of the toolbar.
const TOOLBAR_PADDING: f32 = 12.;
/// Gap between toolbar items.
const TOOLBAR_GAP: f32 = 4.;
/// A separator with its margins.
const TOOLBAR_SEPARATOR: f32 = 17.;
/// Clear space kept between the centred actions and the controls beside them.
const TOOLBAR_CLEARANCE: f32 = 16.;
/// Width of the menu caret beside the Pull action.
pub const TOOLBAR_CARET: f32 = 12.;
/// Search field width, and the least it gets as a share of a narrow toolbar.
const SEARCH_WIDTH: f32 = 200.;
const SEARCH_MIN_WIDTH: f32 = 140.;
const SEARCH_MAX_SHARE: f32 = 0.25;
/// Width of each selector when there is room, and the least it may shrink to.
const SELECTOR_WIDTH: f32 = 176.;
const SELECTOR_MIN_WIDTH: f32 = 112.;

/// Width of a group of `commands` toolbar actions, one of which has a menu caret.
pub fn toolbar_actions_width(scale: f32, commands: usize, labels: bool) -> f32 {
    if commands == 0 {
        return 0.;
    }
    let scale = scale.max(0.5);
    let command = if labels {
        geometry::SIZE_REPOSITORY_TOOLBAR_COMMAND_WIDTH
    } else {
        geometry::SIZE_TARGET_REGULAR
    } * scale;
    let caret = TOOLBAR_CARET * scale;
    commands as f32 * command + (commands - 1) as f32 * TOOLBAR_GAP + caret
}

/// Fits the toolbar to `width`: repository and branch selectors on the left,
/// search on the right and `commands` actions centred when they fit.
///
/// `scale` is the text size relative to the 13-pixel default. `right_extra`
/// is the width of anything shown beside the search field, such as match
/// navigation. When the actions cannot be centred they follow the branch
/// selector, then drop their labels, then move to a row of their own, so every
/// action stays visible.
pub fn toolbar_layout(width: f32, scale: f32, commands: usize, right_extra: f32) -> ToolbarLayout {
    let scale = scale.max(0.5);
    let search_width =
        (SEARCH_WIDTH * scale.min(1.5)).min((width * SEARCH_MAX_SHARE).max(SEARCH_MIN_WIDTH));
    let right = search_width + right_extra.max(0.);
    let preferred = SELECTOR_WIDTH * scale;
    let minimum = (SELECTOR_MIN_WIDTH * scale.min(1.6)).min(preferred);
    // Two selectors, the gap between them and the separator after the
    // repository selector.
    let selectors_fixed = TOOLBAR_SEPARATOR + 2. * TOOLBAR_GAP;
    let selectors_for = |room: f32| (room - selectors_fixed) / 2.;

    let labelled = toolbar_actions_width(scale, commands, true);
    let left = (width - labelled) / 2.;
    let side = left - TOOLBAR_CLEARANCE - TOOLBAR_PADDING;
    if side >= right && selectors_for(side) >= minimum {
        return ToolbarLayout {
            selector_width: selectors_for(side).min(preferred),
            search_width,
            actions: ToolbarActions::Centered {
                left,
                width: labelled,
            },
        };
    }

    // Room between the toolbar padding and the search, less the spacer gap.
    let row = (width - 2. * TOOLBAR_PADDING - right - TOOLBAR_GAP).max(0.);
    for (labels, actions) in [
        (true, ToolbarActions::Inline),
        (false, ToolbarActions::Compact),
    ] {
        let group = toolbar_actions_width(scale, commands, labels) + TOOLBAR_SEPARATOR;
        let selector_width = selectors_for(row - group);
        if selector_width >= minimum {
            return ToolbarLayout {
                selector_width: selector_width.min(preferred),
                search_width,
                actions,
            };
        }
    }
    ToolbarLayout {
        selector_width: selectors_for(row).clamp(minimum, preferred),
        search_width,
        actions: ToolbarActions::Wrapped,
    }
}

/// Indices of `names` that match `query`, ignoring case.
///
/// Names starting with the query come first, then names containing it, each
/// in their original order. An empty query matches every name.
pub fn filter_branches<S: AsRef<str>>(names: &[S], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..names.len()).collect();
    }
    let mut prefix = vec![];
    let mut inner = vec![];
    for (index, name) in names.iter().enumerate() {
        let name = name.as_ref().to_lowercase();
        if name.starts_with(&query) {
            prefix.push(index);
        } else if name.contains(&query) {
            inner.push(index);
        }
    }
    prefix.extend(inner);
    prefix
}

/// Decides whether a split comparison stays readable in a pane `width` wide.
///
/// Each side needs room for its gutter (`side_chrome`) and the longest line,
/// counted between 20 and [`SPLIT_MIN_COLUMNS`] characters of `char_width`.
/// Longer lines wrap within their side. When this is false the view shows
/// Unified instead.
pub fn split_fits(width: f32, columns: usize, char_width: f32, side_chrome: f32) -> bool {
    let side = width / 2. - side_chrome;
    side >= columns.clamp(20, SPLIT_MIN_COLUMNS) as f32 * char_width
}

/// Characters each Split side must show before the diff falls back to
/// Unified. Kept low so a full-screen 1080p window with display scaling (1280
/// or 1536 logical pixels) still fits Split beside the details pane.
pub const SPLIT_MIN_COLUMNS: usize = 40;

/// Heading for a failed request, naming the action and what happened.
pub fn failure_heading(action: &str, kind: crate::git::FailureKind) -> String {
    use crate::git::FailureKind;
    match kind {
        FailureKind::Rejected => format!("{action} was not completed"),
        FailureKind::GitFailure => format!("{action} failed in Git"),
        FailureKind::Conflict => format!("{action} stopped with conflicts"),
        FailureKind::Inspect => format!("{action} needs inspection"),
        FailureKind::ConflictMarkers => format!("{action}: conflict markers remain"),
    }
}

pub const SECS_PER_MINUTE: u64 = 60;
pub const SECS_PER_HOUR: u64 = 60 * SECS_PER_MINUTE;
pub const SECS_PER_DAY: u64 = 24 * SECS_PER_HOUR;

/// Short relative time for activity records, from elapsed seconds.
pub fn elapsed_label(seconds: u64) -> String {
    match seconds {
        0..=9 => "just now".into(),
        10..SECS_PER_MINUTE => format!("{seconds} s ago"),
        SECS_PER_MINUTE..SECS_PER_HOUR => format!("{} min ago", seconds / SECS_PER_MINUTE),
        _ => format!("{} h ago", seconds / SECS_PER_HOUR),
    }
}

/// A reference printed beside a commit, with the ref it previews when activated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoration {
    pub kind: DecorationKind,
    pub name: String,
    /// The loaded reference this label resolves to, if any.
    pub target: Option<(crate::git::ReferenceKind, String)>,
}

impl Decoration {
    /// Names the reference with its type, for tooltips and the `+N` list.
    pub fn label(&self) -> String {
        use crate::git::ReferenceKind;
        match (self.kind, self.target.as_ref().map(|t| t.0)) {
            (DecorationKind::HeadBranch, _) => format!("{} (checked out)", self.name),
            (DecorationKind::Head, _) => "Detached HEAD".into(),
            (DecorationKind::Tag, _) => format!("Tag {}", self.name),
            (_, Some(ReferenceKind::Remote)) => format!("Remote {}", self.name),
            _ => format!("Branch {}", self.name),
        }
    }
}

/// Resolves a commit's `--decorate` labels against the loaded references.
/// A plain branch name prefers a local branch over a remote ref.
///
/// The history reader marks the HEAD commit with a bare `HEAD` label. When a
/// branch is checked out, that label becomes the branch as
/// [`DecorationKind::HeadBranch`], in `HEAD`'s place, and the branch's own
/// plain label is dropped. A detached `HEAD` stays [`DecorationKind::Head`].
pub fn resolve_decorations(
    snapshot: &crate::git::RepoSnapshot,
    refs: &[String],
) -> Vec<Decoration> {
    use crate::git::ReferenceKind;
    let checked_out = snapshot
        .checked_out_branch()
        .filter(|_| refs.iter().any(|label| label == "HEAD"));
    // Git shortens a branch to `heads/name` when a tag shares its name.
    let is_checked_out = |label: &str| {
        checked_out
            .is_some_and(|branch| label == branch || label.strip_prefix("heads/") == Some(branch))
    };
    refs.iter()
        .filter(|label| !is_checked_out(label))
        .map(|label| match checked_out {
            Some(branch) if label == "HEAD" => Decoration {
                kind: DecorationKind::HeadBranch,
                name: branch.to_owned(),
                target: Some((ReferenceKind::LocalBranch, branch.to_owned())),
            },
            _ => resolve_decoration(snapshot, label),
        })
        .collect()
}

fn resolve_decoration(snapshot: &crate::git::RepoSnapshot, label: &str) -> Decoration {
    use crate::git::ReferenceKind;
    let (mut kind, name) = classify_decoration(label);
    // The history reader prints tags by their short name, without `tag: `.
    if kind == DecorationKind::Branch
        && !snapshot.branches.iter().any(|branch| branch.name == name)
        && !snapshot
            .remote_refs
            .iter()
            .any(|remote| remote.label == name)
        && snapshot.tags.iter().any(|tag| tag.label == name)
    {
        kind = DecorationKind::Tag;
    }
    let target = match kind {
        DecorationKind::Head => None,
        DecorationKind::HeadBranch => Some((ReferenceKind::LocalBranch, name.to_owned())),
        DecorationKind::Tag => snapshot
            .tags
            .iter()
            .find(|tag| tag.label == name)
            .map(|tag| (ReferenceKind::Tag, tag.full_name.clone())),
        DecorationKind::Branch => {
            if snapshot.branches.iter().any(|branch| branch.name == name) {
                Some((ReferenceKind::LocalBranch, name.to_owned()))
            } else {
                snapshot
                    .remote_refs
                    .iter()
                    .find(|remote| remote.label == name)
                    .map(|remote| (ReferenceKind::Remote, remote.full_name.clone()))
            }
        }
    };
    Decoration {
        kind,
        name: name.to_owned(),
        target,
    }
}

/// Summary length Git tools display without truncating. Longer summaries are
/// still valid.
pub const SUMMARY_RECOMMENDED_CHARS: usize = 72;

/// Characters left before `summary` passes [`SUMMARY_RECOMMENDED_CHARS`],
/// negative once it does.
pub fn summary_chars_left(summary: &str) -> isize {
    SUMMARY_RECOMMENDED_CHARS as isize - summary.chars().count() as isize
}

/// Joins a commit summary and body the way Git stores the message.
pub fn compose_message(summary: &str, body: &str) -> String {
    if body.is_empty() {
        summary.to_string()
    } else {
        format!("{summary}\n\n{body}")
    }
}

/// Splits a commit message into its summary line and body, the inverse of
/// [`compose_message`].
pub fn split_message(message: &str) -> (&str, &str) {
    match message.split_once('\n') {
        Some((summary, body)) => (summary, body.strip_prefix('\n').unwrap_or(body)),
        None => (message, ""),
    }
}

/// Shows a Git ISO-like date as `DD/MM/YYYY @ HH:MM`. Anything else is
/// returned unchanged.
pub fn display_date(date: &str) -> String {
    match (
        date.get(5..7),
        date.get(8..10),
        date.get(..4),
        date.get(11..16),
    ) {
        (Some(month), Some(day), Some(year), Some(time))
            if date.is_ascii() && date.get(4..5) == Some("-") && date.get(7..8) == Some("-") =>
        {
            format!("{day}/{month}/{year} @ {time}")
        }
        _ => date.to_string(),
    }
}

/// Names a merge parent's position in Git's parent order.
pub fn ordinal(index: usize) -> String {
    match index {
        0 => "First".into(),
        1 => "Second".into(),
        2 => "Third".into(),
        n => format!("Parent {}", n + 1),
    }
}

/// Joins a commit body's hard-wrapped lines so the text reflows to its pane.
///
/// Blank lines stay as paragraph breaks. List items, indented lines and
/// trailer lines such as `Co-Authored-By:` keep their own line.
pub fn reflow_body(body: &str) -> Vec<String> {
    fn starts_own_line(line: &str) -> bool {
        let trimmed = line.trim_start();
        line.starts_with([' ', '\t'])
            || trimmed.starts_with(['-', '*', '+', '>', '|'])
            || trimmed
                .split_once(". ")
                .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            || trimmed
                .split_once(": ")
                .is_some_and(|(key, _)| !key.is_empty() && !key.contains(' ') && key.contains('-'))
    }
    let mut lines: Vec<String> = vec![];
    let mut joinable = false;
    for line in body.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            lines.push(String::new());
            joinable = false;
        } else if joinable && !starts_own_line(line) {
            let last = lines.last_mut().unwrap();
            last.push(' ');
            last.push_str(line.trim_start());
        } else {
            lines.push(line.to_owned());
            joinable = true;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_bodies_reflow_paragraphs_but_keep_lists_and_trailers() {
        let body = "History rows follow the subject\njoined onto one line.\n\n- first item\n  continued\n- second\n1. numbered\n\nCo-Authored-By: A <a@b>\nSigned-off-by: B <b@c>";
        assert_eq!(
            reflow_body(body),
            [
                "History rows follow the subject joined onto one line.",
                "",
                "- first item",
                "  continued",
                "- second",
                "1. numbered",
                "",
                "Co-Authored-By: A <a@b>",
                "Signed-off-by: B <b@c>",
            ]
        );
    }

    #[test]
    fn commit_dates_use_day_month_year_and_24_hour_time() {
        assert_eq!(
            display_date("2026-11-05T00:04:59-03:00"),
            "05/11/2026 @ 00:04"
        );
        assert_eq!(
            display_date("2026-01-02T23:07:00+00:00"),
            "02/01/2026 @ 23:07"
        );
        for value in ["", "unknown", "not a timestamp at all", "日付なし"] {
            assert_eq!(display_date(value), value);
        }
    }

    #[test]
    fn commit_messages_split_and_compose_symmetrically() {
        assert_eq!(compose_message("Fix", ""), "Fix");
        assert_eq!(compose_message("Fix", "Details"), "Fix\n\nDetails");
        assert_eq!(split_message("Fix\n\nDetails"), ("Fix", "Details"));
        assert_eq!(split_message("Fix\nDetails"), ("Fix", "Details"));
        assert_eq!(split_message("Fix"), ("Fix", ""));
    }

    #[test]
    fn merge_parents_are_named_in_order() {
        assert_eq!(ordinal(0), "First");
        assert_eq!(ordinal(2), "Third");
        assert_eq!(ordinal(4), "Parent 5");
    }

    #[test]
    fn head_label_becomes_the_checked_out_branch() {
        use crate::git::{LocalBranch, ReferenceKind, ReferenceTip, RepoSnapshot, WriteIdentity};
        let branch = |name: &str| LocalBranch {
            name: name.into(),
            tip: String::new(),
        };
        let mut snapshot = RepoSnapshot {
            branches: vec![branch("feature/a"), branch("feature/b"), branch("main")],
            tags: vec![ReferenceTip {
                label: "v1".into(),
                full_name: "refs/tags/v1".into(),
                tip: String::new(),
            }],
            reviewed: Some(WriteIdentity {
                head: "abc".into(),
                symbolic_head: "refs/heads/main".into(),
                index: Vec::new(),
            }),
            ..Default::default()
        };
        // The history reader lists `HEAD` first, then refs alphabetically.
        let refs = ["HEAD", "feature/a", "feature/b", "main", "v1"].map(String::from);
        let resolved = resolve_decorations(&snapshot, &refs);
        let names: Vec<_> = resolved.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["main", "feature/a", "feature/b", "v1"]);
        assert_eq!(resolved[3].label(), "Tag v1");
        assert_eq!(resolved[0].kind, DecorationKind::HeadBranch);
        assert_eq!(
            resolved[0].target,
            Some((ReferenceKind::LocalBranch, "main".into()))
        );
        assert_eq!(resolved[0].label(), "main (checked out)");

        // A branch disambiguated from a same-named tag is still merged.
        let refs = ["HEAD", "heads/main", "tag: main"].map(String::from);
        let names: Vec<_> = resolve_decorations(&snapshot, &refs)
            .into_iter()
            .map(|d| (d.kind, d.name))
            .collect();
        assert_eq!(
            names,
            [
                (DecorationKind::HeadBranch, "main".into()),
                (DecorationKind::Tag, "main".into())
            ]
        );

        // Commits other than HEAD keep the branch as a plain label.
        let other = resolve_decorations(&snapshot, &["main".into()]);
        assert_eq!(other[0].kind, DecorationKind::Branch);

        // Detached HEAD stays a bare HEAD label.
        snapshot.reviewed.as_mut().unwrap().symbolic_head.clear();
        let detached = resolve_decorations(&snapshot, &refs[..1]);
        assert_eq!(detached[0].kind, DecorationKind::Head);
        assert_eq!(detached[0].label(), "Detached HEAD");
    }

    #[test]
    fn decorations_resolve_to_loaded_refs_preferring_local_branches() {
        use crate::git::{LocalBranch, ReferenceKind, ReferenceTip, RepoSnapshot};
        let tip = |label: &str, full: &str| ReferenceTip {
            label: label.into(),
            full_name: full.into(),
            tip: String::new(),
        };
        let snapshot = RepoSnapshot {
            branches: vec![LocalBranch {
                name: "main".into(),
                tip: String::new(),
            }],
            remote_refs: vec![tip("origin/main", "refs/remotes/origin/main")],
            tags: vec![tip("v1", "refs/tags/v1")],
            ..Default::default()
        };
        let refs = ["HEAD -> main", "origin/main", "tag: v1", "gone"].map(String::from);
        let resolved = resolve_decorations(&snapshot, &refs);
        assert_eq!(
            resolved[0].target,
            Some((ReferenceKind::LocalBranch, "main".into()))
        );
        assert_eq!(resolved[0].label(), "main (checked out)");
        assert_eq!(resolved[1].label(), "Remote origin/main");
        assert_eq!(
            resolved[2].target,
            Some((ReferenceKind::Tag, "refs/tags/v1".into()))
        );
        assert_eq!(resolved[3].target, None);
        assert_eq!(resolved[3].label(), "Branch gone");
    }

    #[test]
    fn split_falls_back_when_a_side_cannot_show_its_lines() {
        // 7.8 px characters at 12 px code size, 70 px of gutter per side.
        assert!(split_fits(1400., 60, 7.8, 70.));
        assert!(!split_fits(700., 60, 7.8, 70.));
        // Very long lines count as 40 characters; they wrap within each side.
        assert!(split_fits(1400., 4000, 7.8, 70.));
        assert!(split_fits(560., 5, 7.8, 70.));
        // Full-screen 1080p at 150% scaling: 1280 logical pixels minus the
        // 44 px rail and the 360 px details pane leaves 876 px for the diff.
        assert!(split_fits(876., 120, 7.8, 86.));
        assert!(!split_fits(600., 120, 7.8, 86.));
    }

    #[test]
    fn failure_headings_and_activity_times_are_specific() {
        use crate::git::FailureKind;
        assert_eq!(
            failure_heading("Merge", FailureKind::Conflict),
            "Merge stopped with conflicts"
        );
        assert_eq!(
            failure_heading("Stage file", FailureKind::Rejected),
            "Stage file was not completed"
        );
        assert_eq!(elapsed_label(3), "just now");
        assert_eq!(elapsed_label(42), "42 s ago");
        assert_eq!(elapsed_label(130), "2 min ago");
        assert_eq!(elapsed_label(7300), "2 h ago");
    }

    #[test]
    fn verbatim_windows_paths_read_normally() {
        assert_eq!(display_path(r"\\?\C:\src\gitcrab"), r"C:\src\gitcrab");
        assert_eq!(
            display_path(r"\\?\UNC\host\share\repo"),
            r"\\host\share\repo"
        );
        assert_eq!(display_path("/home/me/repo"), "/home/me/repo");
        assert_eq!(
            display_location(std::path::Path::new(r"\\?\C:\src\gitcrab\.git")),
            r"C:\src\gitcrab\.git"
        );
    }

    #[test]
    fn shortest_unique_suffix_distinguishes_same_named_clones() {
        let paths = vec![
            r"C:\work\client\gitcrab".to_string(),
            r"C:\personal\gitcrab".to_string(),
            r"C:\work\other\gitcrab".to_string(),
            "/srv/tools".to_string(),
        ];
        let labels = path_labels(&paths);
        assert_eq!(labels[0].suffix, r"client\gitcrab");
        assert_eq!(labels[1].suffix, r"personal\gitcrab");
        assert_eq!(labels[2].suffix, r"other\gitcrab");
        assert_eq!(labels[3].suffix, "tools");
        assert_eq!(labels[0].name, "gitcrab");
        assert_eq!(labels[0].location, r"C:\work\client");
        assert_eq!(labels[3].location, "/srv");
    }

    #[test]
    fn nested_and_duplicate_paths_still_get_labels() {
        let paths = vec![
            "/a/repo".to_string(),
            "/b/a/repo".to_string(),
            "/a/repo".to_string(),
            r"\\?\D:\repo".to_string(),
        ];
        let labels = path_labels(&paths);
        assert_eq!(labels[1].suffix, "b/a/repo");
        assert_eq!(labels[0].suffix, "/a/repo");
        assert_eq!(labels[3].suffix, r"D:\repo");
        assert_eq!(labels[3].location, r"D:\");
    }

    #[test]
    fn spellings_of_one_folder_share_its_name() {
        let paths = vec![
            r"C:\Temp\audit\reference".to_string(),
            r"\\?\C:\Temp\audit\reference".to_string(),
            "C:/Temp/audit/reference/".to_string(),
            r"c:\temp\AUDIT\reference".to_string(),
            r"C:\Users\me\gitcrab".to_string(),
            r"C:\Users\me\work\gitcrab".to_string(),
        ];
        let labels = path_labels(&paths);
        for label in &labels[..4] {
            assert_eq!(label.suffix, "reference");
        }
        assert_eq!(labels[4].suffix, r"me\gitcrab");
        assert_eq!(labels[5].suffix, r"work\gitcrab");
        // Unix paths stay case-sensitive.
        let labels = path_labels(&["/a/Repo".to_string(), "/b/repo".to_string()]);
        assert_eq!(labels[0].suffix, "Repo");
    }

    #[test]
    fn unique_paths_keeps_the_first_spelling_in_readable_form() {
        let paths = [
            r"\\?\C:\Projects\gitcrab",
            r"C:\Temp\reference",
            r"C:\projects\GitCrab\",
            "C:/Temp/reference",
            "/home/me/Repo",
            "/home/me/repo",
        ]
        .map(String::from);
        assert_eq!(
            unique_paths(paths),
            [
                r"C:\Projects\gitcrab",
                r"C:\Temp\reference",
                "/home/me/Repo",
                "/home/me/repo"
            ]
        );
        assert!(same_path(r"\\?\C:\Repo", "c:/repo/"));
        assert!(!same_path("/Repo", "/repo"));
    }

    #[test]
    fn search_status_reports_position_scope_and_next_step() {
        let matches = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(
            search_status(&matches, Some("b"), 40, false),
            "2 of 3 in 40 loaded commits"
        );
        assert_eq!(
            search_status(&matches, Some("z"), 40, false),
            "3 matches in 40 loaded commits"
        );
        assert_eq!(
            search_status(&[], None, 1, true),
            "No matches in 1 loaded commit. Search older commits to keep looking."
        );
        assert_eq!(
            search_status(&[], None, 7, false),
            "No matches in 7 loaded commits"
        );
    }

    #[test]
    fn decorations_keep_their_type() {
        assert_eq!(
            classify_decoration("HEAD -> main"),
            (DecorationKind::HeadBranch, "main")
        );
        assert_eq!(
            classify_decoration("tag: v1.0"),
            (DecorationKind::Tag, "v1.0")
        );
        assert_eq!(classify_decoration("HEAD"), (DecorationKind::Head, "HEAD"));
        assert_eq!(
            classify_decoration("origin/main"),
            (DecorationKind::Branch, "origin/main")
        );
    }

    #[test]
    fn toolbar_centres_actions_on_the_whole_width() {
        let default = toolbar_layout(1280., 1., 7, 0.);
        assert_eq!(default.selector_width, 176.);
        let ToolbarActions::Centered { left, width } = default.actions else {
            panic!("{default:?}");
        };
        assert_eq!(width, toolbar_actions_width(1., 7, true));
        // Centred on the toolbar, not on the room left over beside the selectors.
        assert!((left + width / 2. - 640.).abs() < 0.01);
        // The selectors and search end before the actions begin.
        assert!(12. + 2. * default.selector_width + 25. + 16. <= left);
        assert!(left + width + 16. + default.search_width + 12. <= 1280.);
        assert!(default.labels());
    }

    #[test]
    fn toolbar_falls_back_without_hiding_actions() {
        // Match navigation beside the search pushes the actions off centre.
        let searching = toolbar_layout(1100., 1., 7, 150.);
        assert_eq!(searching.actions, ToolbarActions::Inline);
        let narrow = toolbar_layout(820., 1., 7, 0.);
        assert_eq!(narrow.actions, ToolbarActions::Compact);
        assert!(!narrow.labels());
        assert!(narrow.selector_width >= 112.);
        // Large text wraps the actions onto their own row, selectors stay readable.
        let large = toolbar_layout(960., 2., 7, 0.);
        assert_eq!(large.actions, ToolbarActions::Wrapped);
        assert!(large.labels());
        assert!(large.selector_width >= 112. * 1.6);
        assert!(2. * large.selector_width + large.search_width + 53. <= 960.5);
    }

    #[test]
    fn branch_filter_ranks_prefixes_first_and_ignores_case() {
        let names = ["main", "feature/Login", "fix-main", "Maint"];
        assert_eq!(filter_branches(&names, ""), vec![0, 1, 2, 3]);
        assert_eq!(filter_branches(&names, "MAIN"), vec![0, 3, 2]);
        assert_eq!(filter_branches(&names, " login "), vec![1]);
        assert!(filter_branches(&names, "nope").is_empty());
    }
}
