//! Syntax highlighting for mog.
//!
//! Wraps tree-sitter grammars and turns their highlight captures into a small set of
//! [`Kind`]s that themes know how to color.

use std::path::Path;

use tree_sitter::Language;

pub mod engine;

pub use engine::{Edit, Highlighter, Incremental};

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
    ("python", &["py", "pyi", "pyw"]),
    ("javascript", &["js", "mjs", "cjs"]),
    ("jsx", &["jsx"]),
    ("typescript", &["ts", "mts", "cts"]),
    ("tsx", &["tsx"]),
    ("json", &["json", "jsonc", "json5"]),
    ("toml", &["toml"]),
    ("go", &["go"]),
    ("c", &["c", "h"]),
    ("cpp", &["cpp", "hpp", "cc", "cxx", "hh", "hxx", "ino"]),
    ("csharp", &["cs", "csx"]),
    ("java", &["java"]),
    ("bash", &["sh", "bash", "zsh"]),
    ("powershell", &["ps1", "psm1", "psd1"]),
    ("markdown", &["md", "markdown"]),
    ("html", &["html", "htm", "xhtml"]),
    ("vue", &["vue"]),
    ("css", &["css"]),
    ("xml", &["xml", "svg", "xaml", "csproj", "plist", "xsd"]),
    ("yaml", &["yml", "yaml"]),
    ("lua", &["lua"]),
    ("ruby", &["rb", "rake", "gemspec"]),
    ("php", &["php", "phtml"]),
    ("swift", &["swift"]),
    ("scala", &["scala", "sc", "sbt"]),
    ("haskell", &["hs"]),
    ("ocaml", &["ml"]),
    ("ocaml_interface", &["mli"]),
    ("elixir", &["ex", "exs"]),
    ("zig", &["zig", "zon"]),
    ("sql", &["sql"]),
    ("make", &["mk", "mak"]),
];

/// The highlight query for Vue files.
const VUE_HIGHLIGHTS: &str = include_str!("../queries/vue/highlights.scm");

/// The injection query for Vue files, for their script and style blocks.
const VUE_INJECTIONS: &str = include_str!("../queries/vue/injections.scm");

/// Other names injections use for languages, like `ts` in `<script lang="ts">`.
const ALIASES: &[(&str, &str)] = &[
    ("js", "javascript"),
    ("ts", "typescript"),
    ("sh", "bash"),
    ("shell", "bash"),
    ("zsh", "bash"),
    ("console", "bash"),
    ("py", "python"),
    ("rs", "rust"),
    ("golang", "go"),
    ("c++", "cpp"),
    ("cs", "csharp"),
    ("c#", "csharp"),
    ("yml", "yaml"),
    ("ps1", "powershell"),
    ("pwsh", "powershell"),
    ("scss", "css"),
    ("md", "markdown"),
    ("ml", "ocaml"),
    ("makefile", "make"),
];

/// Returns the language called `name` by an injection, by name, alias or file extension.
fn resolve_language(name: &str) -> Option<&'static str> {
    let name = name.trim().to_lowercase();
    if let Some((language, _)) = LANGUAGES.iter().find(|(language, _)| *language == name) {
        return Some(language);
    }
    if let Some((_, language)) = ALIASES.iter().find(|(alias, _)| *alias == name) {
        return Some(language);
    }
    LANGUAGES
        .iter()
        .find(|(_, extensions)| extensions.contains(&name.as_str()))
        .map(|(language, _)| *language)
}

