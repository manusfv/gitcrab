//! Presentation model for Git unified patches, with line numbers in each blob.

/// A source line or a metadata row in a unified patch.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Line {
    /// One-based line number in the old blob, when present on this side.
    pub old: Option<usize>,
    /// One-based line number in the new blob, when present on this side.
    pub new: Option<usize>,
    /// Patch line including its prefix and excluding the line ending.
    pub text: String,
    /// Diff marker for source lines, `@` for headers, or `\` for newline notices.
    pub kind: char,
    /// UTF-8 byte ranges into [`Self::text`]. Highlights are omitted from serialization.
    #[serde(skip)]
    pub highlights: Vec<crate::highlight::Span>,
}

/// Parses a Git unified patch into numbered source rows and unnumbered headers.
///
/// The input must use Git's standard one-byte source line prefixes (` `, `+`, `-`).
///
/// # Panics
///
/// Panics if a line in a hunk starts with a multibyte character in place of a
/// patch prefix.
pub fn parse(patch: &str) -> Vec<Line> {
    let (mut old, mut new) = (0, 0);
    let mut in_hunk = false;
    patch
        .lines()
        .map(|text| {
            let kind = if text.starts_with("@@ ") {
                let fields: Vec<_> = text.split_whitespace().collect();
                old = fields
                    .get(1)
                    .and_then(|x| x.trim_start_matches('-').split(',').next()?.parse().ok())
                    .unwrap_or(0);
                new = fields
                    .get(2)
                    .and_then(|x| x.trim_start_matches('+').split(',').next()?.parse().ok())
                    .unwrap_or(0);
                in_hunk = true;
                '@'
            } else if text.starts_with("diff --git ") {
                in_hunk = false;
                '@'
            } else if in_hunk {
                text.chars().next().unwrap_or(' ')
            } else {
                '@'
            };
            let (o, n) = match kind {
                '+' => {
                    let n = new;
                    new += 1;
                    (None, Some(n))
                }
                '-' => {
                    let o = old;
                    old += 1;
                    (Some(o), None)
                }
                ' ' => {
                    let (o, n) = (old, new);
                    old += 1;
                    new += 1;
                    (Some(o), Some(n))
                }
                _ => (None, None),
            };
            Line {
                old: o,
                new: n,
                text: text.into(),
                kind,
                highlights: if kind != '@' && !text.is_empty() {
                    crate::highlight::spans(&text[1..])
                        .into_iter()
                        .map(|mut s| {
                            s.start += 1;
                            s.end += 1;
                            s
                        })
                        .collect()
                } else {
                    vec![]
                },
            }
        })
        .collect()
}
/// Zero-based indices into the unified lines for the Before and After cells.
///
/// A missing index represents an empty cell.
pub type SplitRow = (Option<usize>, Option<usize>);

/// Prepared unified and split layouts for the same patch.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Content {
    pub lines: Vec<Line>,
    pub split: Vec<SplitRow>,
    /// Longest patch line in bytes, capped at 8192 for width estimation.
    pub columns: usize,
    /// Zero-based hunk header rows in the unified layout.
    pub hunk_rows: Vec<usize>,
    /// Zero-based hunk header rows in the split layout.
    pub split_hunk_rows: Vec<usize>,
}

/// Builds the split layout and hunk navigation indices for parsed patch lines.
pub fn prepare(lines: Vec<Line>) -> Content {
    let split = split(&lines);
    let columns = lines
        .iter()
        .map(|line| line.text.len().min(8192))
        .max()
        .unwrap_or(0);
    let hunk_rows = lines
        .iter()
        .enumerate()
        .filter_map(|(row, line)| line.text.starts_with("@@ ").then_some(row))
        .collect();
    let split_hunk_rows = split
        .iter()
        .enumerate()
        .filter_map(|(row, (left, _))| {
            left.is_some_and(|index| lines[index].text.starts_with("@@ "))
                .then_some(row)
        })
        .collect();
    Content {
        lines,
        split,
        columns,
        hunk_rows,
        split_hunk_rows,
    }
}

