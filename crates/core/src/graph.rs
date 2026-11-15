//! Lane assignment for Git history in child-before-parent order.

use crate::git::Commit;

/// Commit rows and the lane segments that connect them.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct Graph {
    /// One row per commit in the order supplied to [`layout_with_head`].
    pub rows: Vec<GraphRow>,
    /// Maximum number of lanes needed across the loaded history.
    pub lanes: usize,
    pass_through: Vec<Vec<PassRange>>,
}

impl Graph {
    /// The row whose refs name the nearest branch containing `row`'s commit,
    /// shown as a "ghost" label when `row` has no refs of its own. `None` when
    /// `row` has refs itself or no loaded descendant has any.
    pub fn ghost_source(&self, row: usize) -> Option<usize> {
        self.rows
            .get(row)
            .and_then(|data| data.ref_source)
            .map(|source| source as usize)
            .filter(|source| *source != row)
    }

    /// Marks the rows whose commits are reachable from `row`'s commit,
    /// including itself: the history of a ref pointing at `row`.
    pub fn reachable_from(&self, row: usize) -> Vec<bool> {
        let mut seen = vec![false; self.rows.len()];
        let mut stack = vec![row];
        while let Some(next) = stack.pop() {
            if next >= seen.len() || seen[next] {
                continue;
            }
            seen[next] = true;
            stack.extend(
                self.rows[next]
                    .parents
                    .iter()
                    .map(|parent| *parent as usize),
            );
        }
        seen
    }

    /// Returns the segments from `row` to the next row, including passing lanes.
    ///
    /// # Panics
    ///
    /// Panics if `row` is outside [`Self::rows`].
    pub fn edges_at(&self, row: usize) -> Vec<Edge> {
        let data = &self.rows[row];
        let mut edges = data.joins.clone();
        for (lane, ranges) in self.pass_through.iter().enumerate() {
            let index = ranges.partition_point(|range| range.end <= row);
            if let Some(range) = ranges.get(index).filter(|range| range.start <= row) {
                edges.push(Edge {
                    from_lane: lane,
                    to_lane: lane,
                    color: range.color,
                });
            }
        }
        edges.sort_unstable_by_key(|edge| edge.from_lane);
        edges.extend_from_slice(&data.edges);
        edges
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct PassRange {
    start: usize,
    end: usize,
    color: usize,
}
/// A commit node in the history graph.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct GraphRow {
    /// Zero-based lane occupied by the node.
    pub lane: usize,
    /// Color index to map onto the renderer's lane palette.
    pub color: usize,
    // Segments originating at this row's commit.
    edges: Vec<Edge>,
    // Passing lanes that meet the next row's commit.
    joins: Vec<Edge>,
    /// Rows of this commit's parents that are in the loaded history.
    #[serde(default)]
    parents: Vec<u32>,
    /// The nearest row at or above this one whose commit has refs and
    /// contains this one, preferring first-parent links.
    #[serde(default)]
    ref_source: Option<u32>,
}
impl GraphRow {
    /// Reports whether multiple paths meet at this node from either direction.
    ///
    /// Pass the preceding row's edges as `incoming`, or an empty slice for
    /// the first row.
    pub fn is_junction(&self, incoming: &[Edge]) -> bool {
        incoming
            .iter()
            .filter(|edge| edge.to_lane == self.lane)
            .count()
            > 1
            || self
                .edges
                .iter()
                .filter(|edge| edge.from_lane == self.lane)
                .count()
                > 1
    }
}
/// A segment between two adjacent rows, addressed by zero-based lanes.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from_lane: usize,
    pub to_lane: usize,
    /// Color index to map onto the renderer's lane palette.
    pub color: usize,
}

#[derive(Clone, Copy)]
struct Lane<'a> {
    target: &'a str,
    color: usize,
    pass_start: usize,
}

fn close_pass(ranges: &mut [Vec<PassRange>], lane: usize, active: Lane<'_>, end: usize) {
    if active.pass_start < end {
        ranges[lane].push(PassRange {
            start: active.pass_start,
            end,
            color: active.color,
        });
    }
}

