//! Snapshot syntax analysis. Parsing happens off the UI thread, and its output
//! contains semantic kinds only: changing a theme never reparses a document.
use std::{
    collections::{HashMap, VecDeque},
    hash::{DefaultHasher, Hash, Hasher},
    ops::Range,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use gpui_kit::component::Rope;
use gpui_kit::{
    HighlightStyle,
    base::input::HighlightStyleResolver,
    component::highlighter::{LanguageRegistry, SyntaxHighlighter},
    rgb,
};
use mangrove_core::model::FileDiff;
use tree_sitter::{InputEdit, Point};

use crate::diff::strip_line_ending;

pub const MAX_LINES: usize = 2_000;
pub const MAX_BYTES: usize = 256 * 1_024;
pub const MAX_LINE_BYTES: usize = 20 * 1_024;
pub const MAX_TOKENS: usize = 20_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Plain,
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Function,
    Punctuation,
    Property,
}

impl Kind {
    pub fn palette_index(self) -> Option<usize> {
        match self {
            Self::Plain => None,
            Self::Keyword => Some(0),
            Self::String => Some(1),
            Self::Comment => Some(2),
            Self::Number => Some(3),
            Self::Type => Some(4),
            Self::Function => Some(5),
            Self::Punctuation => Some(6),
            Self::Property => Some(7),
        }
    }
    fn from_capture(name: &str) -> Self {
        match name.split('.').next().unwrap_or(name) {
            "keyword" | "boolean" | "constant" | "attribute" | "tag" | "preproc" => Self::Keyword,
            "string" | "text" | "link_uri" => Self::String,
            "comment" => Self::Comment,
            "number" => Self::Number,
            "type" | "constructor" | "enum" | "title" => Self::Type,
            "function" => Self::Function,
            "punctuation" | "operator" => Self::Punctuation,
            "property" => Self::Property,
            _ => Self::Plain,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub range: Range<usize>,
    pub kind: Kind,
}

pub type Tokens = Arc<Vec<Vec<Token>>>;
type CacheKey = (String, u64);
#[derive(Default)]
struct Cache {
    entries: HashMap<CacheKey, Tokens>,
    order: VecDeque<CacheKey>,
}

fn cached(
    path: &str,
    fingerprint: u64,
    should_cancel: &dyn Fn() -> bool,
    analyze: impl FnOnce() -> Option<Vec<Vec<Token>>>,
) -> Option<Tokens> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    if should_cancel() {
        return None;
    }
    let key = (path.to_owned(), fingerprint);
    {
        let mut cache = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(tokens) = cache.entries.get(&key).cloned() {
            cache.order.retain(|previous| previous != &key);
            cache.order.push_back(key);
            return (!should_cancel()).then_some(tokens);
        }
    }
    // Compute outside the cache lock. An in-flight cancelled snapshot must
    // neither block a newer selection nor publish an empty cache entry.
    let Some(tokens) = analyze() else {
        // A timeout must remain retryable and cannot overwrite a successful
        // snapshot prepared concurrently for the same content.
        return (!should_cancel()).then(|| Arc::new(Vec::new()));
    };
    let tokens = Arc::new(tokens);
    if should_cancel() {
        return None;
    }
    {
        let mut cache = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        cache.entries.insert(key.clone(), tokens.clone());
        cache.order.retain(|previous| previous != &key);
        cache.order.push_back(key);
        // Bounded semantic snapshots; viewers retain their own Arc after
        // eviction, so trimming the cache never invalidates displayed tokens.
        while cache.order.len() > 8 {
            if let Some(oldest) = cache.order.pop_front() {
                cache.entries.remove(&oldest);
            }
        }
    }
    Some(tokens)
}

/// Time allowed to one highlighting pass before it falls back to plain text.
const HIGHLIGHT_BUDGET: Duration = Duration::from_millis(500);

/// `None` means cancellation; unsupported or bounded-out syntax remains a
/// complete plain-text snapshot. Cancelled work never enters the cache.
pub fn cached_diff_tokens_cancellable(
    path: &str,
    diff: &FileDiff,
    should_cancel: impl Fn() -> bool,
) -> Option<Tokens> {
    cached_diff_tokens_within(path, diff, HIGHLIGHT_BUDGET, should_cancel)
}

fn cached_diff_tokens_within(
    path: &str,
    diff: &FileDiff,
    budget: Duration,
    should_cancel: impl Fn() -> bool,
) -> Option<Tokens> {
    if should_cancel() {
        return None;
    }
    let mut fingerprint = DefaultHasher::new();
    "diff".hash(&mut fingerprint);
    for hunk in &diff.hunks {
        if should_cancel() {
            return None;
        }
        hunk.header.hash(&mut fingerprint);
        for line in &hunk.lines {
            line.kind.hash(&mut fingerprint);
            line.content.hash(&mut fingerprint);
        }
    }
    cached(path, fingerprint.finish(), &should_cancel, || {
        diff_tokens_with_cancellation(path, diff, Instant::now() + budget, &should_cancel)
    })
}

pub fn cached_content_tokens_cancellable(
    path: &str,
    lines: &[String],
    should_cancel: impl Fn() -> bool,
) -> Option<Tokens> {
    if should_cancel() {
        return None;
    }
    let mut fingerprint = DefaultHasher::new();
    "content".hash(&mut fingerprint);
    lines.hash(&mut fingerprint);
    cached(path, fingerprint.finish(), &should_cancel, || {
        content_tokens_with_cancellation(
            path,
            lines,
            Instant::now() + HIGHLIGHT_BUDGET,
            &should_cancel,
        )
    })
}

pub fn language(path: &str) -> Option<&'static str> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or("").to_lowercase();
    match name.as_str() {
        "cargo.lock" => return Some("toml"),
        "composer.lock" => return Some("json"),
        "cmakelists.txt" => return Some("cmake"),
        "makefile" | "gnumakefile" => return Some("make"),
        "gemfile" | "rakefile" => return Some("ruby"),
        ".bashrc" | ".bash_profile" | ".bash_login" | ".bash_logout" | ".zshrc" | ".zprofile"
        | ".zshenv" | ".zlogin" | ".zlogout" | ".profile" => return Some("shellscript"),
        _ => (),
    }
    if name == ".env" || name.starts_with(".env.") {
        return Some("dotenv");
    }
    if name == "dockerfile" || name.starts_with("dockerfile.") || name.ends_with(".dockerfile") {
        return Some("docker");
    }
    Some(match name.rsplit_once('.')?.1 {
        "ts" | "mts" | "cts" => "typescript",
        "js" | "mjs" | "cjs" => "javascript",
        "tsx" => "tsx",
        "jsx" => "jsx",
        "vue" => "vue",
        "svelte" => "svelte",
        "astro" => "astro",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" => "scss",
        "less" => "less",
        "json" => "json",
        "jsonc" => "jsonc",
        "json5" => "json5",
        "yaml" | "yml" => "yaml",
        "php" | "phtml" => "php",
        "rs" => "rust",
        "py" | "pyi" => "python",
        "sh" | "bash" | "zsh" => "shellscript",
        "sql" => "sql",
        "md" | "markdown" => "markdown",
        "toml" => "toml",
        "xml" | "svg" => "xml",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "cs" => "csharp",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "go" => "go",
        "rb" => "ruby",
        "swift" => "swift",
        "graphql" | "gql" => "graphql",
        "ini" | "conf" => "ini",
        "properties" => "properties",
        "cmake" => "cmake",
        "mk" => "make",
        "patch" | "diff" => "diff",
        _ => return None,
    })
}

