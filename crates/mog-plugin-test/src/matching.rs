//! Loose JSON matching, so a test only spells out the parts of an answer it cares about.

use serde_json::Value;

/// Returns whether `actual` has everything in `expected`.
///
/// Objects match when every expected key matches, extra keys are fine. Lists match when every
/// expected item matches a different actual item, in order, with others allowed in between.
/// Strings, numbers, booleans and `null` must be equal.
pub fn matches(expected: &Value, actual: &Value) -> bool {
    mismatch(expected, actual).is_none()
}

/// Returns where `actual` first differs from `expected`, as a path like `actions[0].text` and
/// what was there, or `None` when it [`matches`].
pub fn mismatch(expected: &Value, actual: &Value) -> Option<String> {
    find(expected, actual, "")
}

/// Does [`mismatch`] for the value at `path`.
fn find(expected: &Value, actual: &Value, path: &str) -> Option<String> {
    let here = || {
        if path.is_empty() {
            "the answer".to_owned()
        } else {
            path.to_owned()
        }
    };
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => {
            expected.iter().find_map(|(key, value)| {
                let path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match actual.get(key) {
                    Some(actual) => find(value, actual, &path),
                    None if value.is_null() => None,
                    None => Some(format!("{path} is missing")),
                }
            })
        }
        (Value::Array(expected), Value::Array(actual)) => {
            let mut rest = actual.iter().enumerate();
            for (index, wanted) in expected.iter().enumerate() {
                if !rest.any(|(_, item)| matches(wanted, item)) {
                    let closest = actual
                        .get(index)
                        .and_then(|item| find(wanted, item, &format!("{}[{index}]", here())));
                    return Some(closest.unwrap_or_else(|| {
                        format!("{} has nothing like {wanted} at {index} or after", here())
                    }));
                }
            }
            None
        }
        (Value::Number(expected), Value::Number(actual)) => (expected.as_f64() != actual.as_f64())
            .then(|| format!("{} is {actual}, not {expected}", here())),
        (expected, actual) if expected == actual => None,
        (expected, actual) => Some(format!("{} is {actual}, not {expected}", here())),
    }
}

#[cfg(test)]
/// Tests for loose matching.
mod tests {
    use serde_json::json;

    use super::{matches, mismatch};

    /// Extra keys and items are fine, missing or different ones are not.
    #[test]
    fn matches_loosely() {
        let actual = json!({ "actions": [
            { "type": "status", "text": "2 words" },
            { "type": "select", "selections": [{ "anchor": 1, "head": 2 }] },
        ], "extra": true });
        assert!(matches(
            &json!({ "actions": [{ "type": "select" }] }),
            &actual
        ));
        assert!(matches(
            &json!({ "actions": [{ "text": "2 words" }, { "type": "select" }] }),
            &actual
        ));
        assert!(!matches(
            &json!({ "actions": [{ "type": "select" }, { "text": "2 words" }] }),
            &actual
        ));
        assert_eq!(
            mismatch(&json!({ "actions": [{ "text": "3 words" }] }), &actual).as_deref(),
            Some("actions[0].text is \"2 words\", not \"3 words\"")
        );
        assert_eq!(
            mismatch(&json!({ "nope": 1 }), &actual).as_deref(),
            Some("nope is missing")
        );
        assert!(matches(&json!(1.0), &json!(1)));
    }
}
