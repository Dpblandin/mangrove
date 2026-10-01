//! Optional native preferences with validated values and bounds.
use crate::theme::{Palette, ThemeId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub const DEFAULT_FONT_SIZE_OFFSET: f32 = 1.;
pub const DEFAULT_DIFF_FONT_SIZE: f32 = 11.;
pub const DEFAULT_AUTO_FETCH_INTERVAL: u32 = 1;
pub const MAX_AUTO_FETCH_INTERVAL: u32 = 60;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentRepository {
    pub path: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneLayout {
    pub left: f32,
    pub right: f32,
    pub left_min: f32,
    pub left_max: f32,
    pub right_min: f32,
    pub right_max: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Preferences {
    pub theme: ThemeId,
    pub font_size_offset: f32,
    pub diff_font_size: f32,
    pub left_width: f32,
    pub right_width: f32,
    pub auto_fetch_interval: u32,
    pub show_image_previews: bool,
    pub recent: Vec<RecentRepository>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: ThemeId::Classic,
            font_size_offset: DEFAULT_FONT_SIZE_OFFSET,
            diff_font_size: DEFAULT_DIFF_FONT_SIZE,
            left_width: 226.,
            right_width: 390.,
            auto_fetch_interval: DEFAULT_AUTO_FETCH_INTERVAL,
            show_image_previews: true,
            recent: Vec::new(),
        }
    }
}

pub fn parse_auto_fetch_interval(value: &str) -> Option<u32> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let number: f64 = value.parse().ok()?;
    if number.is_finite() && number.fract() == 0. && (0. ..=60.).contains(&number) {
        Some(number as u32)
    } else {
        None
    }
}

fn bounded(value: f32, minimum: f32, maximum: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.round().clamp(minimum, maximum)
    } else {
        fallback
    }
}

