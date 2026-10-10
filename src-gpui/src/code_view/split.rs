//! Side-by-side rows of a diff. Unified rows keep the text, tokens and changed
//! words; a split row only names the unified rows shown in its two cells.
use std::iter;

use crate::diff::{self, LineKind};

/// Entering the side-by-side layout needs this much width beyond its
/// minimum, so a view resized near the threshold does not switch back and forth.
const HYSTERESIS: f32 = 32.;

/// A hunk header across both columns, or the unified row of each cell, `None`
/// for an empty one. Context shows the same row in both cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitRow {
    Hunk(usize),
    Pair {
        old: Option<usize>,
        new: Option<usize>,
    },
}

impl SplitRow {
    fn cells(self) -> impl Iterator<Item = usize> {
        match self {
            Self::Hunk(row) => [Some(row), None],
            Self::Pair { old, new } => [old, new],
        }
        .into_iter()
        .flatten()
    }
}

pub struct SplitLayout {
    pub rows: Vec<SplitRow>,
    /// The split row showing each unified row.
    split_of_unified: Vec<usize>,
}

impl SplitLayout {
    /// The rows paired by `diff::pairs` share a row, as their changed words do.
    pub fn new(kinds: &[LineKind], pairs: &[(usize, usize)]) -> Self {
        let mut rows = Vec::with_capacity(kinds.len());
        let mut blocks = diff::blocks(kinds).into_iter().peekable();
        let mut pairs = pairs.iter().copied().peekable();
        let mut row = 0;
        while row < kinds.len() {
            if let Some(block) = blocks.next_if(|block| block.rows.start == row) {
                row = block.rows.end;
                let paired: Vec<_> =
                    iter::from_fn(|| pairs.next_if(|&(old, _)| old < block.rows.end)).collect();
                rows.extend(block_rows(&block, kinds, &paired));
                continue;
            }
            rows.push(match kinds[row] {
                LineKind::Hunk => SplitRow::Hunk(row),
                _ => SplitRow::Pair {
                    old: Some(row),
                    new: Some(row),
                },
            });
            row += 1;
        }
        let mut split_of_unified = vec![0; kinds.len()];
        for (index, row) in rows.iter().enumerate() {
            for unified in row.cells() {
                split_of_unified[unified] = index;
            }
        }
        Self {
            rows,
            split_of_unified,
        }
    }

    /// The split row showing a unified row. Rows past the end, as a list
    /// scrolled to its bottom reports them, map to the last one.
    pub fn row_of(&self, unified: usize) -> usize {
        self.split_of_unified
            .get(unified)
            .copied()
            .unwrap_or(self.rows.len().saturating_sub(1))
    }

    /// The first unified row shown on `row`, or on the last row past the end.
    pub fn unified_of(&self, row: usize) -> usize {
        self.rows
            .get(row)
            .or(self.rows.last())
            .and_then(|row| row.cells().min())
            .unwrap_or(0)
    }
}

/// Pairs side by side, the unpaired lines between two pairs zipped, then each
/// annotation under the line it follows, on that line's side.
fn block_rows<'a>(
    block: &'a diff::Block,
    kinds: &'a [LineKind],
    pairs: &[(usize, usize)],
) -> impl Iterator<Item = SplitRow> + 'a {
    let (deletions, additions) = (&block.deletions, &block.additions);
    let mut cells: Vec<(Option<usize>, Option<usize>)> = Vec::new();
    let (mut old, mut new) = (0, 0);
    let end = (deletions.len(), additions.len());
    // Pairs by position in the block.
    let positions = pairs.iter().filter_map(|&(old, new)| {
        Some((
            deletions.binary_search(&old).ok()?,
            additions.binary_search(&new).ok()?,
        ))
    });
    for (paired_old, paired_new) in positions.chain([end]) {
        let (removed, added) = (&deletions[old..paired_old], &additions[new..paired_new]);
        for offset in 0..removed.len().max(added.len()) {
            cells.push((removed.get(offset).copied(), added.get(offset).copied()));
        }
        if (paired_old, paired_new) != end {
            cells.push((Some(deletions[paired_old]), Some(additions[paired_new])));
        }
        (old, new) = (paired_old + 1, paired_new + 1);
    }
    for &annotation in &block.annotations {
        let Some(owner) = (block.rows.start..annotation)
            .rev()
            .find(|&row| kinds[row] != LineKind::Annotation)
        else {
            continue;
        };
        let old_side = kinds[owner] == LineKind::Deletion;
        let side = |(old, new): (Option<usize>, Option<usize>)| if old_side { old } else { new };
        let alone = if old_side {
            (Some(annotation), None)
        } else {
            (None, Some(annotation))
        };
        let Some(line) = cells.iter().position(|&cell| side(cell) == Some(owner)) else {
            continue;
        };
        match cells.get_mut(line + 1) {
            Some(below) if side(*below).is_none() => {
                *below = (below.0.or(alone.0), below.1.or(alone.1));
            }
            _ => cells.insert(line + 1, alone),
        }
    }
    cells
        .into_iter()
        .map(|(old, new)| SplitRow::Pair { old, new })
}

