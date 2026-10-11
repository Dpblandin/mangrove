//! Diff rows and their changed words. Pure and deterministic: the words of
//! a line depend only on the patch, never on timing or on the view.
use std::{
    cmp::Ordering,
    hash::{DefaultHasher, Hash, Hasher},
    mem,
    ops::Range,
    sync::Arc,
    time::{Duration, Instant},
};

use mangrove_core::model::FileDiff;

/// Longer lines keep their line background only.
const MAX_LINE_BYTES: usize = 4 * 1024;
const MAX_LINE_TOKENS: usize = 1_024;
/// A line needing more token edits is compared by its common ends only.
const MAX_EDITS: usize = 64;
/// Larger blocks pair their lines in order rather than by resemblance.
const MAX_PAIRING_CELLS: usize = 1_024;
/// Resemblance compares the first visible tokens of each line.
const PAIRING_TOKENS: usize = 64;
/// Resemblance, in thousandths, below which two lines are not paired.
const PAIRING_THRESHOLD: u32 = 400;
/// Common part of the visible characters, in thousandths, below which a pair
/// was rewritten rather than edited. Characters, not bytes, so that emoji
/// and CJK weigh as much as ASCII.
const MIN_COMMON: usize = 500;
/// Deterministic work allowed to one file: tokens and Myers steps.
const MAX_STEPS: usize = 4_000_000;
/// A slower machine shows no highlight rather than delay the patch.
const TIME_BUDGET: Duration = Duration::from_millis(100);
const MAX_RANGES: usize = 20_000;

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

/// Changed byte ranges of each row, sorted and on character boundaries,
/// indexed like `syntax::Tokens`: one row per hunk header, then its lines.
pub type Words = Arc<Vec<Ranges>>;
type Ranges = Vec<Range<usize>>;
/// Each paired deletion and addition, by row, in patch order.
pub type Pairs = Arc<Vec<(usize, usize)>>;

/// Remove the single line terminator introduced by Git, preserving all other
/// whitespace (including a literal trailing carriage return).
pub fn strip_line_ending(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
}

/// The kind and text of the rows a code view shows for `diff`: a header
/// per hunk, then its lines.
pub fn rows(diff: &FileDiff) -> (Vec<LineKind>, Vec<&str>) {
    let mut kinds = Vec::new();
    let mut texts = Vec::new();
    for hunk in &diff.hunks {
        kinds.push(LineKind::Hunk);
        texts.push("");
        for line in &hunk.lines {
            kinds.push(LineKind::of(&line.kind));
            texts.push(strip_line_ending(&line.content));
        }
    }
    (kinds, texts)
}

/// Each deletion paired with an addition, block after block: the rows that
/// share changed words and face each other side by side. `None` means
/// cancellation.
pub fn pairs(
    kinds: &[LineKind],
    texts: &[&str],
    should_cancel: impl Fn() -> bool,
) -> Option<Vec<(usize, usize)>> {
    let mut pairs = Vec::new();
    for block in blocks(kinds) {
        if should_cancel() {
            return None;
        }
        pairs.extend(
            block
                .pairs(texts)
                .into_iter()
                .map(|(old, new)| (block.deletions[old], block.additions[new])),
        );
    }
    Some(pairs)
}

/// The changed words of each pair, indexed like the rows. `None` means
/// cancellation. Like syntax, an exhausted budget leaves no highlight at
/// all rather than highlight part of the file.
pub fn changed_words(
    texts: &[&str],
    pairs: &[(usize, usize)],
    should_cancel: impl Fn() -> bool,
) -> Option<Words> {
    changed_words_within(
        texts,
        pairs,
        MAX_STEPS,
        Instant::now() + TIME_BUDGET,
        should_cancel,
    )
}