/// Lays out commits in child-before-parent order, finding HEAD by its `HEAD` label.
///
/// Use [`layout_with_head`] when the HEAD commit may be outside the loaded history.
pub fn layout(commits: &[Commit]) -> Graph {
    let head = commits
        .iter()
        .find(|commit| commit.refs.iter().any(|label| label == "HEAD"));
    layout_with_head(commits, head.map(|commit| commit.id.as_str()))
}

/// Lays out commits in child-before-parent order, reserving lane zero for `head`.
///
/// Supply the full HEAD object ID to reserve its lane even when HEAD falls
/// outside the loaded history. An absent or empty ID leaves lane zero available.
pub fn layout_with_head(commits: &[Commit], head: Option<&str>) -> Graph {
    let mut lanes: Vec<Option<Lane<'_>>> = head
        .filter(|id| !id.is_empty())
        .map(|id| {
            vec![Some(Lane {
                target: id,
                color: 0,
                pass_start: 0,
            })]
        })
        .unwrap_or_default();
    let mut next_color = lanes.len();
    let mut graph = Graph::default();
    let mut previous_targets = Vec::new();
    for (row_index, commit) in commits.iter().enumerate() {
        let lane = if let Some(i) = lanes
            .iter()
            .position(|active| active.is_some_and(|active| active.target == commit.id))
        {
            i
        } else {
            let i = lanes
                .iter()
                .position(Option::is_none)
                .unwrap_or(lanes.len());
            if i == lanes.len() {
                lanes.push(None);
            }
            lanes[i] = Some(Lane {
                target: &commit.id,
                color: next_color,
                pass_start: row_index + 1,
            });
            next_color += 1;
            i
        };
        let color = lanes[lane].unwrap().color;
        if let Some(previous) = graph.rows.last_mut() {
            for (edge, target) in previous.edges.iter_mut().zip(&previous_targets) {
                if *target == commit.id {
                    edge.to_lane = lane;
                }
            }
        }
        for (index, occupant) in lanes.iter_mut().enumerate() {
            if let Some(active) = occupant.filter(|active| active.target == commit.id) {
                if let Some(previous_row) = row_index
                    .checked_sub(1)
                    .filter(|row| active.pass_start <= *row)
                {
                    close_pass(&mut graph.pass_through, index, active, previous_row);
                    graph.rows[previous_row].joins.push(Edge {
                        from_lane: index,
                        to_lane: lane,
                        color: active.color,
                    });
                }
                *occupant = None;
            }
        }
        let mut parent_lanes = Vec::new();
        for (index, parent) in commit.parents.iter().enumerate() {
            let dest = if let Some(i) = lanes
                .iter()
                .position(|active| active.is_some_and(|active| active.target == parent))
            {
                i
            } else {
                let i = if index == 0 && lanes[lane].is_none() {
                    lane
                } else {
                    lanes
                        .iter()
                        .position(Option::is_none)
                        .unwrap_or(lanes.len())
                };
                if i == lanes.len() {
                    lanes.push(None);
                }
                let parent_color = if index == 0 {
                    color
                } else {
                    let c = next_color;
                    next_color += 1;
                    c
                };
                lanes[i] = Some(Lane {
                    target: parent,
                    color: parent_color,
                    pass_start: row_index + 1,
                });
                i
            };
            if index == 0 && dest < lane {
                // Keep a branch to the right of its shared parent until that
                // parent arrives. The leftmost lane remains the parent's lane.
                lanes[lane] = Some(Lane {
                    target: parent,
                    color,
                    pass_start: row_index + 1,
                });
                parent_lanes.push(lane);
            } else if index == 0 && dest > lane {
                // A branch to the right already waits for this first parent.
                // Claim it here too so the first-parent chain keeps its lane;
                // the branch lane joins in when the parent arrives.
                lanes[lane] = Some(Lane {
                    target: parent,
                    color,
                    pass_start: row_index + 1,
                });
                parent_lanes.push(lane);
            } else {
                parent_lanes.push(dest);
            }
        }
        let mut edges = Vec::with_capacity(parent_lanes.len());
        previous_targets.clear();
        for dest in parent_lanes {
            let active = lanes[dest].unwrap();
            previous_targets.push(active.target);
            edges.push(Edge {
                from_lane: lane,
                to_lane: dest,
                color: active.color,
            });
        }
        graph.lanes = graph.lanes.max(lanes.len()).max(lane + 1);
        graph.pass_through.resize_with(graph.lanes, Vec::new);
        graph.rows.push(GraphRow {
            lane,
            color,
            edges,
            joins: Vec::new(),
            ..Default::default()
        });
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
    }
    for (index, active) in lanes.into_iter().enumerate() {
        if let Some(active) = active {
            close_pass(&mut graph.pass_through, index, active, commits.len());
        }
    }
    link_ancestry(&mut graph, commits);
    graph
}

