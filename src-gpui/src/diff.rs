/// The role of a row in a code view: a patch line, a hunk header or a line
/// of a file's contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Hunk,
    Context,
    Addition,
    Deletion,
    /// « No newline at end of file » and similar notes, with no line number.
    Annotation,
    Content,
}

impl LineKind {
    /// The kind of a patch line, which the engine names `addition`,
    /// `deletion`, `annotation` or `context`.
    pub fn of(kind: &str) -> Self {
        match kind {
            "addition" => Self::Addition,
            "deletion" => Self::Deletion,
            "annotation" => Self::Annotation,
            _ => Self::Context,
        }
    }
}

/// Remove the single line terminator introduced by Git, preserving all other
/// whitespace (including a literal trailing carriage return).
pub fn strip_line_ending(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
}

#[cfg(test)]
mod tests {
    use super::LineKind;

    #[test]
    fn patch_lines_take_the_kind_named_by_the_engine() {
        for (name, kind) in [
            ("addition", LineKind::Addition),
            ("deletion", LineKind::Deletion),
            ("annotation", LineKind::Annotation),
            ("context", LineKind::Context),
        ] {
            assert_eq!(LineKind::of(name), kind);
        }
    }
}
