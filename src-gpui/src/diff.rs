use mangrove_core::model::FileDiff;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct DiffRow {
    pub kind: String,
    pub old_number: String,
    pub new_number: String,
    pub text: String,
}

pub fn rows(diff: Option<&FileDiff>) -> Arc<Vec<DiffRow>> {
    Arc::new(
        diff.into_iter()
            .flat_map(|diff| &diff.hunks)
            .flat_map(|hunk| {
                std::iter::once(DiffRow {
                    kind: "hunk".into(),
                    old_number: String::new(),
                    new_number: String::new(),
                    text: hunk.header.trim_end().to_owned(),
                })
                .chain(hunk.lines.iter().map(|line| DiffRow {
                    kind: line.kind.clone(),
                    old_number: line.old_line.map(|n| n.to_string()).unwrap_or_default(),
                    new_number: line.new_line.map(|n| n.to_string()).unwrap_or_default(),
                    text: strip_line_ending(&line.content).to_owned(),
                }))
            })
            .collect(),
    )
}

/// Remove the single line terminator introduced by Git, preserving all other
/// whitespace (including a literal trailing carriage return).
pub fn strip_line_ending(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
}
