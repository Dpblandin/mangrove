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

pub fn commit_date(seconds: i64) -> String {
    use chrono::Datelike;
    let Some(date) = chrono::DateTime::from_timestamp(seconds, 0) else {
        return "Date inconnue".into();
    };
    let date = date.with_timezone(&chrono::Local);
    let months = [
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
        months[date.month0() as usize],
        date.year(),
        date.format("%H:%M")
    )
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
