//! Building blocks shared by every settings section and the clone screen, so
//! their headers, cards, fields, feedback and navigation look and behave alike.
use crate::theme::{Palette, RADIUS_LG, RADIUS_MD, icon, mix};
use gpui_kit::assets::IconName;
use gpui_kit::{
    base::{Button, Radio},
    component::{
        Sizable,
        input::{Input, InputState},
    },
    prelude::*,
    *,
};

/// Space around a page's content.
pub const PAGE_PADDING: f32 = 32.;
/// Space between a page header and its cards, and between cards.
pub const PAGE_GAP: f32 = 24.;
pub const CONTENT_MAX_WIDTH: f32 = 760.;
/// Fields, selects and buttons share one height so that a row stays aligned.
pub const CONTROL_HEIGHT: f32 = 36.;
/// Settings navigation and the provider list share this width.
pub const NAV_WIDTH: f32 = 226.;

/// A field outline that keeps 3:1 against the cards in every theme.
pub fn field_border(p: Palette) -> u32 {
    mix(p.muted, p.border, 0.4)
}

/// The pointer's fill on a panel, where the app-wide hover tint disappears.
pub fn hover_fill(p: Palette) -> u32 {
    mix(p.text, p.panel, 0.1)
}

/// The fill of a chosen option on a panel, lighter than the panel in every
/// theme, including the near-black GitLab theme.
pub fn selected_fill(p: Palette) -> u32 {
    mix(p.accent, p.panel, 0.14)
}

/// The keyboard focus: a 2 px ring along the control's edge. It is drawn
/// inside, so it neither fills a control without background nor gets clipped.
pub fn focus_ring(style: StyleRefinement, color: u32) -> StyleRefinement {
    style.border_color(rgb(color)).shadow(vec![
        BoxShadow::new(px(0.), px(0.), rgb(color).into())
            .spread_radius(px(1.))
            .inset(),
    ])
}

pub fn page_header(
    title: &str,
    description: impl Into<SharedString>,
    p: Palette,
    offset: f32,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .text_size(px(23. + offset))
                .line_height(relative(1.3))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.to_string()),
        )
        .child(note(description, p))
}

pub fn card(p: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .min_w_0()
        .w_full()
        .p(px(24.))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(RADIUS_LG)
        .bg(rgb(p.panel))
}

pub fn card_title(label: &str, offset: f32) -> Div {
    div()
        .text_size(px(15. + offset))
        .line_height(relative(1.4))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label.to_string())
}

pub fn note(label: impl Into<SharedString>, p: Palette) -> Div {
    div()
        .min_w_0()
        .whitespace_normal()
        .line_height(relative(1.6))
        .text_color(rgb(p.muted))
        .child(label.into())
}

/// The label above a field, or beside a control in a two-column row.
pub fn field_label(label: &str, p: Palette, offset: f32) -> Div {
    div()
        .text_size(px(12. + offset))
        .line_height(relative(1.4))
        .text_color(rgb(p.text))
        .font_weight(FontWeight::MEDIUM)
        .child(label.to_string())
}

/// A 36 px single-line field whose outline turns to the accent with focus.
pub fn input_control(
    state: &Entity<InputState>,
    id: &'static str,
    label: impl Into<SharedString>,
    p: Palette,
    offset: f32,
) -> Input {
    // The kit draws the focus outline only for its own appearance; its colors
    // come from the synchronized theme and the style below refines the rest.
    Input::new(state)
        .id(id)
        .aria_label(label)
        .with_size(px(CONTROL_HEIGHT))
        .w_full()
        .min_w_0()
        .min_h(px(CONTROL_HEIGHT))
        .px(px(12.))
        .py_0()
        .border_1()
        .border_color(rgb(field_border(p)))
        .rounded(RADIUS_MD)
        .bg(rgb(p.bg))
        .text_color(rgb(p.text))
        .text_size(px(12. + offset))
}

/// A labelled field. The label stays visible once the field holds a value,
/// unlike its placeholder.
pub fn field(
    state: &Entity<InputState>,
    id: &'static str,
    label: &'static str,
    p: Palette,
    offset: f32,
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(field_label(label, p, offset))
        .child(input_control(state, id, label, p, offset))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Progress,
    Success,
    Error,
}

