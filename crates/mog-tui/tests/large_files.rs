//! Timings for huge files, run with `cargo test --release -p mog-tui --test large_files --
//! --ignored --nocapture`.
//!
//! They print how long the editor takes to draw and to react to typing, and fail if a frame
//! takes long enough to make the editor feel stuck.

use std::time::{Duration, Instant};

use mog_core::{Command, Editor, MemoryClipboard, Range, Transaction};
use mog_tui::{Compositor, Context, EditorView, Minimap, StatusLine, Tabs, Theme, Ui};
use ratatui::{Terminal, backend::TestBackend};

/// The slowest a frame may be before the editor counts as stuck.
const FRAME_BUDGET: Duration = Duration::from_millis(250);

/// One line of plausible code, repeated to build big files.
const LINE: &str = "    let value = compute(alpha, beta) + other_function(\"text\", 42); // note\n";

/// Builds an editor holding `text` as a Rust file.
fn editor_with(text: &str) -> Editor {
    let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
    let document = editor.document_mut();
    document.apply(Transaction::insert(0, text), Range::point(0), false);
    document.set_path("/big/huge.rs");
    editor.select(0, 0);
    editor
}

/// Draws one frame and returns how long it took.
fn frame(
    compositor: &mut Compositor,
    terminal: &mut Terminal<TestBackend>,
    editor: &mut Editor,
    ui: &mut Ui,
) -> Duration {
    let theme = Theme::default();
    let start = Instant::now();
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
    start.elapsed()
}

/// Opens `text`, draws, types a few chars and reports the slowest frame.
fn measure(name: &str, text: &str) -> Duration {
    let start = Instant::now();
    let mut editor = editor_with(text);
    let opened = start.elapsed();
    let mut compositor = Compositor::new();
    compositor.push(Box::new(EditorView::new()));
    compositor.push(Box::new(Tabs::new()));
    compositor.push(Box::new(Minimap::new()));
    compositor.push(Box::new(StatusLine::new()));
    let mut ui = Ui::default();
    let mut terminal = Terminal::new(TestBackend::new(160, 50)).expect("terminal");
    let first = frame(&mut compositor, &mut terminal, &mut editor, &mut ui);
    let mut worst = first;
    let mut typing = Duration::ZERO;
    for ch in "abc".chars() {
        let start = Instant::now();
        editor.execute(Command::InsertChar(ch));
        let took = start.elapsed() + frame(&mut compositor, &mut terminal, &mut editor, &mut ui);
        typing = typing.max(took);
        worst = worst.max(took);
    }
    editor.execute("move_doc_end".parse().expect("command"));
    let end = frame(&mut compositor, &mut terminal, &mut editor, &mut ui);
    worst = worst.max(end);
    println!(
        "{name}: open {opened:?}, first frame {first:?}, typing {typing:?}, end of file {end:?}"
    );
    worst
}

/// A 100 MB file of ordinary lines stays responsive.
#[test]
#[ignore = "slow, run by hand"]
fn hundred_megabyte_file() {
    let text = LINE.repeat(100 * 1024 * 1024 / LINE.len());
    let worst = measure("100 MB", &text);
    assert!(worst < FRAME_BUDGET, "slowest frame took {worst:?}");
}

/// A single 20 MB line, like minified code, stays responsive.
#[test]
#[ignore = "slow, run by hand"]
fn very_long_line() {
    let text = LINE.trim_end().repeat(20 * 1024 * 1024 / LINE.len());
    let worst = measure("20 MB line", &text);
    assert!(worst < FRAME_BUDGET, "slowest frame took {worst:?}");
}

/// An ordinary 5000 line file for comparison.
#[test]
#[ignore = "slow, run by hand"]
fn ordinary_file() {
    let text = LINE.repeat(5000);
    let worst = measure("5000 lines", &text);
    assert!(worst < FRAME_BUDGET, "slowest frame took {worst:?}");
}
