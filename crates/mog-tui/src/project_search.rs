//! A popup for finding and replacing text across the whole project.

use std::path::Path;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Command, Key, KeyChord, project_search::ProjectResults, search::SearchOptions};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Modifier,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    compositor::{Context, EventResult, Layer},
    popup,
    search::{TOGGLES_WIDTH, Toggle, draw_toggles},
    ui::{Layout, Overlay, Ui},
};

/// The width of the popup.
const WIDTH: u16 = 110;

/// The height of the popup.
const HEIGHT: u16 = 32;

/// The command that searches again for the current query.
pub const RUN_COMMAND: &str = "project_search.run";

/// The command that opens the picked match.
pub const PICK_COMMAND: &str = "project_search.pick";

/// The command that asks to replace every match.
pub const REPLACE_ALL_COMMAND: &str = "project_search.replace_all";

/// The rows of the input area above the results.
const HEADER_ROWS: u16 = 4;

/// The find and replace state, shared with the app that runs searches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectSearchState {
    /// What to find.
    pub query: String,
    /// What to put in its place.
    pub replacement: String,
    /// Whether case has to match.
    pub case_sensitive: bool,
    /// Whether matches have to be whole words.
    pub whole_word: bool,
    /// Whether the query is a regular expression.
    pub regex: bool,
    /// Why the last search could not run, like a broken regex.
    pub error: Option<String>,
    /// Whether typing goes to the replacement instead of the query.
    pub replacing: bool,
    /// What the newest finished search found.
    pub results: ProjectResults,
    /// Bumped every time the query changes, so old searches can be told apart.
    pub generation: u64,
    /// Whether a search for the current generation is running.
    pub searching: bool,
    /// The match to open, an index into the results.
    pub picked: Option<usize>,
}

impl ProjectSearchState {
    /// Returns how the query matches.
    pub fn options(&self) -> SearchOptions {
        SearchOptions {
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            regex: self.regex,
        }
    }

    /// Switches `toggle` on or off.
    pub fn flip(&mut self, toggle: Toggle) {
        match toggle {
            Toggle::Case => self.case_sensitive = !self.case_sensitive,
            Toggle::Word => self.whole_word = !self.whole_word,
            Toggle::Regex => self.regex = !self.regex,
        }
    }

    /// Notes that the query changed and asks for a new search.
    pub fn changed(&mut self, ui_requests: &mut Vec<Command>) {
        self.generation += 1;
        self.searching = !self.query.is_empty();
        self.error = None;
        if self.query.is_empty() {
            self.results = ProjectResults::default();
        }
        ui_requests.push(Command::Custom(RUN_COMMAND.into()));
    }
}

/// One line of the results list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    /// The file the matches under it are in, with how many there are.
    File(usize, usize),
    /// A match, by index.
    Match(usize),
}

/// Lays out the results as file headers each followed by their matches.
fn lines(results: &ProjectResults) -> Vec<Line> {
    let mut out = Vec::new();
    let matches = &results.matches;
    let mut at = 0;
    while at < matches.len() {
        let path = &matches[at].path;
        let count = matches[at..]
            .iter()
            .take_while(|other| other.path == *path)
            .count();
        out.push(Line::File(at, count));
        out.extend((at..at + count).map(Line::Match));
        at += count;
    }
    out
}

/// The project find and replace popup.
#[derive(Debug, Default)]
pub struct ProjectSearchPanel {
    /// The popup generation the selection belongs to.
    generation: u64,
    /// The selected match.
    selected: usize,
    /// The first results line shown.
    scroll: usize,
    /// The results area from the last render.
    list: Rect,
    /// The match on each results row from the last render.
    rows: Vec<(u16, usize)>,
    /// Where the option toggles were drawn.
    toggles: Vec<(Rect, Toggle)>,
    /// The popup box from the last render.
    area: Rect,
}

impl ProjectSearchPanel {
    /// Creates the popup.
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves the selection by `delta` matches.
    fn select_by(&mut self, delta: isize, count: usize) {
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(count.saturating_sub(1));
    }

    /// Opens the selected match.
    fn pick(&self, cx: &mut Context<'_>) {
        if self.selected < cx.ui.project_search.results.matches.len() {
            cx.ui.project_search.picked = Some(self.selected);
            cx.ui.request(Command::Custom(PICK_COMMAND.into()));
        }
    }

    /// Returns `path` relative to the project root for display.
    fn show_path(path: &Path, root: &Path) -> String {
        path.strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string()
            .replace('\\', "/")
    }
}

