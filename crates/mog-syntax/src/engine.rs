//! Highlighting straight on tree-sitter, reusing the last parse after an edit.
//!
//! A document keeps its syntax tree between edits. Each edit is replayed onto the tree, the text
//! is parsed again starting from the old tree, and only the lines that were edited or whose
//! syntax changed are highlighted again. Code in other languages inside a file, like fenced code
//! in Markdown or a `<script>` in Vue, is parsed on its own and colored on top.

use std::{cmp::Reverse, collections::HashMap, ops::Range, sync::Arc};

use ropey::Rope;
use tree_sitter::{
    InputEdit, Language, Node, Parser, Point, Query, QueryCursor, Range as TsRange,
    StreamingIterator, Tree,
};

use crate::{CAPTURES, Kind, Span, resolve_language, source_for};

/// How deep code inside code inside code is followed.
const MAX_DEPTH: usize = 3;

/// Above this share of the text, highlighting everything again is simpler than splicing.
const SPLICE_LIMIT: f32 = 0.5;

/// Every kind, so a kind fits in a byte while painting.
const KINDS: [Kind; 13] = [
    Kind::Keyword,
    Kind::Function,
    Kind::Type,
    Kind::String,
    Kind::Number,
    Kind::Constant,
    Kind::Comment,
    Kind::Operator,
    Kind::Punctuation,
    Kind::Attribute,
    Kind::Property,
    Kind::Namespace,
    Kind::Markup,
];

/// Returns the kind a capture called `name` stands for, by the longest known prefix, so
/// `function.method` counts as `function`.
fn kind_for(name: &str) -> Option<Kind> {
    CAPTURES
        .iter()
        .filter(|(known, _)| {
            name == *known
                || name
                    .strip_prefix(known)
                    .is_some_and(|rest| rest.starts_with('.'))
        })
        .max_by_key(|(known, _)| known.len())
        .map(|(_, kind)| *kind)
}

/// A language with its compiled queries.
pub struct Grammar {
    /// The tree-sitter language.
    language: Language,
    /// What to color.
    highlights: Query,
    /// Where other languages are embedded, if the language has any.
    injections: Option<Query>,
    /// The kind of each highlight capture, by capture index.
    kinds: Vec<Option<u8>>,
}

impl Grammar {
    /// Compiles the grammar called `name`, or `None` if it is unknown or its queries are broken.
    fn load(name: &str) -> Option<Self> {
        let (language, highlights, injections) = source_for(name)?;
        let highlights = Query::new(&language, &highlights).ok()?;
        let injections = (!injections.is_empty())
            .then(|| Query::new(&language, injections).ok())
            .flatten();
        let kinds = highlights
            .capture_names()
            .iter()
            .map(|name| {
                kind_for(name)
                    .and_then(|kind| KINDS.iter().position(|known| *known == kind))
                    .and_then(|index| u8::try_from(index + 1).ok())
            })
            .collect();
        Some(Self {
            language,
            highlights,
            injections,
            kinds,
        })
    }
}

/// One captured node, before overlaps are settled.
struct Capture {
    /// The first byte.
    start: usize,
    /// The byte just past the end.
    end: usize,
    /// How deeply embedded the language is, outer first.
    depth: usize,
    /// When the query found it, captures found later win.
    order: usize,
    /// The kind, as one plus its index in [`KINDS`], or 0 for a name that is not colored.
    kind: u8,
}

/// Parses and highlights documents, keeping compiled grammars around between calls.
pub struct Highlighter {
    /// Grammars by language, `None` for ones that failed to load.
    grammars: HashMap<&'static str, Option<Arc<Grammar>>>,
    /// The parser, reused for every language.
    parser: Parser,
}

impl Default for Highlighter {
    fn default() -> Self {
        Self {
            grammars: HashMap::new(),
            parser: Parser::new(),
        }
    }
}

impl Highlighter {
    /// Creates a highlighter. Grammars load the first time they are needed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the grammar for `language`, loading it the first time.
    pub fn grammar(&mut self, language: &'static str) -> Option<Arc<Grammar>> {
        self.grammars
            .entry(language)
            .or_insert_with(|| Grammar::load(language).map(Arc::new))
            .clone()
    }

    /// Parses `text` in `language`, starting from `old` if it was edited to match.
    fn parse(&mut self, language: &'static str, text: &str, old: Option<&Tree>) -> Option<Tree> {
        let grammar = self.grammar(language)?;
        self.parser.set_language(&grammar.language).ok()?;
        self.parser.set_included_ranges(&[]).ok()?;
        self.parser.parse(text, old)
    }

