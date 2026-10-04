//! A popup showing the notes of a mog release from GitHub.

use std::mem;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Command, Key, KeyChord};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Modifier,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    compositor::{Context, EventResult, Layer},
    popup,
    ui::{Layout, Overlay, Ui},
};

/// The width of the popup.
const WIDTH: u16 = 84;

/// The height of the popup.
const HEIGHT: u16 = 26;

/// The command that installs the release being shown.
pub const UPDATE_COMMAND: &str = "update.install";

/// The command that opens the release being shown in the browser.
pub const OPEN_COMMAND: &str = "update.open";

/// The notes of one release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseNotes {
    /// The popup title, like `what's new in mog v0.2.0`.
    pub title: String,
    /// The release body, in GitHub markdown.
    pub body: String,
    /// The release page.
    pub url: String,
    /// Whether this release can be installed from the popup.
    pub can_update: bool,
}

/// What a line of notes is, for coloring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineKind {
    /// A markdown heading.
    Heading,
    /// The first line of a list item.
    Bullet,
    /// Anything else.
    Text,
}

/// Turns markdown links, emphasis and long GitHub links into plain readable text.
fn tidy(line: &str) -> String {
    let mut text = line.replace("**", "").replace('`', "");
    // [label](url) becomes label
    while let Some(open) = text.find('[') {
        let Some(mid) = text[open..].find("](").map(|at| open + at) else {
            break;
        };
        let Some(close) = text[mid..].find(')').map(|at| mid + at) else {
            break;
        };
        let label = text[open + 1..mid].to_owned();
        text.replace_range(open..=close, &label);
    }
    let words: Vec<String> = text
        .split(' ')
        .map(|word| {
            if let Some(at) = word.find("/pull/").filter(|_| word.starts_with("https://")) {
                format!("#{}", &word[at + "/pull/".len()..])
            } else if let Some(at) = word
                .find("/compare/")
                .filter(|_| word.starts_with("https://"))
            {
                word[at + "/compare/".len()..].to_owned()
            } else {
                word.to_owned()
            }
        })
        .collect();
    words.join(" ")
}

/// Wraps `text` to `width` columns, indenting lines after the first by `indent`.
fn wrap(text: &str, width: usize, indent: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let room = if lines.is_empty() {
            width
        } else {
            width.saturating_sub(indent)
        };
        if !line.is_empty() && line.width() + 1 + word.width() > room {
            lines.push(mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    let pad = " ".repeat(indent);
    lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| if i == 0 { line } else { format!("{pad}{line}") })
        .collect()
}

/// Lays out markdown `body` as lines at most `width` wide.
fn layout(body: &str, width: usize) -> Vec<(String, LineKind)> {
    let mut out = Vec::new();
    let mut blank = true;
    for raw in body.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            if !blank {
                out.push((String::new(), LineKind::Text));
            }
            blank = true;
            continue;
        }
        blank = false;
        if let Some(heading) = trimmed.strip_prefix('#') {
            out.push((
                tidy(heading.trim_start_matches('#').trim()),
                LineKind::Heading,
            ));
        } else if let Some(item) = trimmed
            .strip_prefix("* ")
            .or_else(|| trimmed.strip_prefix("- "))
        {
            for (i, line) in wrap(&format!("\u{2022} {}", tidy(item)), width, 2)
                .into_iter()
                .enumerate()
            {
                let kind = if i == 0 {
                    LineKind::Bullet
                } else {
                    LineKind::Text
                };
                out.push((line, kind));
            }
        } else {
            out.extend(
                wrap(&tidy(trimmed), width, 0)
                    .into_iter()
                    .map(|line| (line, LineKind::Text)),
            );
        }
    }
    while out.last().is_some_and(|(line, _)| line.is_empty()) {
        out.pop();
    }
    if out.is_empty() {
        out.push(("no notes for this release".into(), LineKind::Text));
    }
    out
}

/// The release notes popup.
#[derive(Debug, Default)]
pub struct ReleaseNotesPopup {
    /// The popup generation the scroll belongs to.
    generation: u64,
    /// The first line shown.
    scroll: usize,
    /// How many lines the notes have, from the last render.
    lines: usize,
    /// How many lines fit, from the last render.
    rows: usize,
    /// The popup box from the last render.
    area: Rect,
}

impl ReleaseNotesPopup {
    /// Creates the popup.
    pub fn new() -> Self {
        Self::default()
    }

    /// Scrolls by `delta` lines, staying inside the notes.
    fn scroll_by(&mut self, delta: isize) {
        let max = self.lines.saturating_sub(self.rows);
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
    }
}

