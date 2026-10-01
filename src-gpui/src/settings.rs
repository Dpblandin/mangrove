use crate::{
    preferences::{MAX_AUTO_FETCH_INTERVAL, parse_auto_fetch_interval},
    theme::{Palette, ThemeId, icon, scrollbar_theme},
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    base::{Button, NumberInput, StepAction, input::MaskPattern, step_value},
    component::{
        IndexPath, Sizable,
        input::{Escape as InputEscape, Input, InputEvent, InputState},
        scroll::Scrollbar,
        searchable_list::SearchableVec,
        select::{Select, SelectEvent, SelectState},
        switch::Switch,
    },
    prelude::*,
    *,
};

#[derive(Clone, Debug)]
pub enum SettingsEvent {
    Back,
    ThemeChanged(ThemeId),
    IntervalChanged(u32),
    ImagePreviewsChanged(bool),
}

pub struct SettingsView {
    interval: u32,
    show_image_previews: bool,
    input: Entity<InputState>,
    theme_select: Entity<SelectState<SearchableVec<&'static str>>>,
    scroll: ScrollHandle,
    palette: Palette,
    font_size_offset: f32,
    saved: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(
        interval: u32,
        theme: ThemeId,
        show_image_previews: bool,
        palette: Palette,
        font_size_offset: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                // Stepping is bounded separately; keep invalid drafts available
                // to the existing save validation instead of clamping on blur.
                .mask_pattern(MaskPattern::None)
                .default_value(interval.to_string())
        });
        let initial_theme = ThemeId::ALL.iter().position(|id| *id == theme).unwrap_or(0);
        let theme_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(ThemeId::ALL.iter().map(|id| id.label()).collect::<Vec<_>>()),
                Some(IndexPath::new(initial_theme)),
                window,
                cx,
            )
        });
        let input_subscription =
            cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                InputEvent::Change => {
                    this.saved = false;
                    this.error = None;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.save(window, cx),
                _ => {}
            });
        let theme_subscription = cx.subscribe(&theme_select, |_, _, event, cx| {
            if let SelectEvent::Confirm(Some(label)) = event
                && let Some(theme) = ThemeId::ALL.iter().find(|id| id.label() == *label)
            {
                cx.emit(SettingsEvent::ThemeChanged(*theme));
            }
        });
        Self {
            interval,
            show_image_previews,
            input,
            theme_select,
            scroll: ScrollHandle::default(),
            palette,
            font_size_offset,
            saved: false,
            error: None,
            _subscriptions: vec![input_subscription, theme_subscription],
        }
    }
    pub fn set_theme(&mut self, palette: Palette, font_size_offset: f32, cx: &mut Context<Self>) {
        if self.palette == palette && self.font_size_offset == font_size_offset {
            return;
        }
        self.palette = palette;
        self.font_size_offset = font_size_offset;
        cx.notify();
    }
    pub fn set_interval(&mut self, interval: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.interval = interval;
        self.input.update(cx, |input, cx| {
            input.set_value(interval.to_string(), window, cx)
        });
        self.error = None;
        self.saved = false;
        cx.notify();
    }
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(interval) = parse_auto_fetch_interval(self.input.read(cx).value().as_ref()) {
            self.interval = interval;
            self.error = None;
            self.saved = true;
            if !self.input.read(cx).focus_handle(cx).is_focused(window) {
                self.focus(window, cx);
            }
            cx.emit(SettingsEvent::IntervalChanged(interval));
        } else {
            self.error = Some(format!(
                "Indiquez un nombre entier entre 0 et {MAX_AUTO_FETCH_INTERVAL}."
            ));
            self.focus(window, cx);
        }
        cx.notify();
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let offset = self.font_size_offset;
        let changed = self.input.read(cx).value().as_ref() != self.interval.to_string();
        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        let step_input = self.input.clone();
        let number = NumberInput::new(&self.input)
            .controls_right()
            .w(px(120.))
            .h(px(36.))
            .flex_none()
            .rounded(px(4.))
            .border_1()
            .border_color(rgb(if focused { p.accent } else { p.border }))
            .bg(rgb(p.bg))
            .input(
                Input::new(&self.input)
                    .id("auto-fetch-interval")
                    .aria_label("Intervalle d’auto-fetch")
                    .appearance(false)
                    .with_size(px(34.))
                    .w_full()
                    .min_w_0()
                    .px(px(12.))
                    .py_0()
                    .text_size(px(16. + offset))
                    .line_height(relative(1.5))
                    .text_color(rgb(p.text)),
            )
            .increment_button(move |button| {
                button
                    .accessibility_label("Augmenter l’intervalle d’auto-fetch")
                    .w(px(16.))
                    .mr(px(4.))
                    .mt(px(6.))
                    .p_0()
                    .items_center()
                    .justify_center()
                    .hover(move |style| style.bg(rgb(p.surface)))
                    .child(icon(IconName::ChevronUp, 10., p.muted))
            })
            .decrement_button(move |button| {
                button
                    .accessibility_label("Diminuer l’intervalle d’auto-fetch")
                    .w(px(16.))
                    .mr(px(4.))
                    .mb(px(6.))
                    .p_0()
                    .items_center()
                    .justify_center()
                    .hover(move |style| style.bg(rgb(p.surface)))
                    .child(icon(IconName::ChevronDown, 10., p.muted))
            })
            .on_step(move |action, window, cx| {
                step_input.update(cx, |input, cx| {
                    input.focus(window, cx);
                    if let Some(value) = step_interval(input.value().as_ref(), action) {
                        input.replace_all(value, window, cx);
                    }
                });
            });

        div()
            .id("settings-view")
            .on_action(cx.listener(|_, _: &InputEscape, _, cx| cx.emit(SettingsEvent::Back)))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(p.bg))
            .text_color(rgb(p.text))
            .child(
                div()
                    .min_h(px(64.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(24.))
                    .px(px(24.))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(
                        Button::new("settings-back")
                            .px(px(8.))
                            .py(px(8.))
                            .gap(px(8.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .text_size(px(16. + offset))
                            .line_height(relative(1.5))
                            .text_color(rgb(p.text))
                            .accessibility_label("Retour à l’espace de travail")
                            .hover(move |style| style.bg(rgb(p.hover)))
                            .focus_visible(move |style| style.bg(rgb(p.hover)))
                            .child(icon(IconName::ArrowLeft, 16., p.text))
                            .child("Retour à l’espace de travail")
                            .child(
                                div()
                                    .ml(px(8.))
                                    .border_1()
                                    .border_color(rgb(p.border))
                                    .rounded(px(4.))
                                    .px(px(5.))
                                    .py(px(2.))
                                    .text_size(px(9. + offset))
                                    .text_color(rgb(p.muted))
                                    .opacity(0.7)
                                    .child("Esc"),
                            )
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Back))),
                    )
                    .child(div().h(px(20.)).flex_none().border_l_1().border_color(rgb(p.border)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .text_size(px(16. + offset))
                            .line_height(px(24. + 2. * offset))
                            .font_weight(FontWeight::MEDIUM)
                            .child(icon(IconName::Settings, 18., p.accent))
                            .child("Paramètres"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .w(px(226.))
                            .flex_none()
                            .bg(rgb(p.panel))
                            .border_r_1()
                            .border_color(rgb(p.border))
                            .px(px(12.))
                            .py(px(24.))
                            .child(
                                Button::new("settings-general")
                                    .w_full()
                                    .px(px(16.))
                                    .py(px(12.))
                                    .justify_start()
                                    .gap(px(12.))
                                    .rounded(px(6.))
                                    .bg(rgb(p.accent).opacity(0.1))
                                    .text_color(rgb(p.text))
                                    .text_size(px(16. + offset))
                                    .line_height(relative(1.5))
                                    .font_weight(FontWeight::NORMAL)
                                    .cursor_pointer()
                                    .accessibility_label("Général")
                                    .focus_visible(move |style| style.bg(rgb(p.selection)))
                                    .child(icon(IconName::SlidersHorizontal, 16., p.text))
                                    .child("Général")
                                    .on_click(cx.listener(|this, _, window, cx| this.focus(window, cx))),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-content")
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .child(
                                div()
                                    .id("settings-scroll-area")
                                    .size_full()
                                    .overflow_y_scroll()
                                    .track_scroll(&self.scroll)
                                    .px(px(40.))
                                    .py(px(36.))
                                    .child(
                                        div()
                                            .w_full()
                                            .min_w_0()
                                            .max_w(px(760.))
                                            .flex()
                                            .flex_col()
                                            .child(div().text_size(px(24.)).line_height(px(32.)).font_weight(FontWeight::SEMIBOLD).child("Général"))
                                            .child(div().mt(px(8.)).text_size(px(12. + offset)).line_height(relative(1.625)).text_color(rgb(p.muted)).whitespace_normal().child("Personnalisez l’apparence, les aperçus des fichiers et la fréquence de récupération des références distantes."))
                                            .child(
                                                div()
                                                    .mt(px(32.))
                                                    .w_full()
                                                    .min_w_0()
                                                    .p(px(24.))
                                                    .rounded(px(12.))
                                                    .border_1()
                                                    .border_color(rgb(p.border))
                                                    .bg(rgb(p.panel))
                                                    .child(div().mb(px(24.)).text_size(px(14. + offset)).line_height(px(20. + 2. * offset)).font_weight(FontWeight::MEDIUM).child("Apparence"))
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .items_start()
                                                            .gap(px(20.))
                                                            .child(div().w(px(200.)).flex_none().pt(px(8.)).text_size(px(12. + offset)).line_height(px(16. + 2. * offset)).child("Thème de l’interface"))
                                                            .child(
                                                                div()
                                                                    .flex_1()
                                                                    .min_w_0()
                                                                    .flex()
                                                                    .flex_col()
                                                                    .child(
                                                                        Select::new(&self.theme_select)
                                                                            .w_full()
                                                                            .max_w(px(260.))
                                                                            .h(px(36.))
                                                                            .px(px(12.))
                                                                            .py(px(6.))
                                                                            .border_1()
                                                                            .border_color(rgb(p.border))
                                                                            .rounded(px(4.))
                                                                            .bg(rgb(p.bg))
                                                                            .text_color(rgb(p.text))
                                                                            .text_size(px(12. + offset))
                                                                            .font_weight(FontWeight::NORMAL)
                                                                            .accessibility_label("Thème de l’interface"),
                                                                    )
                                                                    .child(div().mt(px(12.)).text_size(px(12. + offset)).line_height(relative(1.625)).text_color(rgb(p.muted)).whitespace_normal().child("Le thème est appliqué et enregistré immédiatement.")),
                                                            ),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .mt(px(24.))
                                                    .w_full()
                                                    .min_w_0()
                                                    .p(px(24.))
                                                    .rounded(px(12.))
                                                    .border_1()
                                                    .border_color(rgb(p.border))
                                                    .bg(rgb(p.panel))
                                                    .child(div().mb(px(24.)).text_size(px(14. + offset)).line_height(px(20. + 2. * offset)).font_weight(FontWeight::MEDIUM).child("Aperçus des fichiers"))
                                                    .child(
                                                        div().flex().items_start().gap(px(20.))
                                                            .child(div().flex_1().min_w_0().flex().flex_col()
                                                                .child(div().text_size(px(12. + offset)).child("Afficher les aperçus des images"))
                                                                .child(div().mt(px(8.)).text_size(px(12. + offset)).line_height(relative(1.625)).text_color(rgb(p.muted)).whitespace_normal().child("Affiche les images dans les vues Diff et Fichier, avec une comparaison avant/après lorsqu’elles sont modifiées. Ce réglage est enregistré immédiatement.")))
                                                            .child(Switch::new("show-image-previews")
                                                                .checked(self.show_image_previews)
                                                                .accessibility_label("Afficher les aperçus des images")
                                                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                                                    this.show_image_previews = *checked;
                                                                    cx.emit(SettingsEvent::ImagePreviewsChanged(*checked));
                                                                    cx.notify();
                                                                }))),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .mt(px(20.))
                                                    .w_full()
                                                    .min_w_0()
                                                    .p(px(24.))
                                                    .rounded(px(12.))
                                                    .border_1()
                                                    .border_color(rgb(p.border))
                                                    .bg(rgb(p.panel))
                                                    .child(div().mb(px(24.)).text_size(px(14. + offset)).line_height(px(20. + 2. * offset)).font_weight(FontWeight::MEDIUM).child("Auto-fetch"))
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .items_start()
                                                            .gap(px(20.))
                                                            .child(div().w(px(200.)).flex_none().pt(px(8.)).text_size(px(12. + offset)).line_height(px(16. + 2. * offset)).child("Intervalle d’auto-fetch"))
                                                            .child(
                                                                div()
                                                                    .flex_1()
                                                                    .min_w_0()
                                                                    .flex()
                                                                    .flex_col()
                                                                    .child(div().flex().items_center().gap(px(12.)).child(number).child(div().text_size(px(12. + offset)).line_height(px(16. + 2. * offset)).text_color(rgb(p.muted)).child("minutes")))
                                                                    .child(div().mt(px(12.)).text_size(px(12. + offset)).line_height(relative(1.625)).text_color(rgb(p.muted)).whitespace_normal().child("Entre 0 et 60 minutes. 0 désactive l’auto-fetch. Récupère les références de tous les remotes du dépôt actif."))
                                                                    .when_some(self.error.clone(), |view, error| view.child(div().mt(px(8.)).text_size(px(12. + offset)).line_height(px(16. + 2. * offset)).text_color(rgb(p.danger)).whitespace_normal().child(error))),
                                                            ),
                                                    )
                                                    .child(
                                                        div()
                                                            .mt(px(24.))
                                                            .pt(px(16.))
                                                            .border_t_1()
                                                            .border_color(rgb(p.border))
                                                            .flex()
                                                            .items_start()
                                                            .gap(px(8.))
                                                            .text_size(px(12. + offset))
                                                            .line_height(relative(1.625))
                                                            .text_color(rgb(p.muted))
                                                            .child(div().size(px(14.)).flex_none().mt(px(2.)).child(icon(IconName::TriangleAlert, 14., p.muted)))
                                                            .child(div().flex_1().min_w_0().whitespace_normal().child("De nombreux remotes peuvent affecter les performances. Les objets LFS ne sont pas récupérés automatiquement.")),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .mt(px(20.))
                                                    .min_h(px(36.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .gap(px(16.))
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .flex()
                                                            .items_center()
                                                            .gap(px(8.))
                                                            .text_size(px(12. + offset))
                                                            .line_height(px(16. + 2. * offset))
                                                            .text_color(rgb(p.accent))
                                                            .when(self.saved, |view| view.child(icon(IconName::Check, 14., p.accent)).child("Paramètres enregistrés.")),
                                                    )
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .flex()
                                                            .gap(px(12.))
                                                            .child(settings_button("settings-reset", "Annuler les modifications", p, offset).hover(move |style| style.text_color(brightened(p.text)).border_color(brightened(p.border))).disabled(!changed).on_click(cx.listener(|this, _, window, cx| {
                                                                this.set_interval(this.interval, window, cx);
                                                                this.focus(window, cx);
                                                            })))
                                                            .child(settings_button("settings-save", "Enregistrer", p, offset).border_color(transparent_black()).bg(rgb(p.accent)).hover(move |style| style.bg(brightened(p.accent)).border_color(transparent_black())).disabled(!changed).on_click(cx.listener(|this, _, window, cx| this.save(window, cx)))),
                                                    ),
                                            ),
                                    ),
                            )
                            .child(scrollbar_theme(Scrollbar::vertical(&self.scroll).id("settings-scrollbar"))),
                    ),
            )
    }
}

