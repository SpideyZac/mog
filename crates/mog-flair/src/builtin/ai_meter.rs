//! A completely unscientific guess at how much of a file an AI wrote.

use mog_tui::{Segment, Side};
use ratatui::style::{Color, Style};

use crate::flair::{Flair, FlairContext, Placement};

/// Phrases models love, with how suspicious each one is.
const PHRASES: &[(&str, f32)] = &[
    ("delve", 6.0),
    ("as an ai", 12.0),
    ("i hope this helps", 10.0),
    ("certainly!", 6.0),
    ("great question", 8.0),
    ("it's worth noting", 5.0),
    ("it is worth noting", 5.0),
    ("tapestry", 6.0),
    ("in conclusion", 4.0),
    ("let's dive", 5.0),
    ("seamless", 2.5),
    ("robust", 1.5),
    ("leverage", 2.0),
    ("comprehensive", 2.0),
    ("furthermore", 2.0),
    ("moreover", 2.0),
    ("streamline", 2.0),
    ("utilize", 1.5),
    ("ensure that", 1.0),
    ("elevate", 2.5),
    ("pivotal", 3.0),
    ("intricate", 2.5),
    ("realm", 2.5),
    ("unlock", 1.5),
    ("harness", 1.5),
    ("foster", 2.0),
    ("crucial", 1.5),
    ("showcase", 2.0),
    ("here's", 1.5),
    ("absolutely", 1.5),
    ("step 1", 2.5),
    ("your_api_key", 5.0),
    ("todo: implement", 3.0),
    ("example.com", 1.0),
    ("you're absolutely right", 15.0),
];

/// Chars that give it away when they turn up in code.
const TELLS: &[(char, f32)] = &[
    ('\u{2014}', 3.0),
    ('\u{2019}', 1.0),
    ('\u{201c}', 1.0),
    ('\u{2705}', 4.0),
    ('\u{1f680}', 4.0),
    ('\u{2728}', 4.0),
    ('\u{1f389}', 3.0),
    ('\u{1f525}', 3.0),
    ('\u{1f4a1}', 3.0),
];

/// How many points it takes to be about 63 percent sure.
const SATURATION: f32 = 18.0;

/// Returns a made up percentage of how AI generated `text` looks.
pub fn ai_score(text: &str) -> u8 {
    if text.trim().is_empty() {
        return 0;
    }
    let lower = text.to_lowercase();
    let mut points = 0.0;
    for (phrase, weight) in PHRASES {
        points += lower.matches(phrase).count() as f32 * weight;
    }
    for ch in text.chars() {
        if let Some((_, weight)) = TELLS.iter().find(|(tell, _)| *tell == ch) {
            points += weight;
        }
    }
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let comments = lines
        .iter()
        .filter(|line| {
            ["//", "#", "--", "/*", "*", "\"\"\""]
                .iter()
                .any(|p| line.starts_with(p))
        })
        .count();
    let ratio = comments as f32 / lines.len().max(1) as f32;
    // explaining every single line is a classic
    if ratio > 0.35 {
        points += (ratio - 0.35) * 40.0;
    }
    let shouty = lines
        .iter()
        .filter(|line| line.starts_with("//") && line.ends_with('!'))
        .count();
    points += shouty as f32 * 1.5;
    let score = 100.0 * (1.0 - (-points / SATURATION).exp());
    // the clamp keeps the cast lossless
    score.clamp(0.0, 100.0).round() as u8
}

/// Returns a label for a score.
fn verdict(score: u8) -> &'static str {
    match score {
        0..=10 => "organic",
        11..=30 => "free range",
        31..=55 => "sus",
        56..=80 => "copilot vibes",
        _ => "certified slop",
    }
}

/// Shows a little meter of how AI generated the focused file looks.
#[derive(Debug, Default)]
pub struct AiMeter {
    /// The document the score is for, as `(index, version)`, and the score.
    cached: Option<((usize, u64), u8)>,
}

impl AiMeter {
    /// Creates the meter.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Flair for AiMeter {
    fn id(&self) -> &str {
        "ai_meter"
    }

    fn description(&self) -> &str {
        "Guesses how AI generated the file is. Totally scientific."
    }

    fn placement(&self) -> Placement {
        Placement::Status(Side::Right)
    }

    fn segment(&mut self, cx: &FlairContext<'_>) -> Option<Segment> {
        let document = cx.editor.document();
        let key = (cx.editor.active(), document.version());
        let score = match self.cached {
            Some((cached_key, score)) if cached_key == key => score,
            _ => {
                let score = ai_score(&document.text().to_string());
                self.cached = Some((key, score));
                score
            }
        };
        let p = cx.theme.palette;
        let color = match score {
            0..=30 => p.green,
            31..=55 => p.yellow,
            56..=80 => p.orange,
            _ => p.red,
        };
        let filled = usize::from(score.div_ceil(20));
        let bar: String = (0..5)
            .map(|i| if i < filled { '\u{25b0}' } else { '\u{25b1}' })
            .collect();
        let bg = cx.theme.status.bg.unwrap_or(Color::Reset);
        let style = Style::new().fg(color).bg(bg);
        Some(Segment {
            parts: vec![
                ("ai ".into(), cx.theme.status),
                (bar, style),
                (format!(" {score}% {}", verdict(score)), style),
            ],
            side: Side::Right,
        })
    }
}

#[cfg(test)]
/// Tests for the AI meter.
mod tests {
    use super::ai_score;

    /// Plain code scores low and buzzword soup scores high.
    #[test]
    fn scores_make_sense() {
        let human = "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n";
        assert!(ai_score(human) < 10, "{}", ai_score(human));
        let slop = "// Certainly! Let's delve into this robust, seamless solution \u{2014} \
                    it's worth noting that we leverage a comprehensive tapestry \u{1f680}\n";
        assert!(ai_score(slop) > 80, "{}", ai_score(slop));
        assert_eq!(ai_score("   "), 0);
    }
}
