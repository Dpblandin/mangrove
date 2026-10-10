use std::{collections::HashMap, sync::Arc};

use crate::model::{GraphEdge, GraphEdgeKind, GraphRow};

struct Lane {
    sha: Arc<str>,
    color: usize,
}

#[derive(Default)]
pub(crate) struct Layout {
    lanes: Vec<Lane>,
    next_color: usize,
}

impl Layout {
    /// Lane colors keep counting; each theme cycles them over its own palette.
    fn color(&mut self) -> usize {
        let color = self.next_color;
        self.next_color += 1;
        color
    }

    pub fn push(&mut self, sha: &str, parents: &[String]) -> GraphRow {
        let existing = self.lanes.iter().position(|lane| lane.sha.as_ref() == sha);
        let lane = existing.unwrap_or(self.lanes.len());
        if existing.is_none() {
            let color = self.color();
            self.lanes.push(Lane {
                sha: sha.into(),
                color,
            });
        }
        let before: Vec<_> = self
            .lanes
            .iter()
            .map(|lane| (lane.sha.clone(), lane.color))
            .collect();
        let color = self.lanes.remove(lane).color;
        for (index, parent) in parents.iter().enumerate() {
            if !self.lanes.iter().any(|entry| entry.sha.as_ref() == parent) {
                let parent_color = if index == 0 { color } else { self.color() };
                self.lanes.insert(
                    (lane + index).min(self.lanes.len()),
                    Lane {
                        sha: parent.as_str().into(),
                        color: parent_color,
                    },
                );
            }
        }
        let positions: HashMap<_, _> = self
            .lanes
            .iter()
            .enumerate()
            .map(|(index, lane)| (lane.sha.as_ref(), index))
            .collect();
        // One through/incoming segment per active lane plus the parent edges.
        // Reserving this upper bound avoids growth to the next power of two
        // for wide rows, which would otherwise bloat every cached page.
        let mut edges = Vec::with_capacity(before.len() + parents.len());
        for (from, (sha, color)) in before.iter().enumerate() {
            if from != lane {
                if let Some(&to) = positions.get(sha.as_ref()) {
                    edges.push(GraphEdge {
                        from,
                        to,
                        color: *color,
                        kind: GraphEdgeKind::Through,
                    });
                }
            }
        }
        if existing.is_some() {
            edges.push(GraphEdge {
                from: lane,
                to: lane,
                color,
                kind: GraphEdgeKind::Incoming,
            });
        }
        for parent in parents {
            let to = positions[parent.as_str()];
            edges.push(GraphEdge {
                from: lane,
                to,
                color: self.lanes[to].color,
                kind: GraphEdgeKind::Parent,
            });
        }
        GraphRow {
            lane,
            color,
            width: before.len().max(self.lanes.len()),
            edges,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Layout;
    use crate::model::GraphEdgeKind;

    #[test]
    fn merge_lanes_rejoin_at_the_shared_ancestor() {
        let mut layout = Layout::default();
        let merge = layout.push("merge", &["left".into(), "right".into()]);
        assert_eq!(
            merge
                .edges
                .iter()
                .filter(|edge| edge.kind == GraphEdgeKind::Parent)
                .count(),
            2
        );
        let left = layout.push("left", &["root".into()]);
        let right = layout.push("right", &["root".into()]);
        let root = layout.push("root", &[]);
        assert_eq!(left.lane, 0);
        assert_eq!(right.lane, 1);
        assert!(right
            .edges
            .iter()
            .any(|edge| edge.kind == GraphEdgeKind::Parent && edge.to == 0));
        assert_eq!(root.width, 1);
        assert!(layout.lanes.is_empty());
    }

    #[test]
    fn wide_rows_reserve_only_the_segment_upper_bound() {
        let mut layout = Layout::default();
        for index in 0..5000 {
            layout.push(&format!("branch-{index}"), &[format!("parent-{index}")]);
        }
        let row = layout.push("parent-0", &["root".into()]);
        assert_eq!(row.edges.len(), 5001);
        assert_eq!(row.edges.capacity(), row.edges.len());
        assert_eq!(row.width, 5000);
        assert!(row
            .edges
            .iter()
            .any(|edge| { edge.kind == GraphEdgeKind::Parent && edge.from == 0 && edge.to == 0 }));
    }
}
