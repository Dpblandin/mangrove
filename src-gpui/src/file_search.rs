//! Fuzzy search over the files of one revision. Paths are folded once, off
//! the UI thread; a query then only scans the folded text.
use crate::history::{Folded, fold};
use mangrove_core::model::{TreeFile, TreeFiles};
use std::ops::Range;

/// Results kept for one query, best first.
const MAX_RESULTS: usize = 200;

pub struct FileIndex {
    pub sha: String,
    pub truncated: bool,
    files: Vec<IndexedFile>,
}

struct IndexedFile {
    file: TreeFile,
    folded: Folded,
    /// Folded byte where the file name starts.
    name: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMatch {
    pub index: usize,
    /// Source byte ranges of the matched characters, sorted and merged.
    pub ranges: Vec<Range<usize>>,
    score: i64,
}

impl FileIndex {
    pub fn new(listing: TreeFiles) -> Self {
        let files = listing
            .files
            .into_iter()
            .map(|file| {
                let folded = Folded::new(&file.path);
                let name = folded.text.rfind('/').map_or(0, |slash| slash + 1);
                IndexedFile { file, folded, name }
            })
            .collect();
        Self {
            sha: listing.sha,
            truncated: listing.truncated,
            files,
        }
    }

    pub fn file(&self, index: usize) -> &TreeFile {
        &self.files[index].file
    }

