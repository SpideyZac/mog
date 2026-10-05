//! The command palette, file finder, key list and go to line prompt.

use std::path::{Path, PathBuf};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Command, Key, KeyChord, Severity, movement, walk_files};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Style,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    compositor::{Context, EventResult, Layer},
    icons,
    picker::{Picker, PickerAction, PickerItem},
    popup,
    theme::Theme,
    ui::{Focus, Layout, Overlay, PromptKind, Ui},
};

/// The most files the finder lists.
const MAX_FILES: usize = 50_000;

/// The width of picker popups.
const PICKER_WIDTH: u16 = 84;

/// The height of picker popups.
const PICKER_HEIGHT: u16 = 22;

/// The width of text prompts.
const PROMPT_WIDTH: u16 = 64;

/// The command the project symbol picker asks the app to run when the query changes.
pub const SYMBOL_SEARCH_COMMAND: &str = "lsp.workspace_symbols.search";

/// What the rebind popup says under the title.
const REBIND_HINT: &str = "esc cancels, backspace removes the binding";

/// Returns `path` relative to `root` with `/` separators, or the whole path if it is outside.
fn display_path(path: &Path, root: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let parts: Vec<String> = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

/// Where a row of the problems list points.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProblemAt {
    /// A char offset in an open document, by index.
    Document(usize, usize),
    /// A line and column in a file from task output, from 0.
    File(PathBuf, usize, usize),
}

/// The popups that pick from a list, plus the go to line prompt.
#[derive(Debug, Default)]
pub struct Popups {
    /// The list being picked from.
    picker: Picker,
    /// The popup generation the picker was filled for.
    generation: u64,
    /// The files behind the finder rows.
    files: Vec<PathBuf>,
    /// The command names behind the palette or key list rows.
    commands: Vec<String>,
    /// The symbols version the picker was last filled with.
    symbols_version: u64,
    /// Where each problem row points.
    problems: Vec<ProblemAt>,
    /// The box drawn in the last frame.
    area: Rect,
    /// Where the text cursor goes.
    cursor: Option<Position>,
    /// The command whose new chord is being recorded, as `(name, title)`.
    capturing: Option<(String, String)>,
    /// Why the last recorded chord was refused.
    capture_problem: Option<String>,
}

impl Popups {
    /// Creates the popups layer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether `overlay` is one of the popups this layer draws.
    fn owns(overlay: Option<Overlay>) -> bool {
        matches!(
            overlay,
            Some(
                Overlay::Palette
                    | Overlay::Finder
                    | Overlay::Keys
                    | Overlay::Prompt
                    | Overlay::Problems
                    | Overlay::References
                    | Overlay::Symbols
                    | Overlay::WorkspaceSymbols
                    | Overlay::Tasks
            )
        )
    }

    /// Returns the picker rows for the symbols in `ui`.
    fn symbol_items(ui: &Ui) -> Vec<PickerItem> {
        let workspace = ui.overlay == Some(Overlay::WorkspaceSymbols);
        ui.symbols
            .iter()
            .map(|symbol| {
                let indent = if workspace {
                    String::new()
                } else {
                    "  ".repeat(symbol.depth)
                };
                let place = if workspace {
                    format!(
                        "{}:{}",
                        display_path(&symbol.path, &ui.root),
                        symbol.line + 1
                    )
                } else {
                    format!("Ln {}", symbol.line + 1)
                };
                let detail = if symbol.detail.is_empty() {
                    symbol.kind.clone()
                } else {
                    format!("{} {}", symbol.kind, symbol.detail)
                };
                PickerItem::new(format!("{indent}{}", symbol.name))
                    .detail(detail)
                    .hint(place)
            })
            .collect()
    }

