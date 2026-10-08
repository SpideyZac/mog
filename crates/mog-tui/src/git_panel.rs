//! The source control panel: changed files on the left, the diff of the picked one on the right.

use std::path::{Path, PathBuf};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Command, Key, KeyChord};
use mog_git::{FileChange, FileStatus};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Style,
};

use crate::{
    compositor::{Context, EventResult, Layer},
    popup,
    theme::Theme,
    ui::{Focus, Layout, Overlay, Ui},
};

/// The command that loads the diff of the highlighted file.
pub const DIFF_COMMAND: &str = "git.panel.diff";

/// The command that stages or unstages the highlighted file.
pub const TOGGLE_COMMAND: &str = "git.panel.toggle";

/// The command that stages every change.
pub const STAGE_ALL_COMMAND: &str = "git.panel.stage_all";

/// The command that opens the highlighted file in the editor.
pub const OPEN_COMMAND: &str = "git.panel.open";

/// The command that reads the changed files again.
pub const REFRESH_COMMAND: &str = "git.panel.refresh";

/// The widest the file list gets.
const LIST_WIDTH: u16 = 40;

/// How many diff lines a page key scrolls.
const PAGE: usize = 15;

/// What the panel says at the bottom.
const HINTS: &str =
    "space stage or unstage  a stage all  c commit  enter open  r refresh  esc close";

/// One changed file in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRow {
    /// The file.
    pub path: PathBuf,
    /// Whether this row is the staged part of the file.
    pub staged: bool,
    /// How it changed.
    pub status: FileStatus,
}

/// What the source control panel shows.
#[derive(Debug, Clone, Default)]
pub struct GitPanelState {
    /// The changed files, staged rows first.
    pub rows: Vec<GitRow>,
    /// The highlighted row.
    pub selected: usize,
    /// The diff of the highlighted row, a line per entry.
    pub diff: Vec<String>,
    /// The row the diff is for, as `(file, staged)`.
    pub diff_for: Option<(PathBuf, bool)>,
    /// How far the diff is scrolled.
    pub scroll: usize,
    /// Whether the list has been read at least once.
    pub loaded: bool,
}

impl GitPanelState {
    /// Replaces the list with `changes`, keeping the highlight on the same row if it is still
    /// there.
    pub fn set_changes(&mut self, changes: Vec<FileChange>) {
        let current = self.current().cloned();
        let staged = changes.iter().filter_map(|change| {
            change.staged.map(|status| GitRow {
                path: change.path.clone(),
                staged: true,
                status,
            })
        });
        let unstaged = changes.iter().filter_map(|change| {
            change.unstaged.map(|status| GitRow {
                path: change.path.clone(),
                staged: false,
                status,
            })
        });
        self.rows = staged.chain(unstaged).collect();
        self.loaded = true;
        self.selected = current
            .and_then(|current| {
                self.rows
                    .iter()
                    .position(|row| row.path == current.path && row.staged == current.staged)
                    .or_else(|| self.rows.iter().position(|row| row.path == current.path))
            })
            .unwrap_or(0)
            .min(self.rows.len().saturating_sub(1));
    }

    /// Returns the highlighted row.
    pub fn current(&self) -> Option<&GitRow> {
        self.rows.get(self.selected)
    }

    /// Returns whether the shown diff is for the highlighted row.
    pub fn diff_is_current(&self) -> bool {
        self.current().map(|row| (row.path.clone(), row.staged)) == self.diff_for
    }

    /// Stores the diff `text` of `path`, staged or not.
    pub fn set_diff(&mut self, path: PathBuf, staged: bool, text: &str) {
        if self.diff_for.as_ref() != Some(&(path.clone(), staged)) {
            self.scroll = 0;
        }
        self.diff = text.lines().map(str::to_owned).collect();
        self.diff_for = Some((path, staged));
    }
}

/// Returns the letter and style for `status`.
pub(crate) fn status_mark(status: FileStatus, theme: &Theme) -> (&'static str, Style) {
    match status {
        FileStatus::Modified => ("M", theme.git_modified),
        FileStatus::Added => ("A", theme.git_added),
        FileStatus::Untracked => ("U", theme.git_added),
        FileStatus::Deleted => ("D", theme.git_removed),
        FileStatus::Renamed => ("R", theme.git_modified),
        FileStatus::Conflicted => ("!", theme.error),
    }
}

