//! The shown part of a file's patch, read only within its preview limit.
//! Pairs of different blobs are streamed: a materialized `git2::Patch` keeps
//! a record per changed line, and on large rewrites those growing records
//! fragment memory that is not returned.
use std::cell::{Cell, RefCell};

use git2::{Delta, Diff, DiffDelta, DiffFile, DiffOptions, FileMode, Odb, Patch, Repository};

use crate::{
    file_browser::object_error,
    model::{DiffHunk, DiffLine, DiffSources, FileDiff, PreviewLimit},
    Result, MAX_PATCH_SIZE,
};

const MAX_LINES: usize = 2_000;
const MAX_BYTES: usize = 512 * 1024;

/// The patch of the delta at `index`, unless a side is above `limit`. Object
/// headers give the sizes before libgit2 loads any contents: tree deltas often
/// have no size yet, and a text diff attribute lifts libgit2's `max_size`.
pub(crate) fn render_bounded(
    repository: &Repository,
    diff: &Diff<'_>,
    index: usize,
    limit: PreviewLimit,
) -> Result<FileDiff> {
    let delta = diff.get_delta(index).ok_or("Fichier introuvable.")?;
    let odb = repository.odb().map_err(object_error)?;
    if let Some(oversized) = limit.exceeded_by(largest_blob(&odb, &delta)?) {
        return Ok(FileDiff::oversized(oversized));
    }
    if delta.status() != Delta::Typechange
        && delta.old_file().id() != delta.new_file().id()
        && is_blob_or_absent(&delta.old_file())
        && is_blob_or_absent(&delta.new_file())
    {
        return stream_blobs(repository, &delta);
    }
    // Submodules and type changes keep libgit2's own presentation, as do
    // identical contents: they have no line to keep, but may be binary.
    let Some(patch) = Patch::from_diff(diff, index).map_err(|e| e.to_string())? else {
        return Ok(FileDiff::binary());
    };
    render_patch(patch)
}

/// The size of the delta's larger blob, read from the object headers.
pub(crate) fn largest_blob(odb: &Odb<'_>, delta: &DiffDelta<'_>) -> Result<u64> {
    let mut largest = 0;
    for file in [delta.old_file(), delta.new_file()] {
        if file.id().is_zero() || !is_blob(file.mode()) {
            continue;
        }
        let (size, _) = odb.read_header(file.id()).map_err(object_error)?;
        largest = largest.max(size as u64);
    }
    Ok(largest)
}

/// Old trees can still carry the obsolete group-writable mode.
fn is_blob(mode: FileMode) -> bool {
    matches!(
        mode,
        FileMode::Blob | FileMode::BlobGroupWritable | FileMode::BlobExecutable | FileMode::Link
    )
}

fn is_blob_or_absent(file: &DiffFile<'_>) -> bool {
    file.id().is_zero() || is_blob(file.mode())
}

/// Hunks and lines in patch order, kept until the display budget is spent,
/// with the line totals of the whole patch.
#[derive(Default)]
struct Shown {
    diff: FileDiff,
    lines: usize,
    bytes: usize,
}

impl Shown {
    fn hunk(&mut self, header: &[u8]) {
        if !self.diff.truncated {
            self.diff.hunks.push(DiffHunk {
                header: String::from_utf8_lossy(header).trim_end().into(),
                lines: Vec::new(),
            });
        }
    }

