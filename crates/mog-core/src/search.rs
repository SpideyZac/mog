//! Finding text in a document.

use regex::{Error as RegexError, Regex, RegexBuilder};
use ropey::Rope;

use crate::transaction::Change;

/// Returns whether a search for `query` should match case, using smart case: only queries with
/// an uppercase letter are case sensitive.
pub fn smart_case(query: &str) -> bool {
    query.chars().any(char::is_uppercase)
}

/// Lowercases one char, keeping it a single char so offsets stay put.
fn fold(ch: char) -> char {
    ch.to_lowercase().next().unwrap_or(ch)
}

/// Returns the char ranges of every non-overlapping match of `query` in `text`.
pub fn find_all(text: &Rope, query: &str, case_sensitive: bool) -> Vec<(usize, usize)> {
    let needle: Vec<char> = if case_sensitive {
        query.chars().collect()
    } else {
        query.chars().map(fold).collect()
    };
    if needle.is_empty() {
        return Vec::new();
    }
    let hay: Vec<char> = if case_sensitive {
        text.chars().collect()
    } else {
        text.chars().map(fold).collect()
    };
    let mut matches = Vec::new();
    let mut at = 0;
    while at + needle.len() <= hay.len() {
        if hay[at..at + needle.len()] == needle[..] {
            matches.push((at, at + needle.len()));
            at += needle.len();
        } else {
            at += 1;
        }
    }
    matches
}

/// How a search query matches text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SearchOptions {
    /// Whether case has to match.
    pub case_sensitive: bool,
    /// Whether matches have to be whole words.
    pub whole_word: bool,
    /// Whether the query is a regular expression.
    pub regex: bool,
}

/// Returns whether `ch` can be part of a word.
fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// A compiled search query.
#[derive(Debug, Clone)]
pub struct Matcher {
    /// The query as a regular expression, escaped unless it already was one.
    regex: Regex,
    /// How it matches.
    options: SearchOptions,
}

impl Matcher {
    /// Compiles `query` with `options`.
    ///
    /// # Errors
    ///
    /// Returns the reason when `query` is not a valid regular expression.
    pub fn new(query: &str, options: SearchOptions) -> Result<Self, String> {
        let pattern = if options.regex {
            query.to_owned()
        } else {
            regex::escape(query)
        };
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(!options.case_sensitive)
            .multi_line(true)
            .build()
            .map_err(|err| match err {
                RegexError::Syntax(text) => text.lines().last().unwrap_or("").trim().to_owned(),
                other => other.to_string(),
            })?;
        Ok(Self { regex, options })
    }

    /// Returns the byte ranges of every match in `text`, skipping empty ones.
    pub fn find_in(&self, text: &str) -> Vec<(usize, usize)> {
        self.regex
            .find_iter(text)
            .map(|found| (found.start(), found.end()))
            .filter(|&(start, end)| start < end)
            .filter(|&(start, end)| !self.options.whole_word || is_whole_word(text, start, end))
            .collect()
    }

    /// Returns what the match at bytes `start..end` of `text` becomes when replaced.
    ///
    /// In regex mode `$1` and `${name}` in `replacement` bring in capture groups.
    pub fn replacement_for(&self, text: &str, start: usize, replacement: &str) -> String {
        if !self.options.regex {
            return replacement.to_owned();
        }
        let mut out = String::new();
        if let Some(captures) = self.regex.captures_at(text, start) {
            captures.expand(replacement, &mut out);
        }
        out
    }
}

/// Returns whether bytes `start..end` of `text` are not glued to a word on either side.
fn is_whole_word(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back().is_some_and(is_word);
    let after = text[end..].chars().next().is_some_and(is_word);
    !before && !after
}

/// Returns the char ranges of every match of `query` in `text`.
///
/// # Errors
///
/// Returns the reason when the query is a broken regular expression.
pub fn find_matches(
    text: &Rope,
    query: &str,
    options: SearchOptions,
) -> Result<Vec<(usize, usize)>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let matcher = Matcher::new(query, options)?;
    let haystack = text.to_string();
    Ok(matcher
        .find_in(&haystack)
        .into_iter()
        .map(|(start, end)| (text.byte_to_char(start), text.byte_to_char(end)))
        .collect())
}