fn changed_words_within(
    texts: &[&str],
    pairs: &[(usize, usize)],
    steps: usize,
    deadline: Instant,
    should_cancel: impl Fn() -> bool,
) -> Option<Words> {
    let mut words = vec![Vec::new(); texts.len()];
    let mut budget = Budget(Some(steps));
    let mut ranges = 0;
    for &(old, new) in pairs {
        if should_cancel() {
            return None;
        }
        if Instant::now() > deadline {
            return Some(Arc::default());
        }
        let Some((removed, added)) = changed_ranges(texts[old], texts[new], &mut budget) else {
            return Some(Arc::default());
        };
        ranges += removed.len() + added.len();
        if ranges > MAX_RANGES {
            return Some(Arc::default());
        }
        words[old] = removed;
        words[new] = added;
    }
    (!should_cancel()).then(|| Arc::new(words))
}

/// The deletions and additions between two context lines or hunk headers,
/// by row. Annotations do not end a block: those among its lines join it.
#[derive(Default)]
pub struct Block {
    pub deletions: Vec<usize>,
    pub additions: Vec<usize>,
    pub annotations: Vec<usize>,
    /// The consecutive rows of the block.
    pub rows: Range<usize>,
}

impl Block {
    fn is_empty(&self) -> bool {
        self.deletions.is_empty() && self.additions.is_empty()
    }

    /// Each paired deletion and addition, by position in the block.
    fn pairs(&self, texts: &[&str]) -> Vec<(usize, usize)> {
        let lines = |rows: &[usize]| rows.iter().map(|&row| texts[row]).collect::<Vec<_>>();
        pair_block(&lines(&self.deletions), &lines(&self.additions))
    }
}

pub fn blocks(kinds: &[LineKind]) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut block = Block::default();
    for (row, kind) in kinds.iter().enumerate() {
        match kind {
            LineKind::Deletion => block.deletions.push(row),
            LineKind::Addition => block.additions.push(row),
            LineKind::Annotation if !block.is_empty() => block.annotations.push(row),
            _ if block.is_empty() => continue,
            _ => {
                blocks.push(mem::take(&mut block));
                continue;
            }
        }
        if block.rows.is_empty() {
            block.rows.start = row;
        }
        block.rows.end = row + 1;
    }
    if !block.is_empty() {
        blocks.push(block);
    }
    blocks
}

/// Pairs each deletion of a block with the addition that resembles it most,
/// in order: a deletion and an addition never cross another pair. Every
/// view of the block uses these pairs, so highlights and layouts agree.
fn pair_block(deletions: &[&str], additions: &[&str]) -> Vec<(usize, usize)> {
    let (rows, columns) = (deletions.len(), additions.len());
    if rows == 0 || columns == 0 {
        return Vec::new();
    }
    if rows * columns > MAX_PAIRING_CELLS {
        return (0..rows.min(columns)).map(|row| (row, row)).collect();
    }
    let old: Vec<_> = deletions
        .iter()
        .map(|line| Fingerprint::new(line))
        .collect();
    let new: Vec<_> = additions
        .iter()
        .map(|line| Fingerprint::new(line))
        .collect();
    let cell = |row: usize, column: usize| row * (columns + 1) + column;
    // best[cell(i, j)]: the highest total resemblance pairing the first i
    // deletions with the first j additions.
    let mut best = vec![0; (rows + 1) * (columns + 1)];
    for row in 1..=rows {
        for column in 1..=columns {
            let weight = old[row - 1].resemblance(&new[column - 1]);
            let skip = best[cell(row - 1, column)].max(best[cell(row, column - 1)]);
            best[cell(row, column)] = if weight >= PAIRING_THRESHOLD {
                skip.max(best[cell(row - 1, column - 1)] + weight)
            } else {
                skip
            };
        }
    }
    // Walking back, ties leave the last lines unpaired: pairs stay early.
    let mut pairs = Vec::new();
    let (mut row, mut column) = (rows, columns);
    while row > 0 && column > 0 {
        let total = best[cell(row, column)];
        if total == best[cell(row, column - 1)] {
            column -= 1;
        } else if total == best[cell(row - 1, column)] {
            row -= 1;
        } else {
            row -= 1;
            column -= 1;
            pairs.push((row, column));
        }
    }
    pairs.reverse();
    pairs
}