    /// Counts the line, and returns whether it was kept.
    fn line(&mut self, line: &git2::DiffLine<'_>) -> bool {
        match line.origin() {
            '+' => self.diff.additions += 1,
            '-' => self.diff.deletions += 1,
            _ => {}
        }
        if self.diff.truncated {
            return false;
        }
        let Some(hunk) = self.diff.hunks.last_mut() else {
            return false;
        };
        if self.lines >= MAX_LINES || self.bytes + line.content().len() > MAX_BYTES {
            self.diff.truncated = true;
            return false;
        }
        self.lines += 1;
        self.bytes += line.content().len();
        let annotation = matches!(line.origin(), '=' | '>' | '<');
        let content = String::from_utf8_lossy(line.content());
        hunk.lines.push(DiffLine {
            kind: match line.origin() {
                '+' => "addition",
                '-' => "deletion",
                '=' | '>' | '<' => "annotation",
                _ => "context",
            }
            .into(),
            old_line: (!annotation).then(|| line.old_lineno()).flatten(),
            new_line: (!annotation).then(|| line.new_lineno()).flatten(),
            content: if annotation {
                content.trim_matches(['\r', '\n'])
            } else {
                content.trim_end_matches(['\r', '\n'])
            }
            .into(),
        });
        true
    }
}

fn stream_blobs(repository: &Repository, delta: &DiffDelta<'_>) -> Result<FileDiff> {
    let blob = |file: DiffFile<'_>| {
        (!file.id().is_zero())
            .then(|| repository.find_blob(file.id()))
            .transpose()
            .map_err(object_error)
    };
    let (old, new) = (blob(delta.old_file())?, blob(delta.new_file())?);
    // Paths select diff drivers and attributes; git2 only accepts UTF-8 here.
    let path = |file: DiffFile<'_>| {
        file.path_bytes()
            .map(|path| String::from_utf8_lossy(path).into_owned())
    };
    let (old_path, new_path) = (path(delta.old_file()), path(delta.new_file()));
    let mut options = DiffOptions::new();
    options.max_size(MAX_PATCH_SIZE);
    let shown = RefCell::new(Shown::default());
    // libgit2 reports a binary pair here instead of hunks.
    let binary = Cell::new(false);
    repository
        .diff_blobs(
            old.as_ref(),
            old_path.as_deref(),
            new.as_ref(),
            new_path.as_deref(),
            Some(&mut options),
            None,
            Some(&mut |_, _| {
                binary.set(true);
                true
            }),
            Some(&mut |_, hunk| {
                shown.borrow_mut().hunk(hunk.header());
                true
            }),
            Some(&mut |_, _, line| {
                shown.borrow_mut().line(&line);
                true
            }),
        )
        .map_err(|e| e.to_string())?;
    if binary.get() {
        return Ok(FileDiff::binary());
    }
    Ok(FileDiff {
        sources: DiffSources::of(
            old.as_ref().map_or(&[], |blob| blob.content()),
            new.as_ref().map_or(&[], |blob| blob.content()),
        ),
        ..shown.into_inner().diff
    })
}