impl Layer for ProjectSearchPanel {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        if ui.overlay == Some(Overlay::ProjectSearch) {
            layout.screen
        } else {
            Rect::default()
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        if self.generation != cx.ui.overlay_generation {
            self.generation = cx.ui.overlay_generation;
            self.selected = 0;
            self.scroll = 0;
        }
        let theme = cx.theme;
        let state = &cx.ui.project_search;
        self.area = popup::centered(area, WIDTH, HEIGHT);
        popup::dim_around(area, self.area, buf, theme);
        let inner = popup::frame(self.area, buf, theme, "\u{2315} find in project");
        if inner.height <= HEADER_ROWS + 1 || inner.width < 20 {
            return;
        }
        let width = usize::from(inner.width);
        for (row, (label, text, active)) in [
            ("find   ", &state.query, !state.replacing),
            ("replace", &state.replacement, state.replacing),
        ]
        .into_iter()
        .enumerate()
        {
            let y = inner.y + u16::try_from(row).unwrap_or(0);
            let style = if active {
                theme.popup_title
            } else {
                theme.popup_dim
            };
            let x = buf
                .set_stringn(inner.x, y, format!("{label} \u{276f} "), width, style)
                .0;
            let room = usize::from(inner.right().saturating_sub(x + TOGGLES_WIDTH + 1));
            buf.set_stringn(x, y, text, room, theme.popup);
        }
        let toggles_x = inner.right().saturating_sub(TOGGLES_WIDTH);
        self.toggles = draw_toggles(buf, toggles_x, inner.y, state.options(), theme);
        let matches = &state.results.matches;
        let count = matches.len();
        self.selected = self.selected.min(count.saturating_sub(1));
        let summary = if let Some(error) = &state.error {
            format!("bad regex: {error}")
        } else if state.query.is_empty() {
            "type to search every file in the project".to_owned()
        } else if state.searching {
            "searching\u{2026}".to_owned()
        } else if count == 0 {
            "no matches".to_owned()
        } else {
            let more = if state.results.truncated { "+" } else { "" };
            format!("{count}{more} matches in {} files", state.results.files)
        };
        let help = format!(
            "{summary}   alt+c case  alt+w word  alt+r regex  tab switch  enter open  alt+enter replace all"
        );
        let help_style = if state.error.is_some() {
            theme.error
        } else {
            theme.popup_dim
        };
        buf.set_stringn(inner.x, inner.y + 2, help, width, help_style);
        let rule = "\u{2500}".repeat(width);
        buf.set_stringn(inner.x, inner.y + 3, rule, width, theme.popup_border);
        self.list = Rect {
            y: inner.y + HEADER_ROWS,
            height: inner.height - HEADER_ROWS,
            ..inner
        };
        let all = lines(&state.results);
        let rows = usize::from(self.list.height);
        // keep the selected match on screen
        if let Some(at) = all
            .iter()
            .position(|line| *line == Line::Match(self.selected))
        {
            if at < self.scroll {
                self.scroll = at.saturating_sub(1);
            } else if at >= self.scroll + rows {
                self.scroll = at + 1 - rows;
            }
        }
        self.scroll = self.scroll.min(all.len().saturating_sub(rows));
        self.rows.clear();
        for (row, line) in all.iter().skip(self.scroll).take(rows).enumerate() {
            let y = self.list.y + u16::try_from(row).unwrap_or(0);
            match *line {
                Line::File(first, count) => {
                    let path = Self::show_path(&matches[first].path, &cx.ui.root);
                    let text = format!("{path}  {count}");
                    buf.set_stringn(
                        inner.x,
                        y,
                        text,
                        width,
                        theme.directory.add_modifier(Modifier::BOLD),
                    );
                }
                Line::Match(index) => {
                    self.rows.push((y, index));
                    let found = &matches[index];
                    let selected = index == self.selected;
                    let base = if selected {
                        theme.popup_selected
                    } else {
                        theme.popup
                    };
                    buf.set_style(Rect::new(inner.x, y, inner.width, 1), base);
                    let number = format!("  {:>5}  ", found.line + 1);
                    let x = buf
                        .set_stringn(inner.x, y, &number, width, base.patch(theme.popup_dim))
                        .0;
                    let room = usize::from(inner.right().saturating_sub(x));
                    // trim leading space and keep the match in view on long lines
                    let indent = found.preview.len() - found.preview.trim_start().len();
                    let start = found
                        .preview_column
                        .min(indent)
                        .max((found.preview_column + found.len + 8).saturating_sub(room));
                    let shown: String = found.preview.chars().skip(start).collect();
                    let end = buf.set_stringn(x, y, &shown, room, base).0;
                    let hit_x = x + u16::try_from(found.preview_column - start).unwrap_or(0);
                    let hit: String = found
                        .preview
                        .chars()
                        .skip(found.preview_column)
                        .take(found.len)
                        .collect();
                    let hit_room = usize::from(end.saturating_sub(hit_x));
                    if hit_x < end && hit.width() > 0 {
                        buf.set_stringn(
                            hit_x,
                            y,
                            hit,
                            hit_room,
                            base.patch(theme.popup_match)
                                .add_modifier(Modifier::UNDERLINED),
                        );
                    }
                }
            }
        }
    }