    /// Fills the picker for the popup that just opened.
    fn fill(&mut self, cx: &Context<'_>) {
        let ui = &cx.ui;
        let items = match ui.overlay {
            Some(Overlay::Palette) => {
                self.commands = ui.commands.iter().map(|info| info.name.clone()).collect();
                ui.commands
                    .iter()
                    .map(|info| {
                        PickerItem::new(&info.title)
                            .detail(&info.name)
                            .hint(info.keys.join("  "))
                    })
                    .collect()
            }
            Some(Overlay::Keys) => {
                // every palette command plus anything else that has a key, like moving around
                let mut rows: Vec<(String, String, Vec<String>)> = ui
                    .commands
                    .iter()
                    .map(|info| (info.name.clone(), info.title.clone(), info.keys.clone()))
                    .collect();
                for (chord, name) in &ui.bindings {
                    match rows.iter_mut().find(|(other, _, _)| other == name) {
                        Some((_, _, keys)) if !keys.contains(chord) => keys.push(chord.clone()),
                        Some(_) => {}
                        None => rows.push((name.clone(), name.clone(), vec![chord.clone()])),
                    }
                }
                self.commands = rows.iter().map(|(name, _, _)| name.clone()).collect();
                let legacy = ui.legacy_keys;
                rows.into_iter()
                    .map(|(name, title, keys)| {
                        let hint = if keys.is_empty() {
                            "unbound".to_owned()
                        } else {
                            keys.iter()
                                .map(|chord| mark_unsendable(chord, legacy))
                                .collect::<Vec<_>>()
                                .join("  ")
                        };
                        PickerItem::new(title).detail(name).hint(hint)
                    })
                    .collect()
            }
            Some(Overlay::Finder) => {
                let open: Vec<PathBuf> = cx
                    .editor
                    .documents()
                    .iter()
                    .filter_map(|document| document.path().map(ToOwned::to_owned))
                    .collect();
                let mut files = open.clone();
                files.extend(
                    walk_files(&ui.root, MAX_FILES)
                        .into_iter()
                        .filter(|path| !open.contains(path)),
                );
                self.files = files;
                self.files
                    .iter()
                    .map(|path| {
                        let name = display_path(path, &ui.root);
                        let item = PickerItem::new(name);
                        let item = if open.contains(path) {
                            item.hint("open")
                        } else {
                            item
                        };
                        let file_name = path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        match icons::file_color(&file_name).filter(|_| ui.config.ui.icons) {
                            Some(color) => item.marker(Style::new().fg(color)),
                            None => item,
                        }
                    })
                    .collect()
            }
            Some(Overlay::Symbols | Overlay::WorkspaceSymbols) => {
                self.symbols_version = ui.symbols_version;
                Self::symbol_items(ui)
            }
            Some(Overlay::Problems) => {
                let theme_marks = [
                    (Severity::Error, "error"),
                    (Severity::Warning, "warning"),
                    (Severity::Info, "info"),
                    (Severity::Hint, "hint"),
                ];
                let mut rows = Vec::new();
                self.problems.clear();
                for (index, document) in cx.editor.documents().iter().enumerate() {
                    let text = document.text();
                    for diagnostic in document.diagnostics() {
                        let line = text.char_to_line(diagnostic.from.min(text.len_chars())) + 1;
                        let label = diagnostic
                            .message
                            .lines()
                            .next()
                            .unwrap_or_default()
                            .to_owned();
                        let kind = theme_marks
                            .iter()
                            .find(|(severity, _)| *severity == diagnostic.severity)
                            .map_or("", |(_, name)| name);
                        let style = match diagnostic.severity {
                            Severity::Error => cx.theme.error,
                            Severity::Warning => cx.theme.warning,
                            _ => cx.theme.info,
                        };
                        rows.push((
                            diagnostic.severity,
                            PickerItem::new(label)
                                .detail(format!("{}:{line}", document.name()))
                                .hint(kind)
                                .marker(style),
                            ProblemAt::Document(index, diagnostic.from),
                        ));
                    }
                }
                for problem in &ui.task_problems {
                    let kind = theme_marks
                        .iter()
                        .find(|(severity, _)| *severity == problem.severity)
                        .map_or("", |(_, name)| name);
                    let style = match problem.severity {
                        Severity::Error => cx.theme.error,
                        Severity::Warning => cx.theme.warning,
                        _ => cx.theme.info,
                    };
                    let place = format!(
                        "{}:{}",
                        display_path(&problem.path, &ui.root),
                        problem.line + 1
                    );
                    rows.push((
                        problem.severity,
                        PickerItem::new(&problem.message)
                            .detail(place)
                            .hint(format!("{kind}, {}", ui.output.title))
                            .marker(style),
                        ProblemAt::File(problem.path.clone(), problem.line, problem.column),
                    ));
                }
                // worst first, keeping file order within a severity
                rows.sort_by_key(|(severity, _, _)| *severity);
                self.problems = rows.iter().map(|(_, _, at)| at.clone()).collect();
                rows.into_iter().map(|(_, item, _)| item).collect()
            }
            Some(Overlay::Tasks) => ui
                .tasks
                .iter()
                .map(|(name, command)| PickerItem::new(name).hint(command))
                .collect(),
            Some(Overlay::References) => ui
                .references
                .iter()
                .map(|(path, line, _, preview)| {
                    PickerItem::new(preview.trim()).detail(format!(
                        "{}:{}",
                        display_path(path, &ui.root),
                        line + 1
                    ))
                })
                .collect(),
            _ => Vec::new(),
        };
        self.picker.reset(items);
    }