fn settings_button(id: &'static str, label: &'static str, p: Palette, offset: f32) -> Button {
    Button::new(id)
        .min_h(px(37.))
        .min_w_0()
        .px(px(14.))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(px(6.))
        .text_size(px(12. + offset))
        .line_height(px(16. + 2. * offset))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(p.text))
        .cursor_pointer()
        .accessibility_label(label)
        .focus_visible(move |style| style.border_color(rgb(p.accent)))
        .styles(|styles| styles.disabled(|style| style.opacity(0.5).cursor_default()))
        .child(
            div()
                .min_w_0()
                .whitespace_normal()
                .text_center()
                .child(label),
        )
}

fn brightened(color: u32) -> Rgba {
    let mut color = rgb(color);
    color.r = (color.r * 1.1).min(1.);
    color.g = (color.g * 1.1).min(1.);
    color.b = (color.b * 1.1).min(1.);
    color
}

fn step_interval(value: &str, action: StepAction) -> Option<String> {
    // Kit preserves fractional precision; the HTML step=1 reference instead
    // reaches the next integer. Start one step behind that integer so Kit can
    // still enforce its bounds and directional no-op behavior.
    let aligned = value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map(|value| {
            let base = if value.fract() == 0. {
                value
            } else {
                match action {
                    StepAction::Increment => value.ceil() - 1.,
                    StepAction::Decrement => value.floor() + 1.,
                }
            };
            base.to_string()
        });
    step_value(
        aligned.as_deref().unwrap_or(value),
        action,
        1.,
        Some(0.),
        Some(MAX_AUTO_FETCH_INTERVAL as f64),
    )
}