/// A file added or deleted has a single side: it stays unified.
pub fn has_both_sides(kinds: impl Iterator<Item = LineKind> + Clone) -> bool {
    let has = |side: LineKind| {
        kinds
            .clone()
            .any(|kind| kind == side || kind == LineKind::Context)
    };
    has(LineKind::Deletion) && has(LineKind::Addition)
}

/// Whether a view `width` wide shows two columns needing `minimum`. Before
/// its first layout, the width of a view is unknown and does not decide.
pub fn fits(width: f32, minimum: f32, split: bool) -> bool {
    width <= 0. || width >= minimum + if split { 0. } else { HYSTERESIS }
}

#[cfg(test)]
mod tests {
    use super::*;
    use LineKind::{Addition, Annotation, Context, Deletion, Hunk};

    fn layout(rows: &[(LineKind, &str)]) -> SplitLayout {
        let (kinds, texts): (Vec<_>, Vec<_>) = rows.iter().copied().unzip();
        SplitLayout::new(&kinds, &diff::pairs(&kinds, &texts, || false).unwrap())
    }

    fn pair(old: Option<usize>, new: Option<usize>) -> SplitRow {
        SplitRow::Pair { old, new }
    }

    #[test]
    fn pairs_share_a_row_and_unpaired_lines_take_rows_of_their_own() {
        let split = layout(&[
            (Hunk, "@@ -1,4 +1,6 @@"),
            (Context, "fn main() {"),
            (Deletion, "let width = 10;"),
            (Deletion, "let height = 20;"),
            (Addition, "// Size of the frame."),
            (Addition, "let width = 12;"),
            (Addition, "let height = 24;"),
            (Addition, "let depth = 1;"),
            (Context, "}"),
        ]);
        assert_eq!(
            split.rows,
            [
                SplitRow::Hunk(0),
                pair(Some(1), Some(1)),
                pair(None, Some(4)),
                pair(Some(2), Some(5)),
                pair(Some(3), Some(6)),
                pair(None, Some(7)),
                pair(Some(8), Some(8)),
            ]
        );
    }

    #[test]
    fn unpaired_lines_between_two_pairs_are_zipped_on_the_same_rows() {
        let split = layout(&[
            (Deletion, "alpha beta gamma"),
            (Deletion, "value = compute(1)"),
            (Addition, "one two three"),
            (Addition, "four five six"),
            (Addition, "value = compute(2)"),
        ]);
        assert_eq!(
            split.rows,
            [
                pair(Some(0), Some(2)),
                pair(None, Some(3)),
                pair(Some(1), Some(4)),
            ]
        );
    }

    #[test]
    fn annotations_follow_the_line_they_note_on_its_side() {
        let note = "\\ No newline at end of file";
        // Both last lines lost their newline: the two notes share a row.
        let both = layout(&[
            (Deletion, "last = 1"),
            (Annotation, note),
            (Addition, "last = 2"),
            (Annotation, note),
        ]);
        assert_eq!(both.rows, [pair(Some(0), Some(2)), pair(Some(1), Some(3))]);
        // The old side's note takes the empty cell beside a later addition.
        let old = layout(&[
            (Deletion, "last = 1"),
            (Annotation, note),
            (Addition, "last = 2"),
            (Addition, "more = 3"),
        ]);
        assert_eq!(old.rows, [pair(Some(0), Some(2)), pair(Some(1), Some(3))]);
        // The same for the new side, beside a later deletion.
        let new = layout(&[
            (Deletion, "last = 1"),
            (Deletion, "gone = 0"),
            (Addition, "last = 2"),
            (Annotation, note),
        ]);
        assert_eq!(new.rows, [pair(Some(0), Some(2)), pair(Some(1), Some(3))]);
        // A note whose side continues below gets a row of its own.
        let crowded = layout(&[
            (Deletion, "a = 1"),
            (Addition, "a = 2"),
            (Annotation, note),
            (Addition, "b = 3"),
        ]);
        assert_eq!(
            crowded.rows,
            [
                pair(Some(0), Some(1)),
                pair(None, Some(2)),
                pair(None, Some(3))
            ]
        );
        // After context, a note concerns both sides.
        let context = layout(&[(Context, "end"), (Annotation, note)]);
        assert_eq!(
            context.rows,
            [pair(Some(0), Some(0)), pair(Some(1), Some(1))]
        );
    }

