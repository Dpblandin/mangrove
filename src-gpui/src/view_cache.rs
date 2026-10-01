//! Reuse unchanged views inside a parent that supplies a definite size.
use gpui_kit::{prelude::*, *};

pub fn render_cached_view<T: Render + 'static>(view: Entity<T>, window: &Window) -> AnyElement {
    // gpui-pre 0.3.7 replays cached paint/input records, but not accessibility
    // nodes. Build the full subtree while assistive technology is connected.
    // GPUI forces a redraw when accessibility becomes active.
    if window.is_a11y_active() {
        view.into_any_element()
    } else {
        view.cached(StyleRefinement::default().size_full())
            .into_any_element()
    }
}