/// A line and the sorted hashes of its first visible tokens.
struct Fingerprint<'a> {
    line: &'a str,
    hashes: Vec<u64>,
}

impl<'a> Fingerprint<'a> {
    fn new(line: &'a str) -> Self {
        let mut end = line.len().min(MAX_LINE_BYTES);
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        let mut hashes: Vec<_> = tokens(&line[..end])
            .into_iter()
            .map(|token| &line[token])
            .filter(|token| !is_blank(token))
            .take(PAIRING_TOKENS)
            .map(|token| {
                let mut hasher = DefaultHasher::new();
                token.hash(&mut hasher);
                hasher.finish()
            })
            .collect();
        hashes.sort_unstable();
        Self { line, hashes }
    }

    /// Dice coefficient of the two token multisets, in thousandths. Lines
    /// that differ only by whitespace are alike.
    fn resemblance(&self, other: &Self) -> u32 {
        let total = self.hashes.len() + other.hashes.len();
        if self.line == other.line || total == 0 {
            return 1_000;
        }
        let (mut left, mut right, mut common) = (0, 0, 0);
        while left < self.hashes.len() && right < other.hashes.len() {
            match self.hashes[left].cmp(&other.hashes[right]) {
                Ordering::Less => left += 1,
                Ordering::Greater => right += 1,
                Ordering::Equal => {
                    common += 1;
                    left += 1;
                    right += 1;
                }
            }
        }
        (2_000 * common / total) as u32
    }
}

/// Remaining deterministic work of one file, `None` once exhausted.
struct Budget(Option<usize>);

impl Budget {
    /// Whether `steps` were still available. An exhausted budget stays so.
    fn spend(&mut self, steps: usize) -> bool {
        self.0 = self.0.and_then(|remaining| remaining.checked_sub(steps));
        self.0.is_some()
    }
}

/// Changed ranges of a deletion and its paired addition: none when they are
/// equal, too long or rewritten. `None` means the budget ran out.
fn changed_ranges(old: &str, new: &str, budget: &mut Budget) -> Option<(Ranges, Ranges)> {
    let unchanged = Some((Vec::new(), Vec::new()));
    if old == new || old.len() > MAX_LINE_BYTES || new.len() > MAX_LINE_BYTES {
        return unchanged;
    }
    let (old_tokens, new_tokens) = (tokens(old), tokens(new));
    if !budget.spend(old_tokens.len() + new_tokens.len()) {
        return None;
    }
    if old_tokens.len() > MAX_LINE_TOKENS || new_tokens.len() > MAX_LINE_TOKENS {
        return unchanged;
    }
    let a: Vec<_> = old_tokens.iter().map(|token| &old[token.clone()]).collect();
    let b: Vec<_> = new_tokens.iter().map(|token| &new[token.clone()]).collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a_middle, b_middle) = (prefix..a.len() - suffix, prefix..b.len() - suffix);
    let mut a_kept = vec![true; a.len()];
    let mut b_kept = vec![true; b.len()];
    match myers(&a[a_middle.clone()], &b[b_middle.clone()], budget) {
        Some((a_middle_kept, b_middle_kept)) => {
            a_kept[a_middle].copy_from_slice(&a_middle_kept);
            b_kept[b_middle].copy_from_slice(&b_middle_kept);
        }
        None if budget.0.is_none() => return None,
        // Beyond the edit bound, the whole middle changed.
        None => {
            a_kept[a_middle].fill(false);
            b_kept[b_middle].fill(false);
        }
    }
    // Visible characters of a side, and those of its kept tokens.
    let visible = |tokens: &[&str], kept: &[bool]| {
        tokens
            .iter()
            .zip(kept)
            .filter(|(token, _)| !is_blank(token))
            .fold((0, 0), |(all, common), (token, kept)| {
                let length = token.chars().count();
                (all + length, common + if *kept { length } else { 0 })
            })
    };
    // Kept tokens are equal on both sides: they count once per side.
    let ((old_visible, common), (new_visible, _)) = (visible(&a, &a_kept), visible(&b, &b_kept));
    if 2 * common * 1_000 < MIN_COMMON * (old_visible + new_visible) {
        return unchanged;
    }
    Some((
        spans(&old_tokens, &a, &a_kept),
        spans(&new_tokens, &b, &b_kept),
    ))
}