/// A short message under the field or action it concerns.
pub fn feedback(tone: Tone, text: impl Into<SharedString>, p: Palette, offset: f32) -> Div {
    let (glyph, color) = match tone {
        Tone::Progress => (IconName::LoaderCircle, p.accent),
        Tone::Success => (IconName::CircleCheck, p.success),
        Tone::Error => (IconName::CircleAlert, p.danger),
    };
    div()
        .flex()
        .items_start()
        .gap(px(8.))
        .min_w_0()
        .text_size(px(12. + offset))
        .line_height(relative(1.5))
        .text_color(rgb(if tone == Tone::Progress {
            p.muted
        } else {
            color
        }))
        .child(
            div()
                .flex_none()
                .mt(px(1.5 + offset * 0.75))
                .child(icon(glyph, 14., color)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_normal()
                .child(text.into()),
        )
}

fn action_base(id: impl Into<SharedString>, label: &str, p: Palette, offset: f32) -> Button {
    Button::new(id.into())
        .min_h(px(CONTROL_HEIGHT))
        .px(px(14.))
        .py(px(6.))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(RADIUS_MD)
        .text_size(px(12. + offset))
        .line_height(relative(1.3))
        .text_color(rgb(p.text))
        .cursor_pointer()
        .accessibility_label(label.to_string())
        .focus_visible(move |style| focus_ring(style, p.accent))
        .styles(|styles| styles.disabled(|style| style.opacity(0.5).cursor_default()))
        .child(div().min_w_0().whitespace_normal().child(label.to_string()))
}

/// A bordered secondary action.
pub fn action(id: impl Into<SharedString>, label: &str, p: Palette, offset: f32) -> Button {
    action_base(id, label, p, offset).hover(move |style| style.bg(rgb(hover_fill(p))))
}

pub fn primary_action(id: impl Into<SharedString>, label: &str, p: Palette, offset: f32) -> Button {
    action_base(id, label, p, offset)
        .bg(rgb(p.accent))
        .border_color(rgb(p.accent))
        .text_color(rgb(p.on_accent))
        .font_weight(FontWeight::SEMIBOLD)
        .hover(move |style| style.bg(rgb(mix(p.accent, p.text, 0.85))))
        // An accent ring would merge with the fill.
        .focus_visible(move |style| focus_ring(style, p.text))
}

/// An action that removes data or credentials.
pub fn danger_action(id: impl Into<SharedString>, label: &str, p: Palette, offset: f32) -> Button {
    action_base(id, label, p, offset)
        .text_color(rgb(p.danger))
        .border_color(rgb(mix(p.danger, p.border, 0.45)))
        .hover(move |style| style.bg(rgb(mix(p.danger, p.panel, 0.12))))
        .focus_visible(move |style| focus_ring(style, p.danger))
}

/// A text-only action for secondary or inline commands.
pub fn link_action(id: impl Into<SharedString>, label: &str, p: Palette, offset: f32) -> Button {
    action_base(id, label, p, offset)
        .border_color(transparent_black())
        .text_color(rgb(p.accent))
        .px(px(6.))
        .hover(move |style| style.bg(rgb(hover_fill(p))))
}

/// A navigation entry: the current page carries an accent bar, the pointer a
/// neutral fill and the keyboard focus an accent outline, so the three states
/// stay distinct in every theme.
pub fn nav_item(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    selected: bool,
    p: Palette,
    offset: f32,
) -> Button {
    let label = label.into();
    Button::new(id.into())
        .relative()
        .w_full()
        .min_w_0()
        .min_h(px(40.))
        .pl(px(14.))
        .pr(px(10.))
        .py(px(8.))
        .gap(px(10.))
        .justify_start()
        .rounded(RADIUS_MD)
        .border_1()
        .border_color(transparent_black())
        .text_size(px(13. + offset))
        .line_height(relative(1.3))
        .text_color(rgb(if selected { p.text } else { p.muted }))
        .when(selected, |button| {
            button
                .bg(rgb(selected_fill(p)))
                .font_weight(FontWeight::MEDIUM)
                .child(
                    div()
                        .absolute()
                        .left(px(-1.))
                        .top(px(8.))
                        .bottom(px(8.))
                        .w(px(3.))
                        .rounded_full()
                        .bg(rgb(p.accent)),
                )
        })
        .when(!selected, |button| {
            button.hover(move |style| style.bg(rgb(hover_fill(p))).text_color(rgb(p.text)))
        })
        .cursor_pointer()
        .selected(selected)
        .accessibility_label(label)
        .focus_visible(move |style| focus_ring(style, p.accent))
        .styles(|styles| styles.disabled(|style| style.opacity(0.5).cursor_default()))
}

/// A checkbox whose box keeps the field outline's contrast.
pub fn checkbox(
    id: &'static str,
    label: &str,
    checked: bool,
    p: Palette,
    offset: f32,
) -> gpui_kit::base::Checkbox {
    gpui_kit::base::Checkbox::new(id)
        .checked(checked)
        .accessibility_label(label.to_string())
        .flex()
        .items_center()
        .gap(px(10.))
        .py(px(4.))
        .px(px(4.))
        .rounded(RADIUS_MD)
        .border_1()
        .border_color(transparent_black())
        .cursor_pointer()
        .text_size(px(12. + offset))
        .text_color(rgb(p.text))
        .focus_visible(move |style| focus_ring(style, p.accent))
        .styles(|styles| styles.disabled(|style| style.opacity(0.5).cursor_default()))
        .child(
            div()
                .size(px(16.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .border_1()
                .border_color(rgb(if checked { p.accent } else { field_border(p) }))
                .bg(rgb(if checked { p.accent } else { p.bg }))
                .when(checked, |mark| {
                    mark.child(icon(IconName::Check, 12., p.on_accent))
                }),
        )
        .child(label.to_string())
}

/// An option of a radio group drawn as an outlined tile.
pub fn choice(id: impl Into<ElementId>, checked: bool, p: Palette) -> Radio {
    Radio::new(id)
        .checked(checked)
        .flex()
        .items_center()
        .rounded(RADIUS_MD)
        .border_1()
        .border_color(rgb(if checked { p.accent } else { p.border }))
        .bg(rgb(if checked { selected_fill(p) } else { p.panel }))
        .cursor_pointer()
        .when(!checked, |radio| {
            radio.hover(move |style| style.bg(rgb(hover_fill(p))))
        })
        // A chosen option already has an accent outline.
        .focus_visible(move |style| focus_ring(style, p.text))
        .styles(|styles| styles.disabled(|style| style.opacity(0.5).cursor_default()))
}
