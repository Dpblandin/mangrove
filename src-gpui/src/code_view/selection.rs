//! Selections in snapshot byte positions: unmounted rows still take part in
//! copying, and a side-by-side diff selects one side at a time.
use std::ops::Range;

/// The text a selection reads: the unified rows, or one side of a
/// side-by-side diff.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    #[default]
    Unified,
    Old,
    New,
}

/// A byte of the original text of a displayed row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub row: usize,
    pub byte: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct Selection {
    pub side: Side,
    pub anchor: Position,
    pub head: Position,
}

impl Selection {
    pub fn caret(side: Side, position: Position) -> Self {
        Self {
            side,
            anchor: position,
            head: position,
        }
    }
    pub fn ordered(self) -> (Position, Position) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
    pub fn range_in_row(self, row: usize, length: usize) -> Option<Range<usize>> {
        let (start, end) = self.ordered();
        if row < start.row || row > end.row || start == end {
            return None;
        }
        Some(
            (if row == start.row { start.byte } else { 0 })..(if row == end.row {
                end.byte
            } else {
                length
            }),
        )
    }
}

/// The selected text, a line per row that `text` reads on the selected
/// side: the empty cells of a side-by-side diff add no line. Selecting all
/// of a file that ends with a newline copies that newline.
pub fn selected_text<'a>(
    selection: Option<Selection>,
    rows: usize,
    text: impl Fn(usize) -> Option<&'a str>,
    final_newline: bool,
) -> String {
    let Some(selection) = selection else {
        return String::new();
    };
    let (start, end) = selection.ordered();
    if start == end {
        return String::new();
    }
    let mut copied = String::new();
    let mut first = true;
    for index in start.row..=end.row.min(rows.saturating_sub(1)) {
        let Some(line) = text(index) else {
            continue;
        };
        let range = selection.range_in_row(index, line.len()).unwrap_or(0..0);
        if !first {
            copied.push('\n');
        }
        first = false;
        copied.push_str(&line[range]);
    }
    if final_newline
        && start == Position::default()
        && end.row + 1 == rows
        && text(end.row).is_some_and(|line| end.byte == line.len())
    {
        copied.push('\n');
    }
    copied
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_across_virtual_rows_excludes_gutters() {
        let selection = Selection {
            side: Side::Unified,
            anchor: Position { row: 1, byte: 2 },
            head: Position { row: 3, byte: 1 },
        };
        assert_eq!(selection.range_in_row(0, 9), None);
        assert_eq!(selection.range_in_row(1, 9), Some(2..9));
        assert_eq!(selection.range_in_row(2, 9), Some(0..9));
        assert_eq!(selection.range_in_row(3, 9), Some(0..1));
    }

    #[test]
    fn one_side_copies_its_lines_and_hunk_headers_without_empty_cells() {
        // Old side of: header, a pair, an addition alone, context.
        let old = [Some("@@ -1,2 +1,3 @@"), Some("let a = 1;"), None, Some("}")];
        let text = |index: usize| old[index];
        let selection = Selection {
            side: Side::Old,
            anchor: Position { row: 0, byte: 3 },
            head: Position { row: 3, byte: 1 },
        };
        assert_eq!(
            selected_text(Some(selection), old.len(), text, false),
            "-1,2 +1,3 @@\nlet a = 1;\n}"
        );
        // A selection reversed, or empty, copies the same or nothing.
        let reversed = Selection {
            anchor: selection.head,
            head: selection.anchor,
            ..selection
        };
        assert_eq!(
            selected_text(Some(reversed), old.len(), text, false),
            "-1,2 +1,3 @@\nlet a = 1;\n}"
        );
        let caret = Selection::caret(Side::Old, Position { row: 1, byte: 2 });
        assert_eq!(selected_text(Some(caret), old.len(), text, false), "");
        assert_eq!(selected_text(None, old.len(), text, false), "");
    }
}