    /// Collects the captures of `tree` in `grammar` that touch `range`, and of the languages
    /// embedded there, into `out`.
    fn collect(
        &mut self,
        grammar: &Grammar,
        tree: &Tree,
        text: &str,
        range: &Range<usize>,
        depth: usize,
        out: &mut Vec<Capture>,
    ) {
        let bytes = text.as_bytes();
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(range.clone());
        let mut captures = cursor.captures(&grammar.highlights, tree.root_node(), bytes);
        // like tree-sitter-highlight, the last pattern that matches a node decides its color
        let mut by_node: HashMap<usize, usize> = HashMap::new();
        while let Some((found, index)) = captures.next() {
            let capture = found.captures()[*index];
            let kind = grammar.kinds[capture.index as usize].unwrap_or(0);
            if let Some(&earlier) = by_node.get(&capture.node.id()) {
                out[earlier].kind = kind;
                continue;
            }
            let (start, end) = (
                capture.node.start_byte().max(range.start),
                capture.node.end_byte().min(range.end),
            );
            if start < end {
                by_node.insert(capture.node.id(), out.len());
                out.push(Capture {
                    start,
                    end,
                    depth,
                    order: out.len(),
                    kind,
                });
            }
        }
        if depth >= MAX_DEPTH {
            return;
        }
        let Some(injections) = &grammar.injections else {
            return;
        };
        let mut embedded: Vec<(&'static str, TsRange)> = Vec::new();
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(range.clone());
        let names = injections.capture_names();
        let mut matches = cursor.matches(injections, tree.root_node(), bytes);
        while let Some(found) = matches.next() {
            let mut language = injections
                .property_settings(found.pattern_index)
                .iter()
                .find(|setting| &*setting.key == "injection.language")
                .and_then(|setting| setting.value.as_deref().map(str::to_owned));
            let mut content: Option<Node<'_>> = None;
            for capture in found.captures() {
                match names[capture.index as usize] {
                    "injection.language" => {
                        language = capture.node.utf8_text(bytes).ok().map(str::to_owned);
                    }
                    "injection.content" => content = Some(capture.node),
                    _ => {}
                }
            }
            if let (Some(name), Some(content)) =
                (language.as_deref().and_then(resolve_language), content)
            {
                embedded.push((name, content.range()));
            }
        }
        for (name, content) in embedded {
            let Some(inner) = self.grammar(name) else {
                continue;
            };
            let parsed = self.parser.set_language(&inner.language).is_ok()
                && self.parser.set_included_ranges(&[content]).is_ok();
            let tree = parsed.then(|| self.parser.parse(text, None)).flatten();
            let _ = self.parser.set_included_ranges(&[]);
            let Some(tree) = tree else {
                continue;
            };
            let overlap = range.start.max(content.start_byte)..range.end.min(content.end_byte);
            if !overlap.is_empty() {
                self.collect(&inner, &tree, text, &overlap, depth + 1, out);
            }
        }
    }

    /// Highlights `range` of `text`, which `tree` parsed in `language`.
    ///
    /// Spans count chars from `base`, the char offset where `range` starts.
    fn highlight_range(
        &mut self,
        language: &'static str,
        tree: &Tree,
        text: &str,
        range: Range<usize>,
        base: usize,
    ) -> Vec<Span> {
        let Some(grammar) = self.grammar(language) else {
            return Vec::new();
        };
        let mut captures = Vec::new();
        self.collect(&grammar, tree, text, &range, 0, &mut captures);
        // painting in the order the query found captures, outer languages first, gives what
        // tree-sitter-highlight gives: whatever started last covers what started before it
        captures.sort_by_key(|capture| (capture.depth, capture.order));
        let mut paint = vec![0u8; range.len()];
        for capture in captures.iter().filter(|capture| capture.kind > 0) {
            paint[capture.start - range.start..capture.end - range.start].fill(capture.kind);
        }
        let mut spans: Vec<Span> = Vec::new();
        for (char_index, (byte, _)) in text[range.clone()].char_indices().enumerate() {
            let pos = base + char_index;
            let Some(kind) = paint[byte]
                .checked_sub(1)
                .map(|index| KINDS[usize::from(index)])
            else {
                continue;
            };
            match spans.last_mut() {
                Some(last) if last.to == pos && last.kind == kind => last.to = pos + 1,
                _ => spans.push(Span {
                    from: pos,
                    to: pos + 1,
                    kind,
                }),
            }
        }
        spans
    }

    /// Parses and highlights all of `text` in `language`.
    pub fn highlight(&mut self, language: &'static str, text: &str) -> Vec<Span> {
        let Some(tree) = self.parse(language, text, None) else {
            return Vec::new();
        };
        self.highlight_range(language, &tree, text, 0..text.len(), 0)
    }
}

/// A change to a document, in chars of the text before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// The first char replaced.
    pub start: usize,
    /// The char just past the last one replaced.
    pub end: usize,
    /// The new text.
    pub text: String,
}

