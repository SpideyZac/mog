//! Snapshot tests that draw whole screens and compare them as text.
//!
//! Run `cargo insta review` after a layout change to look at and accept the new screens.

use insta::assert_snapshot;
use mog_core::{Editor, MemoryClipboard, Range, Transaction};
use mog_tui::{
    CommandInfo, Compositor, Context, EditorView, Overlay, Pane, Popups, SettingsPanel, SplitState,
    StatusLine, Tabs, Theme, Ui,
};
use ratatui::{Terminal, backend::TestBackend};

/// A small file to draw.
const CODE: &str = "fn main() {\n    let answer = 42;\n    println!(\"{answer}\");\n}\n";

/// Builds an editor with one untitled document holding `text`, cursor at the start.
fn editor_with(text: &str) -> Editor {
    let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
    editor
        .document_mut()
        .apply(Transaction::insert(0, text), Range::point(0), false);
    editor.select(0, 0);
    editor
}

/// Returns ui state with the parts that depend on the machine turned off.
fn quiet_ui() -> Ui {
    let mut ui = Ui::default();
    ui.config.ui.minimap = false;
    ui.config.ui.git_gutter = false;
    ui.config.ui.git_blame = false;
    ui
}

/// Returns the compositor with the main layers in their usual order.
fn compositor() -> Compositor {
    let mut compositor = Compositor::new();
    compositor.push(Box::new(EditorView::new()));
    compositor.push(Box::new(EditorView::side()));
    compositor.push(Box::new(Tabs::new()));
    compositor.push(Box::new(StatusLine::new()));
    compositor.push(Box::new(Popups::new()));
    compositor.push(Box::new(SettingsPanel::new()));
    compositor
}

/// Draws `editor` and `ui` on a `width` by `height` screen and returns it as text.
fn screen(editor: &mut Editor, ui: &mut Ui, width: u16, height: u16) -> String {
    let mut compositor = compositor();
    let theme = Theme::default();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    // the first frame sizes the views, the second draws with the sizes known
    for _ in 0..2 {
        terminal
            .draw(|frame| {
                let mut cx = Context {
                    editor: &mut *editor,
                    theme: &theme,
                    ui: &mut *ui,
                };
                let _ = compositor.render(frame, &mut cx);
            })
            .expect("draw");
    }
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            let row: String = (0..width).map(|x| buffer[(x, y)].symbol()).collect();
            row.trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A file with the tab bar and status line.
#[test]
fn editor_with_tabs() {
    let mut editor = editor_with(CODE);
    let mut ui = quiet_ui();
    assert_snapshot!(screen(&mut editor, &mut ui, 60, 8));
}

/// Two documents side by side.
#[test]
fn split_panes() {
    let mut editor = editor_with(CODE);
    let mut ui = quiet_ui();
    ui.config.ui.tabs = false;
    ui.split = Some(SplitState {
        focused: Pane::Main,
        other_document: editor.active(),
        other_view: editor.view().clone(),
    });
    assert_snapshot!(screen(&mut editor, &mut ui, 70, 7));
}

/// The command palette over the editor.
#[test]
fn command_palette() {
    let mut editor = editor_with(CODE);
    let mut ui = quiet_ui();
    ui.commands = vec![
        CommandInfo {
            name: "save".into(),
            title: "File: Save".into(),
            keys: vec!["ctrl+s".into()],
        },
        CommandInfo {
            name: "quit".into(),
            title: "Quit".into(),
            keys: vec!["ctrl+q".into()],
        },
    ];
    ui.open(Overlay::Palette);
    assert_snapshot!(screen(&mut editor, &mut ui, 80, 24));
}

/// The settings menu.
#[test]
fn settings_menu() {
    let mut editor = editor_with("");
    let mut ui = quiet_ui();
    ui.open(Overlay::Settings);
    assert_snapshot!(screen(&mut editor, &mut ui, 100, 34));
}
