use crate::theme::Palette;
use gpui_kit::{prelude::*, *};
use mangrove_core::model::CommitRow;
use std::sync::Arc;

pub const ROW_HEIGHT: f32 = 44.;

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
            let graph = &row.graph;
            let x =
                |lane: usize| bounds.left() + px(22. + (lane as f32 - lane_offset as f32) * 18.);
            let middle = bounds.top() + px(ROW_HEIGHT / 2.);
            if !filtered {
                for edge in &graph.edges {
                    let from = x(edge.from);
                    let to = x(edge.to);
                    let y1 = if edge.kind == "parent" {
                        middle
                    } else {
                        bounds.top()
                    };
                    let y2 = if edge.kind == "incoming" {
                        middle
                    } else {
                        bounds.bottom()
                    };
                    let mut path = PathBuilder::stroke(px(1.65));
                    path.move_to(point(from, y1));
                    if from == to {
                        path.line_to(point(to, y2));
                    } else {
                        path.cubic_bezier_to(point(to, y2), point(from, middle), point(to, middle));
                    }
                    if let Ok(path) = path.build() {
                        let mut color: Hsla =
                            rgb(palette.graph[edge.color % palette.graph.len()]).into();
                        color.a = 0.68;
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
        },
    )
    .w(px(width))
    .h(px(ROW_HEIGHT))
    .flex_shrink_0()
    .overflow_hidden()
}