/// Byte ranges of the changed tokens. Two changes separated by one short
/// unchanged token, such as a space or a dot, form a single range.
fn spans(tokens: &[Range<usize>], texts: &[&str], kept: &[bool]) -> Ranges {
    let short = |text: &str| text.len() <= 2 || is_blank(text);
    let mut spans: Vec<Range<usize>> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if kept[index] {
            continue;
        }
        let joins = index > 0
            && (!kept[index - 1] || (index > 1 && !kept[index - 2] && short(texts[index - 1])));
        match spans.last_mut() {
            Some(span) if joins => span.end = token.end,
            _ => spans.push(token.clone()),
        }
    }
    spans
}

/// Tokens of `a` and `b` kept by a shortest edit script, or `None` beyond
/// `MAX_EDITS` edits or the budget. One step per diagonal and compared token.
fn myers(a: &[&str], b: &[&str], budget: &mut Budget) -> Option<(Vec<bool>, Vec<bool>)> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let limit = (a.len() + b.len()).min(MAX_EDITS) as isize;
    // Furthest x reached on each diagonal k = x - y, at index k + offset.
    let offset = limit + 1;
    let mut furthest = vec![0_isize; 2 * offset as usize + 1];
    let mut trace = Vec::new();
    for edits in 0..=limit {
        trace.push(furthest.clone());
        let mut steps = 0;
        for k in (-edits..=edits).step_by(2) {
            let index = (k + offset) as usize;
            let mut x = if k == -edits || (k != edits && furthest[index - 1] < furthest[index + 1])
            {
                furthest[index + 1]
            } else {
                furthest[index - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
                steps += 1;
            }
            steps += 1;
            furthest[index] = x;
            if x >= n && y >= m {
                return budget
                    .spend(steps)
                    .then(|| kept_tokens(&trace, a.len(), b.len(), offset));
            }
        }
        if !budget.spend(steps) {
            return None;
        }
    }
    None
}

/// Walks the edit script back from the end through each step's diagonals.
fn kept_tokens(trace: &[Vec<isize>], n: usize, m: usize, offset: isize) -> (Vec<bool>, Vec<bool>) {
    let mut a_kept = vec![false; n];
    let mut b_kept = vec![false; m];
    let (mut x, mut y) = (n as isize, m as isize);
    for (edits, furthest) in trace.iter().enumerate().rev() {
        let edits = edits as isize;
        let k = x - y;
        let index = (k + offset) as usize;
        let previous = if k == -edits || (k != edits && furthest[index - 1] < furthest[index + 1]) {
            k + 1
        } else {
            k - 1
        };
        let previous_x = furthest[(previous + offset) as usize];
        let previous_y = previous_x - previous;
        while x > previous_x && y > previous_y {
            x -= 1;
            y -= 1;
            a_kept[x as usize] = true;
            b_kept[y as usize] = true;
        }
        (x, y) = (previous_x, previous_y);
    }
    (a_kept, b_kept)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Space,
    /// Han, kana and bopomofo are written without spaces: like punctuation,
    /// each character is a token of its own.
    Unspaced,
    Other,
}

impl Class {
    fn of(ch: char) -> Self {
        if ch.is_whitespace() {
            Self::Space
        } else if unspaced(ch) {
            Self::Unspaced
        } else if ch.is_alphanumeric() || ch == '_' {
            Self::Word
        } else {
            Self::Other
        }
    }
}

/// Han ideographs, kana, bopomofo and the CJK symbols written among them.
/// Hangul separates its words with spaces and stays word-based.
fn unspaced(ch: char) -> bool {
    matches!(
        ch,
        '\u{2e80}'..='\u{2fff}'
            | '\u{3000}'..='\u{30ff}'
            | '\u{3100}'..='\u{312f}'
            | '\u{3190}'..='\u{31ff}'
            | '\u{3200}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
            | '\u{fe30}'..='\u{fe4f}'
            | '\u{ff61}'..='\u{ff9f}'
            | '\u{1b000}'..='\u{1b16f}'
            | '\u{20000}'..='\u{3134f}'
    )
}

