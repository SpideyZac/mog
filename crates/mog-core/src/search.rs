//! Finding text in a document.

use ropey::Rope;

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

    use super::{find_all, next_match, prev_match, smart_case};

    /// Matches do not overlap and case folding is optional.
    #[test]
    fn finds_matches() {
        let text = Rope::from_str("aaaa Mog mog");
        assert_eq!(find_all(&text, "aa", true), [(0, 2), (2, 4)]);
        assert_eq!(find_all(&text, "mog", false), [(5, 8), (9, 12)]);
        assert_eq!(find_all(&text, "Mog", true), [(5, 8)]);
        assert!(find_all(&text, "", false).is_empty());
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
