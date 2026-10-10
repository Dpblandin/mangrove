use crate::{
    format::count_label,
    integrations::{IntegrationsSection, IntegrationsView},
    preferences::{MAX_AUTO_FETCH_INTERVAL, parse_auto_fetch_interval},
    theme::{Palette, RADIUS_LG, RADIUS_MD, RADIUS_SM, ThemeId, icon, scrollbar_theme},
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
use std::time::Duration;

/// A typing pause after which a valid interval draft is applied.
const INTERVAL_COMMIT_DELAY: Duration = Duration::from_millis(600);

#[derive(Clone, Debug)]
pub enum SettingsEvent {
    Back,
    ThemeChanged(ThemeId),
    IntervalChanged(u32),
    ImagePreviewsChanged(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsSection {
    General,
    Profiles,
    Ssh,
    Integrations,
}

pub struct SettingsView {
    section: SettingsSection,
    integrations: Option<Entity<IntegrationsView>>,
    /// The last applied interval, restored if the field is left with an
    /// invalid draft. Drafts apply only once stable, never mid-typing.
    interval: u32,
    commit_task: Option<Task<()>>,
    show_image_previews: bool,
    input: Entity<InputState>,
    theme_select: Entity<SelectState<SearchableVec<&'static str>>>,
    scroll: ScrollHandle,
    focus_handle: FocusHandle,
    palette: Palette,
    font_size_offset: f32,
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
                // Stepping is bounded separately; invalid drafts stay visible
                // with their error until blur restores the focus interval.
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
                InputEvent::Change => this.draft_changed(cx),
                InputEvent::PressEnter { .. } => {
                    this.commit_draft(cx);
                }
                InputEvent::Blur => this.leave_draft(window, cx),
                InputEvent::Focus => {}
            });
        let theme_subscription = cx.subscribe(&theme_select, |_, _, event, cx| {
            if let SelectEvent::Confirm(Some(label)) = event
                && let Some(theme) = ThemeId::ALL.iter().find(|id| id.label() == *label)
            {
                cx.emit(SettingsEvent::ThemeChanged(*theme));
            }
        });
        Self {
            section: SettingsSection::General,
            integrations: None,
            interval,
            commit_task: None,
            show_image_previews,
            input,
            theme_select,
            scroll: ScrollHandle::default(),
            focus_handle: cx.focus_handle(),
            palette,
            font_size_offset,
            error: None,
            _subscriptions: vec![input_subscription, theme_subscription],
        }
    }
    pub fn set_integrations(
        &mut self,
        integrations: Entity<IntegrationsView>,
        cx: &mut Context<Self>,
    ) {
        self.integrations = Some(integrations);
        cx.notify();
    }

    pub fn select_section(
        &mut self,
        section: SettingsSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let switched = self.section != section;
        self.section = section;
        if section == SettingsSection::General {
            self.leave(window, cx);
            self.focus(window, cx);
        } else if let Some(integrations) = &self.integrations {
            integrations.update(cx, |view, cx| {
                if switched {
                    view.leave_settings(window, cx);
                }
                view.set_section(
                    match section {
                        SettingsSection::Profiles => IntegrationsSection::Profiles,
                        SettingsSection::Ssh => IntegrationsSection::Ssh,
                        _ => IntegrationsSection::Integrations,
                    },
                    cx,
                )
            });
        }
        cx.notify();
    }

    pub fn show_integrations(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_section(SettingsSection::Integrations, window, cx);
    }

    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_section(self.section, window, cx);
    }

    pub fn leave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(integrations) = &self.integrations {
            integrations.update(cx, |view, cx| view.leave_settings(window, cx));
        }
    }

    pub fn set_theme(&mut self, palette: Palette, font_size_offset: f32, cx: &mut Context<Self>) {
        if self.palette == palette && self.font_size_offset == font_size_offset {
            return;
        }
        self.palette = palette;
        self.font_size_offset = font_size_offset;
        if let Some(integrations) = &self.integrations {
            integrations.update(cx, |view, cx| view.set_theme(palette, font_size_offset, cx));
        }
        cx.notify();
    }
    pub fn set_interval(&mut self, interval: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.interval = interval;
        self.commit_task = None;
        self.input.update(cx, |input, cx| {
            input.set_value(interval.to_string(), window, cx)
        });
        self.error = None;
        cx.notify();
    }
    pub fn set_image_previews(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.show_image_previews = enabled;
        cx.notify();
    }
    pub fn set_selected_theme(
        &mut self,
        theme: ThemeId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = ThemeId::ALL.iter().position(|id| *id == theme).unwrap_or(0);
        self.theme_select.update(cx, |select, cx| {
            select.set_selected_index(Some(IndexPath::new(index)), window, cx);
        });
        cx.notify();
    }
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }
    /// Only a stable draft is applied: after a typing pause, on Enter, on a
    /// step or when the field is left.
    fn draft_changed(&mut self, cx: &mut Context<Self>) {
        self.commit_task = None;
        match resolve_interval(self.input.read(cx).value().as_ref(), self.interval) {
            Ok(interval) => {
                self.error = None;
                if interval != self.interval {
                    self.commit_task = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(INTERVAL_COMMIT_DELAY).await;
                        let _ = this.update(cx, |this, cx| this.commit_draft(cx));
                    }));
                }
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
    fn commit_draft(&mut self, cx: &mut Context<Self>) -> bool {
        self.commit_task = None;
        let Some(interval) = parse_auto_fetch_interval(self.input.read(cx).value().as_ref()) else {
            return false;
        };
        self.apply_interval(interval, cx);
        true
    }
    fn leave_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_draft(cx);
        self.set_interval(self.interval, window, cx);
    }
    fn apply_interval(&mut self, interval: u32, cx: &mut Context<Self>) {
        if interval != self.interval {
            self.interval = interval;
            cx.emit(SettingsEvent::IntervalChanged(interval));
        }
    }
    fn step(&mut self, action: StepAction, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.focus(window, cx);
            if let Some(value) = step_interval(input.value().as_ref(), action) {
                input.replace_all(value, window, cx);
            }
        });
        self.commit_draft(cx);
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let offset = self.font_size_offset;
        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        let view = cx.entity().downgrade();
        let number = NumberInput::new(&self.input)
            .controls_right()
            .w(px(120.))
            .h(px(36.))
            .flex_none()
            .rounded(RADIUS_MD)
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
                let _ = view.update(cx, |view, cx| view.step(action, window, cx));
            });

        div()
            .id("settings-view")
            .track_focus(&self.focus_handle)
            // The first Escape leaves the field; the next one reaches the workspace.
            .on_action(cx.listener(|this, _: &InputEscape, window, cx| {
                if this.input.read(cx).focus_handle(cx).is_focused(window) {
                    this.focus_handle.focus(window, cx);
                } else {
                    cx.propagate();
                }
            }))
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
                            .rounded(RADIUS_MD)
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
                                    .rounded(RADIUS_SM)
                                    .px(px(5.))
                                    .py(px(2.))
                                    .text_size(px(11. + offset))
                                    .line_height(relative(1.2))
                                    .text_color(rgb(p.muted))
                                    .child("Échap"),
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
                                    .rounded(RADIUS_MD)
                                    .when(self.section == SettingsSection::General, |button| button.bg(rgb(p.accent).opacity(0.1)))
                                    .text_color(rgb(p.text))
                                    .text_size(px(16. + offset))
                                    .line_height(relative(1.5))
                                    .font_weight(FontWeight::NORMAL)
                                    .cursor_pointer()
                                    .accessibility_label("Général")
                                    .focus_visible(move |style| style.bg(rgb(p.selection)))
                                    .child(icon(IconName::SlidersHorizontal, 16., p.text))
                                    .child("Général")
                                    .on_click(cx.listener(|this, _, window, cx| this.select_section(SettingsSection::General, window, cx))),
                            )
                            .children([(SettingsSection::Profiles, "settings-profiles", "Profils", IconName::Users), (SettingsSection::Ssh, "settings-ssh", "SSH", IconName::ShieldCheck), (SettingsSection::Integrations, "settings-integrations", "Intégrations", IconName::Plug)].into_iter().map(|(section, id, label, glyph)| {
                                Button::new(id).w_full().mt(px(4.)).px(px(16.)).py(px(12.)).justify_start().rounded(RADIUS_MD)
                                    .when(self.section == section, |button| button.bg(rgb(p.accent).opacity(0.1)))
                                    .text_color(rgb(p.text)).text_size(px(16. + offset)).cursor_pointer().accessibility_label(label)
                                    .focus_visible(move |style| style.bg(rgb(p.selection)))
                                    .gap(px(12.))
                                    .child(icon(glyph, 16., p.text))
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, window, cx| this.select_section(section, window, cx)))
                            })),
                    )
                    .child(
                        div()
                            .id("settings-content")
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .when(self.section == SettingsSection::General, |view| view.child(
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
                                                    .rounded(RADIUS_LG)
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
                                                                            .rounded(RADIUS_MD)
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
                                                    .rounded(RADIUS_LG)
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
                                                    .rounded(RADIUS_LG)
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
                                                                    .child(div().mt(px(12.)).text_size(px(12. + offset)).line_height(relative(1.625)).text_color(rgb(p.muted)).whitespace_normal().child("Entre 0 et 60 minutes ; 0 désactive l’auto-fetch. Récupère les références de tous les remotes du dépôt actif. Une valeur valide est enregistrée après une courte pause de saisie, avec Entrée ou les flèches, ou en quittant le champ."))
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
                                    ),
                            ))
                            .when(self.section != SettingsSection::General, |view| view.when_some(self.integrations.clone(), |view, integrations| view.child(integrations)))
                            .when(self.section == SettingsSection::General, |view| view.child(scrollbar_theme(Scrollbar::vertical(&self.scroll).id("settings-scrollbar")))),
                    ),
            )
    }
}