/// Returns `path` relative to `root` with `/` separators.
pub(crate) fn relative(path: &Path, root: &Path) -> String {
    let parts: Vec<String> = path
        .strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

/// The source control panel layer.
#[derive(Debug, Default)]
pub struct GitPanel {
    /// The whole panel from the last frame.
    area: Rect,
    /// The file list from the last frame.
    list: Rect,
    /// The diff from the last frame.
    diff: Rect,
    /// The first file row shown.
    list_scroll: usize,
}

impl GitPanel {
    /// Creates the panel.
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves the highlight by `delta` rows and asks for the new row's diff.
    fn move_selection(cx: &mut Context<'_>, delta: isize) {
        let state = &mut cx.ui.git_panel;
        if state.rows.is_empty() {
            return;
        }
        let last = state.rows.len() - 1;
        state.selected = state.selected.saturating_add_signed(delta).min(last);
        if !state.diff_is_current() {
            cx.ui.request(Command::Custom(DIFF_COMMAND.into()));
        }
    }

    /// Scrolls the diff by `delta` lines.
    fn scroll_diff(ui: &mut Ui, delta: isize) {
        let state = &mut ui.git_panel;
        let last = state.diff.len().saturating_sub(1);
        state.scroll = state.scroll.saturating_add_signed(delta).min(last);
    }

    /// Draws the file list into `area`.
    fn render_list(&mut self, area: Rect, buf: &mut Buffer, ui: &Ui, theme: &Theme) {
        let state = &ui.git_panel;
        if state.rows.is_empty() {
            let message = if state.loaded {
                "nothing changed, mog approves"
            } else {
                "asking git..."
            };
            buf.set_stringn(
                area.x,
                area.y,
                message,
                usize::from(area.width),
                theme.popup_dim,
            );
            return;
        }
        // each section gets a heading row, so rows sit below their index
        let mut lines: Vec<(Option<usize>, String)> = Vec::new();
        let mut last_staged = None;
        for (index, row) in state.rows.iter().enumerate() {
            if last_staged != Some(row.staged) {
                let heading = if row.staged { "STAGED" } else { "CHANGES" };
                lines.push((None, heading.to_owned()));
                last_staged = Some(row.staged);
            }
            lines.push((Some(index), relative(&row.path, &ui.root)));
        }
        let height = usize::from(area.height);
        let selected_line = lines
            .iter()
            .position(|(index, _)| *index == Some(state.selected))
            .unwrap_or(0);
        if selected_line < self.list_scroll {
            self.list_scroll = selected_line;
        } else if selected_line >= self.list_scroll + height {
            self.list_scroll = selected_line + 1 - height;
        }
        let width = usize::from(area.width);
        for (offset, (index, text)) in lines.iter().skip(self.list_scroll).take(height).enumerate()
        {
            let y = area.y + u16::try_from(offset).unwrap_or(0);
            let Some(index) = index else {
                buf.set_stringn(area.x, y, text, width, theme.popup_title);
                continue;
            };
            let row = &state.rows[*index];
            let base = if *index == state.selected {
                theme.popup_selected
            } else {
                theme.popup
            };
            buf.set_style(Rect::new(area.x, y, area.width, 1), base);
            let (letter, style) = status_mark(row.status, theme);
            buf.set_string(area.x + 1, y, letter, base.patch(style));
            buf.set_stringn(area.x + 3, y, text, width.saturating_sub(3), base);
        }
    }

    /// Draws the diff into `area`.
    fn render_diff(area: Rect, buf: &mut Buffer, ui: &Ui, theme: &Theme) {
        let state = &ui.git_panel;
        let width = usize::from(area.width);
        for (offset, line) in state
            .diff
            .iter()
            .skip(state.scroll)
            .take(usize::from(area.height))
            .enumerate()
        {
            let y = area.y + u16::try_from(offset).unwrap_or(0);
            let style = if line.starts_with("+++") || line.starts_with("---") {
                theme.popup_dim
            } else if line.starts_with('+') {
                theme.popup.patch(theme.git_added)
            } else if line.starts_with('-') {
                theme.popup.patch(theme.git_removed)
            } else if line.starts_with("@@") {
                theme.popup.patch(theme.info)
            } else if line.starts_with("diff ") || line.starts_with("index ") {
                theme.popup_dim
            } else {
                theme.popup
            };
            let shown = line.replace('\t', "    ");
            buf.set_stringn(area.x, y, shown, width, style);
        }
    }
}

impl Layer for GitPanel {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        if ui.overlay == Some(Overlay::Git) {
            layout.screen
        } else {
            Rect::default()
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let theme = cx.theme;
        let width = area.width.saturating_sub(6);
        let height = area.height.saturating_sub(3);
        self.area = popup::centered(area, width, height);
        popup::dim_around(area, self.area, buf, theme);
        let title = match cx.ui.branch.as_deref() {
            Some(branch) => format!("\u{2387} source control, {branch}"),
            None => "\u{2387} source control".to_owned(),
        };
        let inner = popup::frame(self.area, buf, theme, &title);
        if inner.height < 3 || inner.width < 20 {
            return;
        }
        let body = Rect {
            height: inner.height - 1,
            ..inner
        };
        let list_width = LIST_WIDTH.min(body.width / 3);
        self.list = Rect {
            width: list_width,
            ..body
        };
        self.diff = Rect {
            x: body.x + list_width + 2,
            width: body.width.saturating_sub(list_width + 2),
            ..body
        };
        for y in body.top()..body.bottom() {
            buf.set_string(body.x + list_width, y, "\u{2502}", theme.popup_border);
        }
        self.render_list(self.list, buf, cx.ui, theme);
        Self::render_diff(self.diff, buf, cx.ui, theme);
        buf.set_stringn(
            inner.x,
            inner.bottom() - 1,
            HINTS,
            usize::from(inner.width),
            theme.popup_dim,
        );
    }

    fn handle_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        if cx.ui.overlay != Some(Overlay::Git) {
            return EventResult::Ignored;
        }
        if chord.mods.ctrl || chord.mods.alt {
            match chord.key {
                Key::Char('d') => Self::scroll_diff(cx.ui, PAGE.cast_signed()),
                Key::Char('u') => Self::scroll_diff(cx.ui, -PAGE.cast_signed()),
                _ => return EventResult::Ignored,
            }
            return EventResult::Consumed;
        }
        match chord.key {
            Key::Up | Key::Char('k') => Self::move_selection(cx, -1),
            Key::Down | Key::Char('j') => Self::move_selection(cx, 1),
            Key::PageDown => Self::scroll_diff(cx.ui, PAGE.cast_signed()),
            Key::PageUp => Self::scroll_diff(cx.ui, -PAGE.cast_signed()),
            Key::Char(' ' | 's' | 'u') => cx.ui.request(Command::Custom(TOGGLE_COMMAND.into())),
            Key::Char('a') => cx.ui.request(Command::Custom(STAGE_ALL_COMMAND.into())),
            Key::Char('c') => cx.ui.request(Command::Custom("git.commit".into())),
            Key::Char('r') => cx.ui.request(Command::Custom(REFRESH_COMMAND.into())),
            Key::Enter => cx.ui.request(Command::Custom(OPEN_COMMAND.into())),
            Key::Esc | Key::Char('q') => {
                cx.ui.close();
                cx.ui.focus = Focus::Editor;
            }
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
        let at = Position::new(event.column, event.row);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if !self.area.contains(at) => {
                cx.ui.close();
                cx.ui.focus = Focus::Editor;
            }
            MouseEventKind::Down(MouseButton::Left) if self.list.contains(at) => {
                // map the clicked line back to a row, skipping section headings
                let line = usize::from(event.row - self.list.y) + self.list_scroll;
                let mut seen = 0;
                let mut last_staged = None;
                for (index, row) in cx.ui.git_panel.rows.iter().enumerate() {
                    if last_staged != Some(row.staged) {
                        seen += 1;
                        last_staged = Some(row.staged);
                    }
                    if seen == line {
                        let delta = index.cast_signed() - cx.ui.git_panel.selected.cast_signed();
                        Self::move_selection(cx, delta);
                        break;
                    }
                    seen += 1;
                }
            }
            MouseEventKind::ScrollDown if self.diff.contains(at) => Self::scroll_diff(cx.ui, 3),
            MouseEventKind::ScrollUp if self.diff.contains(at) => Self::scroll_diff(cx.ui, -3),
            MouseEventKind::ScrollDown => Self::move_selection(cx, 1),
            MouseEventKind::ScrollUp => Self::move_selection(cx, -1),
            _ => {}
        }
        EventResult::Consumed
    }
}

#[cfg(test)]
/// Tests for the source control panel.
mod tests {
    use std::path::PathBuf;

    use mog_git::{FileChange, FileStatus};

    use super::GitPanelState;

    /// Staged parts come first and the highlight follows its file through a refresh.
    #[test]
    fn lists_staged_first_and_keeps_selection() {
        let change = |name: &str, staged, unstaged| FileChange {
            path: PathBuf::from(name),
            staged,
            unstaged,
        };
        let mut state = GitPanelState::default();
        state.set_changes(vec![
            change("a.rs", None, Some(FileStatus::Modified)),
            change("b.rs", Some(FileStatus::Added), Some(FileStatus::Modified)),
        ]);
        let rows: Vec<(String, bool)> = state
            .rows
            .iter()
            .map(|row| (row.path.display().to_string(), row.staged))
            .collect();
        assert_eq!(
            rows,
            [
                ("b.rs".into(), true),
                ("a.rs".into(), false),
                ("b.rs".into(), false)
            ]
        );
        state.selected = 2;
        state.set_changes(vec![change("b.rs", None, Some(FileStatus::Modified))]);
        assert_eq!(state.selected, 0);
        assert!(!state.current().expect("row").staged);
    }
}