struct Markers;
impl HighlightStyleResolver for Markers {
    fn style(&self, name: &str) -> Option<HighlightStyle> {
        Some(HighlightStyle {
            color: Kind::from_capture(name)
                .palette_index()
                .map(|index| rgb(index as u32 + 1).into()),
            ..Default::default()
        })
    }
}

fn append(tokens: &mut Vec<Token>, range: Range<usize>, kind: Kind) {
    if range.is_empty() {
        return;
    }
    if let Some(last) = tokens.last_mut()
        && last.kind == kind
        && last.range.end == range.start
    {
        last.range.end = range.end;
    } else {
        tokens.push(Token { range, kind });
    }
}

fn grammar(language: &str, source: &str, header: &str) -> (&'static str, String) {
    match language {
        "typescript" => ("typescript", String::new()),
        "javascript" => ("javascript", String::new()),
        "tsx" | "jsx" => ("tsx", String::new()),
        "shellscript" => ("bash", String::new()),
        "scss" | "less" => ("css", String::new()),
        "jsonc" | "json5" => ("json", String::new()),
        "xml" => ("html", String::new()),
        "vue" | "svelte" | "astro" => {
            let first = source.trim_start();
            // A hunk often starts within a script/style block. Inspect its
            // context instead of parsing code as plain HTML text.
            let script = [first, header.split("@@").last().unwrap_or("").trim()]
                .iter()
                .any(|line| {
                    [
                        "import ",
                        "export ",
                        "const ",
                        "let ",
                        "var ",
                        "function ",
                        "interface ",
                        "type ",
                        "class ",
                        "return ",
                        "async ",
                        "if (",
                        "for (",
                        "throw ",
                    ]
                    .iter()
                    .any(|word| line.starts_with(word))
                });
            if script && !first.starts_with('<') {
                return ("typescript", String::new());
            }
            if !first.starts_with('<')
                && (first.starts_with('.') || first.starts_with('#') || first.starts_with('&'))
            {
                return ("css", String::new());
            }
            if language == "svelte" {
                ("svelte", String::new())
            } else if language == "astro" {
                ("astro", String::new())
            } else {
                ("html", String::new())
            }
        }
        _ => (
            match language {
                "html" => "html",
                "css" => "css",
                "json" => "json",
                "yaml" => "yaml",
                "php" => "php",
                "rust" => "rust",
                "python" => "python",
                "sql" => "sql",
                "markdown" => "markdown",
                "toml" => "toml",
                "c" => "c",
                "cpp" => "cpp",
                "csharp" => "csharp",
                "java" => "java",
                "kotlin" => "kotlin",
                "go" => "go",
                "ruby" => "ruby",
                "swift" => "swift",
                "graphql" => "graphql",
                "cmake" => "cmake",
                "make" => "make",
                "diff" => "diff",
                _ => "text",
            },
            String::new(),
        ),
    }
}

