use crate::{
    preferences::{MAX_AUTO_FETCH_INTERVAL, parse_auto_fetch_interval},
    theme::{Palette, icon, scrollbar_theme},
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    component::{
        Disableable, Sizable,
        button::*,
        input::{Input, InputEvent, InputState},
        scroll::Scrollbar,
    },
    prelude::*,
    *,
};

#[derive(Clone, Debug)]
pub enum SettingsEvent {
    Back,
    IntervalChanged(u32),
}

pub struct SettingsView {
    interval: u32,
    input: Entity<InputState>,
    scroll: ScrollHandle,
    palette: Palette,
    font_size_offset: f32,
    saved: bool,
    error: Option<String>,
    _subscription: Subscription,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(
        interval: u32,
        palette: Palette,
        font_size_offset: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(interval.to_string()));
        let subscription =
            cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                InputEvent::Change => {
                    this.saved = false;
                    this.error = None;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.save(window, cx),
                _ => {}
            });
        Self {
            interval,
            input,
            scroll: ScrollHandle::default(),
            palette,
            font_size_offset,
            saved: false,
            error: None,
            _subscription: subscription,
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let offset = self.font_size_offset;
        let changed = self.input.read(cx).value().as_ref() != self.interval.to_string();
        div().id("settings-view").size_full().flex().flex_col().bg(rgb(p.bg)).text_color(rgb(p.text))
            .child(div().h(px(64.)).flex_none().flex().items_center().gap(px(24.)).px(px(24.)).border_b_1().border_color(rgb(p.border))
                .child(Button::new("settings-back").ghost().with_size(px(36.)).text_size(px(12.+offset))
                    .icon(icon(IconName::ArrowLeft,16.,p.muted)).label("Retour à l’espace de travail")
                    .child(div().ml(px(8.)).border_1().border_color(rgb(p.border)).rounded(px(4.)).px(px(5.)).py(px(2.)).text_size(px(10.)).opacity(0.7).child("Esc"))
                    .tooltip("Retour à l’espace de travail · Esc")
                    .on_click(cx.listener(|_,_,_,cx| cx.emit(SettingsEvent::Back))))
                .child(div().h(px(20.)).border_l_1().border_color(rgb(p.border)))
                .child(div().flex().items_center().gap(px(10.)).text_size(px(16.+offset)).font_weight(FontWeight::MEDIUM)
                    .child(icon(IconName::Settings,18.,p.accent)).child("Paramètres")))
            .child(div().flex().flex_1().min_h_0()
                .child(div().w(px(226.)).flex_none().bg(rgb(p.panel)).border_r_1().border_color(rgb(p.border)).px(px(12.)).py(px(24.))
                    .child(Button::new("settings-general").ghost().w_full().h(px(44.)).px(px(16.)).bg(rgb(p.accent_soft)).text_color(rgb(p.accent)).text_size(px(12.+offset))
                        .icon(icon(IconName::SlidersHorizontal,16.,p.accent)).label("Général")
                        .on_click(cx.listener(|this,_,window,cx| this.focus(window,cx)))))
                .child(div().id("settings-content").relative().flex_1().min_w_0().min_h_0().overflow_hidden()
                    .child(div().id("settings-scroll-area").size_full().overflow_y_scroll().track_scroll(&self.scroll).px(px(40.)).py(px(36.))
                    .child(div().max_w(px(760.)).flex().flex_col()
                        .child(div().text_size(px(24.)).font_weight(FontWeight::SEMIBOLD).child("Général"))
                        .child(div().mt(px(8.)).text_size(px(12.+offset)).line_height(px((12.+offset)*1.625)).text_color(rgb(p.muted)).child("Réglez la fréquence de récupération des références distantes."))
                        .child(div().mt(px(32.)).p(px(24.)).rounded(px(12.)).border_1().border_color(rgb(p.border)).bg(rgb(p.panel))
                            .child(div().mb(px(24.)).text_size(px(14.+offset)).font_weight(FontWeight::MEDIUM).child("Auto-fetch"))
                            .child(div().flex().items_start().gap(px(20.))
                                .child(div().w(px(200.)).flex_none().pt(px(8.)).text_size(px(12.+offset)).child("Intervalle d’auto-fetch"))
                                .child(div().flex_1().min_w_0().flex().flex_col()
                                    .child(div().flex().items_center().gap(px(12.))
                                        .child(Input::new(&self.input).w(px(120.)).h(px(36.)).text_size(px(12.+offset)).px(px(12.)))
                                        .child(div().text_size(px(12.+offset)).text_color(rgb(p.muted)).child("minutes")))
                                    .child(div().mt(px(12.)).text_size(px(12.+offset)).line_height(px((12.+offset)*1.625)).text_color(rgb(p.muted))
                                        .child("Entre 0 et 60 minutes. 0 désactive l’auto-fetch. Récupère les références de tous les remotes du dépôt actif."))
                                    .when_some(self.error.clone(),|view,error| view.child(div().mt(px(8.)).text_size(px(12.+offset)).text_color(rgb(p.danger)).child(error)))))
                            .child(div().mt(px(24.)).pt(px(16.)).border_t_1().border_color(rgb(p.border)).flex().items_start().gap(px(8.)).text_size(px(12.+offset)).line_height(px((12.+offset)*1.625)).text_color(rgb(p.muted))
                                .child(icon(IconName::TriangleAlert,14.,p.muted))
                                .child("De nombreux remotes peuvent affecter les performances. Les objets LFS ne sont pas récupérés automatiquement.")))
                        .child(div().mt(px(20.)).min_h(px(36.)).flex().items_center().justify_between().gap(px(16.))
                            .child(div().flex().items_center().gap(px(8.)).text_size(px(12.+offset)).text_color(rgb(p.accent))
                                .when(self.saved,|view| view.child(icon(IconName::Check,14.,p.accent)).child("Paramètres enregistrés.")))
                            .child(div().flex().gap(px(12.))
                                .child(Button::new("settings-reset").outline().h(px(37.)).px(px(14.)).text_size(px(12.+offset)).label("Annuler les modifications").disabled(!changed)
                                    .on_click(cx.listener(|this,_,window,cx| { this.set_interval(this.interval,window,cx); })))
                                .child(Button::new("settings-save").primary().h(px(37.)).px(px(14.)).text_size(px(12.+offset)).label("Enregistrer").disabled(!changed)
                                    .on_click(cx.listener(|this,_,window,cx| this.save(window,cx))))))))
                    .child(scrollbar_theme(Scrollbar::vertical(&self.scroll).id("settings-scrollbar")))))
    }
}