/// Languages for files known by their whole name instead of an extension.
const FILE_NAMES: &[(&str, &str)] = &[
    ("makefile", "make"),
    ("gnumakefile", "make"),
    ("gemfile", "ruby"),
    ("rakefile", "ruby"),
    ("pkgbuild", "bash"),
    (".bashrc", "bash"),
    (".zshrc", "bash"),
    (".profile", "bash"),
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

/// Returns the language name for `path` based on its file name or extension.
pub fn language_for(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?.to_lowercase();
    if let Some((_, language)) = FILE_NAMES.iter().find(|(file, _)| *file == name) {
        return Some(language);
    }
    let extension = path.extension()?.to_str()?.to_lowercase();
    LANGUAGES
        .iter()
        .find(|(_, extensions)| extensions.contains(&extension.as_str()))
        .map(|(name, _)| *name)
}

/// Returns the grammar, highlight query and injection query of `language`.
fn source_for(language: &str) -> Option<(Language, String, &'static str)> {
    let (lang, highlights, injections, _locals) = grammar_parts(language)?;
    Some((lang, highlights, injections))
}

/// Returns the grammar and the highlight, injection and locals queries of `language`.
fn grammar_parts(language: &str) -> Option<(Language, String, &'static str, &'static str)> {
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
        "cpp" => (
            tree_sitter_cpp::LANGUAGE.into(),
            format!(
                "{}\n{}",
                tree_sitter_cpp::HIGHLIGHT_QUERY,
                tree_sitter_c::HIGHLIGHT_QUERY
            ),
            "",
            "",
        ),
        "csharp" => (
            tree_sitter_c_sharp::LANGUAGE.into(),
            tree_sitter_c_sharp::HIGHLIGHTS_QUERY.to_owned(),
            "",
            "",
        ),
        "java" => (
            tree_sitter_java::LANGUAGE.into(),
            tree_sitter_java::HIGHLIGHTS_QUERY.to_owned(),
            "",
            "",
        ),
        "bash" => (
            tree_sitter_bash::LANGUAGE.into(),
            tree_sitter_bash::HIGHLIGHT_QUERY.to_owned(),
            "",
            "",
        ),
        "powershell" => (
            tree_sitter_powershell::LANGUAGE.into(),
            tree_sitter_powershell::HIGHLIGHTS_QUERY.to_owned(),
            "",
            "",
        ),
        "markdown" => (
            tree_sitter_md::LANGUAGE.into(),
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.to_owned(),
            tree_sitter_md::INJECTION_QUERY_BLOCK,
            "",
        ),
        "html" => (
            tree_sitter_html::LANGUAGE.into(),
            tree_sitter_html::HIGHLIGHTS_QUERY.to_owned(),
            tree_sitter_html::INJECTIONS_QUERY,
            "",
        ),
        "vue" => (
            tree_sitter_vue3::LANGUAGE.into(),
            VUE_HIGHLIGHTS.to_owned(),
            VUE_INJECTIONS,
            "",
        ),
        "css" => (
            tree_sitter_css::LANGUAGE.into(),
            tree_sitter_css::HIGHLIGHTS_QUERY.to_owned(),
            "",
            "",
        ),
        "xml" => (
            tree_sitter_xml::LANGUAGE_XML.into(),
            tree_sitter_xml::XML_HIGHLIGHT_QUERY.to_owned(),
            "",
            "",
        ),
        "yaml" => (
            tree_sitter_yaml::LANGUAGE.into(),
            tree_sitter_yaml::HIGHLIGHTS_QUERY.to_owned(),
            "",
            "",
        ),
        "lua" => (
            tree_sitter_lua::LANGUAGE.into(),
            tree_sitter_lua::HIGHLIGHTS_QUERY.to_owned(),
            "",
            tree_sitter_lua::LOCALS_QUERY,
        ),
        "ruby" => (
            tree_sitter_ruby::LANGUAGE.into(),
            tree_sitter_ruby::HIGHLIGHTS_QUERY.to_owned(),
            "",
            tree_sitter_ruby::LOCALS_QUERY,
        ),
        "php" => (
            tree_sitter_php::LANGUAGE_PHP.into(),
            tree_sitter_php::HIGHLIGHTS_QUERY.to_owned(),
            "",
            "",
        ),
        "swift" => (
            tree_sitter_swift::LANGUAGE.into(),
            tree_sitter_swift::HIGHLIGHTS_QUERY.to_owned(),
            "",
            tree_sitter_swift::LOCALS_QUERY,
        ),
        "scala" => (
            tree_sitter_scala::LANGUAGE.into(),
            tree_sitter_scala::HIGHLIGHTS_QUERY.to_owned(),
            "",
            tree_sitter_scala::LOCALS_QUERY,
        ),
        "haskell" => (
            tree_sitter_haskell::LANGUAGE.into(),
            tree_sitter_haskell::HIGHLIGHTS_QUERY.to_owned(),
            "",
            tree_sitter_haskell::LOCALS_QUERY,
        ),
        "ocaml" => (
            tree_sitter_ocaml::LANGUAGE_OCAML.into(),
            tree_sitter_ocaml::HIGHLIGHTS_QUERY.to_owned(),
            "",
            tree_sitter_ocaml::LOCALS_QUERY,
        ),
        "ocaml_interface" => (
            tree_sitter_ocaml::LANGUAGE_OCAML_INTERFACE.into(),
            tree_sitter_ocaml::HIGHLIGHTS_QUERY.to_owned(),
            "",
            tree_sitter_ocaml::LOCALS_QUERY,
        ),
        "elixir" => (
            tree_sitter_elixir::LANGUAGE.into(),
            tree_sitter_elixir::HIGHLIGHTS_QUERY.to_owned(),
            "",
            "",
        ),
        "zig" => (
            tree_sitter_zig::LANGUAGE.into(),
            tree_sitter_zig::HIGHLIGHTS_QUERY.to_owned(),
            "",
            "",
        ),
        "sql" => (
            tree_sitter_sequel::LANGUAGE.into(),
            tree_sitter_sequel::HIGHLIGHTS_QUERY.to_owned(),
            "",
            "",
        ),
        "make" => (
            tree_sitter_make::LANGUAGE.into(),
            tree_sitter_make::HIGHLIGHTS_QUERY.to_owned(),
            "",
            "",
        ),
        _ => return None,
    };
    Some((lang, highlights, injections, locals))
}