    fn handle_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        if cx.ui.overlay != Some(Overlay::ProjectSearch) {
            return EventResult::Ignored;
        }
        let count = cx.ui.project_search.results.matches.len();
        let page = isize::try_from(usize::from(self.list.height.max(1))).unwrap_or(1);
        let state = &mut cx.ui.project_search;
        match chord.key {
            Key::Esc => cx.ui.close(),
            Key::Tab => state.replacing = !state.replacing,
            Key::Up => self.select_by(-1, count),
            Key::Down => self.select_by(1, count),
            Key::PageUp => self.select_by(-page, count),
            Key::PageDown => self.select_by(page, count),
            Key::Enter if chord.mods.alt || chord.mods.ctrl => {
                cx.ui.request(Command::Custom(REPLACE_ALL_COMMAND.into()));
            }
            Key::Enter => self.pick(cx),
            Key::Char(key @ ('c' | 'w' | 'r')) if chord.mods.alt && !chord.mods.ctrl => {
                state.flip(match key {
                    'c' => Toggle::Case,
                    'w' => Toggle::Word,
                    _ => Toggle::Regex,
                });
                state.changed(&mut cx.ui.requests);
            }
            Key::Backspace => {
                let replacing = state.replacing;
                let field = if replacing {
                    &mut state.replacement
                } else {
                    &mut state.query
                };
                if chord.mods.ctrl {
                    field.clear();
                } else {
                    field.pop();
                }
                if !replacing {
                    self.selected = 0;
                    state.changed(&mut cx.ui.requests);
                }
            }
            _ => match chord.typed_char() {
                Some(ch) => {
                    if state.replacing {
                        state.replacement.push(ch);
                    } else {
                        state.query.push(ch);
                        self.selected = 0;
                        state.changed(&mut cx.ui.requests);
                    }
                }
                None if chord.mods.ctrl || chord.mods.alt => return EventResult::Ignored,
                None => {}
            },
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
        let count = cx.ui.project_search.results.matches.len();
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if !inside => cx.ui.close(),
            MouseEventKind::Down(MouseButton::Left) => {
                let point = Position::new(event.column, event.row);
                if let Some(&(_, toggle)) =
                    self.toggles.iter().find(|(rect, _)| rect.contains(point))
                {
                    let state = &mut cx.ui.project_search;
                    state.flip(toggle);
                    state.changed(&mut cx.ui.requests);
                } else if let Some(&(_, index)) = self.rows.iter().find(|(y, _)| *y == event.row) {
                    self.selected = index;
                    self.pick(cx);
                } else if event.row == self.area.y + 1 || event.row == self.area.y + 2 {
                    cx.ui.project_search.replacing = event.row == self.area.y + 2;
                }
            }
            MouseEventKind::ScrollUp => self.select_by(-3, count),
            MouseEventKind::ScrollDown => self.select_by(3, count),
            _ => {}
        }
        EventResult::Consumed
    }

    fn cursor(&self, _area: Rect, cx: &Context<'_>) -> Option<Position> {
        let state = &cx.ui.project_search;
        let (row, text) = if state.replacing {
            (1, &state.replacement)
        } else {
            (0, &state.query)
        };
        // the label and prompt take 10 cells
        let x = self.area.x + 1 + 10 + u16::try_from(text.width()).unwrap_or(0);
        let at = Position::new(x, self.area.y + 1 + row);
        self.area.contains(at).then_some(at)
    }
}

#[cfg(test)]
/// Tests for project search.
mod tests {
    use std::path::PathBuf;

    use mog_core::project_search::{ProjectMatch, ProjectResults};

    use super::{Line, lines};

    /// Returns a match in `file` on `line`.
    fn found(file: &str, line: usize) -> ProjectMatch {
        ProjectMatch {
            path: PathBuf::from(file),
            line,
            column: 0,
            preview_column: 0,
            len: 1,
            preview: "x".into(),
        }
    }

    /// Matches are grouped under a header for their file.
    #[test]
    fn groups_by_file() {
        let results = ProjectResults {
            matches: vec![found("a", 1), found("a", 2), found("b", 1)],
            files: 2,
            truncated: false,
        };
        assert_eq!(
            lines(&results),
            [
                Line::File(0, 2),
                Line::Match(0),
                Line::Match(1),
                Line::File(2, 1),
                Line::Match(2)
            ]
        );
    }
}
