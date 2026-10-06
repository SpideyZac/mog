//! Conditions like `language == md && selection`, saying when something applies, like a plugin
//! command in the right click menu.
//!
//! A condition is made of names, compared to a value with `==` or `!=` or on their own, which
//! is true when the name has a value that is not empty or `false`. `!`, `&&`, `||` and
//! parentheses combine them, `&&` binding tighter than `||`. Values are single words or quoted.

use std::{
    error::Error,
    fmt,
    iter::Peekable,
    str::{Chars, FromStr},
};

/// A parsed condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct When {
    /// The expression.
    expr: Expr,
    /// The text it was read from.
    source: String,
}

/// A part of a condition.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    /// True when the name has a value that is not empty or `false`.
    Set(String),
    /// True when the name has this value.
    Is(String, String),
    /// True when the name does not have this value.
    IsNot(String, String),
    /// True when the inner one is not.
    Not(Box<Expr>),
    /// True when both are.
    And(Box<Expr>, Box<Expr>),
    /// True when either is.
    Or(Box<Expr>, Box<Expr>),
}

/// A piece of a condition's text.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// A name or a bare value.
    Word(String),
    /// A quoted value.
    Quoted(String),
    /// `==`.
    Eq,
    /// `!=`.
    Ne,
    /// `!`.
    Not,
    /// `&&`.
    And,
    /// `||`.
    Or,
    /// `(`.
    Open,
    /// `)`.
    Close,
}

/// Why a condition could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhenError(String);

impl fmt::Display for WhenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for WhenError {}

/// Returns whether `ch` can be part of a word.
fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | '*' | ':')
}

/// Splits `text` into tokens.
fn tokens(text: &str) -> Result<Vec<Token>, WhenError> {
    let mut found = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        let two = |next: char, chars: &mut Peekable<Chars<'_>>| chars.next_if_eq(&next).is_some();
        let token = match ch {
            ch if ch.is_whitespace() => continue,
            '(' => Token::Open,
            ')' => Token::Close,
            '=' if two('=', &mut chars) => Token::Eq,
            '!' if two('=', &mut chars) => Token::Ne,
            '!' => Token::Not,
            '&' if two('&', &mut chars) => Token::And,
            '|' if two('|', &mut chars) => Token::Or,
            '"' | '\'' => {
                let mut value = String::new();
                loop {
                    match chars.next() {
                        Some(end) if end == ch => break,
                        Some(inside) => value.push(inside),
                        None => return Err(WhenError(format!("`{text}` has an unclosed quote"))),
                    }
                }
                Token::Quoted(value)
            }
            ch if is_word(ch) => {
                let mut word = String::from(ch);
                while let Some(next) = chars.next_if(|next| is_word(*next)) {
                    word.push(next);
                }
                Token::Word(word)
            }
            other => {
                return Err(WhenError(format!(
                    "`{text}` has `{other}`, which means nothing"
                )));
            }
        };
        found.push(token);
    }
    Ok(found)
}

/// Reads tokens into an expression.
struct Parser {
    /// The tokens, reversed so the next one pops off the end.
    tokens: Vec<Token>,
}

impl Parser {
    /// Reads `a || b || ...`.
    fn or(&mut self) -> Result<Expr, String> {
        let mut expr = self.and()?;
        while self.tokens.last() == Some(&Token::Or) {
            self.tokens.pop();
            expr = Expr::Or(Box::new(expr), Box::new(self.and()?));
        }
        Ok(expr)
    }

    /// Reads `a && b && ...`.
    fn and(&mut self) -> Result<Expr, String> {
        let mut expr = self.unary()?;
        while self.tokens.last() == Some(&Token::And) {
            self.tokens.pop();
            expr = Expr::And(Box::new(expr), Box::new(self.unary()?));
        }
        Ok(expr)
    }

    /// Reads `!a`, `(a)`, `name`, `name == value` or `name != value`.
    fn unary(&mut self) -> Result<Expr, String> {
        match self.tokens.pop() {
            Some(Token::Not) => Ok(Expr::Not(Box::new(self.unary()?))),
            Some(Token::Open) => {
                let expr = self.or()?;
                match self.tokens.pop() {
                    Some(Token::Close) => Ok(expr),
                    _ => Err("a `(` is never closed".into()),
                }
            }
            Some(Token::Word(name)) => {
                let negated = match self.tokens.last() {
                    Some(Token::Eq) => false,
                    Some(Token::Ne) => true,
                    _ => return Ok(Expr::Set(name)),
                };
                self.tokens.pop();
                let value = match self.tokens.pop() {
                    Some(Token::Word(value) | Token::Quoted(value)) => value,
                    _ => return Err(format!("`{name}` is compared to nothing")),
                };
                Ok(if negated {
                    Expr::IsNot(name, value)
                } else {
                    Expr::Is(name, value)
                })
            }
            Some(other) => Err(format!("{other:?} is in the wrong place")),
            None => Err("it ends too early".into()),
        }
    }
}

impl FromStr for When {
    type Err = WhenError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut found = tokens(text)?;
        found.reverse();
        let mut parser = Parser { tokens: found };
        let expr = parser
            .or()
            .map_err(|why| WhenError(format!("`{text}`: {why}")))?;
        if !parser.tokens.is_empty() {
            return Err(WhenError(format!("`{text}` has more after the condition")));
        }
        Ok(Self {
            expr,
            source: text.to_owned(),
        })
    }
}

impl fmt::Display for When {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

impl Expr {
    /// Returns whether the expression holds for the values `value` gives names.
    fn holds(&self, value: &dyn Fn(&str) -> Option<String>) -> bool {
        match self {
            Self::Set(name) => {
                value(name).is_some_and(|value| !value.is_empty() && value != "false")
            }
            Self::Is(name, wanted) => value(name).as_deref() == Some(wanted.as_str()),
            Self::IsNot(name, wanted) => value(name).as_deref() != Some(wanted.as_str()),
            Self::Not(inner) => !inner.holds(value),
            Self::And(a, b) => a.holds(value) && b.holds(value),
            Self::Or(a, b) => a.holds(value) || b.holds(value),
        }
    }
}

impl When {
    /// Returns whether the condition holds, `value` giving the value of each name, or `None` for
    /// one that has none.
    pub fn holds(&self, value: &dyn Fn(&str) -> Option<String>) -> bool {
        self.expr.holds(value)
    }
}

#[cfg(test)]
/// Tests for conditions.
mod tests {
    use super::When;

    /// Checks `text` against a markdown file with a selection.
    fn check(text: &str) -> bool {
        let when: When = text.parse().expect("valid");
        when.holds(&|name| match name {
            "language" => Some("md".into()),
            "selection" => Some("true".into()),
            "modified" => Some("false".into()),
            "name" => Some("notes.md".into()),
            _ => None,
        })
    }

    /// Names, comparisons, negation, grouping and precedence all work.
    #[test]
    fn evaluates() {
        assert!(check("language == md"));
        assert!(check("language != rs"));
        assert!(check("selection"));
        assert!(!check("modified"));
        assert!(!check("untitled"));
        assert!(check("!modified && selection"));
        assert!(check("language == rs || language == md && selection"));
        assert!(!check("(language == rs || language == md) && modified"));
        assert!(check("name == 'notes.md'"));
        assert!(check("name == \"notes.md\""));
    }

    /// Broken conditions say what is wrong.
    #[test]
    fn refuses_nonsense() {
        for text in [
            "",
            "language ==",
            "(selection",
            "a && ",
            "a b",
            "'open",
            "a = b",
            "a @ b",
        ] {
            assert!(text.parse::<When>().is_err(), "{text}");
        }
    }
}