struct AnalysisContext<'a> {
    highlighters: HashMap<&'static str, SyntaxHighlighter>,
    deadline: Instant,
    should_cancel: &'a dyn Fn() -> bool,
}

fn end_point(bytes: impl Iterator<Item = u8>) -> Point {
    bytes.fold(Point::new(0, 0), |point, byte| {
        if byte == b'\n' {
            Point::new(point.row + 1, 0)
        } else {
            Point::new(point.row, point.column + 1)
        }
    })
}

impl<'a> AnalysisContext<'a> {
    fn new(deadline: Instant, should_cancel: &'a dyn Fn() -> bool) -> Self {
        Self {
            highlighters: HashMap::new(),
            deadline,
            should_cancel,
        }
    }

    fn stopped(&self) -> bool {
        Instant::now() >= self.deadline || (self.should_cancel)()
    }

    fn tokenize_side(
        &mut self,
        language: &str,
        lines: &[&str],
        header: &str,
    ) -> Option<Vec<Vec<Token>>> {
        if self.stopped() {
            return None;
        }
        if lines.is_empty() {
            return Some(Vec::new());
        }
        let code = lines.join("\n");
        let (grammar, prefix) = grammar(language, &code, header);
        let source = prefix.clone() + &code;
        // These dialects always use the lexical tokenizer. Select it before
        // query compilation and parsing, whose results would be discarded.
        let lexical_only = matches!(language, "jsonc" | "json5" | "scss" | "less");
        let mut ranges = Vec::new();
        let mut available = false;
        if !lexical_only {
            configure_queries();
            available = LanguageRegistry::singleton()
                .language(grammar)
                .is_some_and(|language| language.has_grammar() && !language.highlights.is_empty());
            if self.stopped() {
                return None;
            }
            if available {
                // One compiled highlighter per grammar for the entire snapshot,
                // including both sides and all hunks. Trees describe fragments,
                // so every changed fragment must replace the whole old source.
                let highlighter = self
                    .highlighters
                    .entry(grammar)
                    .or_insert_with(|| SyntaxHighlighter::new(grammar));
                // Incompatible Kit queries construct an inert text highlighter.
                available = highlighter.language().as_ref() == grammar;
                if available {
                    let text = Rope::from(source.as_str());
                    let remaining = self
                        .deadline
                        .checked_duration_since(Instant::now())
                        .filter(|remaining| !remaining.is_zero())?;
                    if (self.should_cancel)() {
                        return None;
                    }
                    if highlighter.text() != &text {
                        let previous = highlighter.text().clone();
                        let edit = InputEdit {
                            start_byte: 0,
                            old_end_byte: previous.len(),
                            new_end_byte: source.len(),
                            start_position: Point::new(0, 0),
                            old_end_position: end_point(previous.bytes()),
                            new_end_position: end_point(source.bytes()),
                        };
                        // Kit's update(None) models an insertion at byte zero,
                        // not a full replacement. Clear injection trees too: an
                        // old script/style/fenced block cannot span this fragment.
                        highlighter.edit_tree(None, &previous);
                        if !highlighter.update(Some(edit), &text, Some(remaining)) {
                            return None;
                        }
                    }
                    if Instant::now() >= self.deadline || (self.should_cancel)() {
                        return None;
                    }
                    for (range, style) in highlighter.styles(&(0..source.len()), &Markers) {
                        let kind = [
                            Kind::Keyword,
                            Kind::String,
                            Kind::Comment,
                            Kind::Number,
                            Kind::Type,
                            Kind::Function,
                            Kind::Punctuation,
                            Kind::Property,
                        ]
                        .into_iter()
                        .find(|kind| {
                            style.color
                                == kind
                                    .palette_index()
                                    .map(|index| rgb(index as u32 + 1).into())
                        })
                        .unwrap_or_default();
                        ranges.push(Token { range, kind });
                    }
                }
            }
        }
        if self.stopped() {
            return None;
        }
        // Preserve the stateful fallback for unavailable grammars and queries.
        if !available {
            ranges = lexical(&source, language);
        }
        if self.stopped() {
            return None;
        }
        let mut offset = prefix.len();
        let mut token_index = 0;
        let result = lines
            .iter()
            .map(|line| {
                let end = offset + line.len();
                let mut cursor = offset;
                let mut tokens = Vec::new();
                while token_index < ranges.len() {
                    let token = &ranges[token_index];
                    if token.range.end <= cursor {
                        token_index += 1;
                        continue;
                    }
                    if token.range.start >= end {
                        break;
                    }
                    let start = cursor.max(token.range.start);
                    let to = end.min(token.range.end);
                    append(&mut tokens, cursor - offset..start - offset, Kind::Plain);
                    append(&mut tokens, start - offset..to - offset, token.kind);
                    cursor = to;
                    if token.range.end <= end {
                        token_index += 1;
                    } else {
                        break;
                    }
                }
                append(&mut tokens, cursor - offset..end - offset, Kind::Plain);
                offset = end + 1;
                tokens
            })
            .collect();
        (!self.stopped()).then_some(result)
    }
}