impl Preferences {
    pub fn path() -> PathBuf {
        if let Some(path) = std::env::var_os("MANGROVE_GPUI_PREFERENCES") {
            return path.into();
        }
        #[cfg(feature = "app-benchmark")]
        if let Some(output) = std::env::var_os("MANGROVE_BENCH_OUTPUT") {
            return PathBuf::from(output).with_extension("preferences.json");
        }
        dirs::config_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(std::env::temp_dir)
            .join("mangrove-gpui")
            .join("preferences.json")
    }
    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }
    pub fn load_from(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .map(Self::from_value)
            .unwrap_or_default()
    }
    fn from_value(value: Value) -> Self {
        let mut result = Self::default();
        if let Some(theme) = value.get("theme").and_then(Value::as_str) {
            result.theme = match theme {
                "mangrove" => ThemeId::Mangrove,
                "macchiato" => ThemeId::Macchiato,
                _ => ThemeId::Classic,
            };
        }
        let number = |name| value.get(name).and_then(Value::as_f64).map(|v| v as f32);
        if let Some(v) = number("font_size_offset") {
            result.set_font_size_offset(v);
        }
        if let Some(v) = number("diff_font_size") {
            result.set_diff_font_size(v);
        }
        if let Some(v) = number("left_width") {
            result.left_width = bounded(v, 180., 400., 226.);
        }
        if let Some(v) = number("right_width") {
            result.right_width = bounded(v, 280., f32::MAX, 390.);
        }
        if let Some(v) = value.get("auto_fetch_interval").and_then(Value::as_f64)
            && v.is_finite()
            && v.fract() == 0.
            && (0. ..=60.).contains(&v)
        {
            result.auto_fetch_interval = v as u32;
        }
        if let Some(value) = value.get("show_image_previews").and_then(Value::as_bool) {
            result.show_image_previews = value;
        }
        if let Some(recent) = value.get("recent").and_then(Value::as_array) {
            for value in recent {
                if let Ok(entry) = serde_json::from_value::<RecentRepository>(value.clone())
                    && !result
                        .recent
                        .iter()
                        .any(|existing| existing.path == entry.path)
                {
                    result.recent.push(entry);
                    if result.recent.len() == 6 {
                        break;
                    }
                }
            }
        }
        result
    }
    pub fn save(&self) -> io::Result<()> {
        self.save_to(&Self::path())
    }
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        fs::write(&temporary, data)?;
        fs::rename(&temporary, path)
    }
    pub fn palette(&self) -> Palette {
        Palette::for_theme(self.theme)
    }
    pub fn apply_palette(&self, cx: &mut gpui_kit::App) {
        self.palette().apply(self.font_size_offset, cx);
    }
    pub fn set_font_size_offset(&mut self, value: f32) {
        self.font_size_offset = bounded(value, 0., 5., DEFAULT_FONT_SIZE_OFFSET);
    }
    pub fn set_diff_font_size(&mut self, value: f32) {
        self.diff_font_size = bounded(value, 9., 20., DEFAULT_DIFF_FONT_SIZE);
    }
    /// `None` resets; +/-1 changes UI and diff sizes together, with the UI bounds.
    pub fn font_shortcut(&mut self, delta: Option<i32>) {
        let previous = self.font_size_offset;
        self.set_font_size_offset(delta.map_or(DEFAULT_FONT_SIZE_OFFSET, |d| previous + d as f32));
        self.set_diff_font_size(delta.map_or(DEFAULT_DIFF_FONT_SIZE, |_| {
            self.diff_font_size + self.font_size_offset - previous
        }));
    }
    pub fn remember_repository(&mut self, entry: RecentRepository) {
        self.recent.retain(|existing| existing.path != entry.path);
        self.recent.insert(0, entry);
        self.recent.truncate(6);
    }
    pub fn remove_recent(&mut self, path: &str) {
        self.recent.retain(|entry| entry.path != path);
    }
    pub fn pane_layout(&self, available: f32) -> PaneLayout {
        let available = if available.is_finite() {
            available.max(880.)
        } else {
            1440.
        };
        let left_max = 400_f32.min(available - 280. - 340. - 12.);
        let left = self.left_width.clamp(180., left_max);
        let right_max = available - left - 340. - 12.;
        PaneLayout {
            left,
            right: self.right_width.clamp(280., right_max),
            left_min: 180.,
            left_max,
            right_min: 280.,
            right_max,
        }
    }
    pub fn resize_pane(&mut self, pane: Pane, width: f32, available: f32) {
        if !width.is_finite() {
            return;
        }
        let layout = self.pane_layout(available);
        match pane {
            Pane::Left => self.left_width = width.round().clamp(layout.left_min, layout.left_max),
            Pane::Right => {
                self.right_width = width.round().clamp(layout.right_min, layout.right_max)
            }
        }
    }
    pub fn reset_pane(&mut self, pane: Pane) {
        match pane {
            Pane::Left => self.left_width = 226.,
            Pane::Right => self.right_width = 390.,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tolerant_preferences_preserve_valid_values_and_bound_untrusted_fields() {
        let p = Preferences::from_value(
            serde_json::json!({"theme":"macchiato","font_size_offset":99,"diff_font_size":1,"left_width":999,"right_width":1,"auto_fetch_interval":1.5,"recent":[{"path":"a","name":"A"},{"path":"a","name":"duplicate"},{"path":4,"name":"invalid"},{"path":"b","name":"B"}]}),
        );
        assert_eq!(
            (
                p.theme,
                p.font_size_offset,
                p.diff_font_size,
                p.left_width,
                p.right_width,
                p.auto_fetch_interval
            ),
            (ThemeId::Macchiato, 5., 9., 400., 280., 1)
        );
        assert_eq!(p.recent.len(), 2);
    }
    #[test]
    fn narrow_window_preserves_preferences_and_history_minimum() {
        let p = Preferences {
            left_width: 400.,
            right_width: 900.,
            ..Preferences::default()
        };
        let layout = p.pane_layout(880.);
        assert_eq!(layout.left, 248.);
        assert_eq!(layout.right, 280.);
        assert_eq!(p.pane_layout(1600.).right, 848.);
        assert_eq!(p.right_width, 900.);
    }
    #[test]
    fn shortcuts_keep_diff_size_unchanged_when_ui_size_is_at_its_limit() {
        let mut p = Preferences {
            font_size_offset: 5.,
            diff_font_size: 15.,
            ..Preferences::default()
        };
        p.font_shortcut(Some(1));
        assert_eq!(p.diff_font_size, 15.);
        p.font_shortcut(Some(-1));
        assert_eq!((p.font_size_offset, p.diff_font_size), (4., 14.));
        p.font_shortcut(None);
        assert_eq!((p.font_size_offset, p.diff_font_size), (1., 11.));
    }
    #[test]
    fn interval_requires_a_whole_number_in_the_supported_range() {
        assert_eq!(parse_auto_fetch_interval(" 0 "), Some(0));
        assert_eq!(parse_auto_fetch_interval("60"), Some(60));
        for value in ["", "-1", "61", "1.5", "NaN", "inf"] {
            assert_eq!(parse_auto_fetch_interval(value), None);
        }
    }
    #[test]
    fn persisted_preferences_roundtrip_and_bad_file_is_optional() {
        let path = std::env::temp_dir().join(format!(
            "mangrove-gpui-preferences-test-{}.json",
            std::process::id()
        ));
        let mut p = Preferences::default();
        for n in 0..8 {
            p.remember_repository(RecentRepository {
                path: format!("/{n}"),
                name: format!("repo {n}"),
            });
        }
        p.theme = ThemeId::Mangrove;
        p.show_image_previews = false;
        p.save_to(&path).unwrap();
        assert_eq!(Preferences::load_from(&path), p);
        assert_eq!(p.recent.len(), 6);
        fs::write(&path, "damaged").unwrap();
        assert_eq!(Preferences::load_from(&path), Preferences::default());
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn older_or_malformed_image_preferences_keep_the_default() {
        for value in [
            serde_json::json!({}),
            serde_json::json!({"show_image_previews": "false"}),
        ] {
            assert!(Preferences::from_value(value).show_image_previews);
        }
    }
}