/// Combining marks, joiners, variation selectors, emoji modifiers and tags
/// belong to the character before them.
fn continues(ch: char) -> bool {
    matches!(
        ch,
        '\u{0300}'..='\u{036f}'
            | '\u{1ab0}'..='\u{1aff}'
            | '\u{1dc0}'..='\u{1dff}'
            | '\u{200c}'..='\u{200d}'
            | '\u{20d0}'..='\u{20ff}'
            | '\u{3099}'..='\u{309a}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{fe20}'..='\u{fe2f}'
            | '\u{1f3fb}'..='\u{1f3ff}'
            | '\u{e0020}'..='\u{e007f}'
            | '\u{e0100}'..='\u{e01ef}'
    )
}

/// Words of letters, digits and `_`, runs of whitespace, and one token per
/// other character, including each character of scripts written without
/// spaces. A character after a zero-width joiner joins its token.
fn tokens(line: &str) -> Vec<Range<usize>> {
    let mut tokens: Vec<Range<usize>> = Vec::new();
    let mut class = Class::Other;
    let mut joined = false;
    for (offset, ch) in line.char_indices() {
        let end = offset + ch.len_utf8();
        let next = Class::of(ch);
        match tokens.last_mut() {
            Some(token)
                if joined
                    || continues(ch)
                    || (next == class && matches!(next, Class::Word | Class::Space)) =>
            {
                token.end = end
            }
            _ => {
                tokens.push(offset..end);
                class = next;
            }
        }
        joined = ch == '\u{200d}';
    }
    tokens
}

