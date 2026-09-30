//! Scroll variable virtual lists from their complete row geometry rather than
//! GPUI's measured-only SumTree. The existing ListState remains the only scroll
//! position; this module supplies its pixel coordinates and wheel dispatch.
use std::{cell::RefCell, ops::Range, rc::Rc, sync::Arc};

use gpui_kit::{component::scroll::ScrollbarHandle, prelude::*, *};

#[derive(Clone)]
pub struct RowGeometry {
    offsets: Arc<Vec<Pixels>>,
}

impl Default for RowGeometry {
    fn default() -> Self {
        Self {
            offsets: Arc::new(vec![px(0.)]),
        }
    }
}

impl RowGeometry {
    pub fn from_heights(heights: impl IntoIterator<Item = Pixels>) -> Self {
        let heights = heights.into_iter();
        let mut offsets = Vec::with_capacity(heights.size_hint().0.saturating_add(1));
        offsets.push(px(0.));
        for height in heights {
            offsets.push(*offsets.last().unwrap() + height);
        }
        Self {
            offsets: Arc::new(offsets),
        }
    }

    /// Snap cumulative CSS boundaries, not each row individually. Rendering
    /// height(index) preserves the average line height at either screen scale.
    pub fn from_tenths(heights: impl IntoIterator<Item = f64>, scale_factor: f32) -> Self {
        let heights = heights.into_iter();
        let mut offsets = Vec::with_capacity(heights.size_hint().0.saturating_add(1));
        offsets.push(px(0.));
        let mut tenths = 0.;
        for height in heights {
            tenths += height;
            // GPUI rounds physical half-pixels towards zero.
            let physical = (tenths * f64::from(scale_factor) / 10. - 0.5).ceil();
            offsets.push(px(physical as f32 / scale_factor));
        }
        Self {
            offsets: Arc::new(offsets),
        }
    }

    pub fn total(&self) -> Pixels {
        self.offsets.last().copied().unwrap_or(px(0.))
    }

    pub fn start(&self, index: usize) -> Pixels {
        self.offsets
            .get(index)
            .copied()
            .unwrap_or_else(|| self.total())
    }

    pub fn height(&self, index: usize) -> Option<Pixels> {
        self.offsets
            .get(index..index + 2)
            .map(|pair| pair[1] - pair[0])
    }

    pub fn list_offset(&self, position: Pixels, viewport_height: Pixels) -> ListOffset {
        let position = position.clamp(px(0.), (self.total() - viewport_height).max(px(0.)));
        let item_ix = self
            .offsets
            .partition_point(|&top| top <= position)
            .saturating_sub(1)
            .min(self.offsets.len().saturating_sub(2));
        ListOffset {
            item_ix,
            offset_in_item: position - self.start(item_ix),
        }
    }

    fn splice(&mut self, range: Range<usize>, heights: impl IntoIterator<Item = Pixels>) {
        let mut previous: Vec<_> = self
            .offsets
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .collect();
        drop(previous.splice(range, heights));
        *self = Self::from_heights(previous);
    }

    fn measured_height(&mut self, index: usize, height: Pixels) {
        let Some(previous) = self.height(index) else {
            return;
        };
        let difference = height - previous;
        if difference.abs() < px(0.1) {
            return;
        }
        for top in &mut Arc::make_mut(&mut self.offsets)[index + 1..] {
            *top += difference;
        }
    }
}

#[derive(Clone, Copy)]
enum HeightPolicy {
    Exact,
    MeasureVisible,
}

/// Adapts an existing variable ListState, optionally with its horizontal
/// ScrollHandle. Clones share geometry; they never add a second scroll position.
#[derive(Clone)]
pub struct VirtualScrollHandle {
    list: ListState,
    horizontal: Option<ScrollHandle>,
    geometry: Rc<RefCell<RowGeometry>>,
    policy: HeightPolicy,
}

impl VirtualScrollHandle {
    pub fn exact(list: ListState, horizontal: ScrollHandle, geometry: RowGeometry) -> Self {
        Self {
            list,
            horizontal: Some(horizontal),
            geometry: Rc::new(RefCell::new(geometry)),
            policy: HeightPolicy::Exact,
        }
    }

