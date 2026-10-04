//! A fake live stream chat under the file explorer that reacts to how you code.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use mog_tui::UiEvent;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    flair::{Flair, FlairContext, Placement, sidebar_frame},
    rng::Rng,
};

/// Who is in chat.
const USERS: &[&str] = &[
    "rustacean99",
    "segfault_sam",
    "borrowck_fan",
    "vim_refugee",
    "moglover42",
    "null_ptr",
    "tabs_gang",
    "cargo_cult",
    "linus_alt",
    "xX_coder_Xx",
    "force_pusher",
    "stackoverflower",
    "semicolon_sniper",
    "async_andy",
    "unwrap_enjoyer",
    "chmod777",
];

/// What chat says while you type fast.
const HYPE: &[&str] = &[
    "W",
    "he's cooking",
    "typing speed is crazy",
    "chat is this real",
    "LETS GO",
    "absolute cinema",
    "10x dev fr",
    "keyboard on fire",
    "how is he this fast",
    "pog",
];

/// What chat says after a save.
const SAVED: &[&str] = &[
    "ctrl+s gang",
    "saved, respect",
    "W save",
    "autosave is for cowards",
    "he actually saved",
    "commit it already",
];

/// What chat says when new errors show up.
const ERRORS: &[&str] = &[
    "L",
    "skill issue",
    "ratio",
    "bro forgot a semicolon",
    "F in chat",
    "compiler said no",
    "red squiggles lmao",
    "rip",
    "just use python",
    "have you tried turning it off",
];

/// What chat says when the errors are gone.
const FIXED: &[&str] = &[
    "W fix",
    "clutch",
    "LETS GOOO",
    "he fixed it",
    "compiler stays losing",
    "GG",
];

/// What chat says when you stop typing for a while.
const IDLE: &[&str] = &[
    "hello?",
    "is he afk",
    "zzz",
    "did he leave",
    "bro is thinking",
    "stream died?",
    "touch grass arc",
];

/// What chat says otherwise.
const AMBIENT: &[&str] = &[
    "what editor is this",
    "mog >>> vscode",
    "drop the dotfiles",
    "how is this in a terminal",
    "what language is this",
    "do a fizzbuzz",
    "lurking",
    "nice theme",
    "first",
    "hi from brazil",
    "who's here from the readme",
];

/// What chat says when a file is opened.
const OPENED: &[&str] = &[
    "new file just dropped",
    "what's in this one",
    "ooh secret file",
    "file reveal",
];

/// What chat says during a backspace spree.
const DELETING: &[&str] = &["delete it all", "backspace speedrun", "rewrite it in rust"];

/// How long without typing until chat thinks you left.
const IDLE_AFTER: Duration = Duration::from_secs(45);

/// Chars typed since the last message that count as cooking.
const HYPE_CHARS: usize = 40;

/// Deletes since the last message that count as a spree.
const DELETE_SPREE: usize = 25;

/// How many messages are kept.
const BACKLOG: usize = 32;

/// The fewest viewers, the ones who never leave.
const MIN_VIEWERS: f32 = 3.0;

/// Shows a stream chat with a viewer count.
#[derive(Debug)]
pub struct Stream {
    /// Messages as `(user, text)`, oldest first.
    messages: VecDeque<(usize, &'static str)>,
    /// When the last message came in.
    last_message: Instant,
    /// How long until the next message on its own.
    gap: Duration,
    /// When you last typed.
    last_typed: Instant,
    /// Chars typed since the last message.
    typed: usize,
    /// Deletes since the last message.
    deleted: usize,
    /// The error count at the last diagnostics event.
    errors: usize,
    /// How many people are watching.
    viewers: f32,
    /// Where usernames and lines come from.
    rng: Rng,
}

impl Default for Stream {
    fn default() -> Self {
        let mut stream = Self {
            messages: VecDeque::new(),
            last_message: Instant::now(),
            gap: Duration::from_secs(3),
            last_typed: Instant::now(),
            typed: 0,
            deleted: 0,
            errors: 0,
            viewers: 12.0,
            rng: Rng::default(),
        };
        stream.say(&["stream starting", "first"]);
        stream
    }
}

impl Stream {
    /// Goes live.
    pub fn new() -> Self {
        Self::default()
    }

