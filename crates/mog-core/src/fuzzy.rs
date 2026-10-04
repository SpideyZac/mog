//! Fuzzy matching for pickers like the command palette and file finder.

/// The score for each matched char.
const MATCH: i32 = 16;

/// The bonus for a match right after the previous one.
const CONSECUTIVE: i32 = 24;

/// The bonus for a match at the start of a word.
const WORD_START: i32 = 20;

/// The penalty for every skipped char between matches.
const GAP: i32 = 1;

/// A successful match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// How good the match is. Higher is better.
    pub score: i32,
    /// The char indexes in the candidate that matched the query.
    pub indexes: Vec<usize>,
}

/// Returns whether the char at `index` of `chars` starts a word.
fn is_word_start(chars: &[char], index: usize) -> bool {
    let Some(prev) = index.checked_sub(1).map(|i| chars[i]) else {
        return true;
    };
    let ch = chars[index];
    !prev.is_alphanumeric() || (prev.is_lowercase() && ch.is_uppercase())
}

/// Matches `query` against `candidate`, ignoring case.
///
/// Every query char must appear in order. Spaces in the query are ignored. An empty query matches
/// everything with a score of 0.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<Match> {
    let chars: Vec<char> = candidate.chars().collect();
    let lower: Vec<char> = chars
        .iter()
        .map(|ch| ch.to_lowercase().next().unwrap_or(*ch))
        .collect();
    let mut indexes = Vec::new();
    let mut score = 0;
    let mut from = 0;
    for wanted in query.chars().filter(|ch| !ch.is_whitespace()) {
        let wanted = wanted.to_lowercase().next().unwrap_or(wanted);
        // prefer a word start ahead over the first plain occurrence
        let first = (from..lower.len()).find(|&i| lower[i] == wanted)?;
        let consecutive = indexes.last().is_some_and(|&last| first == last + 1);
        let start = if consecutive {
            first
        } else {
            (first..lower.len())
                .find(|&i| lower[i] == wanted && is_word_start(&chars, i))
                .unwrap_or(first)
        };
        score += MATCH;
        match indexes.last() {
            Some(&last) if start == last + 1 => score += CONSECUTIVE,
            Some(&last) => score -= GAP * i32::try_from(start - last - 1).unwrap_or(i32::MAX / 4),
            None => score -= GAP * i32::try_from(start).unwrap_or(i32::MAX / 4),
        }
        if is_word_start(&chars, start) {
            score += WORD_START;
        }
        indexes.push(start);
        from = start + 1;
    }
    // shorter candidates win ties, but an empty query keeps the given order
    if !indexes.is_empty() {
        score -= i32::try_from(chars.len() / 8).unwrap_or(0);
    }
    Some(Match { score, indexes })
}

#[cfg(test)]
/// Tests for [`fuzzy_match`].
mod tests {
    use super::fuzzy_match;

    /// Query chars must appear in order.
    #[test]
    fn requires_order() {
        assert!(fuzzy_match("abc", "a_b_c").is_some());
        assert!(fuzzy_match("cba", "abc").is_none());
        assert!(fuzzy_match("", "anything").is_some());
    }

    /// Word starts and runs beat scattered matches.
    #[test]
    fn prefers_word_starts() {
        let good = fuzzy_match("ed", "editor.rs").expect("match");
        let bad = fuzzy_match("ed", "unrelated.rs").expect("match");
        assert!(good.score > bad.score);
        let camel = fuzzy_match("ev", "EditorView").expect("match");
        assert_eq!(camel.indexes, [0, 6]);
    }

    /// Matching ignores case and spaces in the query.
    #[test]
    fn ignores_case_and_spaces() {
        let found = fuzzy_match("To Ex", "toggle explorer").expect("match");
        assert_eq!(found.indexes, [0, 1, 7, 8]);
    }
}