/// Returns where byte `byte` of `rope` is as a row and byte column.
fn point(rope: &Rope, byte: usize) -> Point {
    let row = rope.byte_to_line(byte);
    Point::new(row, byte - rope.line_to_byte(row))
}

/// Moves char offset `pos` through an edit of `start..end` that inserted `inserted` chars, the
/// way [`Edit`]s move positions in the editor.
fn map_char(pos: usize, start: usize, end: usize, inserted: usize) -> usize {
    if pos < start {
        pos
    } else if pos < end || (pos == start && start == end) {
        start + inserted
    } else {
        pos + inserted - (end - start)
    }
}

/// Moves byte offset `pos` through an edit of `start..old_end` that now ends at `new_end`.
fn map_byte(pos: usize, start: usize, old_end: usize, new_end: usize) -> usize {
    if pos <= start {
        pos
    } else if pos >= old_end {
        pos - old_end + new_end
    } else {
        new_end
    }
}

/// Replaces the spans in chars `from..to` of `spans` with `fresh`, cutting ones that cross.
fn splice(spans: &mut Vec<Span>, from: usize, to: usize, fresh: Vec<Span>) {
    let mut kept = Vec::with_capacity(spans.len() + fresh.len());
    for span in spans.drain(..) {
        if span.to <= from || span.from >= to {
            kept.push(span);
            continue;
        }
        if span.from < from {
            kept.push(Span { to: from, ..span });
        }
        if span.to > to {
            kept.push(Span { from: to, ..span });
        }
    }
    kept.extend(fresh);
    kept.sort_by_key(|span| span.from);
    *spans = kept;
}

/// The highlighting of one document, kept up to date edit by edit.
pub struct Incremental {
    /// The language.
    language: &'static str,
    /// The tree of `text`.
    tree: Option<Tree>,
    /// The text the tree and spans are for.
    text: String,
    /// The colors of `text`.
    spans: Vec<Span>,
}

impl Incremental {
    /// Starts highlighting a document in `language`.
    pub fn new(language: &'static str) -> Self {
        Self {
            language,
            tree: None,
            text: String::new(),
            spans: Vec::new(),
        }
    }

    /// Returns the language.
    pub fn language(&self) -> &'static str {
        self.language
    }

    /// Returns the colors of the last text.
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// Brings the colors up to date with `text`.
    ///
    /// `edits` are the changes since the last text, one list per transaction in order. Without
    /// them, or if they do not lead to `text`, everything is parsed and highlighted again.
    pub fn update(&mut self, engine: &mut Highlighter, text: &str, edits: Option<&[Vec<Edit>]>) {
        if let (Some(edits), Some(tree)) = (edits, self.tree.as_mut())
            && let Some(dirty) = replay(tree, &mut self.spans, &self.text, text, edits)
            && let Some(new_tree) = engine.parse(self.language, text, Some(tree))
        {
            let rope = Rope::from_str(text);
            let mut ranges: Vec<Range<usize>> = dirty;
            ranges.extend(
                tree.changed_ranges(&new_tree)
                    .map(|range| range.start_byte..range.end_byte),
            );
            let ranges = whole_lines(&rope, ranges);
            let changed: usize = ranges.iter().map(ExactSizeIterator::len).sum();
            #[expect(clippy::cast_precision_loss, reason = "a rough share is enough here")]
            let share = changed as f32 / text.len().max(1) as f32;
            if share <= SPLICE_LIMIT {
                for range in ranges {
                    let (from, to) = (rope.byte_to_char(range.start), rope.byte_to_char(range.end));
                    let fresh = engine.highlight_range(self.language, &new_tree, text, range, from);
                    splice(&mut self.spans, from, to, fresh);
                }
                self.tree = Some(new_tree);
                text.clone_into(&mut self.text);
                return;
            }
        }
        self.tree = engine.parse(self.language, text, None);
        self.spans = match &self.tree {
            Some(tree) => engine.highlight_range(self.language, tree, text, 0..text.len(), 0),
            None => Vec::new(),
        };
        text.clone_into(&mut self.text);
    }
}

