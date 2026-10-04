//! Syntax highlighting for mog.
//!
//! Wraps tree-sitter grammars and turns their highlight captures into a small set of
//! [`Kind`]s that themes know how to color.

use std::{collections::HashMap, path::Path};

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter as TsHighlighter};

/// The capture names highlights are recognized by, paired with the kind they map to.
///
/// Captures match the longest listed prefix, so `function.method` falls back to `function`.
const CAPTURES: &[(&str, Kind)] = &[
    ("attribute", Kind::Attribute),
    ("boolean", Kind::Constant),
    ("character", Kind::String),
    ("comment", Kind::Comment),
    ("constant", Kind::Constant),
    ("constant.builtin", Kind::Constant),
    ("constructor", Kind::Type),
    ("embedded", Kind::Punctuation),
    ("escape", Kind::Constant),
    ("function", Kind::Function),
    ("function.macro", Kind::Attribute),
    ("keyword", Kind::Keyword),
    ("label", Kind::Attribute),
    ("module", Kind::Namespace),
    ("namespace", Kind::Namespace),
    ("number", Kind::Number),
    ("float", Kind::Number),
    ("operator", Kind::Operator),
    ("property", Kind::Property),
    ("punctuation", Kind::Punctuation),
    ("string", Kind::String),
    ("string.special", Kind::Constant),
    ("tag", Kind::Markup),
    ("text.title", Kind::Markup),
    ("text.literal", Kind::String),
    ("text.uri", Kind::Function),
    ("text.reference", Kind::Function),
    ("text.emphasis", Kind::Attribute),
    ("text.strong", Kind::Keyword),
    ("type", Kind::Type),
    ("type.builtin", Kind::Type),
    ("variable.builtin", Kind::Constant),
    ("variable.parameter", Kind::Property),
];

/// The languages mog can highlight, by name, with the file extensions they claim.
const LANGUAGES: &[(&str, &[&str])] = &[
    ("rust", &["rs"]),
    ("python", &["py", "pyi"]),
    ("javascript", &["js", "mjs", "cjs"]),
    ("jsx", &["jsx"]),
    ("typescript", &["ts", "mts", "cts"]),
    ("tsx", &["tsx"]),
    ("json", &["json", "jsonc"]),
    ("toml", &["toml"]),
    ("go", &["go"]),
    ("c", &["c", "h", "cpp", "hpp", "cc", "cxx"]),
    ("bash", &["sh", "bash", "zsh"]),
    ("markdown", &["md", "markdown"]),
];

/// What a piece of code is, as far as coloring goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A language keyword.
    Keyword,
    /// A function or method name.
    Function,
    /// A type name.
    Type,
    /// A string or char literal.
    String,
    /// A number literal.
    Number,
    /// A constant, boolean or escape.
    Constant,
    /// A comment.
    Comment,
    /// An operator.
    Operator,
    /// A bracket or separator.
    Punctuation,
    /// A macro, attribute or label.
    Attribute,
    /// A field, property or parameter.
    Property,
    /// A module or namespace.
    Namespace,
    /// A markup heading or tag.
    Markup,
}

/// A highlighted range of chars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// The first char offset.
    pub from: usize,
    /// The char offset just past the end.
    pub to: usize,
    /// What the range is.
    pub kind: Kind,
}

/// Returns the language name for `path` based on its extension.
pub fn language_for(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_lowercase();
    LANGUAGES
        .iter()
        .find(|(_, extensions)| extensions.contains(&extension.as_str()))
        .map(|(name, _)| *name)
}

/// Builds the highlight config for `language`, or `None` if it is unknown or broken.
fn config_for(language: &str) -> Option<HighlightConfiguration> {
    let js = tree_sitter_javascript::HIGHLIGHT_QUERY;
    let jsx = tree_sitter_javascript::JSX_HIGHLIGHT_QUERY;
    let ts = tree_sitter_typescript::HIGHLIGHTS_QUERY;
    let (lang, highlights, injections, locals) = match language {
        "rust" => (
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY.to_owned(),
            tree_sitter_rust::INJECTIONS_QUERY,
            "",
        ),
        "python" => (
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::HIGHLIGHTS_QUERY.to_owned(),
            "",
            "",
        ),
        "javascript" => (
            tree_sitter_javascript::LANGUAGE.into(),
            js.to_owned(),
            "",
            tree_sitter_javascript::LOCALS_QUERY,
        ),
        "jsx" => (
            tree_sitter_javascript::LANGUAGE.into(),
            format!("{jsx}\n{js}"),
            "",
            tree_sitter_javascript::LOCALS_QUERY,
        ),
        "typescript" => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            format!("{ts}\n{js}"),
            "",
            tree_sitter_typescript::LOCALS_QUERY,
        ),
        "tsx" => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            format!("{ts}\n{jsx}\n{js}"),
            "",
            tree_sitter_typescript::LOCALS_QUERY,
        ),
        "json" => (
            tree_sitter_json::LANGUAGE.into(),
            tree_sitter_json::HIGHLIGHTS_QUERY.to_owned(),
            "",
            "",
        ),
        "toml" => (
            tree_sitter_toml_ng::LANGUAGE.into(),
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY.to_owned(),
            "",
            "",
        ),
        "go" => (
            tree_sitter_go::LANGUAGE.into(),
            tree_sitter_go::HIGHLIGHTS_QUERY.to_owned(),
            "",
            "",
        ),
        "c" => (
            tree_sitter_c::LANGUAGE.into(),
            tree_sitter_c::HIGHLIGHT_QUERY.to_owned(),
            "",
            "",
        ),
        "bash" => (
            tree_sitter_bash::LANGUAGE.into(),
            tree_sitter_bash::HIGHLIGHT_QUERY.to_owned(),
            "",
            "",
        ),
        "markdown" => (
            tree_sitter_md::LANGUAGE.into(),
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.to_owned(),
            "",
            "",
        ),
        _ => return None,
    };
    let mut config =
        HighlightConfiguration::new(lang, language, &highlights, injections, locals).ok()?;
    let names: Vec<&str> = CAPTURES.iter().map(|(name, _)| *name).collect();
    config.configure(&names);
    Some(config)
}