    /// Wrapping notices start with estimates and refine only already rendered
    /// rows. Their geometry is frozen while the scrollbar thumb is dragged.
    pub fn measured(list: ListState) -> Self {
        Self {
            list,
            horizontal: None,
            geometry: Rc::default(),
            policy: HeightPolicy::MeasureVisible,
        }
    }

    pub fn set_geometry(&self, geometry: RowGeometry) {
        *self.geometry.borrow_mut() = geometry;
    }

    pub fn set_heights(&self, heights: impl IntoIterator<Item = Pixels>) {
        self.set_geometry(RowGeometry::from_heights(heights));
    }

    pub fn splice(&self, range: Range<usize>, heights: impl IntoIterator<Item = Pixels>) {
        let heights: Vec<_> = heights.into_iter().collect();
        self.list.splice(range.clone(), heights.len());
        self.geometry.borrow_mut().splice(range, heights);
    }

    /// Safe inside the ListState render callback: reads only our geometry, not
    /// the ListState that GPUI is currently borrowing mutably.
    pub fn row_height(&self, index: usize) -> Option<Pixels> {
        self.geometry.borrow().height(index)
    }

    fn refresh_visible_heights(&self) {
        if !matches!(self.policy, HeightPolicy::MeasureVisible) || self.list.is_scrollbar_dragging()
        {
            return;
        }
        let viewport = self.list.viewport_bounds();
        let mut geometry = self.geometry.borrow_mut();
        for index in self.list.logical_scroll_top().item_ix..self.list.item_count() {
            let Some(bounds) = self.list.bounds_for_item(index) else {
                break;
            };
            geometry.measured_height(index, bounds.size.height);
            if bounds.bottom() >= viewport.bottom() {
                break;
            }
        }
    }

    pub fn vertical_offset(&self) -> Pixels {
        self.refresh_visible_heights();
        let top = self.list.logical_scroll_top();
        let geometry = self.geometry.borrow();
        (geometry.start(top.item_ix) + top.offset_in_item).clamp(
            px(0.),
            (geometry.total() - self.viewport_bounds().size.height).max(px(0.)),
        )
    }

    pub fn scroll_vertical_to(&self, position: Pixels) {
        self.refresh_visible_heights();
        let geometry = self.geometry.borrow();
        let height = self.viewport_bounds().size.height;
        let maximum = (geometry.total() - height).max(px(0.));
        // An unseen wrapping notice may exceed its estimate. Let the native
        // list backfill the real final viewport without measuring other rows.
        let end_tolerance = if matches!(self.policy, HeightPolicy::MeasureVisible) {
            px(1.)
        } else {
            px(0.)
        };
        if maximum > px(0.) && position >= maximum - end_tolerance {
            self.list.scroll_to_end();
            return;
        }
        let target = geometry.list_offset(position, height);
        let current = self.list.logical_scroll_top();
        if current.item_ix != target.item_ix || current.offset_in_item != target.offset_in_item {
            self.list.scroll_to(target);
        }
    }

    pub fn scroll_vertical_by(&self, distance: Pixels) {
        self.scroll_vertical_to(self.vertical_offset() + distance);
    }

    pub fn reveal_row(&self, index: usize) {
        let (start, end) = {
            let geometry = self.geometry.borrow();
            let Some(height) = geometry.height(index) else {
                return;
            };
            (geometry.start(index), geometry.start(index) + height)
        };
        let top = self.vertical_offset();
        let height = self.viewport_bounds().size.height;
        if start < top {
            self.scroll_vertical_to(start);
        } else if end > top + height {
            self.scroll_vertical_to(end - height);
        }
    }

    pub fn scroll_wheel(&self, delta: ScrollDelta, shift: bool) -> bool {
        let mut delta = delta.pixel_delta(px(20.));
        if self.horizontal.is_none() && delta.y == px(0.) {
            return false;
        }
        if self.horizontal.is_some() && shift && delta.x == px(0.) {
            delta.x = delta.y;
            delta.y = px(0.);
        }
        let before = self.offset();
        self.set_offset(before + delta);
        self.offset() != before
    }