/// Pairs removals with additions within each contiguous block of changed lines.
///
/// Context and metadata appear in both panes. Unpaired changes leave an empty
/// cell on the opposite side.
pub fn split(lines: &[Line]) -> Vec<SplitRow> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if matches!(lines[i].kind, '+' | '-') {
            let (mut removed, mut added) = (Vec::new(), Vec::new());
            while i < lines.len() && matches!(lines[i].kind, '+' | '-') {
                if lines[i].kind == '-' {
                    removed.push(i);
                } else {
                    added.push(i);
                }
                i += 1;
            }
            for j in 0..removed.len().max(added.len()) {
                result.push((removed.get(j).copied(), added.get(j).copied()));
            }
        } else {
            result.push((Some(i), Some(i)));
            i += 1;
        }
    }
    result
}

/// Pane of a split layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Side {
    Before,
    #[default]
    After,
}

/// Copies source text from a zero-based range of visible rows.
///
/// With a split layout, `split` names the rows and the pane to copy from.
/// Headers, missing cells and out-of-range rows are skipped. Source lines lose
/// their diff prefix and are joined with newlines. An empty result means there
/// is nothing to copy, and callers should leave the clipboard unchanged.
pub fn copy_source(
    lines: &[Line],
    split: Option<(&[SplitRow], Side)>,
    rows: std::ops::Range<usize>,
) -> String {
    rows.filter_map(|row| {
        let line = match split {
            Some((split, Side::Before)) => lines.get(split.get(row)?.0?),
            Some((split, Side::After)) => lines.get(split.get(row)?.1?),
            None => lines.get(row),
        }?;
        matches!(line.kind, ' ' | '+' | '-').then_some(())?;
        line.text.get(1..)
    })
    .collect::<Vec<_>>()
    .join("\n")
}

/// Copies the selected rows as patch text, keeping `+`, `-` and context markers.
///
/// Hunk headers and newline notices inside the selection are kept, so a
/// selection that starts at a header is a readable patch fragment. Split rows
/// contribute both cells once, in patch order.
pub fn copy_patch(
    lines: &[Line],
    split: Option<&[SplitRow]>,
    rows: std::ops::Range<usize>,
) -> String {
    let mut indices: Vec<usize> = match split {
        Some(split) => rows
            .filter_map(|row| split.get(row))
            .flat_map(|(before, after)| [*before, *after])
            .flatten()
            .collect(),
        None => rows.filter(|row| *row < lines.len()).collect(),
    };
    indices.sort_unstable();
    indices.dedup();
    indices
        .into_iter()
        .filter_map(|index| lines.get(index))
        .filter(|line| matches!(line.kind, ' ' | '+' | '-' | '\\') || line.text.starts_with("@@ "))
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Comparable identity of each displayed row, used to follow a selection
/// when the patch is replaced.
pub fn row_keys(content: &Content, split: bool) -> Vec<(Option<&str>, Option<&str>)> {
    let text = |index: Option<usize>| index.map(|i| content.lines[i].text.as_str());
    if split {
        content
            .split
            .iter()
            .map(|(before, after)| (text(*before), text(*after)))
            .collect()
    } else {
        content
            .lines
            .iter()
            .map(|line| (Some(line.text.as_str()), None))
            .collect()
    }
}

/// Finds a selected run of rows again after the patch changed.
///
/// Returns the occurrence of the same rows nearest to the old position, or
/// `None` when the selected text no longer appears, so the caller clears the
/// selection instead of highlighting different lines.
pub fn reanchor<T: PartialEq>(
    old: &[T],
    new: &[T],
    range: std::ops::Range<usize>,
) -> Option<std::ops::Range<usize>> {
    let selected = old.get(range.clone()).filter(|rows| !rows.is_empty())?;
    let len = selected.len();
    (0..=new.len().checked_sub(len)?)
        .filter(|&start| &new[start..start + len] == selected)
        .min_by_key(|&start| start.abs_diff(range.start))
        .map(|start| start..start + len)
}

/// The hunk shown at the top of the diff viewport and where the arrows lead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HunkPosition {
    /// Zero-based hunk the reader is in.
    pub current: usize,
    /// Hunk the Previous control scrolls to.
    pub previous: Option<usize>,
    /// Hunk the Next control scrolls to.
    pub next: Option<usize>,
}

