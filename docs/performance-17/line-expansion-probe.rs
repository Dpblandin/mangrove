use std::{sync::Arc, time::Instant};
type SharedString = Arc<str>;
#[derive(Clone, Debug)]
struct DisplayLine {
    text: SharedString,
    /// Original byte positions mapped to display byte positions, only for tabs.
    mapping: Option<Vec<usize>>,
    original_end: usize,
    original_len: usize,
    content_end: usize,
    columns: usize,
}
// Bound expansion, shaping and glyph preparation even for a line made entirely
// of tabs. The omitted bytes remain in Row::original for selection and copying.
const MAX_DISPLAY_BYTES: usize = 16 * 1024;
const OMITTED_LINE: &str = " … [suite masquée ; sélectionner la ligne pour tout copier]";

impl DisplayLine {
    fn new(text: Arc<str>) -> Self {
        let budget = MAX_DISPLAY_BYTES - OMITTED_LINE.len();
        let mut end = text.len().min(budget);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if !text[..end].contains('\t') {
            let columns = text[..end].chars().count();
            let display = if end == text.len() {
                text.clone().into()
            } else {
                format!("{}{OMITTED_LINE}", &text[..end]).into()
            };
            return Self {
                text: display,
                mapping: None,
                original_end: end,
                original_len: text.len(),
                content_end: end,
                columns: columns
                    + if end < text.len() {
                        OMITTED_LINE.chars().count()
                    } else {
                        0
                    },
            };
        }
        let mut expanded = String::new();
        let mut map = vec![0];
        let mut column = 0;
        let mut original_end = 0;
        for (offset, ch) in text.char_indices() {
            let before = expanded.len();
            let bytes = if ch == '\t' {
                8 - column % 8
            } else {
                ch.len_utf8()
            };
            if before + bytes > budget {
                break;
            }
            if ch == '\t' {
                expanded.extend(std::iter::repeat_n(' ', bytes));
                column += bytes;
            } else {
                expanded.push(ch);
                column += 1;
            }
            map.extend(std::iter::repeat_n(before, ch.len_utf8() - 1));
            map.push(expanded.len());
            original_end = offset + ch.len_utf8();
        }
        let content_end = expanded.len();
        if original_end < text.len() {
            expanded.push_str(OMITTED_LINE);
            column += OMITTED_LINE.chars().count();
        }
        Self {
            text: expanded.into(),
            mapping: Some(map),
            original_end,
            original_len: text.len(),
            content_end,
            columns: column,
        }
    }
    fn truncated(&self) -> bool {
        self.original_end < self.original_len
    }
    fn display_byte(&self, original: usize) -> usize {
        if original > self.original_end {
            return self.text.len();
        }
        self.mapping
            .as_ref()
            .map_or(original, |map| map[original.min(map.len() - 1)])
    }
    fn original_byte(&self, display: usize) -> usize {
        if display > self.content_end {
            return self.original_len;
        }
        self.mapping.as_ref().map_or(display, |map| {
            let right = map.partition_point(|byte| *byte < display);
            if right == 0 {
                0
            } else if right >= map.len() {
                map.len() - 1
            } else if display - map[right - 1] < map[right] - display {
                right - 1
            } else {
                right
            }
        })
    }
}

fn old_display(text: &str) -> (String, Option<Vec<usize>>) {
    if !text.contains('\t') {
        return (text.to_owned(), None);
    }
    let mut expanded = String::new();
    let mut map = vec![0; text.len() + 1];
    let mut column = 0;
    for (offset, ch) in text.char_indices() {
        let before = expanded.len();
        map[offset] = before;
        if ch == '\t' {
            let spaces = 8 - column % 8;
            expanded.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            expanded.push(ch);
            column += 1;
        }
        for entry in map.iter_mut().take(offset + ch.len_utf8()).skip(offset + 1) {
            *entry = before;
        }
        map[offset + ch.len_utf8()] = expanded.len();
    }
    (expanded, Some(map))
}
fn measure(name: &str, original: Arc<str>) {
    std::hint::black_box(old_display(&original));
    std::hint::black_box(DisplayLine::new(original.clone()));
    let mut old_ms = Vec::new();
    let mut new_ms = Vec::new();
    let mut sizes = (0, 0, 0, 0);
    for _ in 0..20 {
        let start = Instant::now();
        let value = old_display(&original);
        old_ms.push(start.elapsed().as_secs_f64() * 1000.);
        sizes.0 = value.0.len();
        sizes.1 = value
            .1
            .as_ref()
            .map_or(0, |map| map.len() * std::mem::size_of::<usize>());
        std::hint::black_box(&value);
        drop(value);
        let start = Instant::now();
        let value = DisplayLine::new(original.clone());
        new_ms.push(start.elapsed().as_secs_f64() * 1000.);
        sizes.2 = value.text.len();
        sizes.3 = value
            .mapping
            .as_ref()
            .map_or(0, |map| map.len() * std::mem::size_of::<usize>());
        std::hint::black_box(&value);
        drop(value);
    }
    old_ms.sort_by(f64::total_cmp);
    new_ms.sort_by(f64::total_cmp);
    println!(
        "{{\"fixture\":\"{name}\",\"originalBytes\":{},\"samples\":20,\"oldP50Ms\":{:.6},\"oldP95Ms\":{:.6},\"newP50Ms\":{:.6},\"newP95Ms\":{:.6},\"oldDisplayBytes\":{},\"oldMappingBytes\":{},\"newDisplayBytes\":{},\"newMappingBytes\":{}}}",
        original.len(),
        old_ms[10],
        old_ms[18],
        new_ms[10],
        new_ms[18],
        sizes.0,
        sizes.1,
        sizes.2,
        sizes.3
    );
}
fn main() {
    measure("512 KiB tabs", "\t".repeat(512 * 1024).into());
    measure("400 KiB ASCII", "x".repeat(400 * 1024).into());
    measure("UTF8 plus tabs", "\télève🦀".repeat(50_000).into());
}
