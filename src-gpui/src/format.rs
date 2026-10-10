//! Display formats shared by native views, matching src/lib/format.ts.

pub fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

pub fn initials(name: &str) -> String {
    name.split_whitespace()
        .take(2)
        .filter_map(|part| part.chars().next())
        .collect::<String>()
        .to_uppercase()
}

/// Full date for details, tooltips and blame: « 9 octobre 2026 à 14:32 ».
pub fn commit_date(seconds: i64) -> String {
    use chrono::Datelike;
    let Some(date) = local_date(seconds) else {
        return "Date inconnue".into();
    };
    const MONTHS: [&str; 12] = [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ];
    format!(
        "{} {} {} à {}",
        date.day(),
        MONTHS[date.month0() as usize],
        date.year(),
        date.format("%H:%M")
    )
}

/// Compact date for dense lists: « 9 oct. 14:32 » this year, « 9 oct. 2025 » before.
pub fn compact_date(seconds: i64) -> String {
    compact_date_at(seconds, chrono::Local::now())
}

fn compact_date_at(seconds: i64, now: chrono::DateTime<chrono::Local>) -> String {
    use chrono::Datelike;
    let Some(date) = local_date(seconds) else {
        return "Date inconnue".into();
    };
    const MONTHS: [&str; 12] = [
        "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
        "déc.",
    ];
    let day = format!("{} {}", date.day(), MONTHS[date.month0() as usize]);
    if date.year() == now.year() {
        format!("{day} {}", date.format("%H:%M"))
    } else {
        format!("{day} {}", date.year())
    }
}

fn local_date(seconds: i64) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::from_timestamp(seconds, 0).map(|date| date.with_timezone(&chrono::Local))
}

/// Shortcut label for the current platform: « ⌘K » on macOS, « Ctrl+K » elsewhere.
pub fn shortcut_label(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// Counted noun with French agreement: « 1 changement », « 2 changements ».
pub fn count_label(value: usize, singular: &str, plural: &str) -> String {
    let noun = if value < 2 { singular } else { plural };
    format!("{} {noun}", format_count(value))
}

/// Path for display, with the home directory shortened to `~`.
pub fn display_path(path: &str) -> String {
    let Some(home) = dirs::home_dir() else {
        return path.into();
    };
    let home = home.to_string_lossy();
    match path.strip_prefix(home.as_ref()) {
        Some("") => "~".into(),
        Some(rest) if rest.starts_with(std::path::MAIN_SEPARATOR) => format!("~{rest}"),
        _ => path.into(),
    }
}

pub fn format_count(value: usize) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push('\u{202f}');
        }
        formatted.push(ch);
    }
    formatted
}

/// Shortcut that keeps the Control key on every platform: « ⌃⇥ » or « ⌃⇧⇥ »
/// on macOS, « Ctrl+Tab » or « Ctrl+Maj+Tab » elsewhere.
pub fn control_shortcut_label(key: &str, shift: bool) -> String {
    if cfg!(target_os = "macos") {
        let key = if key == "Tab" { "⇥" } else { key };
        format!("⌃{}{key}", if shift { "⇧" } else { "" })
    } else {
        format!("Ctrl+{}{key}", if shift { "Maj+" } else { "" })
    }
}

#[cfg(test)]
mod tests {
    use super::{compact_date_at, count_label, display_path};
    use chrono::TimeZone;

    #[test]
    fn compact_date_shows_time_this_year_and_year_otherwise() {
        let now = chrono::Local
            .with_ymd_and_hms(2026, 10, 10, 12, 0, 0)
            .unwrap();
        let this_year = chrono::Local
            .with_ymd_and_hms(2026, 10, 9, 14, 32, 0)
            .unwrap();
        let last_year = chrono::Local.with_ymd_and_hms(2025, 3, 2, 8, 5, 0).unwrap();
        assert_eq!(compact_date_at(this_year.timestamp(), now), "9 oct. 14:32");
        assert_eq!(compact_date_at(last_year.timestamp(), now), "2 mars 2025");
    }

    #[test]
    fn count_label_agrees_in_french() {
        assert_eq!(count_label(0, "commit", "commits"), "0 commit");
        assert_eq!(count_label(1, "changement", "changements"), "1 changement");
        assert_eq!(count_label(2, "changement", "changements"), "2 changements");
        assert_eq!(
            count_label(1500, "commit", "commits"),
            "1\u{202f}500 commits"
        );
    }

    #[test]
    fn display_path_shortens_only_the_home_directory() {
        let home = dirs::home_dir().unwrap();
        let home = home.to_string_lossy();
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(display_path(&home), "~");
        assert_eq!(
            display_path(&format!("{home}{sep}code")),
            format!("~{sep}code")
        );
        assert_eq!(
            display_path(&format!("{home}other")),
            format!("{home}other")
        );
        assert_eq!(display_path("/tmp/repo"), "/tmp/repo");
    }
}
