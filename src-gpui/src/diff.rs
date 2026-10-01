/// Remove the single line terminator introduced by Git, preserving all other
/// whitespace (including a literal trailing carriage return).
pub fn strip_line_ending(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
}
