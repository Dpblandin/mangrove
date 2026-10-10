pub(crate) mod ui;

use crate::{
    format::count_label,
    integrations::{IntegrationsSection, IntegrationsView},
    preferences::{
        DEFAULT_DIFF_FONT_SIZE, DEFAULT_FONT_SIZE_OFFSET, MAX_AUTO_FETCH_INTERVAL,
        MAX_DIFF_FONT_SIZE, MAX_FONT_SIZE_OFFSET, MIN_DIFF_FONT_SIZE, MIN_FONT_SIZE_OFFSET,
        Preferences, parse_auto_fetch_interval,
    },
    theme::{Palette, RADIUS_MD, RADIUS_SM, ThemeId, icon, scrollbar_theme},
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
use ui::{
    CONTENT_MAX_WIDTH, NAV_WIDTH, PAGE_GAP, PAGE_PADDING, Tone, card, card_title, field_border,
    link_action, nav_item, note, page_header,
};

/// A typing pause after which a valid interval draft is applied.
const INTERVAL_COMMIT_DELAY: Duration = Duration::from_millis(600);
/// How long "Enregistré" stays beside the setting that was just saved.
const SAVED_NOTICE: Duration = Duration::from_millis(2500);

#[derive(Clone, Debug)]
pub enum SettingsEvent {
    Back,
    ThemeChanged(ThemeId),
    IntervalChanged(u32),
    ImagePreviewsChanged(bool),
    FontSizeChanged(f32),
    DiffFontSizeChanged(f32),
}

impl SettingsEvent {
    /// The control whose change this event saves.
    pub fn field(&self) -> Option<SettingsField> {
        match self {
            Self::Back => None,
            Self::ThemeChanged(_) => Some(SettingsField::Theme),
            Self::IntervalChanged(_) => Some(SettingsField::Interval),
            Self::ImagePreviewsChanged(_) => Some(SettingsField::ImagePreviews),
            Self::FontSizeChanged(_) => Some(SettingsField::FontSize),
            Self::DiffFontSizeChanged(_) => Some(SettingsField::DiffFontSize),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsField {
    Theme,
    FontSize,
    DiffFontSize,
    ImagePreviews,
    Interval,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsSection {
    General,
    Profiles,
    Ssh,
    Integrations,
}

impl SettingsSection {
    const ALL: [Self; 4] = [Self::General, Self::Profiles, Self::Ssh, Self::Integrations];

    fn label(self) -> &'static str {
        match self {
            Self::General => "Général",
            Self::Profiles => "Profils",
            Self::Ssh => "SSH",
            Self::Integrations => "Intégrations",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::General => IconName::SlidersHorizontal,
            Self::Profiles => IconName::Users,
            Self::Ssh => IconName::KeyRound,
            Self::Integrations => IconName::Plug,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Profiles => "profiles",
            Self::Ssh => "ssh",
            Self::Integrations => "integrations",
        }
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|section| *section == self)
            .unwrap_or(0)
    }
}

pub struct SettingsView {
    section: SettingsSection,
    integrations: Option<Entity<IntegrationsView>>,
    /// The last applied interval, restored if the field is left with an
    /// invalid draft. Drafts apply only once stable, never mid-typing.
    interval: u32,
    commit_task: Option<Task<()>>,
    show_image_previews: bool,
    diff_font_size: f32,
    input: Entity<InputState>,
    theme_select: Entity<SelectState<SearchableVec<&'static str>>>,
    scroll: ScrollHandle,
    focus_handle: FocusHandle,
    navigation: [FocusHandle; 4],
    palette: Palette,
    font_size_offset: f32,
    error: Option<String>,
    /// The last saved control, or why it could not be saved.
    saved: Option<(SettingsField, Result<(), String>)>,
    saved_task: Option<Task<()>>,
    /// Back returns to the clone screen the settings were opened from.
    return_to_clone: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(preferences: &Preferences, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let interval = preferences.auto_fetch_interval;
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                // Stepping is bounded separately; invalid drafts stay visible
                // with their error until blur restores the focus interval.
                .mask_pattern(MaskPattern::None)
                .default_value(interval.to_string())
        });
        let initial_theme = ThemeId::ALL
            .iter()
            .position(|id| *id == preferences.theme)
            .unwrap_or(0);
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
            show_image_previews: preferences.show_image_previews,
            diff_font_size: preferences.diff_font_size,
            input,
            theme_select,
            scroll: ScrollHandle::default(),
            focus_handle: cx.focus_handle(),
            navigation: std::array::from_fn(|_| cx.focus_handle()),
            palette: preferences.palette(),
            font_size_offset: preferences.font_size_offset,
            error: None,
            saved: None,
            saved_task: None,
            return_to_clone: false,
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

    /// Shows `section` and gives the keyboard to its navigation entry, so
    /// that opening a section never lands in a field further down the page.
    pub fn select_section(
        &mut self,
        section: SettingsSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let switched = self.section != section;
        self.section = section;
        if switched {
            self.scroll.set_offset(point(px(0.), px(0.)));
            self.saved = None;
        }
        if section == SettingsSection::General {
            self.leave(window, cx);
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
        self.navigation[section.index()].focus(window, cx);
        cx.notify();
    }

    pub fn set_return_to_clone(&mut self, return_to_clone: bool, cx: &mut Context<Self>) {
        self.return_to_clone = return_to_clone;
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

    /// Mirrors preferences that change outside the settings, such as the
    /// zoom shortcuts or a profile switch.
    pub fn sync_preferences(&mut self, preferences: &Preferences, cx: &mut Context<Self>) {
        self.palette = preferences.palette();
        self.font_size_offset = preferences.font_size_offset;
        self.diff_font_size = preferences.diff_font_size;
        self.show_image_previews = preferences.show_image_previews;
        if let Some(integrations) = &self.integrations {
            let (palette, offset) = (self.palette, self.font_size_offset);
            integrations.update(cx, |view, cx| view.set_theme(palette, offset, cx));
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
    /// Reports whether the change of `field` reached the preferences file.
    pub fn confirm_saved(
        &mut self,
        field: SettingsField,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        let saved = result.is_ok();
        self.saved = Some((field, result));
        self.saved_task = saved.then(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SAVED_NOTICE).await;
                let _ = this.update(cx, |this, cx| {
                    if this
                        .saved
                        .as_ref()
                        .is_some_and(|(shown, _)| *shown == field)
                    {
                        this.saved = None;
                        cx.notify();
                    }
                });
            })
        });
        cx.notify();
    }
    #[cfg(test)]
    fn focus_interval(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }
    /// ↑ and ↓ move between the navigation entries; Enter or Space opens one.
    fn navigation_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = &event.keystroke;
        if key.modifiers.alt || key.modifiers.control || key.modifiers.platform {
            return;
        }
        let Some(current) = self
            .navigation
            .iter()
            .position(|handle| handle.is_focused(window))
        else {
            return;
        };
        let last = self.navigation.len() - 1;
        let target = match key.key.as_str() {
            "up" => current.saturating_sub(1),
            "down" => (current + 1).min(last),
            "home" => 0,
            "end" => last,
            _ => return,
        };
        self.navigation[target].focus(window, cx);
        cx.stop_propagation();
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
            Err(error) => {
                self.error = Some(error);
                if self
                    .saved
                    .as_ref()
                    .is_some_and(|(field, _)| *field == SettingsField::Interval)
                {
                    self.saved = None;
                }
            }
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
    /// A refused draft gives way to the saved interval, with the reason left
    /// under the field.
    fn leave_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.input.read(cx).value().trim().to_string();
        let applied = self.commit_draft(cx);
        self.set_interval(self.interval, window, cx);
        if !applied {
            self.error = Some(abandoned_interval(&draft, self.interval));
        }
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

    fn saved_notice(&self, field: SettingsField) -> Option<Div> {
        let (_, result) = self.saved.as_ref().filter(|(saved, _)| *saved == field)?;
        Some(match result {
            Ok(()) => ui::feedback(
                Tone::Success,
                "Enregistré",
                self.palette,
                self.font_size_offset,
            ),
            Err(error) => ui::feedback(
                Tone::Error,
                error.clone(),
                self.palette,
                self.font_size_offset,
            ),
        })
    }

    fn interval_control(&self, window: &mut Window, cx: &mut Context<Self>) -> NumberInput {
        let p = self.palette;
        let offset = self.font_size_offset;
        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        let view = cx.entity().downgrade();
        let border = if self.error.is_some() {
            p.danger
        } else if focused {
            p.accent
        } else {
            field_border(p)
        };
        // Full-height − and + keep both steps as large as the other controls.
        let step = move |button: Button, label: &'static str, glyph: IconName| {
            button
                .accessibility_label(label)
                .w(px(34.))
                .h_full()
                .items_center()
                .justify_center()
                .border_color(rgb(field_border(p)))
                .cursor_pointer()
                .hover(move |style| style.bg(rgb(ui::hover_fill(p))))
                .styles(|styles| styles.disabled(|style| style.opacity(0.4)))
                .child(icon(glyph, 14., p.text))
        };
        NumberInput::new(&self.input)
            .w(px(156.))
            .h(px(ui::CONTROL_HEIGHT))
            .flex_none()
            .rounded(RADIUS_MD)
            .overflow_hidden()
            .border_1()
            .border_color(rgb(border))
            .bg(rgb(p.bg))
            .input(
                Input::new(&self.input)
                    .id("auto-fetch-interval")
                    .aria_label("Intervalle d’auto-fetch en minutes")
                    .appearance(false)
                    .with_size(px(34.))
                    .w_full()
                    .min_w_0()
                    .px(px(12.))
                    .py_0()
                    .text_size(px(12. + offset))
                    .line_height(relative(1.5))
                    .text_color(rgb(p.text)),
            )
            .decrement_button(move |button| {
                step(
                    button.border_r_1(),
                    "Diminuer l’intervalle d’auto-fetch",
                    IconName::Minus,
                )
            })
            .increment_button(move |button| {
                step(
                    button.border_l_1(),
                    "Augmenter l’intervalle d’auto-fetch",
                    IconName::Plus,
                )
            })
            .on_step(move |action, window, cx| {
                let _ = view.update(cx, |view, cx| view.step(action, window, cx));
            })
    }

    /// − value + buttons for a text size, with a reset to the default.
    fn text_size_control(
        &self,
        id: &'static str,
        label: &'static str,
        value: String,
        (can_decrease, can_increase, is_default): (bool, bool, bool),
        change: fn(&Self, i32) -> SettingsEvent,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.palette;
        let offset = self.font_size_offset;
        let step = |direction: i32, glyph: IconName, enabled: bool, name: String| {
            Button::new(SharedString::from(format!("{id}-{direction}")))
                .size(px(ui::CONTROL_HEIGHT))
                .flex_none()
                .rounded(RADIUS_MD)
                .border_1()
                .border_color(rgb(field_border(p)))
                .bg(rgb(p.bg))
                .cursor_pointer()
                .accessibility_label(name)
                .hover(move |style| style.bg(rgb(p.hover)))
                .focus_visible(move |style| ui::focus_ring(style, p.accent))
                .disabled(!enabled)
                .styles(|styles| styles.disabled(|style| style.opacity(0.4).cursor_default()))
                .child(icon(glyph, 14., p.text))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let event = change(this, direction);
                    cx.emit(event);
                }))
        };
        div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(8.))
            .child(step(
                -1,
                IconName::Minus,
                can_decrease,
                format!("Réduire {label}"),
            ))
            .child(
                div()
                    .min_w(px(64.))
                    .text_center()
                    .text_size(px(12. + offset))
                    .font_weight(FontWeight::MEDIUM)
                    .child(value),
            )
            .child(step(
                1,
                IconName::Plus,
                can_increase,
                format!("Agrandir {label}"),
            ))
            .child(
                link_action(format!("{id}-reset"), "Par défaut", p, offset)
                    .accessibility_label(format!("Rétablir {label} par défaut"))
                    .disabled(is_default)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let event = change(this, 0);
                        cx.emit(event);
                    })),
            )
    }

    fn general_content(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let offset = self.font_size_offset;
        let profile = self
            .integrations
            .as_ref()
            .map(|integrations| integrations.read(cx).profile_name().to_string())
            .unwrap_or_default();
        let theme = Select::new(&self.theme_select)
            .id("theme-select")
            .w_full()
            .max_w(px(260.))
            .h(px(ui::CONTROL_HEIGHT))
            .px(px(12.))
            .py(px(6.))
            .border_1()
            .border_color(rgb(field_border(p)))
            .rounded(RADIUS_MD)
            .bg(rgb(p.bg))
            .text_color(rgb(p.text))
            .text_size(px(12. + offset))
            .font_weight(FontWeight::NORMAL)
            .accessibility_label("Thème de l’interface");
        let interface_size = self.text_size_control(
            "interface-text-size",
            "la taille du texte",
            format!("{}\u{202f}%", interface_scale(offset)),
            (
                offset > MIN_FONT_SIZE_OFFSET,
                offset < MAX_FONT_SIZE_OFFSET,
                offset == DEFAULT_FONT_SIZE_OFFSET,
            ),
            |this, direction| {
                SettingsEvent::FontSizeChanged(if direction == 0 {
                    DEFAULT_FONT_SIZE_OFFSET
                } else {
                    this.font_size_offset + direction as f32
                })
            },
            cx,
        );
        let diff_size = self.text_size_control(
            "diff-text-size",
            "le texte des diffs",
            format!("{}\u{a0}px", self.diff_font_size),
            (
                self.diff_font_size > MIN_DIFF_FONT_SIZE,
                self.diff_font_size < MAX_DIFF_FONT_SIZE,
                self.diff_font_size == DEFAULT_DIFF_FONT_SIZE,
            ),
            |this, direction| {
                SettingsEvent::DiffFontSizeChanged(if direction == 0 {
                    DEFAULT_DIFF_FONT_SIZE
                } else {
                    this.diff_font_size + direction as f32
                })
            },
            cx,
        );
        let appearance = card(p)
            .gap(px(20.))
            .child(card_title("Apparence", offset))
            .child(setting_row(
                "Thème",
                None,
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(12.))
                    .child(div().flex_1().min_w(px(180.)).max_w(px(260.)).child(theme))
                    .children(self.saved_notice(SettingsField::Theme)),
                p,
                offset,
            ))
            .child(setting_row(
                "Taille du texte",
                Some(format!(
                    "Aussi avec {mod} + et {mod} −.",
                    mod = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl" }
                )),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(interface_size)
                    .children(self.saved_notice(SettingsField::FontSize)),
                p,
                offset,
            ))
            .child(setting_row(
                "Texte des diffs",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(diff_size)
                    .children(self.saved_notice(SettingsField::DiffFontSize)),
                p,
                offset,
            ));
        let previews = card(p).child(card_title("Aperçus des fichiers", offset)).child(
            div()
                .flex()
                .items_start()
                .gap(px(20.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(ui::field_label("Afficher les aperçus des images", p, offset))
                        .child(note(
                            "Dans les vues Diff et Fichier, avec une comparaison avant/après pour les images modifiées.",
                            p,
                        ))
                        .children(self.saved_notice(SettingsField::ImagePreviews)),
                )
                .child(
                    Switch::new("show-image-previews")
                        .checked(self.show_image_previews)
                        .accessibility_label("Afficher les aperçus des images")
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.show_image_previews = *checked;
                            cx.emit(SettingsEvent::ImagePreviewsChanged(*checked));
                            cx.notify();
                        })),
                ),
        );
        let interval = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(12.))
                    .child(self.interval_control(window, cx))
                    .child(
                        div()
                            .text_size(px(12. + offset))
                            .text_color(rgb(p.muted))
                            .child("minutes"),
                    )
                    .when(self.error.is_none(), |row| {
                        row.children(self.saved_notice(SettingsField::Interval))
                    }),
            )
            .when_some(self.error.clone(), |view, error| {
                view.child(ui::feedback(Tone::Error, error, p, offset))
            })
            .child(note(
                "De 0 à 60 minutes ; 0 désactive l’auto-fetch. Récupère les références de tous les remotes du dépôt actif.",
                p,
            ));
        let auto_fetch = card(p)
            .child(card_title("Auto-fetch", offset))
            .child(setting_row("Intervalle", None, interval, p, offset))
            .child(
                div()
                    .pt(px(16.))
                    .border_t_1()
                    .border_color(rgb(p.border))
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .text_size(px(11. + offset))
                    .line_height(relative(1.6))
                    .text_color(rgb(p.muted))
                    .child(
                        div()
                            .flex_none()
                            .mt(px(2.))
                            .child(icon(IconName::TriangleAlert, 13., p.muted)),
                    )
                    .child(div().flex_1().min_w_0().whitespace_normal().child(
                        "De nombreux remotes peuvent ralentir l’application. Les objets LFS ne sont pas récupérés automatiquement.",
                    )),
            );
        div()
            .w_full()
            .min_w_0()
            .max_w(px(CONTENT_MAX_WIDTH))
            .mx_auto()
            .flex()
            .flex_col()
            .gap(px(PAGE_GAP))
            .child(page_header(
                "Général",
                format!(
                    "Apparence, aperçus et auto-fetch du profil « {profile} ». Chaque profil garde ses propres réglages."
                ),
                p,
                offset,
            ))
            .child(appearance)
            .child(previews)
            .child(auto_fetch)
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let offset = self.font_size_offset;
        let back = if self.return_to_clone {
            "Retour au clonage"
        } else {
            "Retour à l’espace de travail"
        };
        let switcher = self.integrations.clone().map(|integrations| {
            integrations.update(cx, |view, cx| view.profile_switcher(window, cx))
        });
        let mut navigation = div()
            .id("settings-navigation")
            .w(px(NAV_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.))
            .bg(rgb(p.panel))
            .border_r_1()
            .border_color(rgb(p.border))
            .px(px(12.))
            .py(px(24.))
            .on_key_down(cx.listener(Self::navigation_key));
        for section in SettingsSection::ALL {
            let selected = self.section == section;
            navigation =
                navigation.child(
                    nav_item(
                        SharedString::from(format!("settings-{}", section.id())),
                        section.label(),
                        selected,
                        p,
                        offset,
                    )
                    .track_focus(&self.navigation[section.index()])
                    // One tab stop: ↑ and ↓ reach the other entries.
                    .tab_stop(selected)
                    .child(icon(
                        section.icon(),
                        16.,
                        if selected { p.accent } else { p.muted },
                    ))
                    .child(section.label())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_section(section, window, cx)
                    })),
                );
        }
        div()
            .id("settings-view")
            .track_focus(&self.focus_handle)
            // The first Escape leaves the field; the next one reaches the workspace.
            .on_action(cx.listener(|this, _: &InputEscape, window, cx| {
                if this.input.read(cx).focus_handle(cx).is_focused(window) {
                    this.navigation[this.section.index()].focus(window, cx);
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
                            .border_1()
                            .border_color(transparent_black())
                            .cursor_pointer()
                            .text_size(px(16. + offset))
                            .line_height(relative(1.5))
                            .text_color(rgb(p.text))
                            .accessibility_label(back)
                            .hover(move |style| style.bg(rgb(p.hover)))
                            .focus_visible(move |style| ui::focus_ring(style, p.accent))
                            .child(icon(IconName::ArrowLeft, 16., p.text))
                            .child(back)
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
                    .child(
                        div()
                            .h(px(20.))
                            .flex_none()
                            .border_l_1()
                            .border_color(rgb(p.border)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .text_size(px(16. + offset))
                            .line_height(px(24. + 2. * offset))
                            .font_weight(FontWeight::MEDIUM)
                            .child(icon(IconName::Settings, 18., p.accent))
                            .child("Paramètres"),
                    )
                    .when_some(switcher, |header, switcher| {
                        header.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(
                                    div()
                                        .text_size(px(12. + offset))
                                        .text_color(rgb(p.muted))
                                        .child("Profil"),
                                )
                                .child(switcher),
                        )
                    }),
            )
            .child(
                div().flex().flex_1().min_h_0().child(navigation).child(
                    div()
                        .id("settings-content")
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .overflow_hidden()
                        .when(self.section == SettingsSection::General, |view| {
                            view.child(
                                div()
                                    .id("settings-scroll-area")
                                    .size_full()
                                    .overflow_y_scroll()
                                    .track_scroll(&self.scroll)
                                    .p(px(PAGE_PADDING))
                                    // The same body size as the other sections.
                                    .text_size(px(12. + offset))
                                    .child(self.general_content(window, cx)),
                            )
                            .child(scrollbar_theme(
                                Scrollbar::vertical(&self.scroll).id("settings-scrollbar"),
                            ))
                        })
                        .when(self.section != SettingsSection::General, |view| {
                            view.when_some(self.integrations.clone(), |view, integrations| {
                                view.child(integrations)
                            })
                        }),
                ),
            )
    }
}

/// A setting with its label on the left and its control on the right.
fn setting_row(
    label: &str,
    hint: Option<String>,
    control: impl IntoElement,
    p: Palette,
    offset: f32,
) -> Div {
    div()
        .flex()
        .items_start()
        .flex_wrap()
        .gap(px(20.))
        .child(
            div()
                .w(px(180.))
                .flex_none()
                .pt(px(9.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(ui::field_label(label, p, offset))
                .when_some(hint, |label, hint| {
                    label.child(note(hint, p).text_size(px(11. + offset)))
                }),
        )
        .child(div().flex_1().min_w(px(240.)).child(control))
}

/// The interface text size relative to the default, in percent.
fn interface_scale(offset: f32) -> u32 {
    ((11. + offset) / (11. + DEFAULT_FONT_SIZE_OFFSET) * 100.).round() as u32
}

/// A valid draft becomes the interval; an invalid one keeps `saved`.
fn resolve_interval(draft: &str, saved: u32) -> Result<u32, String> {
    parse_auto_fetch_interval(draft).ok_or_else(|| {
        format!(
            "Indiquez un nombre entier entre 0 et {MAX_AUTO_FETCH_INTERVAL}. {}",
            kept_interval(saved)
        )
    })
}

/// Why a draft left in the field was not applied.
fn abandoned_interval(draft: &str, saved: u32) -> String {
    let refused = if draft.is_empty() {
        "Le champ vide n’a pas été retenu.".to_string()
    } else {
        format!("La valeur « {draft} » n’a pas été retenue.")
    };
    format!("{refused} {}", kept_interval(saved))
}

fn kept_interval(saved: u32) -> String {
    if saved == 0 {
        "L’auto-fetch reste désactivé.".to_string()
    } else {
        format!(
            "L’intervalle enregistré reste de {}.",
            count_label(saved as usize, "minute", "minutes")
        )
    }
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
        INTERVAL_COMMIT_DELAY, SAVED_NOTICE, SettingsEvent, SettingsField, SettingsView,
        interface_scale, resolve_interval, step_interval,
    };
    use crate::preferences::Preferences;
    use gpui_kit::base::StepAction::{Decrement, Increment};
    use gpui_kit::{
        AnyWindowHandle, AppContext, Entity, Focusable, TestAppContext, WindowOptions,
        test::TestWindowExt,
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
                    let preferences = Preferences {
                        auto_fetch_interval: interval,
                        font_size_offset: 0.,
                        ..Preferences::default()
                    };
                    cx.new(|cx| SettingsView::new(&preferences, window, cx))
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
            view.update(cx, |view, cx| view.focus_interval(window, cx));
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
            view.update(cx, |view, cx| view.focus_interval(window, cx))
        })
        .unwrap();
        keys(cx, window, &["end", "backspace", "backspace"]);
        cx.executor().advance_clock(INTERVAL_COMMIT_DELAY * 2);
        assert!(view.read_with(cx, |view, _| view.error.is_some()));
        keys(cx, window, &["escape"]);
        assert_eq!(draft(cx, &view), "15");
        assert!(applied.borrow().is_empty());
        assert_eq!(
            view.read_with(cx, |view, _| view.error.clone()).as_deref(),
            Some("Le champ vide n’a pas été retenu. L’intervalle enregistré reste de 15 minutes.")
        );

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
        assert_eq!(
            view.read_with(cx, |view, _| view.error.clone()).as_deref(),
            Some(
                "La valeur « 0.5 » n’a pas été retenue. L’intervalle enregistré reste de 30 minutes."
            )
        );

        // The next valid draft retires the explanation.
        edit(cx, window, &view, "45");
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

    #[gpui_kit::test]
    fn opening_a_section_focuses_its_navigation_entry_not_a_field(cx: &mut TestAppContext) {
        let (window, view, _) = open_settings(cx, 15);
        let focus = |cx: &mut TestAppContext| {
            cx.update_window(window, |_, window, cx| {
                let view = view.read(cx);
                (
                    view.navigation
                        .iter()
                        .position(|handle| handle.is_focused(window)),
                    view.input.read(cx).focus_handle(cx).is_focused(window),
                )
            })
            .unwrap()
        };
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| view.show(window, cx))
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(focus(cx), (Some(0), false));

        // ↓ and End move between the entries without opening them.
        keys(cx, window, &["down"]);
        assert_eq!(focus(cx), (Some(1), false));
        keys(cx, window, &["end"]);
        assert_eq!(focus(cx), (Some(3), false));
        keys(cx, window, &["down"]);
        assert_eq!(focus(cx), (Some(3), false));
        keys(cx, window, &["home"]);
        assert_eq!(focus(cx), (Some(0), false));
        assert_eq!(
            view.read_with(cx, |view, _| view.section),
            super::SettingsSection::General
        );
    }

    #[gpui_kit::test]
    fn a_saved_setting_confirms_briefly_and_a_failure_stays(cx: &mut TestAppContext) {
        let (_, view, _) = open_settings(cx, 15);
        view.update(cx, |view, cx| {
            view.confirm_saved(SettingsField::Theme, Ok(()), cx)
        });
        assert!(view.read_with(cx, |view, _| {
            view.saved_notice(SettingsField::Theme).is_some()
        }));
        assert!(view.read_with(cx, |view, _| {
            view.saved_notice(SettingsField::Interval).is_none()
        }));
        cx.executor().advance_clock(SAVED_NOTICE);
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| view.saved.is_none()));

        view.update(cx, |view, cx| {
            view.confirm_saved(SettingsField::Interval, Err("Échec".into()), cx)
        });
        cx.executor().advance_clock(SAVED_NOTICE * 2);
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| view.saved.is_some()));
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

    #[test]
    fn the_interface_scale_reads_100_percent_at_the_default_size() {
        assert_eq!(interface_scale(1.), 100);
        assert_eq!(interface_scale(0.), 92);
        assert_eq!(interface_scale(5.), 133);
    }
}