    #[test]
    fn every_unified_row_maps_to_the_row_showing_it_and_back() {
        let rows = [
            (Hunk, "@@ -1,3 +1,3 @@"),
            (Context, "a"),
            (Deletion, "value = 1"),
            (Addition, "inserted line"),
            (Addition, "value = 2"),
            (Context, "b"),
            (Hunk, "@@ -9 +9 @@"),
            (Deletion, "removed alone"),
            (Annotation, "\\ No newline at end of file"),
        ];
        let split = layout(&rows);
        for (unified, (kind, _)) in rows.iter().enumerate() {
            let row = split.row_of(unified);
            assert!(split.rows[row].cells().any(|cell| cell == unified));
            let back = split.unified_of(row);
            assert!(back <= unified);
            if *kind != Addition {
                assert_eq!(back, unified, "{kind:?}");
            }
        }
        // Past the end, as a list scrolled to its bottom reports, rows map
        // to the last one: the note under the deletion alone.
        let last = split.rows.len() - 1;
        assert_eq!(split.row_of(rows.len()), last);
        assert_eq!(split.unified_of(last + 1), 8);
        assert_eq!(split.row_of(split.unified_of(last + 1)), last);
    }

    #[test]
    fn added_deleted_or_narrow_diffs_stay_unified_with_hysteresis() {
        assert!(!has_both_sides([Hunk, Addition, Addition].into_iter()));
        assert!(!has_both_sides([Hunk, Deletion, Annotation].into_iter()));
        assert!(has_both_sides([Hunk, Context, Addition].into_iter()));
        assert!(has_both_sides([Hunk, Deletion, Addition].into_iter()));
        // Unknown before the first layout, then the threshold depends on the
        // current layout so that a resize near it does not flicker.
        assert!(fits(0., 700., false));
        assert!(!fits(720., 700., false));
        assert!(fits(732., 700., false));
        assert!(fits(720., 700., true));
        assert!(!fits(699., 700., true));
    }
    #[test]
    #[ignore = "release timing probe; run with --release -- --ignored --nocapture"]
    fn split_layout_timing_probe() {
        use std::time::Instant;
        // About 2,000 rows: blocks of 20 edited pairs like the fixture's
        // search.ts, and blocks of 32 long pairs, the most pairing work.
        let fixture = |pairs: usize, blocks: usize, width: usize| {
            let mut rows = vec![(Hunk, String::new())];
            for block in 0..blocks {
                for n in 0..pairs {
                    let item = block * pairs + n;
                    rows.push((Deletion, format!("  {{ field: \"item{item}\", weight: {item}, label: \"Fixture entry {item}\" }},{}", "x".repeat(width))));
                }
                for n in 0..pairs {
                    let item = block * pairs + n;
                    rows.push((Addition, format!("  {{ field: \"item{item}\", weight: {}, label: \"Edited entry {item}\" }},{}", item * 2, "x".repeat(width))));
                }
                rows.push((Context, format!("  // group {block}")));
            }
            rows
        };
        for (label, rows) in [
            ("blocks of 20 pairs", fixture(20, 48, 0)),
            ("blocks of 32 long pairs", fixture(32, 31, 200)),
        ] {
            let kinds: Vec<_> = rows.iter().map(|(kind, _)| *kind).collect();
            let texts: Vec<_> = rows.iter().map(|(_, text)| text.as_str()).collect();
            let mut pairing = Vec::new();
            let mut layout = Vec::new();
            for _ in 0..41 {
                let started = Instant::now();
                let pairs = diff::pairs(&kinds, &texts, || false).unwrap();
                pairing.push(started.elapsed().as_secs_f64() * 1_000.);
                let started = Instant::now();
                let split = SplitLayout::new(&kinds, &pairs);
                layout.push(started.elapsed().as_secs_f64() * 1_000.);
                assert!(split.rows.len() < kinds.len());
            }
            for samples in [&mut pairing, &mut layout] {
                samples.sort_by(f64::total_cmp);
            }
            println!(
                "split {label}, {} rows: pairing p50={:.2} p95={:.2} ms, layout p50={:.2} p95={:.2} ms",
                kinds.len(),
                pairing[20],
                pairing[38],
                layout[20],
                layout[38]
            );
        }
    }
}