fn configure_queries() {
    // Kit 0.7's TSX config omits JavaScript keywords/strings. The complete
    // TypeScript query is compatible with TSX and keeps those base captures.
    static QUERIES: OnceLock<()> = OnceLock::new();
    QUERIES.get_or_init(|| {
        let registry = LanguageRegistry::singleton();
        if let (Some(mut tsx), Some(typescript)) =
            (registry.language("tsx"), registry.language("typescript"))
        {
            tsx.highlights = format!("{}\n(jsx_opening_element (identifier) @tag)\n(jsx_closing_element (identifier) @tag)\n(jsx_self_closing_element (identifier) @tag)\n", typescript.highlights).into();
            tsx.injections = typescript.injections;
            registry.register("tsx", &tsx);
        }
    });
}

#[cfg(test)]
fn tokenize_side(
    language: &str,
    lines: &[&str],
    header: &str,
    deadline: Instant,
) -> Option<Vec<Vec<Token>>> {
    AnalysisContext::new(deadline, &|| false).tokenize_side(language, lines, header)
}

/// Linear fallback with multiline comment/string state and UTF-8 byte ranges.
/// It deliberately retains unsupported constructs as original plain text.
fn lexical(text: &str, language: &str) -> Vec<Token> {
    let mut result = Vec::new();
    let bytes = text.as_bytes();
    let mut cursor = 0;
    let hash_comments = matches!(
        language,
        "ini"
            | "properties"
            | "dotenv"
            | "docker"
            | "shellscript"
            | "python"
            | "ruby"
            | "yaml"
            | "toml"
            | "make"
            | "cmake"
    );
    while cursor < bytes.len() {
        let start = cursor;
        let byte = bytes[cursor];
        let kind;
        if (hash_comments && byte == b'#')
            || (language == "ini" && byte == b';')
            || text[cursor..].starts_with("//")
        {
            cursor += text[cursor..].find('\n').unwrap_or(text.len() - cursor);
            kind = Kind::Comment;
        } else if text[cursor..].starts_with("/*") {
            cursor += text[cursor + 2..]
                .find("*/")
                .map_or(text.len() - cursor, |end| end + 4);
            kind = Kind::Comment;
        } else if matches!(byte, b'\'' | b'"' | b'`') {
            cursor += 1;
            while cursor < bytes.len() {
                if bytes[cursor] == b'\\' {
                    cursor = (cursor + 2).min(bytes.len());
                } else if bytes[cursor] == byte {
                    cursor += 1;
                    break;
                } else {
                    cursor += 1;
                }
            }
            // JSON keys use the same property color as the web tokenizer.
            kind = if matches!(language, "json" | "jsonc" | "json5")
                && text[cursor..].trim_start().starts_with(':')
            {
                Kind::Property
            } else {
                Kind::String
            };
        } else if byte.is_ascii_digit() {
            cursor += 1;
            while cursor < bytes.len()
                && (bytes[cursor].is_ascii_alphanumeric()
                    || matches!(bytes[cursor], b'.' | b'_' | b'+' | b'-'))
            {
                cursor += 1;
            }
            kind = Kind::Number;
        } else if byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$' | b'@' | b'-') {
            cursor += 1;
            while cursor < bytes.len()
                && (bytes[cursor].is_ascii_alphanumeric()
                    || matches!(bytes[cursor], b'_' | b'-' | b'$' | b'.'))
            {
                cursor += 1;
            }
            let word = &text[start..cursor];
            let next = text[cursor..].trim_start();
            kind = if "true false null undefined const let var import export from return if else for while fn pub struct enum impl use async await function class interface type extends new throw try catch def self pass None True False SELECT FROM WHERE AND OR AS INSERT UPDATE DELETE JOIN RUN FROM COPY ADD ARG ENV WORKDIR EXPOSE CMD ENTRYPOINT USER LABEL VOLUME SHELL ONBUILD STOPSIGNAL HEALTHCHECK MAINTAINER export include set option project message find_package".split_whitespace().any(|keyword| keyword==word) {
                Kind::Keyword
            } else if next.starts_with(':') || next.starts_with('=') { Kind::Property }
            else if next.starts_with('(') { Kind::Function }
            else if word.starts_with('$') || word.starts_with('@') { Kind::Property }
            else { Kind::Plain };
        } else {
            cursor += text[cursor..].chars().next().unwrap().len_utf8();
            kind = if byte.is_ascii_punctuation() {
                Kind::Punctuation
            } else {
                Kind::Plain
            };
        }
        // Escaped Unicode within strings can end on a continuation byte.
        while cursor < bytes.len() && !text.is_char_boundary(cursor) {
            cursor += 1;
        }
        append(&mut result, start..cursor, kind);
    }
    result
}