/// Highlights documents, keeping the parsed grammar configs around between calls.
#[derive(Default)]
pub struct Highlighter {
    /// The tree-sitter highlighter, which holds reusable parser state.
    inner: TsHighlighter,
    /// Grammar configs by language, `None` for ones that failed to load.
    configs: HashMap<&'static str, Option<HighlightConfiguration>>,
}

impl Highlighter {
    /// Creates a highlighter. Grammars are loaded the first time they are needed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the highlighted spans of `text` written in `language`, sorted by position.
    ///
    /// Nested captures produce overlapping spans with the innermost last. Unknown languages and
    /// parse failures give no spans.
    pub fn highlight(&mut self, language: &'static str, text: &str) -> Vec<Span> {
        let config = self
            .configs
            .entry(language)
            .or_insert_with(|| config_for(language));
        let Some(config) = config.as_ref() else {
            return Vec::new();
        };
        let Ok(events) = self
            .inner
            .highlight(config, text.as_bytes(), None, None, |_| None)
        else {
            return Vec::new();
        };
        let mut spans = Vec::new();
        let mut stack: Vec<Kind> = Vec::new();
        // byte offsets come in increasing order so chars can be counted incrementally
        let mut byte_pos = 0;
        let mut char_pos = 0;
        for event in events {
            let Ok(event) = event else {
                break;
            };
            match event {
                HighlightEvent::HighlightStart(highlight) => {
                    if let Some((_, kind)) = CAPTURES.get(highlight.0) {
                        stack.push(*kind);
                    }
                }
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } => {
                    let skipped = text.get(byte_pos..start).map_or(0, |s| s.chars().count());
                    let from = char_pos + skipped;
                    let len = text.get(start..end).map_or(0, |s| s.chars().count());
                    byte_pos = end;
                    char_pos = from + len;
                    if let Some(kind) = stack.last() {
                        spans.push(Span {
                            from,
                            to: from + len,
                            kind: *kind,
                        });
                    }
                }
            }
        }
        spans
    }
}

#[cfg(test)]
/// Tests for highlighting.
mod tests {
    use std::path::Path;

    use super::{Highlighter, Kind, LANGUAGES, language_for};

    /// Extensions map to languages and unknown ones do not.
    #[test]
    fn languages_by_extension() {
        assert_eq!(language_for(Path::new("src/main.rs")), Some("rust"));
        assert_eq!(language_for(Path::new("App.TSX")), Some("tsx"));
        assert_eq!(language_for(Path::new("README")), None);
    }

    /// Rust keywords, functions and strings are found at the right char offsets.
    #[test]
    fn highlights_rust() {
        let mut highlighter = Highlighter::new();
        let text = "// é\nfn main() { let s = \"hi\"; }";
        let spans = highlighter.highlight("rust", text);
        let kind_at = |pos| {
            spans
                .iter()
                .rev()
                .find(|span| (span.from..span.to).contains(&pos))
                .map(|span| span.kind)
        };
        assert_eq!(kind_at(0), Some(Kind::Comment));
        assert_eq!(kind_at(5), Some(Kind::Keyword));
        assert_eq!(kind_at(8), Some(Kind::Function));
        assert_eq!(kind_at(26), Some(Kind::String));
    }

    /// Every bundled grammar loads with its queries.
    #[test]
    fn every_language_loads() {
        let mut highlighter = Highlighter::new();
        for (name, _) in LANGUAGES {
            highlighter.highlight(name, "x");
            assert!(highlighter.configs[name].is_some(), "{name} failed to load");
        }
    }
}