    /// Acts on the picked row `index` of the open popup.
    fn accept(&mut self, index: usize, cx: &mut Context<'_>) {
        let overlay = cx.ui.overlay;
        if overlay == Some(Overlay::Keys) {
            if let Some(name) = self.commands.get(index) {
                let title = cx.ui.title_of(name).to_owned();
                self.capturing = Some((name.clone(), title));
                self.capture_problem = None;
            }
            return;
        }
        cx.ui.close();
        match overlay {
            Some(Overlay::Palette) => {
                let Some(name) = self.commands.get(index) else {
                    return;
                };
                match name.parse::<Command>() {
                    Ok(command) => cx.ui.request(command),
                    Err(err) => cx.editor.set_status(err.to_string()),
                }
            }
            Some(Overlay::References) => {
                let Some((path, line, column, _)) = cx.ui.references.get(index).cloned() else {
                    return;
                };
                cx.ui.focus = Focus::Editor;
                if let Err(err) = cx.editor.open(path.clone()) {
                    cx.editor
                        .set_status(format!("could not open {}: {err}", path.display()));
                    return;
                }
                let text = cx.editor.document().text();
                let line = line.min(text.len_lines() - 1);
                let start = text.line_to_char(line);
                let pos = start + column.min(movement::line_len(text, line));
                cx.editor.select(pos, pos);
            }
            Some(Overlay::Symbols | Overlay::WorkspaceSymbols) => {
                let Some(symbol) = cx.ui.symbols.get(index).cloned() else {
                    return;
                };
                cx.ui.focus = Focus::Editor;
                if let Err(err) = cx.editor.open(symbol.path.clone()) {
                    cx.editor
                        .set_status(format!("could not open {}: {err}", symbol.path.display()));
                    return;
                }
                let text = cx.editor.document().text();
                let line = symbol.line.min(text.len_lines() - 1);
                let pos =
                    text.line_to_char(line) + symbol.column.min(movement::line_len(text, line));
                cx.editor.select(pos, pos);
            }
            Some(Overlay::Problems) => match self.problems.get(index).cloned() {
                Some(ProblemAt::Document(document, pos)) => {
                    cx.ui.focus = Focus::Editor;
                    cx.editor.focus(document);
                    cx.editor.select(pos, pos);
                }
                Some(ProblemAt::File(path, line, column)) => {
                    cx.ui.focus = Focus::Editor;
                    if let Err(err) = cx.editor.open(path.clone()) {
                        cx.editor
                            .set_status(format!("could not open {}: {err}", path.display()));
                        return;
                    }
                    let text = cx.editor.document().text();
                    let line = line.min(text.len_lines() - 1);
                    let pos = text.line_to_char(line) + column.min(movement::line_len(text, line));
                    cx.editor.select(pos, pos);
                }
                None => {}
            },
            Some(Overlay::Tasks) => {
                cx.ui.picked_task = Some(index);
                cx.ui.request(Command::Custom("task.start".into()));
            }
            Some(Overlay::Finder) => {
                let Some(path) = self.files.get(index) else {
                    return;
                };
                cx.ui.focus = Focus::Editor;
                if let Err(err) = cx.editor.open(path.clone()) {
                    cx.editor
                        .set_status(format!("could not open {}: {err}", path.display()));
                }
            }
            _ => {}
        }
    }