/// A valid draft becomes the interval; an invalid one keeps `saved`.
fn resolve_interval(draft: &str, saved: u32) -> Result<u32, String> {
    parse_auto_fetch_interval(draft).ok_or_else(|| {
        let kept = if saved == 0 {
            "L’auto-fetch reste désactivé.".to_string()
        } else {
            format!(
                "L’intervalle enregistré reste de {}.",
                count_label(saved as usize, "minute", "minutes")
            )
        };
        format!("Indiquez un nombre entier entre 0 et {MAX_AUTO_FETCH_INTERVAL}. {kept}")
    })
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
    use super::{
        INTERVAL_COMMIT_DELAY, SettingsEvent, SettingsView, resolve_interval, step_interval,
    };
    use crate::theme::{Palette, ThemeId};
    use gpui_kit::base::StepAction::{Decrement, Increment};
    use gpui_kit::{
        AnyWindowHandle, AppContext, Entity, TestAppContext, WindowOptions, test::TestWindowExt,
    };
    use std::{cell::RefCell, rc::Rc, time::Duration};

    fn open_settings(
        cx: &mut TestAppContext,
        interval: u32,
    ) -> (AnyWindowHandle, Entity<SettingsView>, Rc<RefCell<Vec<u32>>>) {
        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                    let palette = Palette::for_theme(ThemeId::Classic);
                    cx.new(|cx| {
                        SettingsView::new(interval, ThemeId::Classic, true, palette, 0., window, cx)
                    })
                })
            })
            .unwrap();
        // Blur is only reported for an active window.
        cx.update_window(window, |_, window, _| window.activate_window())
            .unwrap();
        let applied = Rc::new(RefCell::new(Vec::new()));
        let captured = applied.clone();
        cx.update(|cx| {
            cx.subscribe(&view, move |_, event: &SettingsEvent, _| {
                if let SettingsEvent::IntervalChanged(interval) = event {
                    captured.borrow_mut().push(*interval);
                }
            })
            .detach()
        });
        (window, view, applied)
    }

    fn keys(cx: &mut TestAppContext, window: AnyWindowHandle, keys: &[&str]) {
        for key in keys {
            cx.update_window(window, |_, window, cx| window.press(key, cx))
                .unwrap();
            cx.run_until_parked();
        }
    }

    fn edit(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        view: &Entity<SettingsView>,
        text: &str,
    ) {
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| view.focus(window, cx));
            window.press("secondary-a", cx);
            window.input(text, cx);
        })
        .unwrap();
    }

    fn draft(cx: &mut TestAppContext, view: &Entity<SettingsView>) -> String {
        view.read_with(cx, |view, cx| view.input.read(cx).value().to_string())
    }

    #[gpui_kit::test]
    fn abandoning_an_invalid_draft_restores_the_last_applied_interval(cx: &mut TestAppContext) {
        let (window, view, applied) = open_settings(cx, 15);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| view.focus(window, cx))
        })
        .unwrap();
        keys(cx, window, &["end", "backspace", "backspace"]);
        cx.executor().advance_clock(INTERVAL_COMMIT_DELAY * 2);
        assert!(view.read_with(cx, |view, _| view.error.is_some()));
        keys(cx, window, &["escape"]);
        assert_eq!(draft(cx, &view), "15");
        assert!(applied.borrow().is_empty());

        // A paused valid draft applies and becomes the value an abandon keeps.
        edit(cx, window, &view, "30");
        cx.executor().advance_clock(INTERVAL_COMMIT_DELAY);
        assert_eq!(*applied.borrow(), [30]);
        edit(cx, window, &view, "0.5");
        cx.executor().advance_clock(INTERVAL_COMMIT_DELAY * 2);
        assert_eq!(*applied.borrow(), [30]);
        keys(cx, window, &["escape"]);
        assert_eq!(*applied.borrow(), [30]);
        assert_eq!(draft(cx, &view), "30");
        assert!(view.read_with(cx, |view, _| view.error.is_none()));
    }

    #[gpui_kit::test]
    fn valid_drafts_apply_after_a_pause_on_enter_on_steps_and_on_blur(cx: &mut TestAppContext) {
        let (window, view, applied) = open_settings(cx, 15);
        edit(cx, window, &view, "20");
        cx.executor()
            .advance_clock(INTERVAL_COMMIT_DELAY - Duration::from_millis(1));
        assert!(applied.borrow().is_empty());
        cx.executor().advance_clock(Duration::from_millis(1));
        assert_eq!(*applied.borrow(), [20]);

        edit(cx, window, &view, "45");
        keys(cx, window, &["enter"]);
        assert_eq!(*applied.borrow(), [20, 45]);
        keys(cx, window, &["up"]);
        assert_eq!(*applied.borrow(), [20, 45, 46]);

        edit(cx, window, &view, "05");
        keys(cx, window, &["escape"]);
        assert_eq!(*applied.borrow(), [20, 45, 46, 5]);
        assert_eq!(draft(cx, &view), "5");
        cx.executor().advance_clock(INTERVAL_COMMIT_DELAY * 2);
        assert_eq!(*applied.borrow(), [20, 45, 46, 5]);
    }

    #[test]
    fn valid_drafts_apply_and_invalid_drafts_explain_the_kept_interval() {
        assert_eq!(resolve_interval("15", 1), Ok(15));
        assert_eq!(resolve_interval(" 0 ", 5), Ok(0));
        assert_eq!(resolve_interval("60", 5), Ok(60));
        assert_eq!(
            resolve_interval("61", 5),
            Err("Indiquez un nombre entier entre 0 et 60. L’intervalle enregistré reste de 5 minutes.".into())
        );
        assert!(
            resolve_interval("1.5", 1)
                .unwrap_err()
                .ends_with("reste de 1 minute.")
        );
        assert!(
            resolve_interval("", 0)
                .unwrap_err()
                .ends_with("L’auto-fetch reste désactivé.")
        );
    }

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