/// The shown part of a materialized patch, whose contents were read within
/// the caller's preview limit.
pub(crate) fn render_patch(patch: Patch<'_>) -> Result<FileDiff> {
    if patch.delta().flags().contains(git2::DiffFlags::BINARY) {
        return Ok(FileDiff::binary());
    }
    let mut shown = Shown::default();
    'hunks: for index in 0..patch.num_hunks() {
        let (hunk, lines) = patch.hunk(index).map_err(|e| e.to_string())?;
        shown.hunk(hunk.header());
        for line_index in 0..lines {
            let line = patch
                .line_in_hunk(index, line_index)
                .map_err(|e| e.to_string())?;
            if !shown.line(&line) {
                break 'hunks;
            }
        }
    }
    // The loop stops at the display budget; take the totals of every line.
    let (_, additions, deletions) = patch.line_stats().map_err(|e| e.to_string())?;
    Ok(FileDiff {
        additions,
        deletions,
        ..shown.diff
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::commit;

    #[test]
    fn streamed_blobs_match_materialized_patches() {
        let directory = tempfile::tempdir().unwrap();
        let repository = Repository::init_bare(directory.path()).unwrap();
        let numbered = |count: usize, edit: usize| {
            (0..count)
                .map(|n| {
                    if n % edit == 0 {
                        format!("edited {n}\n")
                    } else {
                        format!("line {n}\n")
                    }
                })
                .collect::<String>()
        };
        let (many_old, many_new) = (numbered(6_000, usize::MAX), numbered(6_000, 7));
        let long_old = vec![b'a'; 600 * 1024];
        let long_new = vec![b'b'; 600 * 1024];
        let old: &[(&[u8], &[u8], i32)] = &[
            (
                b"hunks.txt",
                b"a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\n",
                0o100644,
            ),
            (b"crlf.txt", b"one\r\ntwo\r\nthree\r\n", 0o100644),
            (b"newline.txt", b"no final newline", 0o100644),
            (b"deleted.txt", b"gone\n", 0o100644),
            (
                b"renamed.txt",
                b"keep\nthese\nlines\nfor\nsimilarity\nold\n",
                0o100644,
            ),
            (b"binary.bin", b"\0\x01old", 0o100644),
            (b"moved.bin", b"\0\x01same", 0o100644),
            (b"chmod.bin", b"\0\x01mode", 0o100644),
            (b"empty.txt", b"", 0o100644),
            (b"mode.sh", b"echo\n", 0o100644),
            (b"link", b"old-target", 0o120000),
            (b"raw-\xff.txt", b"before\n", 0o100644),
            (b"many.txt", many_old.as_bytes(), 0o100644),
            (b"long.txt", &long_old, 0o100644),
        ];
        let new: &[(&[u8], &[u8], i32)] = &[
            (
                b"hunks.txt",
                b"A\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nM\n",
                0o100644,
            ),
            (b"crlf.txt", b"one\r\n2\r\nthree\r\n", 0o100644),
            (b"newline.txt", b"no final newline, edited", 0o100644),
            (b"added.txt", b"new\nfile\n", 0o100644),
            (
                b"moved.txt",
                b"keep\nthese\nlines\nfor\nsimilarity\nnew\n",
                0o100644,
            ),
            (b"binary.bin", b"\0\x01new", 0o100644),
            (b"moved-elsewhere.bin", b"\0\x01same", 0o100644),
            (b"chmod.bin", b"\0\x01mode", 0o100755),
            (b"empty.txt", b"filled\n", 0o100644),
            (b"mode.sh", b"echo\n", 0o100755),
            (b"link", b"new-target", 0o120000),
            (b"raw-\xff.txt", b"after\n", 0o100644),
            (b"many.txt", many_new.as_bytes(), 0o100644),
            (b"long.txt", &long_new, 0o100644),
        ];
        let parent = commit(&repository, &[], old);
        let id = commit(&repository, &[parent], new);
        let (_, diff, _) = crate::commit_diff(&repository, &id.to_string()).unwrap();
        let mut statuses = Vec::new();
        for index in 0..diff.deltas().len() {
            let delta = diff.get_delta(index).unwrap();
            statuses.push(delta.status());
            let bounded = render_bounded(&repository, &diff, index, PreviewLimit::MIN).unwrap();
            let materialized = match Patch::from_diff(&diff, index).unwrap() {
                Some(patch) => render_patch(patch).unwrap(),
                None => FileDiff::binary(),
            };
            let path = delta.new_file().path_bytes().unwrap();
            assert_eq!(
                serde_json::to_value(&bounded).unwrap(),
                serde_json::to_value(&materialized).unwrap(),
                "{path:?}"
            );
            // Renaming or changing the mode of a binary keeps it binary.
            assert_eq!(bounded.binary, path.ends_with(b".bin"), "{path:?}");
        }
        for status in [
            Delta::Added,
            Delta::Deleted,
            Delta::Renamed,
            Delta::Modified,
        ] {
            assert!(statuses.contains(&status), "{status:?} in {statuses:?}");
        }
    }

    /// Syntax analysis reads each shown line at its number in its side.
    fn assert_lines_match_sources(diff: &FileDiff) {
        let sources = diff.sources.as_ref().unwrap();
        for line in diff.hunks.iter().flat_map(|hunk| &hunk.lines) {
            let (text, number) = match line.kind.as_str() {
                "deletion" => (&sources.old, line.old_line),
                "annotation" => continue,
                _ => (&sources.new, line.new_line),
            };
            let shown = text.lines().nth(number.unwrap() as usize - 1).unwrap();
            assert_eq!(shown.trim_end_matches('\r'), line.content);
        }
    }

    #[test]
    fn text_patches_keep_their_sources_within_the_source_limit() {
        let directory = tempfile::tempdir().unwrap();
        let repository = Repository::init_bare(directory.path()).unwrap();
        let large = vec![b'a'; crate::model::MAX_SOURCE_BYTES + 1];
        let old: &[(&[u8], &[u8], i32)] = &[
            (
                b"edited.php",
                b"<?php\n\nnamespace App;\n\nfinal class A {}\n",
                0o100644,
            ),
            (b"crlf.txt", b"one\r\ntwo\r\n", 0o100644),
            (b"deleted.txt", b"gone\n", 0o100644),
            (b"newline.txt", b"no final newline", 0o100644),
            (b"large.txt", &large, 0o100644),
            (b"binary.bin", b"\0\x01old", 0o100644),
        ];
        let new: &[(&[u8], &[u8], i32)] = &[
            (
                b"edited.php",
                b"<?php\n\nnamespace App;\n\nfinal class B {}\n",
                0o100644,
            ),
            (b"crlf.txt", b"one\r\n2\r\n", 0o100644),
            (b"added.txt", b"new\n", 0o100644),
            (b"newline.txt", b"no final newline, edited", 0o100644),
            (b"large.txt", b"small\n", 0o100644),
            (b"binary.bin", b"\0\x01new", 0o100644),
        ];
        let parent = commit(&repository, &[], old);
        let id = commit(&repository, &[parent], new);
        let (_, diff, _) = crate::commit_diff(&repository, &id.to_string()).unwrap();
        for index in 0..diff.deltas().len() {
            let delta = diff.get_delta(index).unwrap();
            let path = delta.new_file().path_bytes().unwrap();
            let shown = render_bounded(&repository, &diff, index, PreviewLimit::MIN).unwrap();
            match path {
                b"large.txt" | b"binary.bin" => assert!(shown.sources.is_none(), "{path:?}"),
                _ => assert_lines_match_sources(&shown),
            }
            if path == b"added.txt" {
                assert_eq!(shown.sources.as_ref().unwrap().old, "");
            }
        }
    }

    #[test]
    fn group_writable_blobs_are_measured_before_being_read() {
        let directory = tempfile::tempdir().unwrap();
        let repository = Repository::init_bare(directory.path()).unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        // Tree builders refuse this obsolete mode, but old trees still carry it.
        let commit = |contents: &[u8], parents: &[&git2::Commit<'_>]| {
            let blob = repository.blob(contents).unwrap();
            let mut entry = b"100664 legacy\0".to_vec();
            entry.extend_from_slice(blob.as_bytes());
            let tree = repository
                .odb()
                .unwrap()
                .write(git2::ObjectType::Tree, &entry)
                .unwrap();
            let tree = repository.find_tree(tree).unwrap();
            let id = repository
                .commit(None, &signature, &signature, "legacy", &tree, parents)
                .unwrap();
            repository.find_commit(id).unwrap()
        };
        let large = vec![b'x'; PreviewLimit::MIN.bytes() as usize + 1];
        let parent = commit(b"small\n", &[]);
        let id = commit(&large, &[&parent]).id();
        let (_, diff, _) = crate::commit_diff(&repository, &id.to_string()).unwrap();
        assert_eq!(
            diff.get_delta(0).unwrap().new_file().mode(),
            FileMode::BlobGroupWritable
        );
        let skipped = render_bounded(&repository, &diff, 0, PreviewLimit::MIN).unwrap();
        assert_eq!(
            skipped.oversized.map(|oversized| oversized.size),
            Some(large.len() as u64)
        );
        let read = render_bounded(&repository, &diff, 0, PreviewLimit::from_mib(4)).unwrap();
        assert_eq!((read.additions, read.deletions), (1, 1));
    }
}