    /// Records `chord` as the new binding for the command being rebound.
    fn capture_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) {
        let Some((name, _)) = self.capturing.clone() else {
            return;
        };
        let no_mods = !chord.mods.ctrl && !chord.mods.alt && !chord.mods.shift;
        let new = match chord.key {
            Key::Esc if no_mods => None,
            Key::Backspace if no_mods => Some(None),
            // a plain letter would stop that letter from typing
            _ if chord.typed_char().is_some() => {
                self.capture_problem = Some(format!("{chord} types text, add ctrl or alt"));
                return;
            }
            _ => Some(Some(chord.to_string())),
        };
        self.capturing = None;
        if let Some(chord) = new {
            cx.ui.rebind = Some((name, chord));
            cx.ui.request(Command::Custom("keys.rebind".into()));
        }
    }

    /// Draws the box asking for a new chord over the key list.
    fn render_capture(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let Some((_, title)) = &self.capturing else {
            return;
        };
        let rect = popup::centered(area, PROMPT_WIDTH, 5);
        let inner = popup::frame(rect, buf, theme, &format!("\u{2328} rebind {title}"));
        let width = usize::from(inner.width.saturating_sub(2));
        buf.set_stringn(
            inner.x + 1,
            inner.y,
            "press the new keys now",
            width,
            theme.popup_title,
        );
        let (text, style) = match &self.capture_problem {
            Some(problem) => (problem.as_str(), theme.warning),
            None => (REBIND_HINT, theme.popup_dim),
        };
        buf.set_stringn(inner.x + 1, inner.y + 2, text, width, style);
    }

    /// Handles a key in a text prompt.
    fn prompt_key(chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        let Some(prompt) = cx.ui.prompt.as_mut() else {
            return EventResult::Ignored;
        };
        match chord.key {
            Key::Esc => cx.ui.close(),
            Key::Enter => {
                let prompt = cx.ui.prompt.take();
                cx.ui.close();
                cx.ui.submitted = prompt;
                cx.ui.request(Command::Custom("prompt.submit".into()));
            }
            Key::Backspace if chord.mods.ctrl => prompt.text.clear(),
            Key::Backspace => {
                prompt.text.pop();
            }
            _ => match chord.typed_char() {
                Some(ch) if prompt.kind != PromptKind::GotoLine || ch.is_ascii_digit() => {
                    prompt.text.push(ch);
                }
                Some(_) => {}
                None => return EventResult::Ignored,
            },
        }
        EventResult::Consumed
    }
}

/// Returns `chord` with a `(!)` after it if `legacy` keys cannot send it.
fn mark_unsendable(chord: &str, legacy: bool) -> String {
    let unsendable = legacy
        && chord
            .parse::<KeyChord>()
            .is_ok_and(|chord| chord.legacy_problem().is_some());
    if unsendable {
        format!("{chord} (!)")
    } else {
        chord.to_owned()
    }
}