#[cfg(test)]
fn diff_tokens_with_deadline(
    path: &str,
    diff: &FileDiff,
    deadline: Instant,
) -> Option<Vec<Vec<Token>>> {
    diff_tokens_with_cancellation(path, diff, deadline, &|| false)
}

fn diff_tokens_with_cancellation(
    path: &str,
    diff: &FileDiff,
    deadline: Instant,
    should_cancel: &dyn Fn() -> bool,
) -> Option<Vec<Vec<Token>>> {
    let mut analysis = AnalysisContext::new(deadline, should_cancel);
    if analysis.stopped() {
        return None;
    }
    let language = language(path)?;
    if diff.hunks.len() > MAX_LINES {
        return None;
    }
    let mut lines = 0;
    let mut bytes = 0;
    for line in diff.hunks.iter().flat_map(|hunk| &hunk.lines) {
        if analysis.stopped() {
            return None;
        }
        lines += 1;
        bytes += line.content.len();
        if lines > MAX_LINES || bytes > MAX_BYTES || line.content.len() > MAX_LINE_BYTES {
            return None;
        }
    }
    if lines == 0 {
        return None;
    }
    let mut result = Vec::with_capacity(lines + diff.hunks.len());
    let mut total = 0;
    for hunk in &diff.hunks {
        if analysis.stopped() {
            return None;
        }
        result.push(Vec::new()); // Hunk metadata is never parsed as source.
        let old: Vec<_> = hunk
            .lines
            .iter()
            .filter(|line| matches!(line.kind.as_str(), "context" | "deletion"))
            .map(|line| strip_line_ending(&line.content))
            .collect();
        let new: Vec<_> = hunk
            .lines
            .iter()
            .filter(|line| matches!(line.kind.as_str(), "context" | "addition"))
            .map(|line| strip_line_ending(&line.content))
            .collect();
        let mut old = analysis
            .tokenize_side(language, &old, &hunk.header)?
            .into_iter();
        let mut new = analysis
            .tokenize_side(language, &new, &hunk.header)?
            .into_iter();
        for line in &hunk.lines {
            let tokens = match line.kind.as_str() {
                "addition" => new.next().unwrap_or_default(),
                "deletion" => old.next().unwrap_or_default(),
                "context" => {
                    let previous = old.next();
                    new.next().or(previous).unwrap_or_default()
                }
                _ => Vec::new(),
            };
            total += tokens.len();
            if total > MAX_TOKENS {
                return None;
            }
            result.push(tokens);
        }
    }
    (!analysis.stopped()).then_some(result)
}

#[cfg(test)]
fn content_tokens(path: &str, lines: &[String]) -> Option<Vec<Vec<Token>>> {
    content_tokens_with_cancellation(path, lines, Instant::now() + HIGHLIGHT_BUDGET, &|| false)
}

#[cfg(test)]
fn content_tokens_with_deadline(
    path: &str,
    lines: &[String],
    deadline: Instant,
) -> Option<Vec<Vec<Token>>> {
    content_tokens_with_cancellation(path, lines, deadline, &|| false)
}

