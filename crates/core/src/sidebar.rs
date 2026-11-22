//! The reference sidebar: which branches, remote refs and tags show, how
//! they nest into folders, and what each section header counts.
//!
//! Collapsed sections and folders are remembered as keys in
//! [`crate::preferences::Preferences::collapsed`]. A filter shows matches
//! inside collapsed sections and folders and counts them.
use std::collections::BTreeSet;

/// The text typed into the sidebar filter.
#[derive(Clone, Debug, Default)]
pub struct RefFilter {
    query: String,
}

impl RefFilter {
    /// A case-insensitive filter. An empty query shows everything.
    pub fn new(query: &str) -> Self {
        Self {
            query: query.to_lowercase(),
        }
    }

    /// Whether a query is entered.
    pub fn is_active(&self) -> bool {
        !self.query.is_empty()
    }

    /// Whether `name` contains the query, ignoring case.
    pub fn matches(&self, name: &str) -> bool {
        name.to_lowercase().contains(&self.query)
    }

    /// A section header count: `3 of 12` while filtering, otherwise `12`.
    pub fn count_label(&self, matched: usize, total: usize) -> String {
        if self.is_active() {
            format!("{matched} of {total}")
        } else {
            total.to_string()
        }
    }
}

/// Key under which a section's collapsed state is saved. `section` is empty
/// for local branches.
pub fn section_key(repository: &str, section: &str) -> String {
    format!("{repository}|{section}")
}

/// One sidebar section as it should be shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// Collapsed-state key, from [`section_key`].
    pub key: String,
    /// Whether the section's rows are hidden. Never while filtering.
    pub collapsed: bool,
    /// Header count, from [`RefFilter::count_label`].
    pub count: String,
    /// Indexes of the matching items, in order. Empty while collapsed.
    pub visible: Vec<usize>,
}

/// Decides what a section shows for items named by `labels`.
pub fn section<'a>(
    repository: &str,
    name: &str,
    labels: impl IntoIterator<Item = &'a str>,
    collapsed: &BTreeSet<String>,
    filter: &RefFilter,
) -> Section {
    let key = section_key(repository, name);
    let is_collapsed = collapsed.contains(&key) && !filter.is_active();
    let mut total = 0;
    let matching: Vec<usize> = labels
        .into_iter()
        .inspect(|_| total += 1)
        .enumerate()
        .filter(|(_, label)| filter.matches(label))
        .map(|(index, _)| index)
        .collect();
    Section {
        key,
        collapsed: is_collapsed,
        count: filter.count_label(matching.len(), total),
        visible: if is_collapsed { vec![] } else { matching },
    }
}

/// A row in a slash-separated reference tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeRow {
    /// A folder for a shared name prefix, shown once before its first leaf.
    Folder {
        /// Collapsed-state key.
        key: String,
        /// Nesting level, starting at 1 for a top-level folder.
        depth: usize,
        /// This folder's own name segment.
        name: String,
        /// The full prefix, such as `feature/ui`.
        path: String,
        collapsed: bool,
    },
    /// A reference, with its position in the input.
    Leaf {
        index: usize,
        /// Number of folders above it.
        depth: usize,
        /// The last name segment.
        name: String,
    },
}

/// Lays out `labels` as folders and leaves. Leaves under a collapsed folder
/// are left out; while `filtering`, no folder is collapsed.
///
/// `scope` names the section for folder keys; it is empty for local branches.
pub fn tree_rows<'a>(
    repository: &str,
    scope: &str,
    labels: impl IntoIterator<Item = &'a str>,
    collapsed: &BTreeSet<String>,
    filtering: bool,
) -> Vec<TreeRow> {
    let mut rows = vec![];
    let mut seen = BTreeSet::new();
    'leaves: for (index, label) in labels.into_iter().enumerate() {
        let parts = label.split('/').collect::<Vec<_>>();
        for depth in 1..parts.len() {
            let path = parts[..depth].join("/");
            let key = if scope.is_empty() {
                format!("{repository}|{path}")
            } else {
                format!("{repository}|{scope}/{path}")
            };
            let is_collapsed = collapsed.contains(&key) && !filtering;
            if seen.insert(key.clone()) {
                rows.push(TreeRow::Folder {
                    key,
                    depth,
                    name: parts[depth - 1].to_owned(),
                    path,
                    collapsed: is_collapsed,
                });
            }
            if is_collapsed {
                continue 'leaves;
            }
        }
        let depth = parts.len() - 1;
        rows.push(TreeRow::Leaf {
            index,
            depth,
            name: parts[depth].to_owned(),
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|k| (*k).to_owned()).collect()
    }

    #[test]
    fn filter_ignores_case_and_counts_matches() {
        let filter = RefFilter::new("FEAT");
        assert!(filter.is_active());
        assert!(filter.matches("feature/x"));
        assert!(!filter.matches("main"));
        assert_eq!(filter.count_label(2, 9), "2 of 9");
        assert_eq!(RefFilter::new("").count_label(2, 9), "9");
    }

    #[test]
    fn a_filter_opens_collapsed_sections_and_counts_matches() {
        let labels = ["main", "feature/a", "feature/b"];
        let collapsed = keys(&["repo|"]);
        let shut = section("repo", "", labels, &collapsed, &RefFilter::new(""));
        assert!(shut.collapsed);
        assert!(shut.visible.is_empty());
        assert_eq!(shut.count, "3");

        let found = section("repo", "", labels, &collapsed, &RefFilter::new("feat"));
        assert!(!found.collapsed);
        assert_eq!(found.visible, [1, 2]);
        assert_eq!(found.count, "2 of 3");
        assert_eq!(section_key("repo", "Tags"), "repo|Tags");
    }

    #[test]
    fn tree_rows_nest_folders_once_and_hide_collapsed_leaves() {
        let labels = ["feature/a", "feature/b", "fix/deep/c", "main"];
        let rows = tree_rows("repo", "", labels, &BTreeSet::new(), false);
        let names: Vec<String> = rows
            .iter()
            .map(|row| match row {
                TreeRow::Folder { path, .. } => format!("{path}/"),
                TreeRow::Leaf { name, depth, .. } => format!("{name}@{depth}"),
            })
            .collect();
        assert_eq!(
            names,
            [
                "feature/",
                "a@1",
                "b@1",
                "fix/",
                "fix/deep/",
                "c@2",
                "main@0"
            ]
        );

        let collapsed = keys(&["repo|Remotes/origin"]);
        let rows = tree_rows(
            "repo",
            "Remotes",
            ["origin/main", "up/x"],
            &collapsed,
            false,
        );
        assert!(
            matches!(&rows[0], TreeRow::Folder { collapsed: true, key, .. } if key == "repo|Remotes/origin")
        );
        assert!(matches!(
            &rows[1],
            TreeRow::Folder {
                collapsed: false,
                ..
            }
        ));
        assert!(matches!(&rows[2], TreeRow::Leaf { index: 1, .. }));
        assert_eq!(rows.len(), 3);

        // While filtering, collapsed folders open.
        let rows = tree_rows("repo", "Remotes", ["origin/main"], &collapsed, true);
        assert!(matches!(&rows[1], TreeRow::Leaf { index: 0, .. }));
    }
}