impl Layer for Popups {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        if Self::owns(ui.overlay) {
            layout.screen
        } else {
            Rect::default()
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        if self.generation != cx.ui.overlay_generation {
            self.generation = cx.ui.overlay_generation;
            self.capturing = None;
            self.fill(cx);
        } else if cx.ui.overlay == Some(Overlay::WorkspaceSymbols)
            && self.symbols_version != cx.ui.symbols_version
        {
            // new results for what was typed, so keep the query
            self.symbols_version = cx.ui.symbols_version;
            self.picker.set_items(Self::symbol_items(cx.ui));
        }
        let theme = cx.theme;
        let (title, placeholder) = match cx.ui.overlay {
            Some(Overlay::Palette) => ("\u{2318} command palette", "type a command..."),
            Some(Overlay::Finder) => ("\u{2315} find a file", "type part of a file name..."),
            Some(Overlay::Keys) if cx.ui.legacy_keys => (
                "\u{2328} key bindings, enter to change. (!) your terminal cannot send it",
                "search keys or commands...",
            ),
            Some(Overlay::Keys) => (
                "\u{2328} key bindings, enter to change",
                "search keys or commands...",
            ),
            Some(Overlay::Problems) => ("\u{26a0} problems", "search problems..."),
            Some(Overlay::References) => ("\u{21c4} references", "search references..."),
            Some(Overlay::Symbols) => ("\u{2261} symbols in this file", "search symbols..."),
            Some(Overlay::Tasks) => ("\u{2699} run a task", "build, test, run..."),
            Some(Overlay::WorkspaceSymbols) => {
                ("\u{2261} symbols in the project", "type a symbol name...")
            }
            _ => ("", ""),
        };
        if let Some(prompt) = cx
            .ui
            .prompt
            .as_ref()
            .filter(|_| cx.ui.overlay == Some(Overlay::Prompt))
        {
            self.area = popup::centered(area, PROMPT_WIDTH, 4);
            popup::dim_around(area, self.area, buf, theme);
            let inner = popup::frame(self.area, buf, theme, &prompt.title);
            let room = usize::from(inner.width.saturating_sub(3));
            // keep the end of long input visible
            let chars: Vec<char> = prompt.text.chars().collect();
            let shown: String = chars[chars.len().saturating_sub(room)..].iter().collect();
            buf.set_string(inner.x + 1, inner.y, "\u{276f} ", theme.popup_title);
            buf.set_stringn(inner.x + 3, inner.y, &shown, room, theme.popup);
            buf.set_stringn(
                inner.x + 1,
                inner.y + 1,
                &prompt.hint,
                usize::from(inner.width.saturating_sub(2)),
                theme.popup_dim,
            );
            let typed = u16::try_from(shown.width()).unwrap_or(0);
            self.cursor = Some(Position::new(inner.x + 3 + typed, inner.y));
            return;
        }
        self.area = popup::centered(area, PICKER_WIDTH, PICKER_HEIGHT);
        popup::dim_around(area, self.area, buf, theme);
        let inner = popup::frame(self.area, buf, theme, title);
        let padded = Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(2),
            ..inner
        };
        self.picker.render(padded, buf, theme, placeholder);
        self.cursor = self.picker.cursor();
        if self.capturing.is_some() {
            self.render_capture(area, buf, theme);
            self.cursor = None;
        }
    }

    fn handle_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        if !Self::owns(cx.ui.overlay) {
            return EventResult::Ignored;
        }
        if cx.ui.overlay == Some(Overlay::Prompt) {
            return Self::prompt_key(chord, cx);
        }
        if self.capturing.is_some() {
            self.capture_key(chord, cx);
            return EventResult::Consumed;
        }
        match self.picker.handle_key(chord) {
            PickerAction::Accept(index) => self.accept(index, cx),
            PickerAction::Cancel => cx.ui.close(),
            PickerAction::Ignored => return EventResult::Ignored,
            PickerAction::None => {}
        }
        if cx.ui.overlay == Some(Overlay::WorkspaceSymbols)
            && self.picker.query() != cx.ui.symbol_query
        {
            cx.ui.symbol_query = self.picker.query().to_owned();
            cx.ui.request(Command::Custom(SYMBOL_SEARCH_COMMAND.into()));
        }
        EventResult::Consumed
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        if self.capturing.is_some() {
            if matches!(event.kind, MouseEventKind::Down(_)) {
                self.capturing = None;
            }
            return EventResult::Consumed;
        }
        let inside = self.area.contains(Position::new(event.column, event.row));
        if !inside {
            if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
                cx.ui.close();
            }
            return EventResult::Consumed;
        }
        if let PickerAction::Accept(index) = self.picker.handle_mouse(event) {
            self.accept(index, cx);
        }
        EventResult::Consumed
    }

    fn cursor(&self, _area: Rect, _cx: &Context<'_>) -> Option<Position> {
        self.cursor
    }
}