    /// Files whose path contains the query's characters in order, ignoring
    /// case, accents and spaces. An empty query lists the first files.
    pub fn search(&self, query: &str) -> Vec<FileMatch> {
        let query: Vec<char> = fold(query).chars().filter(|c| !c.is_whitespace()).collect();
        if query.is_empty() {
            return (0..self.files.len().min(MAX_RESULTS))
                .map(|index| FileMatch {
                    index,
                    ranges: Vec::new(),
                    score: 0,
                })
                .collect();
        }
        let mut matches: Vec<_> = self
            .files
            .iter()
            .enumerate()
            .filter_map(|(index, file)| {
                score(file, &query, None).map(|score| FileMatch {
                    index,
                    ranges: Vec::new(),
                    score,
                })
            })
            .collect();
        let order = |a: &FileMatch, b: &FileMatch| {
            b.score
                .cmp(&a.score)
                .then_with(|| {
                    self.files[a.index]
                        .file
                        .path
                        .len()
                        .cmp(&self.files[b.index].file.path.len())
                })
                .then_with(|| a.index.cmp(&b.index))
        };
        if matches.len() > MAX_RESULTS {
            matches.select_nth_unstable_by(MAX_RESULTS - 1, order);
            matches.truncate(MAX_RESULTS);
        }
        matches.sort_by(order);
        // Only the kept results need their highlighted characters.
        for found in &mut matches {
            let file = &self.files[found.index];
            let mut positions = Vec::with_capacity(query.len());
            score(file, &query, Some(&mut positions));
            found.ranges = ranges(&file.folded, &positions);
        }
        matches
    }
}

/// The query's score in this path: its file name first, then the whole path.
fn score(
    file: &IndexedFile,
    query: &[char],
    positions: Option<&mut Vec<Range<usize>>>,
) -> Option<i64> {
    let text = file.folded.text.as_str();
    let (window, in_name) = match window(text, file.name, query) {
        Some(window) => (window, true),
        None => (window(text, 0, query)?, false),
    };
    let source = file
        .file
        .path
        .is_ascii()
        .then_some(file.file.path.as_bytes());
    let mut total = if in_name { 40 } else { 0 };
    // A match starting farther in costs a little.
    total -= (window.start - if in_name { file.name } else { 0 }).min(12) as i64;
    let mut previous_end: Option<usize> = None;
    let mut matched = Vec::with_capacity(query.len());
    let mut wanted = query.iter().peekable();
    for (offset, character) in text[window.clone()].char_indices() {
        let Some(&&next) = wanted.peek() else {
            break;
        };
        if character != next {
            continue;
        }
        wanted.next();
        let start = window.start + offset;
        let end = start + character.len_utf8();
        total += 16;
        let before = text[..start].chars().next_back();
        if before.is_none_or(|c| matches!(c, '/' | '_' | '-' | '.' | ' ')) {
            total += 10;
        } else if source.is_some_and(|bytes| {
            bytes[start].is_ascii_uppercase() && bytes[start - 1].is_ascii_lowercase()
        }) {
            // Camel case starts a word too: `fb` finds FileBrowser.
            total += 10;
        }
        if start == file.name {
            total += 12;
        }
        match previous_end {
            Some(end) if end == start => total += 14,
            Some(end) => total -= (start - end).min(8) as i64,
            None => {}
        }
        previous_end = Some(end);
        matched.push(start..end);
    }
    if let Some(positions) = positions {
        *positions = matched;
    }
    Some(total)
}

/// The shortest span from `from` ending at the first complete match of
/// `query`, found by matching forward, then backward from that end.
fn window(text: &str, from: usize, query: &[char]) -> Option<Range<usize>> {
    let mut wanted = query.iter();
    let mut next = wanted.next();
    let mut end = None;
    for (offset, character) in text[from..].char_indices() {
        if Some(&character) == next {
            next = wanted.next();
            if next.is_none() {
                end = Some(from + offset + character.len_utf8());
                break;
            }
        }
    }
    let end = end?;
    let mut wanted = query.iter().rev();
    let mut next = wanted.next();
    for (offset, character) in text[from..end].char_indices().rev() {
        if Some(&character) == next {
            next = wanted.next();
            if next.is_none() {
                return Some(from + offset..end);
            }
        }
    }
    None
}

fn ranges(folded: &Folded, positions: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(positions.len());
    for position in positions {
        let range = folded.source(position.clone());
        match merged.last_mut() {
            Some(previous) if range.start <= previous.end => {
                previous.end = previous.end.max(range.end)
            }
            _ => merged.push(range),
        }
    }
    merged
}

/// Ranges of `ranges` inside `span`, relative to its start.
pub fn ranges_within(ranges: &[Range<usize>], span: Range<usize>) -> Vec<Range<usize>> {
    ranges
        .iter()
        .filter_map(|range| {
            let start = range.start.max(span.start);
            let end = range.end.min(span.end);
            (start < end).then(|| start - span.start..end - span.start)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{FileIndex, MAX_RESULTS, ranges_within};
    use mangrove_core::model::{TreeFile, TreeFiles};

    fn index(paths: &[&str]) -> FileIndex {
        FileIndex::new(TreeFiles {
            sha: "head".into(),
            truncated: false,
            files: paths
                .iter()
                .map(|path| TreeFile {
                    path: (*path).into(),
                    file_key: path.bytes().map(|byte| format!("{byte:02x}")).collect(),
                })
                .collect(),
        })
    }

    fn found<'a>(index: &'a FileIndex, query: &str) -> Vec<&'a str> {
        index
            .search(query)
            .into_iter()
            .map(|found| index.file(found.index).path.as_str())
            .collect()
    }

    #[test]
    fn file_names_and_word_starts_rank_before_scattered_letters() {
        let files = index(&[
            "src/foo/bar.rs",
            "docs/fable.md",
            "src-gpui/src/file_browser.rs",
            "src/FileBrowser.ts",
        ]);
        assert_eq!(
            found(&files, "fb"),
            [
                "src/FileBrowser.ts",
                "src-gpui/src/file_browser.rs",
                "docs/fable.md",
                "src/foo/bar.rs"
            ]
        );
        let files = index(&["libs/other/x.rs", "src/lib.rs", "src/calibrate.rs"]);
        assert_eq!(
            found(&files, "lib"),
            ["src/lib.rs", "src/calibrate.rs", "libs/other/x.rs"]
        );
        // The directory narrows the search once the name alone cannot match.
        let files = index(&["a/mod.rs", "b/mod.rs"]);
        assert_eq!(found(&files, "b/mod"), ["b/mod.rs"]);
        assert_eq!(found(&files, "amod"), ["a/mod.rs"]);
    }

    #[test]
    fn queries_ignore_case_accents_and_spaces_and_highlight_source_characters() {
        let files = index(&["src/Élève.ts", "src/file_browser.rs"]);
        let results = files.search("ELEVE");
        assert_eq!(results.len(), 1);
        let path = &files.file(results[0].index).path;
        let start = path.find('É').unwrap();
        assert_eq!(results[0].ranges, vec![start..path.len() - 3]);
        let results = files.search(" file browser ");
        assert_eq!(files.file(results[0].index).path, "src/file_browser.rs");
        assert_eq!(results[0].ranges, [4..8, 9..16]);
        assert!(files.search("zzz").is_empty());
    }

    #[test]
    fn empty_queries_list_the_first_files_and_results_are_bounded() {
        let paths: Vec<String> = (0..MAX_RESULTS + 50)
            .map(|n| format!("dir/file-{n}.txt"))
            .collect();
        let files = index(&paths.iter().map(String::as_str).collect::<Vec<_>>());
        let listed = files.search("");
        assert_eq!(listed.len(), MAX_RESULTS);
        assert_eq!(listed[0].index, 0);
        assert!(listed.iter().all(|found| found.ranges.is_empty()));
        let results = files.search("file");
        assert_eq!(results.len(), MAX_RESULTS);
        // Equal scores keep the shorter path, then the tree order.
        assert_eq!(files.file(results[0].index).path, "dir/file-0.txt");
        assert_eq!(files.file(results[10].index).path, "dir/file-10.txt");
    }

    #[test]
    fn highlights_split_into_the_file_name_and_its_directory() {
        let ranges = [0..3, 5..9];
        assert_eq!(ranges_within(&ranges, 0..4), vec![0..3]);
        assert_eq!(ranges_within(&ranges, 4..12), vec![1..5]);
        assert!(ranges_within(&ranges, 9..12).is_empty());
    }
}