/// Fills in each row's loaded parents and the nearest decorated row that
/// contains it. Rows come child before parent, so one pass downwards sees
/// every child of a commit before the commit itself.
fn link_ancestry(graph: &mut Graph, commits: &[Commit]) {
    let index: std::collections::HashMap<&str, u32> = commits
        .iter()
        .enumerate()
        .map(|(row, commit)| (commit.id.as_str(), row as u32))
        .collect();
    // The source handed down to each row, and whether a first parent link
    // handed it down; a first-parent link replaces one from a merge.
    let mut inherited: Vec<Option<(u32, bool)>> = vec![None; commits.len()];
    for ((row, commit), data) in commits.iter().enumerate().zip(&mut graph.rows) {
        data.parents = commit
            .parents
            .iter()
            .filter_map(|parent| index.get(parent.as_str()).copied())
            .collect();
        data.ref_source = if commit.refs.is_empty() {
            inherited[row].map(|(source, _)| source)
        } else {
            Some(row as u32)
        };
        let Some(source) = data.ref_source else {
            continue;
        };
        for (k, parent) in commit.parents.iter().enumerate() {
            let Some(&parent) = index.get(parent.as_str()) else {
                continue;
            };
            let first = k == 0;
            let slot = &mut inherited[parent as usize];
            if slot.is_none() || first && slot.is_some_and(|(_, via_first)| !via_first) {
                *slot = Some((source, first));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn c(id: &str, parents: &[&str]) -> Commit {
        Commit {
            id: id.into(),
            parents: parents.iter().map(|s| (*s).into()).collect(),
            ..Default::default()
        }
    }
    fn r(id: &str, parents: &[&str], refs: &[&str]) -> Commit {
        Commit {
            refs: refs.iter().map(|s| (*s).into()).collect(),
            ..c(id, parents)
        }
    }

    #[test]
    fn ghost_source_names_the_nearest_containing_ref() {
        // main: m2 -> m1 -> base; feature: f1 -> base, merged nowhere.
        let commits = [
            r("m2", &["m1"], &["main"]),
            r("f1", &["base"], &["feature"]),
            c("m1", &["base"]),
            c("base", &[]),
        ];
        let graph = layout(&commits);
        assert_eq!(graph.ghost_source(0), None, "a row with refs has no ghost");
        assert_eq!(graph.ghost_source(2), Some(0));
        // Both reach base by a first parent; the nearer row, f1, names it.
        assert_eq!(graph.ghost_source(3), Some(1));
    }

    #[test]
    fn ghost_source_prefers_first_parent_over_an_earlier_merge() {
        // The merge's second parent is the feature tip; its own branch names it.
        let commits = [
            r("merge", &["m1", "f1"], &["main"]),
            r("f1", &["base"], &["feature"]),
            c("m1", &["base"]),
            c("base", &[]),
        ];
        let graph = layout(&commits);
        assert_eq!(graph.ghost_source(1), None);
        assert_eq!(graph.ghost_source(2), Some(0));
        assert_eq!(
            graph.ghost_source(3),
            Some(1),
            "f1 reaches base by its first parent"
        );
    }

    #[test]
    fn reachable_from_follows_every_parent() {
        let commits = [
            r("merge", &["m1", "f1"], &["main"]),
            r("other", &["base"], &["other"]),
            c("f1", &["base"]),
            c("m1", &["base"]),
            c("base", &[]),
        ];
        let graph = layout(&commits);
        assert_eq!(graph.reachable_from(0), vec![true, false, true, true, true]);
        assert_eq!(
            graph.reachable_from(1),
            vec![false, true, false, false, true]
        );
    }

    #[test]
    #[ignore = "repeatable graph allocation and timing fixture"]
    fn benchmark_graph_layout() {
        for (shape, width) in [
            ("linear", 1000),
            ("wide", 250),
            ("wide", 500),
            ("wide", 1000),
        ] {
            let commits = (0..=width)
                .map(|i| {
                    let parents = if i == width {
                        vec![]
                    } else if shape == "linear" {
                        vec![format!("{:040x}", i + 1)]
                    } else {
                        vec![format!("{width:040x}")]
                    };
                    Commit {
                        id: format!("{i:040x}"),
                        parents,
                        ..Default::default()
                    }
                })
                .collect::<Vec<_>>();
            let mut samples = vec![];
            for _ in 0..11 {
                let start = std::time::Instant::now();
                let graph = layout(std::hint::black_box(&commits));
                samples.push(start.elapsed().as_secs_f64() * 1000.0);
                std::hint::black_box(graph);
            }
            samples.sort_by(f64::total_cmp);
            let graph = layout(&commits);
            let stored_edges = graph
                .rows
                .iter()
                .map(|row| row.edges.len() + row.joins.len())
                .sum::<usize>();
            let ranges = graph.pass_through.iter().map(Vec::len).sum::<usize>();
            let bytes = stored_edges * std::mem::size_of::<Edge>()
                + ranges * std::mem::size_of::<PassRange>();
            let expanded = (0..graph.rows.len())
                .map(|row| graph.edges_at(row).len())
                .sum::<usize>();
            let expected = if shape == "wide" {
                width * (width + 1) / 2
            } else {
                width
            };
            assert_eq!(expanded, expected);
            println!(
                "{shape} {width}: {:.2} ms median, {bytes} bytes of edge payload",
                samples[5]
            );
        }
    }

    #[test]
    fn linear_is_one_continuous_lane() {
        let graph = layout(&[c("a", &["b"]), c("b", &["c"]), c("c", &[])]);
        assert_eq!(graph.lanes, 1);
        assert!(graph.rows.iter().all(|r| r.lane == 0 && r.color == 0));
        assert!(graph.edges_at(2).is_empty());
        for (index, row) in graph.rows.iter().enumerate() {
            let incoming = index
                .checked_sub(1)
                .map(|i| graph.edges_at(i))
                .unwrap_or_default();
            assert!(!row.is_junction(&incoming));
        }
    }
    #[test]
    fn junctions_mark_merges_and_branch_points_but_not_parallel_commits() {
        let graph = layout(&[
            c("merge", &["a", "b"]),
            c("a", &["base"]),
            c("b", &["base"]),
            c("base", &["root"]),
            c("root", &[]),
        ]);
        let junctions: Vec<_> = graph
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let incoming = index
                    .checked_sub(1)
                    .map(|i| graph.edges_at(i))
                    .unwrap_or_default();
                row.is_junction(&incoming)
            })
            .collect();
        assert_eq!(junctions, [true, false, false, true, false]);
    }
    #[test]
    fn octopus_and_disconnected_roots_preserve_topology() {
        let graph = layout(&[
            c("m", &["a", "b", "c"]),
            c("a", &["r"]),
            c("b", &["r"]),
            c("c", &["r"]),
            c("r", &[]),
            c("orphan", &[]),
        ]);
        assert_eq!(graph.edges_at(0).len(), 3);
        assert_eq!(graph.lanes, 3);
        assert_eq!(graph.rows[1].lane, 0);
        assert_eq!(graph.rows[2].lane, 1);
        assert_eq!(graph.rows[3].lane, 2);
        assert_eq!(graph.rows[4].lane, 0);
        assert!(graph.edges_at(5).is_empty());
        assert_ne!(graph.rows[4].color, graph.rows[5].color);
    }

    #[test]
    fn branches_keep_their_lanes_until_the_shared_parent_row() {
        let commits = [
            c("merge", &["main", "feature", "side"]),
            c("main", &["base"]),
            c("feature", &["base"]),
            c("side", &["base"]),
            c("unrelated", &[]),
            c("base", &[]),
            c("later", &[]),
        ];
        let graph = layout(&commits);
        let feature = &graph.rows[2];
        let side = &graph.rows[3];
        let unrelated = &graph.rows[4];
        let base = &graph.rows[5];
        assert_eq!(unrelated.lane, 3);
        for (index, branch) in [(2, feature), (3, side)] {
            assert!(graph
                .edges_at(index)
                .iter()
                .any(|edge| edge.from_lane == branch.lane
                    && edge.to_lane == branch.lane
                    && edge.color == branch.color));
            assert!(graph
                .edges_at(4)
                .iter()
                .any(|edge| edge.from_lane == branch.lane
                    && edge.to_lane == base.lane
                    && edge.color == branch.color));
        }
        assert!(base.is_junction(&graph.edges_at(4)));
        assert!(graph.edges_at(5).is_empty());
        assert_eq!(graph.rows[6].lane, 0);
    }
    #[test]
    fn first_parent_chain_keeps_its_lane_when_a_branch_reaches_the_base_first() {
        let mut merge = c("merge", &["main2", "fix"]);
        merge.refs.push("HEAD".into());
        let graph = layout(&[
            merge,
            c("fix", &["base"]),
            c("main2", &["main1"]),
            c("main1", &["base"]),
            c("base", &["root"]),
            c("root", &[]),
        ]);
        let main = (graph.rows[0].lane, graph.rows[0].color);
        assert_eq!(main, (0, 0));
        for row in [2, 3, 4, 5] {
            assert_eq!((graph.rows[row].lane, graph.rows[row].color), main);
        }
        assert_eq!(graph.rows[1].lane, 1);
        assert!(graph.rows[4].is_junction(&graph.edges_at(3)));
        assert!(graph
            .edges_at(3)
            .iter()
            .any(|edge| edge.from_lane == 1 && edge.to_lane == 0));
    }
    #[test]
    fn loading_more_history_does_not_move_existing_rows() {
        let commits = [
            c("m", &["a", "b"]),
            c("a", &["r"]),
            c("b", &["r"]),
            c("r", &[]),
        ];
        let prefix = layout(&commits[..2]);
        let full = layout(&commits);
        for i in 0..2 {
            assert_eq!(prefix.rows[i].lane, full.rows[i].lane);
            assert_eq!(prefix.rows[i].color, full.rows[i].color);
            assert_eq!(prefix.edges_at(i), full.edges_at(i));
        }
    }
    #[test]
    fn head_keeps_primary_lane_after_a_newer_disconnected_root() {
        let mut head = c("head", &["base"]);
        head.refs.push("HEAD".into());
        let graph = layout(&[c("newer-orphan", &[]), head, c("base", &[])]);
        assert_eq!((graph.rows[0].lane, graph.rows[0].color), (1, 1));
        assert_eq!((graph.rows[1].lane, graph.rows[1].color), (0, 0));
        assert_eq!((graph.rows[2].lane, graph.rows[2].color), (0, 0));
        assert!(graph
            .edges_at(0)
            .iter()
            .any(|edge| edge.from_lane == 0 && edge.to_lane == 0));
    }
    #[test]
    fn head_reservation_is_stable_when_head_arrives_on_a_later_page() {
        let commits = [c("newer-orphan", &[]), c("head", &["base"]), c("base", &[])];
        let prefix = layout_with_head(&commits[..1], Some("head"));
        let full = layout_with_head(&commits, Some("head"));
        assert_eq!(prefix.rows[0].lane, full.rows[0].lane);
        assert_eq!(prefix.rows[0].color, full.rows[0].color);
        assert_eq!(prefix.edges_at(0), full.edges_at(0));
    }
}