#[cfg(test)]
/// Tests for highlighting.
mod tests {
    use std::path::Path;

    use super::{Highlighter, Kind, LANGUAGES, Span, language_for, resolve_language};

    /// Extensions map to languages and unknown ones do not.
    #[test]
    fn languages_by_extension() {
        assert_eq!(language_for(Path::new("src/main.rs")), Some("rust"));
        assert_eq!(language_for(Path::new("App.TSX")), Some("tsx"));
        assert_eq!(language_for(Path::new("README")), None);
        assert_eq!(language_for(Path::new("lib/foo.hpp")), Some("cpp"));
        assert_eq!(language_for(Path::new("dir/Makefile")), Some("make"));
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

    /// Every grammar colors something in a small sample of its language.
    #[test]
    fn every_language_highlights() {
        let samples = [
            ("cpp", "class A { int x = 1; };"),
            ("csharp", "class A { int x = 1; }"),
            ("java", "class A { int x = 1; }"),
            ("powershell", "$x = \"hi\""),
            ("html", "<p class=\"a\">hi</p>"),
            ("css", "a { color: red; }"),
            ("xml", "<a b=\"c\"/>"),
            ("yaml", "a: \"b\""),
            ("lua", "local x = \"hi\""),
            ("ruby", "def a; \"hi\"; end"),
            ("php", "<?php function a() { return 1; }"),
            ("swift", "func a() { let x = 1 }"),
            ("scala", "def a = \"hi\""),
            ("haskell", "main = putStrLn \"hi\""),
            ("ocaml", "let x = \"hi\""),
            ("elixir", "def a, do: \"hi\""),
            ("zig", "const x = \"hi\";"),
            ("sql", "SELECT a FROM b;"),
            ("make", "all:\n\techo hi"),
            ("vue", "<template><p :a=\"b\">{{ c }}</p></template>"),
        ];
        let mut highlighter = Highlighter::new();
        for (name, text) in samples {
            assert!(
                !highlighter.highlight(name, text).is_empty(),
                "{name} highlighted nothing"
            );
        }
    }

    /// Returns the kind of the innermost span covering char `pos`.
    fn kind_at(spans: &[Span], pos: usize) -> Option<Kind> {
        spans
            .iter()
            .rev()
            .find(|span| (span.from..span.to).contains(&pos))
            .map(|span| span.kind)
    }

    /// Script and style blocks in Vue and fences in Markdown get their own colors.
    #[test]
    fn highlights_injections() {
        let mut highlighter = Highlighter::new();
        let vue = "<script lang=\"ts\">const x: number = 1</script>";
        let spans = highlighter.highlight("vue", vue);
        let at = vue.find("const").expect("in sample");
        assert_eq!(kind_at(&spans, at), Some(Kind::Keyword));
        let plain = "<script>let s = \"hi\"</script><style>a { color: red; }</style>";
        let spans = highlighter.highlight("vue", plain);
        assert_eq!(
            kind_at(&spans, plain.find("let").expect("in sample")),
            Some(Kind::Keyword)
        );
        let md = "# hi

```rust
fn main() {}
```
";
        let spans = highlighter.highlight("markdown", md);
        assert_eq!(
            kind_at(&spans, md.find("fn").expect("in sample")),
            Some(Kind::Keyword)
        );
    }

    /// Injection names resolve by name, alias and extension.
    #[test]
    fn resolves_injected_names() {
        assert_eq!(resolve_language("TypeScript"), Some("typescript"));
        assert_eq!(resolve_language("ts"), Some("typescript"));
        assert_eq!(resolve_language("rs"), Some("rust"));
        assert_eq!(resolve_language("hs"), Some("haskell"));
        assert_eq!(resolve_language("klingon"), None);
    }

    /// Every bundled grammar loads with its queries.
    #[test]
    fn every_language_loads() {
        let mut highlighter = Highlighter::new();
        for (name, _) in LANGUAGES {
            assert!(highlighter.grammar(name).is_some(), "{name} failed to load");
        }
    }
}
