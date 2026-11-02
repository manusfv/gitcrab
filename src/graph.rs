//! Original streaming lane assignment; independent of the reference app implementation.
//! Input order is Git's child-before-parent date order. Coordinates and rendering are UI concerns.
use crate::git::Commit;

#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub rows: Vec<GraphRow>,
    pub lanes: usize,
}
#[derive(Clone, Debug, Default)]
pub struct GraphRow {
    pub lane: usize,
    pub color: usize,
    /// Segments from this row's center to the next row's center.
    pub edges: Vec<Edge>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from_lane: usize,
    pub to_lane: usize,
    pub color: usize,
    pub parent: String,
}

pub fn layout(commits: &[Commit]) -> Graph {
    let head = commits
        .iter()
        .find(|commit| commit.refs.iter().any(|label| label == "HEAD"));
    layout_with_head(commits, head.map(|commit| commit.id.as_str()))
}

/// Reserve HEAD's lane even if a newer disconnected history appears first.
/// Supply the full repository HEAD identity to retain this reservation when
/// HEAD itself falls outside the current loaded page.
pub fn layout_with_head(commits: &[Commit], head: Option<&str>) -> Graph {
    let mut lanes: Vec<Option<(String, usize)>> = head
        .filter(|id| !id.is_empty())
        .map(|id| vec![Some((id.to_string(), 0))])
        .unwrap_or_default();
    let mut next_color = lanes.len();
    let mut graph = Graph::default();
    for commit in commits {
        let lane = if let Some(i) = lanes
            .iter()
            .position(|x| x.as_ref().is_some_and(|(id, _)| id == &commit.id))
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
            lanes[i] = Some((commit.id.clone(), next_color));
            next_color += 1;
            i
        };
        let color = lanes[lane].as_ref().unwrap().1;
        let before = lanes.clone();
        lanes[lane] = None;
        let mut parent_lanes = Vec::new();
        for (index, parent) in commit.parents.iter().enumerate() {
            let dest = if let Some(i) = lanes
                .iter()
                .position(|x| x.as_ref().is_some_and(|(id, _)| id == parent))
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
                lanes[i] = Some((parent.clone(), parent_color));
                i
            };
            parent_lanes.push(dest);
        }
        let mut edges = Vec::new();
        for (i, occupant) in before.iter().enumerate() {
            if i != lane {
                if let Some((id, c)) = occupant {
                    edges.push(Edge {
                        from_lane: i,
                        to_lane: i,
                        color: *c,
                        parent: id.clone(),
                    });
                }
            }
        }
        for dest in parent_lanes {
            let (id, c) = lanes[dest].as_ref().unwrap();
            edges.push(Edge {
                from_lane: lane,
                to_lane: dest,
                color: *c,
                parent: id.clone(),
            });
        }
        graph.lanes = graph.lanes.max(lanes.len()).max(lane + 1);
        graph.rows.push(GraphRow { lane, color, edges });
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
    }
    graph
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
    #[test]
    fn linear_is_one_continuous_lane() {
        let graph = layout(&[c("a", &["b"]), c("b", &["c"]), c("c", &[])]);
        assert_eq!(graph.lanes, 1);
        assert!(graph.rows.iter().all(|r| r.lane == 0 && r.color == 0));
        assert!(graph.rows[2].edges.is_empty());
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
        assert_eq!(graph.rows[0].edges.len(), 3);
        assert_eq!(graph.lanes, 3);
        assert_eq!(graph.rows[1].lane, 0);
        assert_eq!(graph.rows[2].lane, 1);
        assert_eq!(graph.rows[3].lane, 2);
        assert_eq!(graph.rows[4].lane, 0);
        assert!(graph.rows[5].edges.is_empty());
        assert_ne!(graph.rows[4].color, graph.rows[5].color);
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
            assert_eq!(prefix.rows[i].edges, full.rows[i].edges);
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
        assert!(graph.rows[0]
            .edges
            .iter()
            .any(|edge| edge.parent == "head" && edge.from_lane == 0 && edge.to_lane == 0));
    }
    #[test]
    fn head_reservation_is_stable_when_head_arrives_on_a_later_page() {
        let commits = [c("newer-orphan", &[]), c("head", &["base"]), c("base", &[])];
        let prefix = layout_with_head(&commits[..1], Some("head"));
        let full = layout_with_head(&commits, Some("head"));
        assert_eq!(prefix.rows[0].lane, full.rows[0].lane);
        assert_eq!(prefix.rows[0].color, full.rows[0].color);
        assert_eq!(prefix.rows[0].edges, full.rows[0].edges);
    }
}
