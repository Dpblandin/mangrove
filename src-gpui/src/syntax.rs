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

#[derive(Clone, Debug)]
pub struct Token {
    pub range: Range<usize>,
    pub kind: Kind,
}

pub type Tokens = Arc<Vec<Vec<Token>>>;
type CacheKey = (String, u64);
type Analysis = Arc<OnceLock<Tokens>>;
#[derive(Default)]
struct Cache {
    entries: HashMap<CacheKey, Analysis>,
    order: VecDeque<CacheKey>,
}

fn cached(
    path: &str,
    fingerprint: u64,
    analyze: impl FnOnce() -> Option<Vec<Vec<Token>>>,
) -> Tokens {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let key = (path.to_owned(), fingerprint);
    let analysis = {
        let mut cache = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let analysis = cache.entries.entry(key.clone()).or_default().clone();
        cache.order.retain(|previous| previous != &key);
        cache.order.push_back(key);
        // Bounded semantic snapshots. An in-flight analysis holds its own slot,
        // so eviction never cancels another viewer's work.
        while cache.order.len() > 8 {
            if let Some(oldest) = cache.order.pop_front() {
                cache.entries.remove(&oldest);
            }
        }
        analysis
    };
    analysis
        .get_or_init(|| Arc::new(analyze().unwrap_or_default()))
        .clone()
}

pub fn cached_diff_tokens(path: &str, diff: &FileDiff) -> Tokens {
    let mut fingerprint = DefaultHasher::new();
    "diff".hash(&mut fingerprint);
    for hunk in &diff.hunks {
        hunk.header.hash(&mut fingerprint);
        for line in &hunk.lines {
            line.kind.hash(&mut fingerprint);
            line.content.hash(&mut fingerprint);
        }
    }
    cached(path, fingerprint.finish(), || diff_tokens(path, diff))
}

pub fn cached_content_tokens(path: &str, lines: &[String]) -> Tokens {
    let mut fingerprint = DefaultHasher::new();
    "content".hash(&mut fingerprint);
    lines.hash(&mut fingerprint);
    cached(path, fingerprint.finish(), || content_tokens(path, lines))
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

fn tokenize_side(
    language: &str,
    lines: &[&str],
    header: &str,
    deadline: Instant,
) -> Option<Vec<Vec<Token>>> {
    if Instant::now() >= deadline {
        return None;
    }
    if lines.is_empty() {
        return Some(Vec::new());
    }
    let code = lines.join("\n");
    let (grammar, prefix) = grammar(language, &code, header);
    let source = prefix.clone() + &code;
    // Kit 0.7's TSX config includes only the TypeScript extension query,
    // omitting JavaScript's keywords/strings. Its full TypeScript query is
    // compatible with the TSX parser and retains those base semantic captures.
    static QUERIES: OnceLock<()> = OnceLock::new();
    QUERIES.get_or_init(|| {
        let registry = LanguageRegistry::singleton();
        if let (Some(mut tsx),Some(typescript)) = (registry.language("tsx"),registry.language("typescript")) {
            tsx.highlights = format!("{}\n(jsx_opening_element (identifier) @tag)\n(jsx_closing_element (identifier) @tag)\n(jsx_self_closing_element (identifier) @tag)\n",typescript.highlights).into();
            tsx.injections = typescript.injections;
            registry.register("tsx",&tsx);
        }
    });
    let mut available = LanguageRegistry::singleton()
        .language(grammar)
        .is_some_and(|language| language.has_grammar() && !language.highlights.is_empty());
    if Instant::now() >= deadline {
        return None;
    }
    let mut ranges = Vec::new();
    if available {
        let mut highlighter = SyntaxHighlighter::new(grammar);
        // A grammar can be registered while its query is incompatible with
        // the parser version. Kit then constructs an inert plain highlighter.
        available = highlighter.language().as_ref() == grammar;
        let text = Rope::from(source.as_str());
        // Query construction is outside Kit's parse timeout and can consume
        // the whole budget before update begins.
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())?;
        if !highlighter.update(None, &text, Some(remaining)) || Instant::now() >= deadline {
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
    if Instant::now() >= deadline {
        return None;
    }
    // A stateful lexical fallback covers configuration formats without a
    // bundled parser. It also colors syntax accepted by JSON5/SCSS grammars
    // that the stricter host grammar does not understand.
    if !available || matches!(language, "jsonc" | "json5" | "scss" | "less") {
        ranges = lexical(&source, language);
    }
    if Instant::now() >= deadline {
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
    (Instant::now() < deadline).then_some(result)
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

pub fn diff_tokens(path: &str, diff: &FileDiff) -> Option<Vec<Vec<Token>>> {
    diff_tokens_with_deadline(path, diff, Instant::now() + Duration::from_millis(500))
}

fn diff_tokens_with_deadline(
    path: &str,
    diff: &FileDiff,
    deadline: Instant,
) -> Option<Vec<Vec<Token>>> {
    let language = language(path)?;
    if diff.hunks.len() > MAX_LINES {
        return None;
    }
    let mut lines = 0;
    let mut bytes = 0;
    for line in diff.hunks.iter().flat_map(|hunk| &hunk.lines) {
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
        if Instant::now() >= deadline {
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
        let mut old = tokenize_side(language, &old, &hunk.header, deadline)?.into_iter();
        let mut new = tokenize_side(language, &new, &hunk.header, deadline)?.into_iter();
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
    (Instant::now() < deadline).then_some(result)
}

pub fn content_tokens(path: &str, lines: &[String]) -> Option<Vec<Vec<Token>>> {
    content_tokens_with_deadline(path, lines, Instant::now() + Duration::from_millis(500))
}

fn content_tokens_with_deadline(
    path: &str,
    lines: &[String],
    deadline: Instant,
) -> Option<Vec<Vec<Token>>> {
    let language = language(path)?;
    if lines.len() > MAX_LINES
        || lines.iter().any(|line| line.len() > MAX_LINE_BYTES)
        || lines.iter().map(String::len).sum::<usize>() > MAX_BYTES
    {
        return None;
    }
    let borrowed: Vec<_> = lines.iter().map(String::as_str).collect();
    let result = tokenize_side(language, &borrowed, "", deadline)?;
    (result.iter().map(Vec::len).sum::<usize>() <= MAX_TOKENS && Instant::now() < deadline)
        .then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mangrove_core::model::{DiffHunk, DiffLine};

    fn semantic_deadline() -> Instant {
        // These assertions verify token semantics, not cold-query speed on a
        // shared CI runner. Production retains its separate 500 ms budget.
        Instant::now() + Duration::from_secs(30)
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