    /// Has someone in chat say one of `lines`.
    fn say(&mut self, lines: &[&'static str]) {
        let user = self.rng.below(USERS.len());
        let line = *self.rng.pick(lines);
        if self.messages.len() == BACKLOG {
            self.messages.pop_front();
        }
        self.messages.push_back((user, line));
        self.last_message = Instant::now();
        self.gap = Duration::from_millis(1500 + u64::try_from(self.rng.below(4500)).unwrap_or(0));
        self.typed = 0;
        self.deleted = 0;
    }

    /// Lets chat talk on its own when it has been quiet long enough.
    fn chatter(&mut self) {
        if self.last_message.elapsed() < self.gap {
            return;
        }
        let idle = self.last_typed.elapsed() >= IDLE_AFTER;
        let target = if idle { MIN_VIEWERS } else { 1200.0 };
        // viewers drift toward a big crowd while you work and leave when you stop
        let rate = if idle { 0.15 } else { 0.03 };
        self.viewers += (target - self.viewers) * rate * self.rng.range(0.2, 1.0);
        self.viewers = self.viewers.max(MIN_VIEWERS);
        if self.typed >= HYPE_CHARS {
            self.say(HYPE);
        } else if self.deleted >= DELETE_SPREE {
            self.say(DELETING);
        } else if idle {
            self.say(IDLE);
        } else {
            self.say(AMBIENT);
        }
    }
}

/// Formats a viewer count like `1.2k`.
fn viewers_text(viewers: f32) -> String {
    let count = viewers.round();
    if count >= 1000.0 {
        format!("{:.1}k", count / 1000.0)
    } else {
        format!("{count:.0}")
    }
}

impl Flair for Stream {
    fn id(&self) -> &str {
        "stream"
    }

    fn description(&self) -> &str {
        "A fake stream chat under the file explorer that reacts to your coding."
    }

    fn placement(&self) -> Placement {
        Placement::Sidebar { height: 9 }
    }

    fn observe(&mut self, event: &UiEvent) {
        match event {
            UiEvent::Typed(_) => {
                self.typed += 1;
                self.last_typed = Instant::now();
            }
            UiEvent::Deleted => self.deleted += 1,
            UiEvent::Saved => self.say(SAVED),
            UiEvent::Opened => self.say(OPENED),
            UiEvent::Diagnostics { errors, .. } => {
                if *errors > self.errors {
                    self.say(ERRORS);
                } else if *errors == 0 && self.errors > 0 {
                    self.say(FIXED);
                }
                self.errors = *errors;
            }
            UiEvent::Activity => {}
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &FlairContext<'_>) {
        self.chatter();
        let theme = cx.theme;
        let p = theme.palette;
        let title = format!("\u{25cf} live  {} watching", viewers_text(self.viewers));
        let inner = sidebar_frame(area, buf, theme, &title);
        // the dot is the first char of the title, after the rule and a space
        if let Some(cell) = buf.cell_mut((area.x + 2, area.y)) {
            cell.set_fg(p.red).set_style(Modifier::BOLD);
        }
        let colors = [
            p.accent, p.cyan, p.green, p.yellow, p.orange, p.pink, p.purple, p.blue,
        ];
        let width = usize::from(inner.width);
        let rows = usize::from(inner.height);
        let shown = self.messages.len().min(rows);
        let start = self.messages.len() - shown;
        for (row, &(user, text)) in self.messages.iter().skip(start).enumerate() {
            let y = inner.y + u16::try_from(row).unwrap_or(0);
            let name = USERS[user];
            let style = theme
                .sidebar
                .patch(Style::new().fg(colors[user % colors.len()]))
                .add_modifier(Modifier::BOLD);
            let x = buf.set_stringn(inner.x, y, name, width, style).0;
            let used = name.width().min(width);
            let rest = width.saturating_sub(used);
            buf.set_stringn(x, y, format!(": {text}"), rest, theme.sidebar);
        }
    }
}

#[cfg(test)]
/// Tests for the stream chat.
mod tests {
    use mog_tui::UiEvent;

    use super::{ERRORS, FIXED, SAVED, Stream, viewers_text};
    use crate::flair::Flair;

    /// Saving and errors get a reaction right away.
    #[test]
    fn reacts_to_events() {
        let mut stream = Stream::new();
        stream.observe(&UiEvent::Saved);
        assert!(SAVED.contains(&stream.messages.back().expect("message").1));
        stream.observe(&UiEvent::Diagnostics {
            errors: 2,
            warnings: 0,
        });
        assert!(ERRORS.contains(&stream.messages.back().expect("message").1));
        stream.observe(&UiEvent::Diagnostics {
            errors: 0,
            warnings: 0,
        });
        assert!(FIXED.contains(&stream.messages.back().expect("message").1));
    }

    /// Big crowds are shown in thousands.
    #[test]
    fn formats_viewers() {
        assert_eq!(viewers_text(12.4), "12");
        assert_eq!(viewers_text(1234.0), "1.2k");
    }
}