/// Returns the changes that replace every match of `matcher` in `text` with `replacement`.
pub fn replace_changes(text: &Rope, matcher: &Matcher, replacement: &str) -> Vec<Change> {
    let haystack = text.to_string();
    matcher
        .find_in(&haystack)
        .into_iter()
        .map(|(start, end)| Change {
            start: text.byte_to_char(start),
            end: text.byte_to_char(end),
            text: matcher.replacement_for(&haystack, start, replacement),
        })
        .collect()
}

/// Returns the index of the first match starting at or after `pos`, wrapping to the first match.
pub fn next_match(matches: &[(usize, usize)], pos: usize) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    Some(
        matches
            .iter()
            .position(|(from, _)| *from >= pos)
            .unwrap_or(0),
    )
}

/// Returns the index of the last match starting before `pos`, wrapping to the last match.
pub fn prev_match(matches: &[(usize, usize)], pos: usize) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    Some(
        matches
            .iter()
            .rposition(|(from, _)| *from < pos)
            .unwrap_or(matches.len() - 1),
    )
}

#[cfg(test)]
/// Tests for searching.
mod tests {
    use ropey::Rope;

    use super::{
        Matcher, SearchOptions, find_all, find_matches, next_match, prev_match, replace_changes,
        smart_case,
    };

    /// Matches do not overlap and case folding is optional.
    #[test]
    fn finds_matches() {
        let text = Rope::from_str("aaaa Mog mog");
        assert_eq!(find_all(&text, "aa", true), [(0, 2), (2, 4)]);
        assert_eq!(find_all(&text, "mog", false), [(5, 8), (9, 12)]);
        assert_eq!(find_all(&text, "Mog", true), [(5, 8)]);
        assert!(find_all(&text, "", false).is_empty());
    }

    /// Case, whole words and regexes change what matches.
    #[test]
    fn options_change_matches() {
        let text = Rope::from_str("mog Mog moggy x_mog mog9");
        let plain = SearchOptions::default();
        assert_eq!(find_matches(&text, "mog", plain).expect("valid").len(), 5);
        let case = SearchOptions {
            case_sensitive: true,
            ..plain
        };
        assert_eq!(find_matches(&text, "Mog", case).expect("valid"), [(4, 7)]);
        let word = SearchOptions {
            whole_word: true,
            ..plain
        };
        assert_eq!(
            find_matches(&text, "mog", word).expect("valid"),
            [(0, 3), (4, 7)]
        );
        let regex = SearchOptions {
            regex: true,
            ..plain
        };
        assert_eq!(
            find_matches(&text, r"mog\d", regex).expect("valid"),
            [(20, 24)]
        );
        assert!(find_matches(&text, "(", regex).is_err());
        assert_eq!(find_matches(&text, "(", plain).expect("escaped").len(), 0);
    }

    /// Regex replacements bring in capture groups, plain ones are taken as written.
    #[test]
    fn replacements_expand_groups() {
        let text = Rope::from_str("let a = 1; let b = 2;");
        let regex = SearchOptions {
            regex: true,
            ..SearchOptions::default()
        };
        let matcher = Matcher::new(r"let (\w+)", regex).expect("valid");
        let changes = replace_changes(&text, &matcher, "const $1");
        let texts: Vec<&str> = changes.iter().map(|change| change.text.as_str()).collect();
        assert_eq!(texts, ["const a", "const b"]);
        let plain = Matcher::new("let", SearchOptions::default()).expect("valid");
        assert_eq!(replace_changes(&text, &plain, "$1")[0].text, "$1");
    }

    /// Only uppercase queries are case sensitive.
    #[test]
    fn smart_case_rules() {
        assert!(!smart_case("mog"));
        assert!(smart_case("Mog"));
    }

    /// Next and previous wrap around.
    #[test]
    fn steps_wrap() {
        let matches = [(2, 3), (6, 7)];
        assert_eq!(next_match(&matches, 3), Some(1));
        assert_eq!(next_match(&matches, 9), Some(0));
        assert_eq!(prev_match(&matches, 6), Some(0));
        assert_eq!(prev_match(&matches, 1), Some(1));
        assert_eq!(next_match(&[], 0), None);
    }
}
