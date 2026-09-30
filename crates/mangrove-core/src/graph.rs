use std::collections::HashMap;

use crate::model::{GraphEdge, GraphRow};

struct Lane {
    sha: String,
    color: usize,
}

#[derive(Default)]
pub(crate) struct Layout {
    lanes: Vec<Lane>,
    next_color: usize,
}

impl Layout {
    fn color(&mut self) -> usize {
        let color = self.next_color % 8;
        self.next_color += 1;
        color
    }

    pub fn push(&mut self, sha: &str, parents: &[String]) -> GraphRow {
        let existing = self.lanes.iter().position(|lane| lane.sha == sha);
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
            if !self.lanes.iter().any(|entry| entry.sha == *parent) {
                let parent_color = if index == 0 { color } else { self.color() };
                self.lanes.insert(
                    (lane + index).min(self.lanes.len()),
                    Lane {
                        sha: parent.clone(),
                        color: parent_color,
                    },
                );
            }
        }
        let positions: HashMap<_, _> = self
            .lanes
            .iter()
            .enumerate()
            .map(|(index, lane)| (lane.sha.as_str(), index))
            .collect();
        let mut edges = Vec::new();
        for (from, (sha, color)) in before.iter().enumerate() {
            if from != lane {
                if let Some(&to) = positions.get(sha.as_str()) {
                    edges.push(GraphEdge {
                        from,
                        to,
                        color: *color,
                        kind: "through".into(),
                    });
                }
            }
        }
        if existing.is_some() {
            edges.push(GraphEdge {
                from: lane,
                to: lane,
                color,
                kind: "incoming".into(),
            });
        }
        for parent in parents {
            let to = positions[parent.as_str()];
            edges.push(GraphEdge {
                from: lane,
                to,
                color: self.lanes[to].color,
                kind: "parent".into(),
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

    #[test]
    fn merge_lanes_rejoin_at_the_shared_ancestor() {
        let mut layout = Layout::default();
        let merge = layout.push("merge", &["left".into(), "right".into()]);
        assert_eq!(
            merge
                .edges
                .iter()
                .filter(|edge| edge.kind == "parent")
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
            .any(|edge| edge.kind == "parent" && edge.to == 0));
        assert_eq!(root.width, 1);
        assert!(layout.lanes.is_empty());
    }
}