#[cfg(test)]
mod tests {
    use super::step_interval;
    use gpui_kit::base::StepAction::{Decrement, Increment};

    #[test]
    fn stepping_fractional_drafts_recovers_a_whole_minute_in_the_requested_direction() {
        assert_eq!(step_interval("1.5", Increment).as_deref(), Some("2"));
        assert_eq!(step_interval("1.5", Decrement).as_deref(), Some("1"));
        assert_eq!(step_interval("0.1", Increment).as_deref(), Some("1"));
        assert_eq!(step_interval("0.1", Decrement).as_deref(), Some("0"));
    }

    #[test]
    fn integer_steps_and_range_boundaries_preserve_direction() {
        assert_eq!(step_interval("1", Increment).as_deref(), Some("2"));
        assert_eq!(step_interval("1", Decrement).as_deref(), Some("0"));
        assert_eq!(step_interval("1.0", Increment).as_deref(), Some("2"));
        assert_eq!(step_interval("59.5", Increment).as_deref(), Some("60"));
        assert_eq!(step_interval("60.5", Decrement).as_deref(), Some("60"));
        assert_eq!(step_interval("-0.5", Increment).as_deref(), Some("0"));
        assert!(step_interval("60", Increment).is_none());
        assert!(step_interval("0", Decrement).is_none());
        assert!(step_interval("60.5", Increment).is_none());
        assert!(step_interval("-0.5", Decrement).is_none());
    }
}
