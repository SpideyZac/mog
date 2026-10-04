//! How many FBI agents are watching you type.

use mog_tui::{Segment, Side};
use ratatui::style::{Color, Modifier, Style};

use crate::flair::{Flair, FlairContext, Placement};

/// Words that get you put on a list.
const SUS_WORDS: &[&str] = &[
    "hack",
    "exploit",
    "password",
    "passwd",
    "secret",
    "token",
    "sudo",
    "rm -rf",
    "unsafe",
    "bitcoin",
    "crypto",
    "payload",
    "backdoor",
    "keylog",
    "inject",
    "bypass",
    "mainframe",
    "transmute",
    "0xdeadbeef",
    "/etc/shadow",
];

/// How many agents is the whole bureau.
const WHOLE_BUREAU: usize = 99;

/// Returns how many agents `text` attracts. There is always one, just in case.
pub fn agents(text: &str) -> usize {
    let lower = text.to_lowercase();
    let hits: usize = SUS_WORDS
        .iter()
        .map(|word| lower.matches(word).count())
        .sum();
    (1 + hits).min(WHOLE_BUREAU)
}

/// Shows how many FBI agents are watching, more for sus code.
#[derive(Debug, Default)]
pub struct Fbi {
    /// The document the count is for, as `(index, version)`, and the count.
    cached: Option<((usize, u64), usize)>,
}

impl Fbi {
    /// Creates the counter.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Flair for Fbi {
    fn id(&self) -> &str {
        "fbi"
    }

    fn description(&self) -> &str {
        "Counts the FBI agents watching. Writing unsafe or password brings more."
    }

    fn placement(&self) -> Placement {
        Placement::Status(Side::Right)
    }

    fn segment(&mut self, cx: &FlairContext<'_>) -> Option<Segment> {
        let document = cx.editor.document();
        let key = (cx.editor.active(), document.version());
        let count = match self.cached {
            Some((cached_key, count)) if cached_key == key => count,
            _ => {
                let count = agents(&document.text().to_string());
                self.cached = Some((key, count));
                count
            }
        };
        let p = cx.theme.palette;
        let bg = cx.theme.status.bg.unwrap_or(Color::Reset);
        let color = match count {
            1 => p.dim,
            2..=5 => p.yellow,
            _ => p.red,
        };
        let text = match count {
            1 => "1 fed watching".to_owned(),
            WHOLE_BUREAU => "the whole fbi is watching".to_owned(),
            n => format!("{n} feds watching"),
        };
        Some(Segment {
            parts: vec![
                (
                    "\u{25c9} ".into(),
                    Style::new().fg(color).bg(bg).add_modifier(Modifier::BOLD),
                ),
                (text, Style::new().fg(color).bg(bg)),
            ],
            side: Side::Right,
        })
    }
}

#[cfg(test)]
/// Tests for the FBI counter.
mod tests {
    use super::agents;

    /// Clean code gets one agent and sus code gets more.
    #[test]
    fn counts_agents() {
        assert_eq!(agents("fn main() {}"), 1);
        assert_eq!(agents("unsafe { hack_the_mainframe(PASSWORD) }"), 5);
        assert_eq!(agents(&"hack ".repeat(500)), 99);
    }
}