fn content_tokens_with_cancellation(
    path: &str,
    lines: &[String],
    deadline: Instant,
    should_cancel: &dyn Fn() -> bool,
) -> Option<Vec<Vec<Token>>> {
    let language = language(path)?;
    if lines.len() > MAX_LINES
        || lines.iter().any(|line| line.len() > MAX_LINE_BYTES)
        || lines.iter().map(String::len).sum::<usize>() > MAX_BYTES
    {
        return None;
    }
    let borrowed: Vec<_> = lines.iter().map(String::as_str).collect();
    let mut analysis = AnalysisContext::new(deadline, should_cancel);
    let result = analysis.tokenize_side(language, &borrowed, "")?;
    (result.iter().map(Vec::len).sum::<usize>() <= MAX_TOKENS && !analysis.stopped())
        .then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mangrove_core::model::{DiffHunk, DiffLine};

    fn dispersed_diff(hunks: usize) -> FileDiff {
        FileDiff {
            binary: false,
            truncated: false,
            additions: hunks,
            deletions: hunks,
            hunks: (0..hunks)
                .map(|index| DiffHunk {
                    header: format!("@@ -{},2 +{},2 @@", index * 16 + 1, index * 16 + 1),
                    lines: vec![
                        DiffLine {
                            kind: "deletion".into(),
                            content: format!("/* old fragment {index} */\n"),
                            old_line: Some((index * 16 + 1) as u32),
                            new_line: None,
                        },
                        DiffLine {
                            kind: "addition".into(),
                            content: format!("export const value{index}: number = {index};\n"),
                            old_line: None,
                            new_line: Some((index * 16 + 1) as u32),
                        },
                        DiffLine {
                            kind: "context".into(),
                            content: "const café = 3;\n".into(),
                            old_line: Some((index * 16 + 2) as u32),
                            new_line: Some((index * 16 + 2) as u32),
                        },
                    ],
                })
                .collect(),
        }
    }

    // These assertions verify token semantics, not cold-query speed on a
    // shared CI runner. Production retains its separate `HIGHLIGHT_BUDGET`.
    const SEMANTIC_BUDGET: Duration = Duration::from_secs(30);

    fn semantic_deadline() -> Instant {
        Instant::now() + SEMANTIC_BUDGET
    }

    #[test]
    fn expired_deadline_rejects_parser_and_lexical_fallback_without_partial_tokens() {
        let deadline = Instant::now() - Duration::from_secs(1);
        for (path, language, source) in [
            ("file.ts", "typescript", "/* comment */ const value = 1;"),
            (".env", "dotenv", "NAME=\"éclair\" # comment"),
            ("file.scss", "scss", "$color: red;"),
        ] {
            assert!(tokenize_side(language, &[source], "", deadline).is_none());
            assert!(content_tokens_with_deadline(path, &[source.into()], deadline).is_none());
            let diff = FileDiff {
                binary: false,
                truncated: false,
                additions: 1,
                deletions: 0,
                hunks: vec![DiffHunk {
                    header: "@@ -0,0 +1 @@".into(),
                    lines: vec![DiffLine {
                        kind: "addition".into(),
                        content: source.into(),
                        old_line: None,
                        new_line: Some(1),
                    }],
                }],
            };
            assert!(diff_tokens_with_deadline(path, &diff, deadline).is_none());
        }
    }
    #[test]
    fn filenames_and_language_aliases_match_web() {
        for (path, expected) in [
            ("C:\\repo\\A.TSX", "tsx"),
            ("Cargo.lock", "toml"),
            (".env.local", "dotenv"),
            ("Dockerfile.prod", "docker"),
            ("x.svg", "xml"),
            ("Makefile", "make"),
            ("x.json5", "json5"),
        ] {
            assert_eq!(language(path), Some(expected));
        }
        assert_eq!(language("notes.txt"), None);
    }
    #[test]
    fn old_and_new_multiline_comments_are_independent() {
        let line = |kind: &str, content: &str| DiffLine {
            kind: kind.into(),
            content: content.into(),
            old_line: None,
            new_line: None,
        };
        let diff = FileDiff {
            binary: false,
            truncated: false,
            additions: 2,
            deletions: 1,
            hunks: vec![DiffHunk {
                header: "@@ -1 +1,2 @@".into(),
                lines: vec![
                    line("deletion", "/* old */\n"),
                    line("addition", "/* new\n"),
                    line("addition", "still comment */\n"),
                    line("context", "const café = 3;\n"),
                ],
            }],
        };
        let tokens = diff_tokens_with_deadline("file.ts", &diff, semantic_deadline()).unwrap();
        assert!(tokens[1].iter().any(|token| token.kind == Kind::Comment));
        assert!(tokens[3].iter().any(|token| token.kind == Kind::Comment));
        assert!(tokens[4].iter().any(|token| token.kind == Kind::Keyword));
        for (row, line) in diff.hunks[0].lines.iter().enumerate() {
            for token in &tokens[row + 1] {
                assert!(
                    strip_line_ending(&line.content)
                        .get(token.range.clone())
                        .is_some()
                );
            }
        }
    }

    #[test]
    fn many_hunks_keep_both_sides_independent_and_all_rows_lossless() {
        let diff = dispersed_diff(150);
        let tokens =
            diff_tokens_with_deadline("many-hunks.ts", &diff, semantic_deadline()).unwrap();
        assert_eq!(tokens.len(), 600);
        for (hunk, rows) in diff.hunks.iter().zip(tokens.chunks_exact(4)) {
            assert!(rows[0].is_empty());
            assert!(rows[1].iter().all(|token| token.kind == Kind::Comment));
            assert!(rows[2].iter().any(|token| token.kind == Kind::Keyword));
            assert!(rows[3].iter().any(|token| token.kind == Kind::Keyword));
            for (line, tokens) in hunk.lines.iter().zip(&rows[1..]) {
                let text = strip_line_ending(&line.content);
                let restored: String = tokens
                    .iter()
                    .map(|token| &text[token.range.clone()])
                    .collect();
                assert_eq!(restored, text);
            }
        }
    }

    #[test]
    fn one_compiled_highlighter_handles_three_hundred_different_fragments() {
        let mut analysis = AnalysisContext::new(semantic_deadline(), &|| false);
        for index in 0..150 {
            let old = format!("/* isolated old comment {index}");
            let new = format!("export const café{index}: number = {index};");
            for source in [&old, &new] {
                let tokens = analysis.tokenize_side("typescript", &[source], "").unwrap();
                assert!(!tokens[0].is_empty());
            }
        }
        assert_eq!(analysis.highlighters.len(), 1);
    }

    #[test]
    fn replacing_fragments_matches_fresh_trees_and_injection_layers() {
        for (language, fragments) in [
            (
                "typescript",
                vec![
                    "/* unclosed comment with é */\nconst value = `old`;",
                    "const café: number = 3;",
                    "/* unterminated",
                    "export function fresh() { return \"new\"; }",
                    "const x = 1;\n\n\n// three removed lines\n  ",
                    "let y = 2;",
                ],
            ),
            (
                "html",
                vec![
                    "<script>const café = \"old\";</script><style>body { color: red; }</style>",
                    "<div class=\"new\">é</div>",
                    "<script>const next = 42;</script>",
                    "<script>const next = 42;</script>",
                    "<style>.next { color: blue; }</style>",
                ],
            ),
            (
                "markdown",
                vec![
                    "```ts\nconst old = 1;\n```",
                    "# Fresh\n`éclair`",
                    "plain prose",
                ],
            ),
        ] {
            let mut analysis = AnalysisContext::new(semantic_deadline(), &|| false);
            for source in fragments {
                let lines: Vec<_> = source.split('\n').collect();
                let reused = analysis.tokenize_side(language, &lines, "").unwrap();
                let fresh = tokenize_side(language, &lines, "", semantic_deadline()).unwrap();
                assert_eq!(reused, fresh, "{language}: {source}");
            }
            assert_eq!(analysis.highlighters.len(), 1, "{language}");
        }
    }

    #[test]
    fn component_hunks_choose_and_reuse_their_own_grammar() {
        let mut analysis = AnalysisContext::new(semantic_deadline(), &|| false);
        for (header, source) in [
            ("@@ -1 +1 @@", "<div class=\"new\">é</div>"),
            ("@@ -8 +8 @@ const answer", "answer = 42;"),
            ("@@ -16 +16 @@", ".name { color: red; }"),
            ("@@ -32 +32 @@", "<script>const answer = 42;</script>"),
        ] {
            let reused = analysis.tokenize_side("vue", &[source], header).unwrap();
            let fresh = tokenize_side("vue", &[source], header, semantic_deadline()).unwrap();
            assert_eq!(reused, fresh);
        }
        assert_eq!(analysis.highlighters.len(), 3);
    }

    #[test]
    fn lexical_dialects_never_compile_a_host_grammar() {
        let mut analysis = AnalysisContext::new(semantic_deadline(), &|| false);
        for (language, source) in [
            ("jsonc", "// comment\n{\"answer\": 42}"),
            ("json5", "{answer: 'éclair',}"),
            ("scss", "$color: red;\nbody { color: $color; }"),
            ("less", "@color: red;\nbody { color: @color; }"),
        ] {
            let lines: Vec<_> = source.split('\n').collect();
            let tokens = analysis.tokenize_side(language, &lines, "").unwrap();
            assert!(
                tokens
                    .iter()
                    .flatten()
                    .any(|token| token.kind != Kind::Plain)
            );
        }
        assert!(analysis.highlighters.is_empty());
    }

    #[test]
    fn cancellation_between_sides_stops_reused_analysis() {
        let cancelled = std::cell::Cell::new(false);
        let should_cancel = || cancelled.get();
        let mut analysis = AnalysisContext::new(semantic_deadline(), &should_cancel);
        assert!(
            analysis
                .tokenize_side("typescript", &["/* old side"], "")
                .is_some()
        );
        cancelled.set(true);
        assert!(
            analysis
                .tokenize_side("typescript", &["const newSide = 1;"], "")
                .is_none()
        );
        assert_eq!(analysis.highlighters.len(), 1);
    }

    #[test]
    fn cancellation_after_work_starts_does_not_cache_a_plain_snapshot() {
        let diff = dispersed_diff(150);
        let checks = std::cell::Cell::new(0);
        let cancelled =
            cached_diff_tokens_within("cancel-many-hunks.ts", &diff, SEMANTIC_BUDGET, || {
                checks.set(checks.get() + 1);
                checks.get() > diff.hunks.len() + 450 + 10
            });
        assert!(cancelled.is_none());
        assert!(checks.get() > diff.hunks.len() + 450);
        let retried =
            cached_diff_tokens_within("cancel-many-hunks.ts", &diff, SEMANTIC_BUDGET, || false)
                .unwrap();
        assert_eq!(retried.len(), 600);
        assert!(retried[2].iter().any(|token| token.kind == Kind::Keyword));
    }

    #[test]
    fn cache_reuses_only_complete_content_and_checks_cancellation_on_hits() {
        let lines = vec!["VALUE=\"éclair\"".into()];
        let first =
            cached_content_tokens_cancellable(".env.cache-complete", &lines, || false).unwrap();
        let second =
            cached_content_tokens_cancellable(".env.cache-complete", &lines, || false).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(
            cached_content_tokens_cancellable(".env.cache-complete", &lines, || true).is_none()
        );
        let changed =
            cached_content_tokens_cancellable(".env.cache-complete", &["VALUE=42".into()], || {
                false
            })
            .unwrap();
        assert!(!Arc::ptr_eq(&first, &changed));
        assert!(changed[0].iter().any(|token| token.kind == Kind::Number));
    }

    #[test]
    fn exhausted_analysis_can_be_retried_without_a_cached_empty_result() {
        let first = cached("retry-exhausted-analysis.ts", 1, &|| false, || None).unwrap();
        assert!(first.is_empty());
        let retried = cached("retry-exhausted-analysis.ts", 1, &|| false, || {
            Some(vec![vec![Token {
                range: 0..5,
                kind: Kind::Keyword,
            }]])
        })
        .unwrap();
        assert_eq!(retried[0][0].kind, Kind::Keyword);
    }

    #[test]
    fn a_new_cache_reader_does_not_wait_for_an_in_flight_analysis() {
        use std::sync::mpsc;

        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let old = std::thread::spawn(move || {
            cached("independent-cache-reader", 1, &|| false, || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Some(vec![Vec::new()])
            })
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let new = std::thread::spawn(move || {
            let result = cached("independent-cache-reader", 1, &|| false, || {
                Some(vec![Vec::new()])
            });
            ready_tx.send(result.is_some()).unwrap();
        });
        let ready = ready_rx.recv_timeout(Duration::from_secs(5));
        release_tx.send(()).unwrap();
        old.join().unwrap();
        new.join().unwrap();
        assert_eq!(ready, Ok(true));
    }
    #[test]
    fn configuration_fallback_is_colored_and_lossless() {
        for path in [
            ".env",
            "Dockerfile",
            "settings.ini",
            "x.properties",
            "x.scss",
            "x.less",
            "x.json5",
        ] {
            let lines = vec!["NAME = \"éclair\" # comment".into()];
            let tokens = content_tokens_with_deadline(path, &lines, semantic_deadline()).unwrap();
            assert!(
                tokens[0].iter().any(|token| token.kind != Kind::Plain),
                "{path}"
            );
            let restored: String = tokens[0]
                .iter()
                .map(|token| &lines[0][token.range.clone()])
                .collect();
            assert_eq!(restored, lines[0]);
        }
    }
    #[test]
    fn all_39_supported_languages_have_lossless_colored_source() {
        let cases = [
            ("a.ts", "const answer: number = 42;"),
            ("a.js", "const answer = 42;"),
            ("a.tsx", "const view = <div>Hello</div>;"),
            ("a.jsx", "const view = <div>Hello</div>;"),
            ("a.vue", "<script lang=\"ts\">const answer = 42;</script>"),
            ("a.svelte", "<script>const answer = 42;</script>"),
            ("a.astro", "---\nconst answer = 42;\n---\n<h1>Hello</h1>"),
            ("a.html", "<div class=\"hello\">World</div>"),
            ("a.css", "body { color: red; }"),
            ("a.scss", "$color: red;\nbody { color: $color; }"),
            ("a.less", "@color: red;\nbody { color: @color; }"),
            ("a.json", "{\"answer\": 42}"),
            ("a.jsonc", "// Comment\n{\"answer\": 42}"),
            ("a.json5", "{answer: 42,}"),
            ("a.yaml", "answer: 42"),
            ("a.php", "<?php echo \"Hello\";"),
            ("a.rs", "pub fn answer() -> i32 { 42 }"),
            ("a.py", "def answer():\n    return 42"),
            ("a.sh", "echo \"Hello\""),
            ("a.sql", "SELECT answer FROM values;"),
            ("a.md", "# Hello\n`code`"),
            ("a.toml", "answer = 42"),
            ("a.xml", "<answer value=\"42\" />"),
            ("a.c", "int answer() { return 42; }"),
            ("a.cpp", "class Answer { public: int value = 42; };"),
            ("a.cs", "class Answer { int value = 42; }"),
            ("a.java", "class Answer { int value = 42; }"),
            ("a.kt", "val answer = 42"),
            ("a.go", "package main\nfunc answer() int { return 42 }"),
            ("a.rb", "def answer\n  42\nend"),
            ("a.swift", "let answer = 42"),
            ("a.graphql", "query Answer { answer }"),
            ("a.ini", "[section]\nanswer = 42"),
            ("a.properties", "answer = 42"),
            ("CMakeLists.txt", "set(ANSWER 42)"),
            ("Makefile", "all:\n\techo \"Hello\""),
            (".env", "ANSWER=42"),
            ("Dockerfile", "FROM alpine\nRUN echo \"Hello\""),
            ("a.diff", "@@ -1 +1 @@\n-old\n+new"),
        ];
        assert_eq!(cases.len(), 39);
        for (path, source) in cases {
            let lines: Vec<String> = source.lines().map(str::to_owned).collect();
            let tokens = content_tokens_with_deadline(path, &lines, semantic_deadline())
                .unwrap_or_else(|| panic!("{path}: missing syntax"));
            assert!(
                tokens
                    .iter()
                    .flatten()
                    .any(|token| token.kind != Kind::Plain),
                "{path}: entirely plain"
            );
            for (line, tokens) in lines.iter().zip(tokens) {
                let restored: String = tokens
                    .iter()
                    .map(|token| &line[token.range.clone()])
                    .collect();
                assert_eq!(restored, *line, "{path}: source changed");
            }
        }
    }
    #[test]
    fn highlighting_bounds_do_not_drop_original_content() {
        assert!(content_tokens("a.rs", &vec!["".into(); MAX_LINES + 1]).is_none());
        assert!(content_tokens("a.rs", &["x".repeat(MAX_LINE_BYTES + 1)]).is_none());
    }
}
