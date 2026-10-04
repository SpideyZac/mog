//! The command palette, file finder, key list and go to line prompt.

use std::path::{Path, PathBuf};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Command, Key, KeyChord, walk_files};
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

/// Returns `path` relative to `root` with `/` separators, or the whole path if it is outside.
fn display_path(path: &Path, root: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let parts: Vec<String> = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
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
    /// The box drawn in the last frame.
    area: Rect,
    /// Where the text cursor goes.
    cursor: Option<Position>,
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
            Some(Overlay::Palette | Overlay::Finder | Overlay::Keys | Overlay::Prompt)
        )
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
                self.commands = ui.bindings.iter().map(|(_, name)| name.clone()).collect();
                ui.bindings
                    .iter()
                    .map(|(chord, name)| {
                        PickerItem::new(ui.title_of(name)).detail(name).hint(chord)
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
            _ => Vec::new(),
        };
        self.picker.reset(items);
    }

    /// Acts on the picked row `index` of the open popup.
    fn accept(&mut self, index: usize, cx: &mut Context<'_>) {
        let overlay = cx.ui.overlay;
        cx.ui.close();
        match overlay {
            Some(Overlay::Palette | Overlay::Keys) => {
                let Some(name) = self.commands.get(index) else {
                    return;
                };
                match name.parse::<Command>() {
                    Ok(command) => cx.ui.request(command),
                    Err(err) => cx.editor.set_status(err.to_string()),
                }
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
            self.fill(cx);
        }
        let theme = cx.theme;
        let (title, placeholder) = match cx.ui.overlay {
            Some(Overlay::Palette) => ("\u{2318} command palette", "type a command..."),
            Some(Overlay::Finder) => ("\u{2315} find a file", "type part of a file name..."),
            Some(Overlay::Keys) => ("\u{2328} key bindings", "search keys or commands..."),
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
    }

    fn handle_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        if !Self::owns(cx.ui.overlay) {
            return EventResult::Ignored;
        }
        if cx.ui.overlay == Some(Overlay::Prompt) {
            return Self::prompt_key(chord, cx);
        }
        match self.picker.handle_key(chord) {
            PickerAction::Accept(index) => self.accept(index, cx),
            PickerAction::Cancel => cx.ui.close(),
            PickerAction::Ignored => return EventResult::Ignored,
            PickerAction::None => {}
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
}
