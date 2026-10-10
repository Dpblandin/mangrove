//! Mangrove's CSS tokens, shared by every native view.
use gpui_kit::{
    App, Pixels, SharedString, Styled,
    component::{
        Icon, Sizable, Theme, ThemeMode,
        scroll::{Scrollbar, ScrollbarMode},
    },
    px, rgb,
};
use serde::{Deserialize, Serialize};

// Corner radii shared by every theme and by Kit controls.
/// Badges, chips, keycaps, icon-only buttons and items inside a padded menu.
pub const RADIUS_SM: Pixels = px(4.);
/// Inputs, selects, text buttons, list rows, menus and banners.
pub const RADIUS_MD: Pixels = px(6.);
/// Cards, panels, framed containers and tile buttons.
pub const RADIUS_LG: Pixels = px(8.);

pub const SANS: &str = ".SystemUIFont";
// Match the browser's installed monospace fallback, rather than a display name
// that Core Text resolves to the proportional system font.
#[cfg(target_os = "macos")]
pub const MONO: &str = "Menlo";
#[cfg(target_os = "windows")]
pub const MONO: &str = "Consolas";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const MONO: &str = "DejaVu Sans Mono";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeId {
    #[default]
    Classic,
    Mangrove,
    Macchiato,
    Ardoise,
    Gitlab,
}