/// Replays `edits` from `old` onto `tree` and `spans`, and returns the edited byte ranges of the
/// new text, or `None` if the edits do not lead to `new`.
fn replay(
    tree: &mut Tree,
    spans: &mut [Span],
    old: &str,
    new: &str,
    edits: &[Vec<Edit>],
) -> Option<Vec<Range<usize>>> {
    let mut rope = Rope::from_str(old);
    let mut dirty: Vec<Range<usize>> = Vec::new();
    for transaction in edits {
        // later changes first, so the offsets of earlier ones still hold
        let mut ordered: Vec<&Edit> = transaction.iter().collect();
        ordered.sort_by_key(|edit| Reverse(edit.start));
        for edit in ordered {
            if edit.start > edit.end || edit.end > rope.len_chars() {
                return None;
            }
            let start_byte = rope.char_to_byte(edit.start);
            let old_end_byte = rope.char_to_byte(edit.end);
            let start_position = point(&rope, start_byte);
            let old_end_position = point(&rope, old_end_byte);
            rope.remove(edit.start..edit.end);
            rope.insert(edit.start, &edit.text);
            let new_end_byte = start_byte + edit.text.len();
            let new_end_position = point(&rope, new_end_byte);
            tree.edit(&InputEdit {
                start_byte,
                old_end_byte,
                new_end_byte,
                start_position,
                old_end_position,
                new_end_position,
            });
            let inserted = edit.text.chars().count();
            for span in spans.iter_mut() {
                span.from = map_char(span.from, edit.start, edit.end, inserted);
                span.to = map_char(span.to, edit.start, edit.end, inserted);
            }
            for range in &mut dirty {
                range.start = map_byte(range.start, start_byte, old_end_byte, new_end_byte);
                range.end = map_byte(range.end, start_byte, old_end_byte, new_end_byte);
            }
            dirty.push(start_byte..new_end_byte);
        }
    }
    (rope.len_bytes() == new.len() && rope == new).then_some(dirty)
}

/// Widens `ranges` of bytes to whole lines of `rope` and merges the ones that touch.
fn whole_lines(rope: &Rope, ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let len = rope.len_bytes();
    let mut widened: Vec<Range<usize>> = ranges
        .into_iter()
        .map(|range| {
            let start = rope.line_to_byte(rope.byte_to_line(range.start.min(len)));
            let last = rope.byte_to_line(range.end.min(len));
            let end = if last + 1 < rope.len_lines() {
                rope.line_to_byte(last + 1)
            } else {
                len
            };
            start..end.max(start)
        })
        .collect();
    widened.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in widened {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

#[cfg(test)]
/// Tests for incremental highlighting.
mod tests {
    use super::{Edit, Highlighter, Incremental, kind_for};
    use crate::Kind;

    /// Capture names fall back to their longest known prefix.
    #[test]
    fn maps_capture_names() {
        assert_eq!(kind_for("function.method.call"), Some(Kind::Function));
        assert_eq!(kind_for("function.macro"), Some(Kind::Attribute));
        assert_eq!(kind_for("functional"), None);
        assert_eq!(kind_for("variable"), None);
    }

    /// Highlighting edit by edit ends up the same as highlighting the final text from scratch.
    #[test]
    fn edits_match_a_fresh_highlight() {
        let mut engine = Highlighter::default();
        let mut doc = Incremental::new("rust");
        let mut text = String::from("fn main() {\n    let x = 1;\n}\n\nfn other() {}\n");
        doc.update(&mut engine, &text, None);
        let steps = [
            // type a string on a new line
            vec![Edit {
                start: 26,
                end: 26,
                text: "\n    let s = \"hi\";".into(),
            }],
            // open a block comment that swallows the rest
            vec![Edit {
                start: 0,
                end: 0,
                text: "/* ".into(),
            }],
            // and close it again
            vec![Edit {
                start: 3,
                end: 3,
                text: " */".into(),
            }],
            // two changes in one transaction
            vec![
                Edit {
                    start: 9,
                    end: 13,
                    text: "start".into(),
                },
                Edit {
                    start: 52,
                    end: 57,
                    text: "second".into(),
                },
            ],
        ];
        for edits in steps {
            for edit in edits.iter().rev() {
                let start = text
                    .char_indices()
                    .nth(edit.start)
                    .map_or(text.len(), |(at, _)| at);
                let end = text
                    .char_indices()
                    .nth(edit.end)
                    .map_or(text.len(), |(at, _)| at);
                text.replace_range(start..end, &edit.text);
            }
            doc.update(&mut engine, &text, Some(&[edits]));
            let fresh = Highlighter::default().highlight("rust", &text);
            assert_eq!(
                doc.spans(),
                fresh.as_slice(),
                "after edits the text is {text:?}"
            );
        }
    }

    /// Edits that do not lead to the text start over instead of going wrong.
    #[test]
    fn mismatched_edits_start_over() {
        let mut engine = Highlighter::default();
        let mut doc = Incremental::new("rust");
        doc.update(&mut engine, "fn a() {}", None);
        let wrong = [vec![Edit {
            start: 0,
            end: 0,
            text: "x".into(),
        }]];
        doc.update(&mut engine, "fn b() {}", Some(&wrong));
        assert_eq!(
            doc.spans(),
            Highlighter::default()
                .highlight("rust", "fn b() {}")
                .as_slice()
        );
    }
}