/// Locates the reader among hunk header rows from the first visible row.
///
/// `at_end` reports that the list cannot scroll further. The last hunk is
/// then current when its header is on screen, and Next is unavailable.
/// Previous returns to the current hunk's header when the reader has
/// scrolled past it.
pub fn hunk_position(rows: &[usize], top_row: usize, at_end: bool) -> Option<HunkPosition> {
    let last = rows.len().checked_sub(1)?;
    let mut current = rows.iter().rposition(|row| *row <= top_row).unwrap_or(0);
    let last_on_screen = at_end && rows[last] >= top_row;
    if last_on_screen {
        current = last;
    }
    let previous = if top_row > rows[current] {
        Some(current)
    } else {
        current.checked_sub(1)
    };
    let next = (!at_end && current < last).then_some(current + 1);
    Some(HunkPosition {
        current,
        previous,
        next,
    })
}

/// Lists displayed rows whose text contains `query`, ignoring case.
///
/// Source rows are matched without their diff marker. Hunk headers are
/// searched too. A split row matches when either cell does.
pub fn find_rows(lines: &[Line], split: Option<&[SplitRow]>, query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return vec![];
    }
    let matches = |index: Option<usize>| {
        index.and_then(|i| lines.get(i)).is_some_and(|line| {
            let text = if matches!(line.kind, ' ' | '+' | '-') {
                line.text.get(1..).unwrap_or_default()
            } else {
                line.text.as_str()
            };
            text.to_lowercase().contains(&query)
        })
    };
    match split {
        Some(split) => split
            .iter()
            .enumerate()
            .filter(|(_, (before, after))| matches(*before) || matches(*after))
            .map(|(row, _)| row)
            .collect(),
        None => (0..lines.len()).filter(|&row| matches(Some(row))).collect(),
    }
}

/// What a comparison can show, decided by the worker before display.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Preview {
    /// A text patch with hunks.
    #[default]
    Text,
    /// Git reports a binary change and has no text preview.
    Binary,
    /// Only the path changed. The contents are identical.
    Renamed,
    /// Only the file mode changed.
    ModeChange,
    /// A new file with no content.
    EmptyAdded,
    /// A removed file that had no content.
    EmptyDeleted,
    /// The patch is larger than the preview limits.
    Oversized,
    /// Git reports no change on this side.
    NoChanges,
}

impl Preview {
    /// Whether Unified and Split layouts apply to this comparison.
    pub fn has_layout(self) -> bool {
        self == Self::Text
    }
}