fn is_blank(text: &str) -> bool {
    text.chars().all(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mangrove_core::model::{DiffHunk, DiffLine};

    fn line(kind: &str, content: &str) -> DiffLine {
        DiffLine {
            kind: kind.into(),
            old_line: None,
            new_line: None,
            content: content.into(),
        }
    }

    fn patch(lines: Vec<DiffLine>) -> FileDiff {
        FileDiff {
            binary: false,
            oversized: None,
            truncated: false,
            additions: 0,
            deletions: 0,
            hunks: vec![DiffHunk {
                header: "@@ -1 +1 @@".into(),
                lines,
            }],
        }
    }

    fn words(diff: &FileDiff) -> Vec<Vec<Range<usize>>> {
        let (kinds, texts) = rows(diff);
        let pairs = pairs(&kinds, &texts, || false).unwrap();
        changed_words(&texts, &pairs, || false).unwrap().to_vec()
    }

    /// The changed text of each row, to read the expectations.
    fn changed<'a>(diff: &'a FileDiff, words: &[Vec<Range<usize>>]) -> Vec<Vec<&'a str>> {
        let lines = diff.hunks.iter().flat_map(|hunk| {
            std::iter::once(hunk.header.as_str()).chain(
                hunk.lines
                    .iter()
                    .map(|line| strip_line_ending(&line.content)),
            )
        });
        lines
            .zip(words)
            .map(|(line, ranges)| ranges.iter().map(|range| &line[range.clone()]).collect())
            .collect()
    }

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

    #[test]
    fn tokens_keep_words_whitespace_and_composed_characters_whole() {
        let text = |line: &str| -> Vec<String> {
            tokens(line)
                .into_iter()
                .map(|token| line[token].to_owned())
                .collect()
        };
        assert_eq!(
            text("let élève_2 =\t\tfoo.bar();"),
            [
                "let",
                " ",
                "élève_2",
                " ",
                "=",
                "\t\t",
                "foo",
                ".",
                "bar",
                "(",
                ")",
                ";"
            ]
        );
        // Decomposed accents, CJK, emoji sequences and keycaps.
        assert_eq!(text("e\u{301}le\u{300}ve"), ["e\u{301}le\u{300}ve"]);
        // Han and kana take a token per character, voiced marks included;
        // Hangul, written with spaces, keeps its words.
        assert_eq!(
            text("漢字 か\u{3099}な：テスト"),
            ["漢", "字", " ", "か\u{3099}", "な", "：", "テ", "ス", "ト"]
        );
        assert_eq!(text("한국어 텍스트"), ["한국어", " ", "텍스트"]);
        assert_eq!(text("a👩‍👩‍👧‍👦b👍🏽c"), ["a", "👩‍👩‍👧‍👦", "b", "👍🏽", "c"]);
        assert_eq!(text("1\u{fe0f}\u{20e3}!"), ["1\u{fe0f}\u{20e3}", "!"]);
        // A mark with nothing before it stays alone.
        assert_eq!(text("\u{301}x"), ["\u{301}", "x"]);
        assert!(tokens("").is_empty());
    }

    #[test]
    fn edited_words_are_highlighted_on_both_sides() {
        let diff = patch(vec![
            line("context", "fn main() {\n"),
            line("deletion", "    let total = count + 1;\n"),
            line("addition", "    let sum = count + 2;\n"),
            line("context", "}\n"),
        ]);
        let words = words(&diff);
        assert_eq!(
            changed(&diff, &words),
            [vec![], vec![], vec!["total", "1"], vec!["sum", "2"], vec![]]
        );
    }

    #[test]
    fn an_edit_inside_a_cjk_run_highlights_the_changed_characters() {
        let diff = patch(vec![
            line("deletion", "漢字のテスト：東京と大阪の天気"),
            line("addition", "漢字のテスト：東京と京都の天気"),
        ]);
        let words = words(&diff);
        assert_eq!(changed(&diff, &words)[1..], [vec!["大阪"], vec!["京都"]]);
    }

    #[test]
    fn nearby_changes_merge_across_one_short_unchanged_token() {
        let diff = patch(vec![
            line("deletion", "call(alpha.beta, gamma)"),
            line("addition", "call(delta.omega, gamma)"),
        ]);
        let words = words(&diff);
        assert_eq!(
            changed(&diff, &words)[1..],
            [vec!["alpha.beta"], vec!["delta.omega"]]
        );
    }

    #[test]
    fn whitespace_only_and_inserted_words_highlight_one_side() {
        let diff = patch(vec![
            line("deletion", "if  ready {"),
            line("deletion", "value(a)"),
            line("addition", "if ready {"),
            line("addition", "value(a, b)"),
        ]);
        let words = words(&diff);
        assert_eq!(
            changed(&diff, &words)[1..],
            [vec!["  "], vec![], vec![" "], vec![", b"]]
        );
    }

    #[test]
    fn emoji_and_accents_weigh_as_characters_in_the_rewritten_share() {
        // Two emoji of a line changed: four bytes each, one character each.
        let diff = patch(vec![
            line("deletion", "Famille 👨‍👩‍👧 et pouce 👍🏽 ici"),
            line("addition", "Famille 👨‍👩‍👧‍👦 et pouce 👍🏿 ici"),
        ]);
        let words = words(&diff);
        assert_eq!(
            changed(&diff, &words)[1..],
            [vec!["👨‍👩‍👧", "👍🏽"], vec!["👨‍👩‍👧‍👦", "👍🏿"]]
        );
    }

    #[test]
    fn rewritten_identical_or_long_lines_keep_the_line_background_only() {
        let long = format!("{} = 1", "x".repeat(MAX_LINE_BYTES));
        let diff = patch(vec![
            line("deletion", "return compute(first, second);\n"),
            line("deletion", "same line\r\n"),
            line("deletion", &long),
            line("addition", "panic!(\"unreachable state\");\n"),
            line("addition", "same line\n"),
            line("addition", &long.replace("= 1", "= 2")),
        ]);
        assert!(words(&diff).iter().all(Vec::is_empty));
    }

    #[test]
    fn lines_pair_by_resemblance_in_order_and_dissimilar_lines_stay_alone() {
        // An inserted line before the edited one does not shift the pairs.
        assert_eq!(
            pair_block(
                &["let width = 10;", "let height = 20;"],
                &[
                    "// Size of the frame.",
                    "let width = 12;",
                    "let height = 24;"
                ],
            ),
            [(0, 1), (1, 2)]
        );
        assert_eq!(
            pair_block(&["alpha beta gamma"], &["one two three"]),
            Vec::<(usize, usize)>::new()
        );
        assert_eq!(pair_block(&["x"], &[]), []);
        assert_eq!(pair_block(&["same"], &["same", "same"]), [(0, 0)]);
        // A block above the cell bound pairs in order.
        let old: Vec<_> = (0..40).map(|n| format!("old {n}")).collect();
        let new: Vec<_> = (0..30).map(|n| format!("unrelated {n}")).collect();
        let (old, new): (Vec<_>, Vec<_>) = (
            old.iter().map(String::as_str).collect(),
            new.iter().map(String::as_str).collect(),
        );
        assert_eq!(
            pair_block(&old, &new),
            (0..30).map(|n| (n, n)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn annotations_and_hunk_headers_frame_blocks_as_in_the_patch() {
        let diff = FileDiff {
            hunks: vec![
                DiffHunk {
                    header: "@@ -1 +1 @@".into(),
                    lines: vec![
                        line("deletion", "last = 1"),
                        line("annotation", "\\ No newline at end of file"),
                        line("addition", "last = 2"),
                    ],
                },
                DiffHunk {
                    header: "@@ -9 +9 @@".into(),
                    lines: vec![line("deletion", "next = 3")],
                },
                DiffHunk {
                    header: "@@ -20 +20 @@".into(),
                    lines: vec![line("addition", "next = 4")],
                },
            ],
            ..patch(Vec::new())
        };
        let words = words(&diff);
        assert_eq!(words.len(), 8);
        assert_eq!(
            changed(&diff, &words),
            [
                vec![],
                vec!["1"],
                vec![],
                vec!["2"],
                vec![],
                vec![],
                vec![],
                vec![]
            ]
        );
    }

    #[test]
    fn ranges_are_sorted_disjoint_and_on_character_boundaries() {
        let pairs = [
            ("café crème brûlée sucre", "café crème glacée sucre"),
            ("漢字テスト 一 二", "漢字テスト 三 二"),
            (
                "emoji 👩‍👩‍👧‍👦 in a longer sentence",
                "emoji 👨‍👩‍👧 in a longer sentence",
            ),
            ("e\u{301}cole publique fin", "e\u{300}cole publique fin"),
            ("a\tb\tc", "a\tbb\tc"),
        ];
        let mut lines = Vec::new();
        for (old, _) in pairs {
            lines.push(line("deletion", old));
        }
        for (_, new) in pairs {
            lines.push(line("addition", new));
        }
        let diff = patch(lines);
        let words = words(&diff);
        let texts: Vec<_> = pairs
            .iter()
            .map(|p| p.0)
            .chain(pairs.iter().map(|p| p.1))
            .collect();
        for (text, ranges) in texts.iter().zip(&words[1..]) {
            assert!(!ranges.is_empty(), "{text}");
            for pair in ranges.windows(2) {
                assert!(pair[0].end < pair[1].start, "{text}: {ranges:?}");
            }
            for range in ranges {
                assert!(range.start < range.end && range.end <= text.len());
                assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
            }
        }
        assert_eq!(changed(&diff, &words)[3], ["👩‍👩‍👧‍👦"]);
    }

    #[test]
    fn exhausted_budgets_leave_the_whole_file_plain_and_deterministic() {
        let lines: Vec<_> = (0..200)
            .map(|n| line("deletion", &format!("value_{n} = compute({n}, left);")))
            .chain((0..200).map(|n| {
                line(
                    "addition",
                    &format!("value_{n} = compute({}, right);", n + 1),
                )
            }))
            .collect();
        let diff = patch(lines);
        let (kinds, texts) = rows(&diff);
        let paired = pairs(&kinds, &texts, || false).unwrap();
        assert_eq!(paired.len(), 200);
        let far = Instant::now() + Duration::from_secs(60);
        let within =
            |steps, deadline| changed_words_within(&texts, &paired, steps, deadline, || false);
        let full = within(MAX_STEPS, far).unwrap();
        assert_eq!(full.len(), 401);
        assert!(full[1..].iter().all(|ranges| !ranges.is_empty()));
        assert_eq!(full, within(MAX_STEPS, far).unwrap());
        // A budget too small for the whole file highlights none of it.
        assert!(within(2_000, far).unwrap().is_empty());
        let past = Instant::now() - Duration::from_secs(1);
        assert!(within(MAX_STEPS, past).unwrap().is_empty());
        assert!(pairs(&kinds, &texts, || true).is_none());
        assert!(changed_words(&texts, &paired, || true).is_none());
    }

    #[test]
    fn lines_beyond_the_edit_bound_compare_their_common_ends() {
        let old: String = (0..40).map(|n| format!("a{n} ")).collect();
        let new: String = (0..40).map(|n| format!("b{n} ")).collect();
        let (old, new) = (
            format!("keep {old}; tail and more tail"),
            format!("keep {new}; tail and more tail"),
        );
        let mut budget = Budget(Some(MAX_STEPS));
        let (removed, added) = changed_ranges(&old, &new, &mut budget).unwrap();
        // Rewritten middle, longer than the common ends: no highlight.
        assert!(removed.is_empty() && added.is_empty());
        // A middle short enough beside the common ends is highlighted whole.
        let join = |letter: char| {
            (0..40)
                .map(|n| format!("{letter}{n}"))
                .collect::<Vec<_>>()
                .join("+")
        };
        let common = "same ".repeat(200);
        let (old, new) = (
            format!("{common}{}", join('x')),
            format!("{common}{}", join('y')),
        );
        let (removed, added) = changed_ranges(&old, &new, &mut budget).unwrap();
        let text = |line: &str, ranges: Ranges| -> Vec<String> {
            ranges
                .into_iter()
                .map(|range| line[range].to_owned())
                .collect()
        };
        assert_eq!(text(&old, removed), [join('x')]);
        assert_eq!(text(&new, added), [join('y')]);
    }

    #[test]
    #[ignore = "release timing probe; run with --release -- --ignored --nocapture"]
    fn changed_words_timing_probe() {
        // Pairs of a fixture line whose number, word and flag changed.
        let old = |n: usize| {
            line(
                "deletion",
                &format!(
                    "    export const entry{n} = {{ id: {n}, label: \"Fixture item {n}\", enabled: false }};\n"
                ),
            )
        };
        let new = |n: usize| {
            line(
                "addition",
                &format!(
                    "    export const entry{n} = {{ id: {}, label: \"Fixture entry {n}\", enabled: true }};\n",
                    n * 2
                ),
            )
        };
        for count in [1_000, 2_000] {
            let block = patch((0..count).map(old).chain((0..count).map(new)).collect());
            let blocks = patch(
                (0..count / 10)
                    .flat_map(|block| {
                        let rows = block * 10..block * 10 + 10;
                        rows.clone()
                            .map(old)
                            .chain(rows.map(new))
                            .chain(std::iter::once(line("context", "\n")))
                    })
                    .collect(),
            );
            for (label, diff) in [("one block", block), ("blocks of 10", blocks)] {
                let mut samples: Vec<_> = (0..41)
                    .map(|_| {
                        let started = Instant::now();
                        let (kinds, texts) = rows(&diff);
                        let paired = pairs(&kinds, &texts, || false).unwrap();
                        let words = changed_words(&texts, &paired, || false).unwrap();
                        assert!(words.iter().any(|ranges| !ranges.is_empty()));
                        started.elapsed().as_secs_f64() * 1_000.
                    })
                    .collect();
                samples.sort_by(f64::total_cmp);
                println!(
                    "changed_words pairs={count} {label}: p50={:.2} ms p95={:.2} ms max={:.2} ms",
                    samples[20], samples[38], samples[40]
                );
            }
        }
    }
}