    /// Place before the list and scrollbar as a sibling covering the viewport.
    /// Occluded menus/modals retain their wheel input. Always suppress GPUI's
    /// measured-only ListState/Div bubble scroll, including horizontal/zero.
    pub fn wheel_capture(
        &self,
        mut on_scroll: impl FnMut(bool, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        let handle = self.clone();
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |_, hitbox, window, _| {
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || !hitbox.should_handle_scroll(window) {
                        return;
                    }
                    let changed = handle.scroll_wheel(event.delta, event.modifiers.shift);
                    on_scroll(changed, window, cx);
                    cx.stop_propagation();
                });
            },
        )
        .absolute()
        .size_full()
        .into_any_element()
    }
}

impl ScrollbarHandle for VirtualScrollHandle {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.horizontal
            .as_ref()
            .map_or_else(|| self.list.viewport_bounds(), ScrollHandle::bounds)
    }

    fn offset(&self) -> Point<Pixels> {
        point(
            self.horizontal
                .as_ref()
                .map_or(px(0.), |handle| handle.offset().x),
            -self.vertical_offset(),
        )
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        if let Some(horizontal) = &self.horizontal {
            let max_x = horizontal.max_offset().x.max(px(0.));
            horizontal.set_offset(point(offset.x.clamp(-max_x, px(0.)), px(0.)));
        }
        self.scroll_vertical_to(-offset.y);
    }

    fn content_size(&self) -> Size<Pixels> {
        self.refresh_visible_heights();
        size(
            self.viewport_bounds().size.width
                + self
                    .horizontal
                    .as_ref()
                    .map_or(px(0.), |handle| handle.max_offset().x),
            self.geometry.borrow().total(),
        )
    }

    fn start_drag(&self) {
        if matches!(self.policy, HeightPolicy::MeasureVisible) {
            self.refresh_visible_heights();
            self.list.scrollbar_drag_started();
        }
    }

    fn end_drag(&self) {
        if matches!(self.policy, HeightPolicy::MeasureVisible) {
            self.list.scrollbar_drag_ended();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RowGeometry, VirtualScrollHandle};
    use gpui_kit::{
        ListAlignment, ListState, ScrollDelta, ScrollHandle, component::scroll::ScrollbarHandle,
        point, px,
    };

    fn near(actual: gpui_kit::Pixels, expected: f32) {
        assert!((f32::from(actual) - expected).abs() < 0.01);
    }

    #[test]
    fn exact_and_measured_lists_share_clamped_pixel_scroll_without_native_measurements() {
        for measured in [false, true] {
            let list = ListState::new(10_000, ListAlignment::Top, px(150.));
            let handle = if measured {
                let handle = VirtualScrollHandle::measured(list.clone());
                handle.set_heights((0..10_000).map(|_| px(32.)));
                handle
            } else {
                VirtualScrollHandle::exact(
                    list.clone(),
                    ScrollHandle::new(),
                    RowGeometry::from_heights((0..10_000).map(|_| px(32.))),
                )
            };
            near(handle.content_size().height, 320_000.);
            handle.set_offset(point(px(0.), px(-160_003.)));
            assert_eq!(list.logical_scroll_top().item_ix, 5_000);
            for _ in 0..6 {
                assert!(handle.scroll_wheel(ScrollDelta::Pixels(point(px(0.), px(-40.))), false));
            }
            near(handle.vertical_offset(), 160_243.);
            assert_eq!(list.logical_scroll_top().item_ix, 5_007);
            near(list.logical_scroll_top().offset_in_item, 19.);
            assert!(handle.scroll_wheel(ScrollDelta::Lines(point(0., 3.)), false));
            near(handle.vertical_offset(), 160_183.);
            handle.set_offset(point(px(0.), px(-1_000_000.)));
            assert_eq!(list.logical_scroll_top().item_ix, 10_000);
            near(handle.vertical_offset(), 320_000.);
            handle.set_offset(point(px(0.), px(1_000_000.)));
            near(handle.vertical_offset(), 0.);
        }
    }

    #[test]
    fn scrollbar_splice_retains_measured_wrapping_notice_height() {
        let mut geometry = RowGeometry::from_heights([px(26.5), px(31.), px(32.), px(24.)]);
        geometry.measured_height(1, px(65.));
        let list = ListState::new(4, ListAlignment::Top, px(150.));
        let handle = VirtualScrollHandle::measured(list.clone());
        handle.set_geometry(geometry);
        handle.splice(2..3, (0..10_000).map(|_| px(32.)));
        assert_eq!(list.item_count(), 10_003);
        near(handle.content_size().height, 320_115.5);
        handle.scroll_vertical_to(px(91.5));
        assert_eq!(list.logical_scroll_top().item_ix, 2);
        near(list.logical_scroll_top().offset_in_item, 0.);
        handle.scroll_vertical_to(px(80.));
        assert_eq!(list.logical_scroll_top().item_ix, 1);
        near(list.logical_scroll_top().offset_in_item, 53.5);
    }

    #[test]
    fn geometry_has_physical_heights_and_nonzero_viewport_clamps_after_zoom() {
        for scale in [1., 2.] {
            for font in [9., 11., 20.] {
                let geometry = RowGeometry::from_tenths((0..1_000).map(|_| font * 17.), scale);
                near(geometry.total(), font as f32 * 1_700.);
                for row in 0..1_000 {
                    let physical = f32::from(geometry.height(row).unwrap()) * scale;
                    assert_eq!(physical.fract(), 0.);
                }
                let target = geometry.list_offset(px(1_000_000.), px(600.));
                near(
                    geometry.start(target.item_ix) + target.offset_in_item,
                    font as f32 * 1_700. - 600.,
                );
                let top = geometry.list_offset(px(-10.), px(600.));
                assert_eq!(top.item_ix, 0);
                near(top.offset_in_item, 0.);
            }
            let mixed = RowGeometry::from_tenths([287., 187., 297., 187.], scale);
            near(mixed.total(), 96.);
            let last = mixed.list_offset(px(1_000.), px(30.));
            assert_eq!(last.item_ix, 2);
            near(mixed.start(last.item_ix) + last.offset_in_item, 66.);
        }
        let empty = RowGeometry::default().list_offset(px(100.), px(600.));
        assert_eq!(empty.item_ix, 0);
        near(empty.offset_in_item, 0.);
    }

    #[test]
    fn shared_handles_observe_new_geometry_and_preserve_each_wheel_axis_policy() {
        let list = ListState::new(10, ListAlignment::Top, px(150.));
        let exact = VirtualScrollHandle::exact(
            list.clone(),
            ScrollHandle::new(),
            RowGeometry::from_tenths([187.; 10], 1.),
        );
        let clone = exact.clone();
        exact.scroll_vertical_to(px(60.));
        // Shift redirects a code wheel to its horizontal scroller; an empty
        // horizontal extent must not silently scroll vertically instead.
        assert!(!exact.scroll_wheel(ScrollDelta::Pixels(point(px(0.), px(-40.))), true));
        near(exact.vertical_offset(), 60.);
        list.reset(10);
        exact.set_geometry(RowGeometry::from_tenths([340.; 10], 2.));
        clone.scroll_vertical_to(px(100.));
        near(exact.vertical_offset(), 100.);
        near(clone.content_size().height, 340.);
        exact.reveal_row(0);
        near(clone.vertical_offset(), 0.);

        let measured =
            VirtualScrollHandle::measured(ListState::new(10, ListAlignment::Top, px(150.)));
        measured.set_heights([px(32.); 10]);
        measured.scroll_vertical_to(px(100.));
        assert!(!measured.scroll_wheel(ScrollDelta::Pixels(point(px(40.), px(0.))), false));
        assert!(measured.scroll_wheel(ScrollDelta::Pixels(point(px(0.), px(-40.))), true));
        near(measured.vertical_offset(), 140.);
    }
}
