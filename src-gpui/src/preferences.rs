//! Optional native preferences with validated values and bounds.
use crate::theme::{Palette, ThemeId};
use mangrove_remote::ProfileStore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub const DEFAULT_FONT_SIZE_OFFSET: f32 = 1.;
pub const MIN_FONT_SIZE_OFFSET: f32 = 0.;
pub const MAX_FONT_SIZE_OFFSET: f32 = 5.;
/// Diffs start at the list text size of the interface: 11 px plus the default zoom.
pub const DEFAULT_DIFF_FONT_SIZE: f32 = 12.;
pub const MIN_DIFF_FONT_SIZE: f32 = 9.;
pub const MAX_DIFF_FONT_SIZE: f32 = 20.;
pub const DEFAULT_AUTO_FETCH_INTERVAL: u32 = 1;
pub const MAX_AUTO_FETCH_INTERVAL: u32 = 60;

pub(crate) fn profile_store() -> mangrove_remote::Result<ProfileStore> {
    #[cfg(feature = "app-benchmark")]
    if std::env::var_os("MANGROVE_PROFILES_PATH").is_none()
        && let Some(output) = std::env::var_os("MANGROVE_BENCH_OUTPUT")
    {
        return Ok(ProfileStore::new(
            PathBuf::from(output).with_extension("profiles.json"),
        ));
    }
    ProfileStore::open_default()
}

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
            theme: ThemeId::default(),
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
        let profile = profile_store()
            .and_then(|store| store.load())
            .map(|profiles| profiles.active_profile_id)
            .unwrap_or_else(|_| "personal".into());
        Self::load_for_profile(&profile)
    }
    pub fn load_for_profile(profile: &str) -> Self {
        Self::load_profile_from(&Self::path(), profile)
    }
    fn load_profile_from(base: &Path, profile: &str) -> Self {
        let Ok(path) = Self::profile_path(base, profile) else {
            return Self::default();
        };
        if profile == "personal" && !path.exists() {
            // Import the previous single-profile settings without overwriting them.
            Self::load_from(base)
        } else {
            Self::load_from(&path)
        }
    }
    pub fn profile_path(base: &Path, profile: &str) -> io::Result<PathBuf> {
        if profile.is_empty()
            || profile.len() > 64
            || !profile
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid profile identifier",
            ));
        }
        let stem = base.file_stem().unwrap_or_default().to_string_lossy();
        Ok(base
            .with_file_name(format!("{stem}-profiles"))
            .join(format!("{profile}.json")))
    }
    pub fn save_for_profile(&self, profile: &str) -> io::Result<()> {
        self.save_to(&Self::profile_path(&Self::path(), profile)?)
    }
    /// Deletes a removed profile's preferences; the legacy file is kept.
    pub fn remove_for_profile(profile: &str) -> io::Result<()> {
        Self::remove_profile_from(&Self::path(), profile)
    }
    fn remove_profile_from(base: &Path, profile: &str) -> io::Result<()> {
        match fs::remove_file(Self::profile_path(base, profile)?) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
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
        if let Some(theme) = value.get("theme") {
            result.theme = ThemeId::deserialize(theme).unwrap_or_default();
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
        self.font_size_offset = bounded(
            value,
            MIN_FONT_SIZE_OFFSET,
            MAX_FONT_SIZE_OFFSET,
            DEFAULT_FONT_SIZE_OFFSET,
        );
    }
    pub fn set_diff_font_size(&mut self, value: f32) {
        self.diff_font_size = bounded(
            value,
            MIN_DIFF_FONT_SIZE,
            MAX_DIFF_FONT_SIZE,
            DEFAULT_DIFF_FONT_SIZE,
        );
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
    fn profiles_keep_themes_and_recents_separate_and_import_legacy_only_once() {
        let directory = std::env::temp_dir().join(format!(
            "mangrove-profile-preferences-{}",
            std::process::id()
        ));
        let base = directory.join("preferences.json");
        let personal = Preferences {
            theme: ThemeId::Mangrove,
            recent: vec![RecentRepository {
                path: "/personal".into(),
                name: "Personal".into(),
            }],
            ..Preferences::default()
        };
        personal.save_to(&base).unwrap();
        let legacy = fs::read(&base).unwrap();
        assert_eq!(Preferences::load_profile_from(&base, "personal"), personal);
        assert_eq!(
            Preferences::load_profile_from(&base, "professional"),
            Preferences::default()
        );
        let work = Preferences {
            theme: ThemeId::Macchiato,
            auto_fetch_interval: 0,
            recent: vec![RecentRepository {
                path: "/work".into(),
                name: "Work".into(),
            }],
            ..Preferences::default()
        };
        work.save_to(&Preferences::profile_path(&base, "professional").unwrap())
            .unwrap();
        let changed = Preferences {
            theme: ThemeId::Classic,
            ..personal.clone()
        };
        changed
            .save_to(&Preferences::profile_path(&base, "personal").unwrap())
            .unwrap();
        assert_eq!(Preferences::load_profile_from(&base, "personal"), changed);
        assert_eq!(Preferences::load_profile_from(&base, "professional"), work);
        assert_eq!(fs::read(&base).unwrap(), legacy);
        assert!(Preferences::profile_path(&base, "../outside").is_err());
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn removed_profile_preferences_do_not_reach_a_reused_identifier() {
        let directory = std::env::temp_dir().join(format!(
            "mangrove-removed-profile-preferences-{}",
            std::process::id()
        ));
        let base = directory.join("preferences.json");
        let legacy = Preferences {
            theme: ThemeId::Ardoise,
            ..Preferences::default()
        };
        legacy.save_to(&base).unwrap();
        let removed = Preferences {
            theme: ThemeId::Gitlab,
            ..Preferences::default()
        };
        removed
            .save_to(&Preferences::profile_path(&base, "profile-1").unwrap())
            .unwrap();
        Preferences::remove_profile_from(&base, "profile-1").unwrap();
        assert_eq!(
            Preferences::load_profile_from(&base, "profile-1"),
            Preferences::default()
        );
        // Removing twice, or the default profile without its own file, is a no-op.
        Preferences::remove_profile_from(&base, "profile-1").unwrap();
        Preferences::remove_profile_from(&base, "personal").unwrap();
        assert_eq!(Preferences::load_profile_from(&base, "personal"), legacy);
        assert!(Preferences::remove_profile_from(&base, "../outside").is_err());
        fs::remove_dir_all(directory).unwrap();
    }
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
    fn every_theme_survives_a_reload_and_unknown_themes_use_the_default() {
        for theme in ThemeId::ALL {
            let saved = serde_json::to_value(Preferences {
                theme,
                ..Preferences::default()
            })
            .unwrap();
            assert_eq!(Preferences::from_value(saved).theme, theme);
        }
        let unknown = Preferences::from_value(serde_json::json!({"theme": "solarized"}));
        assert_eq!(unknown.theme, ThemeId::Classic);
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
        assert_eq!((p.font_size_offset, p.diff_font_size), (1., 12.));
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