/// Classifies a Git patch. `fits` is false when the patch exceeds preview limits.
pub fn classify(patch: &str, fits: bool) -> Preview {
    if !fits {
        return Preview::Oversized;
    }
    if patch.trim().is_empty() {
        return Preview::NoChanges;
    }
    let has = |prefix: &str| patch.lines().any(|line| line.starts_with(prefix));
    if has("@@ ") {
        Preview::Text
    } else if has("Binary files ") || has("GIT binary patch") {
        Preview::Binary
    } else if has("rename to ") || has("copy to ") {
        Preview::Renamed
    } else if has("new file mode ") {
        Preview::EmptyAdded
    } else if has("deleted file mode ") {
        Preview::EmptyDeleted
    } else if has("new mode ") {
        Preview::ModeChange
    } else {
        Preview::Text
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blob_coordinates_and_replacement_alignment() {
        let p=parse("--- a/file\n+++ b/file\n@@ -10,3 +10,4 @@\n same\n-old\n+new\n+extra\n tail\n\\ No newline at end of file\n");
        assert_eq!((p[3].old, p[3].new), (Some(10), Some(10)));
        assert_eq!((p[4].old, p[4].new), (Some(11), None));
        assert_eq!((p[6].old, p[6].new), (None, Some(12)));
        assert_eq!((p[7].old, p[7].new), (Some(12), Some(13)));
        let s = split(&p);
        assert_eq!(p[s[4].0.unwrap()].text, "-old");
        assert_eq!(p[s[4].1.unwrap()].text, "+new");
        assert!(s[5].0.is_none());
    }

    #[test]
    fn copy_uses_source_text_and_skips_empty_after_cells() {
        let lines = parse(
            "@@ -1,4 +1,3 @@\n shared\n-old1\n-old2\n+new\n tail\n\\ No newline at end of file\n",
        );
        let rows = split(&lines);
        assert_eq!(
            copy_source(&lines, None, 0..lines.len()),
            "shared\nold1\nold2\nnew\ntail"
        );
        assert_eq!(
            copy_source(&lines, Some((&rows, Side::After)), 0..rows.len()),
            "shared\nnew\ntail"
        );
    }

    #[test]
    fn split_copy_uses_the_focused_side_and_patch_copy_keeps_markers() {
        let lines = parse("@@ -1,2 +1,2 @@\n shared\n-old\n+new\n");
        let rows = split(&lines);
        // Row 2 pairs the removal with the addition.
        assert_eq!(
            copy_source(&lines, Some((&rows, Side::Before)), 2..3),
            "old"
        );
        assert_eq!(copy_source(&lines, Some((&rows, Side::After)), 2..3), "new");
        assert_eq!(copy_source(&lines, None, 0..1), "");
        assert_eq!(
            copy_patch(&lines, None, 0..lines.len()),
            "@@ -1,2 +1,2 @@\n shared\n-old\n+new"
        );
        assert_eq!(copy_patch(&lines, Some(&rows), 1..3), " shared\n-old\n+new");
    }

    #[test]
    fn selection_follows_its_rows_or_is_dropped() {
        let old = ["a", "b", "c", "d"];
        let new = ["x", "a", "b", "c", "d"];
        assert_eq!(reanchor(&old, &new, 1..3), Some(2..4));
        assert_eq!(reanchor(&old, &["a", "z", "d"], 1..3), None);
        // The nearest repeated run wins.
        assert_eq!(
            reanchor(&["b", "x", "b"], &["b", "y", "y", "b"], 2..3),
            Some(3..4)
        );
        assert_eq!(reanchor(&old, &new, 3..3), None);
    }

    #[test]
    fn hunk_position_follows_the_viewport_and_disables_only_at_ends() {
        let rows = [0, 10, 20];
        let at = |top, end| hunk_position(&rows, top, end).unwrap();
        let position = |current, previous, next| HunkPosition {
            current,
            previous,
            next,
        };
        assert_eq!(at(0, false), position(0, None, Some(1)));
        assert_eq!(at(14, false), position(1, Some(1), Some(2)));
        assert_eq!(at(10, false), position(1, Some(0), Some(2)));
        // At the bottom, the last visible hunk is current and Next is disabled.
        assert_eq!(at(15, true), position(2, Some(1), None));
        assert_eq!(at(25, true), position(2, Some(2), None));
        assert!(hunk_position(&[], 0, false).is_none());
    }

    #[test]
    fn find_searches_source_text_without_markers() {
        let lines = parse("@@ -1,2 +1,2 @@ fn main\n keep\n-Old value\n+new VALUE\n");
        assert_eq!(find_rows(&lines, None, "value"), [2, 3]);
        assert!(find_rows(&lines, None, "+new").is_empty());
        assert_eq!(find_rows(&lines, None, "main"), [0]);
        let rows = split(&lines);
        assert_eq!(find_rows(&lines, Some(&rows), "value"), [2]);
        assert!(find_rows(&lines, None, "  ").is_empty());
    }

    #[test]
    fn classifies_comparisons_without_text_hunks() {
        assert_eq!(classify("@@ -1 +1 @@\n-a\n+b\n", true), Preview::Text);
        assert_eq!(classify("@@ -1 +1 @@\n", false), Preview::Oversized);
        assert_eq!(classify("", true), Preview::NoChanges);
        let cases = [
            ("Binary files a/x and b/x differ\n", Preview::Binary),
            (
                "similarity index 100%\nrename from a\nrename to b\n",
                Preview::Renamed,
            ),
            (
                "new file mode 100644\nindex 0000000..e69de29\n",
                Preview::EmptyAdded,
            ),
            ("deleted file mode 100644\n", Preview::EmptyDeleted),
            ("old mode 100644\nnew mode 100755\n", Preview::ModeChange),
        ];
        for (patch, expected) in cases {
            assert_eq!(
                classify(&format!("diff --git a/x b/x\n{patch}"), true),
                expected
            );
        }
        assert!(!Preview::Binary.has_layout());
    }
}