#[cfg(test)]
/// Tests for [`Popups`].
mod tests {
    use mog_core::{Command, Editor, Key, KeyChord, MemoryClipboard, Modifiers};
    use ratatui::{Terminal, backend::TestBackend};

    use super::Popups;
    use crate::{
        compositor::{Compositor, Context},
        theme::Theme,
        ui::{CommandInfo, Overlay, Ui},
    };

    /// Typing in the palette and pressing enter queues the picked command.
    #[test]
    fn palette_runs_commands() {
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        let theme = Theme::default();
        let mut ui = Ui {
            commands: vec![
                CommandInfo {
                    name: "save".into(),
                    title: "File: Save".into(),
                    keys: vec!["ctrl+s".into()],
                },
                CommandInfo {
                    name: "undo".into(),
                    title: "Edit: Undo".into(),
                    keys: Vec::new(),
                },
            ],
            ..Ui::default()
        };
        ui.open(Overlay::Palette);
        let mut compositor = Compositor::new();
        compositor.push(Box::new(Popups::new()));
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
        terminal
            .draw(|frame| {
                let mut cx = Context {
                    editor: &mut editor,
                    theme: &theme,
                    ui: &mut ui,
                };
                compositor.render(frame, &mut cx);
            })
            .expect("draw");
        for key in [Key::Char('u'), Key::Char('n'), Key::Enter] {
            let mut cx = Context {
                editor: &mut editor,
                theme: &theme,
                ui: &mut ui,
            };
            compositor.handle_key(KeyChord::new(key, Modifiers::default()), &mut cx);
        }
        assert_eq!(ui.requests, [Command::Undo]);
        assert_eq!(ui.overlay, None);
    }

    /// Enter in the key list records the next chord as the new binding.
    #[test]
    fn key_list_rebinds() {
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        let theme = Theme::default();
        let mut ui = Ui {
            commands: vec![CommandInfo {
                name: "save".into(),
                title: "File: Save".into(),
                keys: vec!["ctrl+s".into()],
            }],
            ..Ui::default()
        };
        ui.open(Overlay::Keys);
        let mut compositor = Compositor::new();
        compositor.push(Box::new(Popups::new()));
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
        terminal
            .draw(|frame| {
                let mut cx = Context {
                    editor: &mut editor,
                    theme: &theme,
                    ui: &mut ui,
                };
                compositor.render(frame, &mut cx);
            })
            .expect("draw");
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        let presses = [
            KeyChord::new(Key::Enter, Modifiers::default()),
            KeyChord::new(Key::Char('x'), Modifiers::default()),
            KeyChord::new(Key::Char('w'), alt),
        ];
        for chord in presses {
            let mut cx = Context {
                editor: &mut editor,
                theme: &theme,
                ui: &mut ui,
            };
            compositor.handle_key(chord, &mut cx);
        }
        // the plain x is refused since it types text
        assert_eq!(ui.rebind, Some(("save".into(), Some("alt+w".into()))));
        assert_eq!(ui.requests, [Command::Custom("keys.rebind".into())]);
    }
}