impl ThemeId {
    pub const ALL: [Self; 5] = [
        Self::Classic,
        Self::Mangrove,
        Self::Macchiato,
        Self::Ardoise,
        Self::Gitlab,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Classic => "Classique",
            Self::Mangrove => "Mangrove",
            Self::Macchiato => "Catppuccin Macchiato",
            Self::Ardoise => "Ardoise",
            Self::Gitlab => "Gitlab",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub bg: u32,
    pub reading_bg: u32,
    pub panel: u32,
    pub surface: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub success: u32,
    pub danger: u32,
    pub warning: u32,
    pub renamed: u32,
    /// Text and icons on an accent fill, such as primary buttons.
    pub on_accent: u32,
    pub addition_bg: u32,
    pub deletion_bg: u32,
    pub selection: u32,
    pub accent_soft: u32,
    pub accent_border: u32,
    pub hover: u32,
    pub hunk_bg: u32,
    pub addition_gutter: u32,
    pub deletion_gutter: u32,
    pub graph: [u32; 8],
    /// keyword, string, comment, number, type, function, punctuation, property.
    pub syntax: [u32; 8],
}

impl Default for Palette {
    fn default() -> Self {
        Self::for_theme(ThemeId::default())
    }
}

/// Resolve an sRGB color-mix without a color-space conversion, as in style.css.
pub fn mix(foreground: u32, background: u32, fraction: f32) -> u32 {
    let channel = |shift| {
        let front = ((foreground >> shift) & 255_u32) as f32;
        let back = ((background >> shift) & 255_u32) as f32;
        (front * fraction + back * (1. - fraction)).round() as u32
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

impl Palette {
    pub fn for_theme(theme: ThemeId) -> Self {
        let mut palette = Self {
            bg: 0x1e2025,
            reading_bg: 0x1e2025,
            panel: 0x292c33,
            surface: 0x33363e,
            border: 0x454952,
            text: 0xf0f0f2,
            muted: 0xb7bac5,
            accent: 0x64b5ff,
            success: 0x8bd49c,
            danger: 0xf09091,
            warning: 0xedc27d,
            renamed: 0x64b5ff,
            addition_bg: 0x315638,
            deletion_bg: 0x542e32,
            on_accent: 0x13212f,
            graph: [
                0x64b5ff, 0x9fd190, 0xc49af2, 0xe2bf77, 0xec98a5, 0x82ccd2, 0xf1ad80, 0xc3b3f3,
            ],
            syntax: [
                0x75cfff, 0xffb87c, 0xb6d891, 0xffa4e4, 0x63ecdc, 0xffffff, 0xffe66d, 0xf0f0f2,
            ],
            selection: 0,
            accent_soft: 0,
            accent_border: 0,
            hover: 0,
            hunk_bg: 0,
            addition_gutter: 0,
            deletion_gutter: 0,
        };
        match theme {
            ThemeId::Classic => {}
            // Reading-first slate: off-white text avoids halation, and every
            // text or status color keeps about 7:1 contrast on panels.
            ThemeId::Ardoise => {
                palette.bg = 0x161b21;
                palette.reading_bg = 0x1f252e;
                palette.panel = 0x1e232c;
                palette.surface = 0x272d39;
                palette.border = 0x363e4c;
                palette.text = 0xe4e9f0;
                palette.muted = 0xadb8c9;
                palette.accent = 0x72bbf5;
                palette.success = 0x78d6a0;
                palette.danger = 0xf5908b;
                palette.warning = 0xf2c76c;
                palette.renamed = 0x9db4ff;
                palette.addition_bg = 0x234532;
                palette.deletion_bg = 0x4c2a31;
                palette.on_accent = 0x0d1724;
                palette.graph = [
                    0x72bbf5, 0x8bd99a, 0xc7a2f4, 0xe9c37a, 0xee9eab, 0x77d0d6, 0xf2b088, 0xb9adf6,
                ];
                palette.syntax = [
                    0x8ab9f8, 0xa9d9a2, 0xa1adbf, 0xf2bb86, 0x80d4d1, 0xe5cc94, 0xb3bccb, 0xd6dce6,
                ];
            }
            ThemeId::Mangrove => {
                palette.bg = 0x101719;
                palette.reading_bg = palette.bg;
                palette.panel = 0x151e20;
                palette.surface = 0x1b272a;
                palette.border = 0x354743;
                palette.text = 0xdbe7e4;
                palette.muted = 0xadbfba;
                palette.accent = 0x78dfb9;
                palette.success = 0xa6dfbf;
                palette.danger = 0xe49291;
                palette.warning = 0xd5bb87;
                palette.renamed = 0x88aefa;
                palette.addition_bg = 0x244d3f;
                palette.deletion_bg = 0x542f33;
                palette.on_accent = 0x10261d;
                palette.graph = [
                    0x78dfb9, 0x88aefa, 0xcf9ce8, 0xe8be79, 0xea97a6, 0x73c6ce, 0xb5ce81, 0xb5a1ef,
                ];
            }
            ThemeId::Macchiato => {
                palette.bg = 0x24273a;
                palette.panel = 0x1e2030;
                palette.reading_bg = palette.panel;
                palette.surface = 0x363a4f;
                palette.border = 0x494d64;
                palette.text = 0xcad3f5;
                palette.muted = 0xb8c0e0;
                palette.accent = 0x8aadf4;
                palette.success = 0xa6da95;
                palette.danger = 0xed8796;
                palette.warning = 0xeed49f;
                palette.renamed = 0x8aadf4;
                palette.addition_bg = 0x354b39;
                palette.deletion_bg = 0x4c313b;
                palette.on_accent = 0x181926;
                palette.graph = [
                    0x8aadf4, 0xa6da95, 0xc6a0f6, 0xeed49f, 0xed8796, 0x8bd5ca, 0xf5a97f, 0xb7bdf8,
                ];
            }
            // GitLab's dark neutral UI, diff and Highlight.js tokens, sampled
            // from https://gitlab.com/dpascaud/tailord on 2026-10-02.
            ThemeId::Gitlab => {
                palette.bg = 0x050506;
                palette.reading_bg = 0x18171d;
                palette.panel = 0x18171d;
                palette.surface = 0x28272d;
                palette.border = 0x4c4b51;
                palette.text = 0xececef;
                palette.muted = 0xbfbfc3;
                palette.accent = 0x9dc8f2;
                palette.success = 0x84d4b6;
                palette.danger = 0xffafb8;
                palette.warning = 0xf7b951;
                palette.renamed = palette.accent;
                palette.addition_bg = 0x233726;
                palette.deletion_bg = 0x4b2526;
                palette.on_accent = 0x18171d;
                palette.graph = [
                    0x9dc8f2, 0x84d4b6, 0xa997e1, 0xf7b951, 0xffafb8, 0x32c5d2, 0xe99b60, 0xf88aaf,
                ];
                palette.syntax = [
                    0xb294bb, 0xb5bd68, 0x969896, 0xde935f, 0xf0c674, 0x81a2be, 0xc5c8c6, 0x81a2be,
                ];
            }
        }
        palette.selection = mix(palette.accent, palette.bg, 0.14);
        palette.accent_soft = mix(palette.accent, palette.bg, 0.10);
        palette.accent_border = mix(palette.accent, palette.border, 0.28);
        palette.hover = mix(palette.text, palette.bg, 0.06);
        palette.hunk_bg = mix(palette.accent, palette.reading_bg, 0.12);
        palette.addition_gutter = mix(palette.success, palette.addition_bg, 0.10);
        palette.deletion_gutter = mix(palette.danger, palette.deletion_bg, 0.10);
        if theme == ThemeId::Gitlab {
            // GitLab keeps navigation selection and hover neutral.
            palette.selection = palette.surface;
            palette.hover = palette.surface;
        }
        palette
    }

    /// Theme Kit's editable controls and menus with the application's colors.
    pub fn apply(self, _font_size_offset: f32, cx: &mut App) {
        Theme::change(ThemeMode::Dark, None, cx);
        // Kit controls read renderable tokens as well as solid colors. Keep
        // those and the Base projection synchronized when applying CSS colors.
        Theme::update(cx, |theme| {
            theme.font_family = SharedString::from(SANS);
            // The CSS root keeps rem at 16px; zoom changes text, never panel spacing.
            theme.font_size = gpui_kit::px(16.);
            theme.mono_font_family = SharedString::from(MONO);
            theme.radius = RADIUS_MD;
            theme.radius_lg = RADIUS_LG;
            let c = &mut theme.colors;
            c.background = rgb(self.bg).into();
            c.foreground = rgb(self.text).into();
            c.border = rgb(self.border).into();
            c.input = rgb(self.border).into();
            c.muted = rgb(self.surface).into();
            c.muted_foreground = rgb(self.muted).into();
            c.accent = rgb(self.accent_soft).into();
            c.accent_foreground = rgb(self.accent).into();
            c.primary = rgb(self.accent).into();
            c.primary_foreground = rgb(self.on_accent).into();
            c.primary_hover = rgb(mix(self.accent, self.text, 0.85)).into();
            c.primary_active = c.primary_hover;
            c.button_primary = c.primary;
            c.button_primary_foreground = c.primary_foreground;
            c.button_primary_hover = c.primary_hover;
            c.button_primary_active = c.primary_active;
            c.button = rgb(self.surface).into();
            c.button_foreground = rgb(self.text).into();
            c.button_hover = rgb(mix(self.text, self.surface, 0.06)).into();
            c.popover = rgb(self.panel).into();
            c.popover_foreground = rgb(self.text).into();
            c.list = rgb(self.panel).into();
            c.list_active = rgb(self.selection).into();
            c.list_hover = rgb(self.hover).into();
            c.ring = rgb(self.accent).into();
            c.caret = rgb(self.accent).into();
            c.selection = rgb(self.selection).into();
            c.danger = rgb(self.danger).into();
            c.success = rgb(self.success).into();
            c.warning = rgb(self.warning).into();
            c.scrollbar_thumb = rgb(self.muted).opacity(0.4).into();
            c.scrollbar_thumb_hover = rgb(self.muted).opacity(0.6).into();
            c.scrollbar = gpui_kit::transparent_black();
        });
    }
}

/// Match the CSS scrollbar geometry; colors come from the synchronized theme.
pub fn scrollbar_theme(scrollbar: Scrollbar) -> Scrollbar {
    scrollbar.mode(ScrollbarMode::Always).styles(|styles| {
        styles
            .track(|style| {
                style
                    .width(gpui_kit::px(6.))
                    .bg(gpui_kit::transparent_black())
                    .border_color(gpui_kit::transparent_black())
            })
            .track_hover(|style| {
                style
                    .bg(gpui_kit::transparent_black())
                    .border_color(gpui_kit::transparent_black())
            })
            .track_active(|style| {
                style
                    .bg(gpui_kit::transparent_black())
                    .border_color(gpui_kit::transparent_black())
            })
            .thumb(|style| {
                style
                    .width(gpui_kit::px(6.))
                    .inset(gpui_kit::px(0.))
                    .radius(gpui_kit::px(3.))
            })
            .thumb_hover(|style| {
                style
                    .width(gpui_kit::px(6.))
                    .inset(gpui_kit::px(0.))
                    .radius(gpui_kit::px(3.))
            })
            .thumb_active(|style| {
                style
                    .width(gpui_kit::px(6.))
                    .inset(gpui_kit::px(0.))
                    .radius(gpui_kit::px(3.))
            })
    })
}

pub fn icon(name: gpui_kit::assets::IconName, size: f32, color: u32) -> Icon {
    Icon::new(name)
        .with_size(gpui_kit::px(size))
        .text_color(rgb(color))
}

#[cfg(test)]
mod tests {
    use super::{Palette, ThemeId, mix};

    fn luminance(color: u32) -> f64 {
        let channel = |shift: u32| {
            let value = f64::from((color >> shift) & 255) / 255.;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }

    fn contrast(foreground: u32, background: u32) -> f64 {
        let (a, b) = (luminance(foreground), luminance(background));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn text_on_accent_meets_wcag_aa_in_every_theme_and_state() {
        for theme in ThemeId::ALL {
            let p = Palette::for_theme(theme);
            // Rest, Kit's primary hover and the 90% opacity hover over panels.
            for background in [
                p.accent,
                mix(p.accent, p.text, 0.85),
                mix(p.accent, p.panel, 0.9),
            ] {
                let ratio = contrast(p.on_accent, background);
                assert!(
                    ratio >= 4.5,
                    "{}: {ratio:.2}:1 on {background:06x}",
                    theme.label()
                );
            }
        }
    }
}
