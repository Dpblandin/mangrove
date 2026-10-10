use crate::theme::Palette;
use gpui_kit::{prelude::*, *};
use mangrove_core::model::{CommitRow, GraphEdgeKind};
use std::sync::Arc;

pub const ROW_HEIGHT: f32 = 28.;
const LANE_START: f32 = 14.;
const LANE_WIDTH: f32 = 14.;
const EDGE_WIDTH: f32 = 1.5;

fn lane_x(lane: usize, offset: usize) -> f32 {
    LANE_START + (lane as f32 - offset as f32) * LANE_WIDTH
}

/// Lanes that keep at least one lane of margin inside a graph column.
pub fn visible_lanes(width: f32) -> usize {
    ((width - LANE_START) / LANE_WIDTH).floor().max(1.) as usize
}

// Every Bézier control point has one of the endpoint x coordinates, so this
// conservative interval also keeps curves that cross the visible column.
fn edge_visible(from: f32, to: f32, width: f32) -> bool {
    from.min(to) - EDGE_WIDTH / 2. <= width && from.max(to) + EDGE_WIDTH / 2. >= 0.
}

/// Draw graph geometry only for the virtualized rows.
pub fn row(
    row: Arc<CommitRow>,
    selected: bool,
    filtered: bool,
    lane_offset: usize,
    width: f32,
    palette: Palette,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                let graph = &row.graph;
                let x = |lane: usize| bounds.left() + px(lane_x(lane, lane_offset));
                let middle = bounds.top() + px(ROW_HEIGHT / 2.);
                if !filtered {
                    for edge in &graph.edges {
                        let from_x = lane_x(edge.from, lane_offset);
                        let to_x = lane_x(edge.to, lane_offset);
                        if !edge_visible(from_x, to_x, f32::from(bounds.size.width)) {
                            continue;
                        }
                        let from = x(edge.from);
                        let to = x(edge.to);
                        let y1 = if edge.kind == GraphEdgeKind::Parent {
                            middle
                        } else {
                            bounds.top()
                        };
                        let y2 = if edge.kind == GraphEdgeKind::Incoming {
                            middle
                        } else {
                            bounds.bottom()
                        };
                        let mut color: Hsla =
                            rgb(palette.graph[edge.color % palette.graph.len()]).into();
                        color.a = 0.68;
                        if from == to {
                            // The path's default butt caps have the same rectangle
                            // geometry, without allocating or tessellating a path.
                            window.paint_quad(quad(
                                Bounds::new(
                                    point(from - px(EDGE_WIDTH / 2.), y1),
                                    size(px(EDGE_WIDTH), y2 - y1),
                                ),
                                px(0.),
                                color,
                                px(0.),
                                color,
                                BorderStyle::Solid,
                            ));
                            continue;
                        }
                        let mut path = PathBuilder::stroke(px(EDGE_WIDTH));
                        path.move_to(point(from, y1));
                        path.cubic_bezier_to(point(to, y2), point(from, middle), point(to, middle));
                        if let Ok(path) = path.build() {
                            window.paint_path(path, color);
                        }
                    }
                }
                let node_x = if filtered {
                    bounds.left() + px(LANE_START)
                } else {
                    x(graph.lane)
                };
                if selected {
                    let mut halo: Hsla = rgb(palette.accent).into();
                    halo.a = 0.18;
                    window.paint_quad(quad(
                        Bounds::new(
                            point(node_x - px(7.), middle - px(7.)),
                            size(px(14.), px(14.)),
                        ),
                        px(7.),
                        halo,
                        px(0.),
                        halo,
                        BorderStyle::Solid,
                    ));
                }
                let radius = if row.parents.len() > 1 { 4.3 } else { 3.5 };
                let node_bounds = Bounds::new(
                    point(node_x - px(radius), middle - px(radius)),
                    size(px(radius * 2.), px(radius * 2.)),
                );
                window.paint_quad(quad(
                    node_bounds,
                    px(radius),
                    rgb(if selected { palette.text } else { palette.bg }),
                    px(if selected { 2. } else { 1.7 }),
                    rgb(palette.graph[graph.color % palette.graph.len()]),
                    BorderStyle::Solid,
                ));
            });
        },
    )
    .w(px(width))
    .h(px(ROW_HEIGHT))
    .flex_shrink_0()
    .overflow_hidden()
}

#[cfg(test)]
mod tests {
    use super::{LANE_WIDTH, edge_visible, lane_x, visible_lanes};

    #[test]
    fn culling_keeps_crossing_curves_and_the_stroke_margin() {
        assert!(edge_visible(-18., 160., 116.));
        assert!(edge_visible(160., -18., 116.));
        assert!(edge_visible(116.7, 116.7, 116.));
        assert!(edge_visible(-0.7, -0.7, 116.));
        assert!(!edge_visible(117., 200., 116.));
        assert!(!edge_visible(-30., -1., 116.));
    }

    #[test]
    fn wide_graph_only_admits_visible_lanes_at_any_horizontal_offset() {
        for offset in [0, 2500, 4990] {
            let visible = (0..5000)
                .filter(|lane| {
                    let x = lane_x(*lane, offset);
                    edge_visible(x, x, 116.)
                })
                .count();
            assert!(visible <= 9);
        }
    }

    #[test]
    fn visible_lanes_keep_whole_nodes_inside_the_column() {
        for width in [72., 92., 116.] {
            let lanes = visible_lanes(width);
            assert!(lane_x(lanes - 1, 0) <= width - LANE_WIDTH);
            assert!(lane_x(lanes, 0) > width - LANE_WIDTH);
        }
        assert_eq!(visible_lanes(1.), 1);
    }
}