impl Layer for ReleaseNotesPopup {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        if ui.overlay == Some(Overlay::ReleaseNotes) && ui.release_notes.is_some() {
            layout.screen
        } else {
            Rect::default()
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let Some(notes) = &cx.ui.release_notes else {
            return;
        };
        if self.generation != cx.ui.overlay_generation {
            self.generation = cx.ui.overlay_generation;
            self.scroll = 0;
        }
        let theme = cx.theme;
        self.area = popup::centered(area, WIDTH, HEIGHT);
        popup::dim_around(area, self.area, buf, theme);
        let inner = popup::frame(self.area, buf, theme, &notes.title);
        if inner.height < 3 || inner.width < 10 {
            return;
        }
        let text = Rect {
            x: inner.x + 1,
            width: inner.width - 2,
            height: inner.height - 2,
            ..inner
        };
        let lines = layout(&notes.body, usize::from(text.width));
        self.lines = lines.len();
        self.rows = usize::from(text.height);
        self.scroll_by(0);
        for (row, (line, kind)) in lines.iter().skip(self.scroll).take(self.rows).enumerate() {
            let y = text.y + u16::try_from(row).unwrap_or(0);
            let style = match kind {
                LineKind::Heading => theme.popup_title.add_modifier(Modifier::UNDERLINED),
                LineKind::Bullet | LineKind::Text => theme.popup,
            };
            buf.set_stringn(text.x, y, line, usize::from(text.width), style);
            if *kind == LineKind::Bullet {
                buf.set_string(text.x, y, "\u{2022}", theme.popup_match);
            }
        }
        let mut help = String::new();
        if notes.can_update {
            help.push_str("u update now  ");
        }
        help.push_str("o open on github  esc close");
        if self.lines > self.rows {
            let end = (self.scroll + self.rows).min(self.lines);
            help.push_str(&format!("  {end}/{}", self.lines));
        }
        let help_y = inner.bottom() - 1;
        buf.set_stringn(
            text.x,
            help_y,
            help,
            usize::from(text.width),
            theme.popup_dim,
        );
    }

    fn handle_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        if cx.ui.overlay != Some(Overlay::ReleaseNotes) {
            return EventResult::Ignored;
        }
        let page = isize::try_from(self.rows.max(1)).unwrap_or(1);
        let can_update = cx.ui.release_notes.as_ref().is_some_and(|n| n.can_update);
        match chord.key {
            Key::Esc | Key::Enter | Key::Char('q') => cx.ui.close(),
            Key::Up => self.scroll_by(-1),
            Key::Down => self.scroll_by(1),
            Key::PageUp => self.scroll_by(-page),
            Key::PageDown | Key::Char(' ') => self.scroll_by(page),
            Key::Home => self.scroll = 0,
            Key::End => self.scroll_by(isize::MAX),
            Key::Char('u') if can_update => {
                cx.ui.close();
                cx.ui.request(Command::Custom(UPDATE_COMMAND.into()));
            }
            Key::Char('o') => cx.ui.request(Command::Custom(OPEN_COMMAND.into())),
            _ if chord.mods.ctrl || chord.mods.alt => return EventResult::Ignored,
            _ => {}
        }
        EventResult::Consumed
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        let inside = self.area.contains(Position::new(event.column, event.row));
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if !inside => cx.ui.close(),
            MouseEventKind::ScrollUp => self.scroll_by(-3),
            MouseEventKind::ScrollDown => self.scroll_by(3),
            _ => {}
        }
        EventResult::Consumed
    }
}

#[cfg(test)]
/// Tests for release notes.
mod tests {
    use super::{LineKind, layout, tidy, wrap};

    /// Links, emphasis and long GitHub urls get short.
    #[test]
    fn tidies_markdown() {
        assert_eq!(
            tidy("feat: x by @sam in https://github.com/SpideyZac/mog/pull/12"),
            "feat: x by @sam in #12"
        );
        assert_eq!(
            tidy("**Full Changelog**: https://github.com/SpideyZac/mog/compare/v0.1.0...v0.2.0"),
            "Full Changelog: v0.1.0...v0.2.0"
        );
        assert_eq!(tidy("see [the docs](https://x.y) now"), "see the docs now");
    }

    /// Long lines wrap on words with the indent after the first line.
    #[test]
    fn wraps_words() {
        assert_eq!(
            wrap("one two three four", 9, 2),
            ["one two", "  three", "  four"]
        );
        assert_eq!(wrap("", 10, 0), [""]);
    }

    /// Headings and bullets are told apart and blank runs collapse.
    #[test]
    fn lays_out_notes() {
        let lines = layout("## What's Changed\n\n\n* feat: a\n- fix: b\n", 40);
        let kinds: Vec<LineKind> = lines.iter().map(|(_, kind)| *kind).collect();
        assert_eq!(
            kinds,
            [
                LineKind::Heading,
                LineKind::Text,
                LineKind::Bullet,
                LineKind::Bullet
            ]
        );
        assert_eq!(lines[0].0, "What's Changed");
        assert_eq!(lines[2].0, "\u{2022} feat: a");
    }
}
