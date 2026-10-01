use crate::theme::Palette;
use gpui_kit::{prelude::*, *};
use mangrove_core::model::{CommitRow, GraphEdgeKind};
use std::sync::Arc;

pub const ROW_HEIGHT: f32 = 44.;
const EDGE_WIDTH: f32 = 1.65;

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
                let x = |lane: usize| {
                    bounds.left() + px(22. + (lane as f32 - lane_offset as f32) * 18.)
                };
                let middle = bounds.top() + px(ROW_HEIGHT / 2.);
                if !filtered {
                    for edge in &graph.edges {
                        let from_x = 22. + (edge.from as f32 - lane_offset as f32) * 18.;
                        let to_x = 22. + (edge.to as f32 - lane_offset as f32) * 18.;
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
                    bounds.left() + px(22.)
                } else {
                    x(graph.lane)
                };
                if selected {
                    let mut halo: Hsla = rgb(palette.accent).into();
                    halo.a = 0.18;
                    window.paint_quad(quad(
                        Bounds::new(
                            point(node_x - px(9.), middle - px(9.)),
                            size(px(18.), px(18.)),
                        ),
                        px(9.),
                        halo,
                        px(0.),
                        halo,
                        BorderStyle::Solid,
                    ));
                }
                let radius = if row.parents.len() > 1 { 4.8 } else { 3.8 };
                let node_bounds = Bounds::new(
                    point(node_x - px(radius), middle - px(radius)),
                    size(px(radius * 2.), px(radius * 2.)),
                );
                window.paint_quad(quad(
                    node_bounds,
                    px(radius),
                    rgb(if selected { palette.text } else { palette.bg }),
                    px(if selected { 2.3 } else { 1.9 }),
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
    use super::edge_visible;

    #[test]
    fn culling_keeps_crossing_curves_and_the_stroke_margin() {
        assert!(edge_visible(-18., 160., 116.));
        assert!(edge_visible(160., -18., 116.));
        assert!(edge_visible(116.8, 116.8, 116.));
        assert!(edge_visible(-0.8, -0.8, 116.));
        assert!(!edge_visible(117., 200., 116.));
        assert!(!edge_visible(-30., -1., 116.));
    }

    #[test]
    fn wide_graph_only_admits_visible_lanes_at_any_horizontal_offset() {
        for offset in [0, 2500, 4990] {
            let visible = (0..5000)
                .filter(|lane| {
                    let x = 22. + (*lane as f32 - offset as f32) * 18.;
                    edge_visible(x, x, 116.)
                })
                .count();
            assert!(visible <= 7);
        }
    }
}
