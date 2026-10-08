//! The sidebar: a bar of buttons, the source control view and the handle that resizes the views.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Command, Key, KeyChord};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    compositor::{Context, EventResult, Layer},
    git_panel::{
        OPEN_COMMAND, REFRESH_COMMAND, STAGE_ALL_COMMAND, TOGGLE_COMMAND, relative, status_mark,
    },
    menu::{self, MenuItem},
    panels::PanelSide,
    ui::{Focus, Layout, SidebarSide, SidebarView, Ui},
};

/// The most the sidebar views can be dragged to, in cells.
const MAX_DRAGGED_WIDTH: u16 = 200;

/// Rows between the tops of two buttons in the bar.
const BUTTON_PITCH: u16 = 2;

/// How many rows one wheel notch scrolls the changed files.
const WHEEL_ROWS: usize = 3;

/// Rows above the changed files: the title, the branch and the buttons.
const GIT_HEADER: u16 = 3;

/// The buttons under the title of the source control view, as `(label, command)`.
const GIT_BUTTONS: [(&str, &str); 4] = [
    (" + all ", STAGE_ALL_COMMAND),
    (" \u{2714} commit ", "git.commit"),
    (" \u{2b12} diff ", "git.panel"),
    (" \u{27f3} ", REFRESH_COMMAND),
];

/// What clicking a sidebar button does.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// Shows a view, or folds it away if it is already shown.
    View(SidebarView),
    /// Runs the app command with this name.
    Command(&'static str),
}

/// One button of the bar.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Button {
    /// The few chars drawn on the button.
    icon: String,
    /// What the button is for, said in the status line when it is clicked.
    title: String,
    /// What it does.
    target: Target,
    /// Whether what it opens is shown right now.
    lit: bool,
}

/// Returns the buttons of the bar as `(top group, bottom group)`.
fn buttons(ui: &Ui) -> (Vec<Button>, Vec<Button>) {
    let shown = ui.sidebar_active();
    let view = |icon: &str, title: &str, view: SidebarView| Button {
        icon: icon.to_owned(),
        title: title.to_owned(),
        lit: shown == Some(&view),
        target: Target::View(view),
    };
    let mut top = Vec::new();
    if ui.has_explorer {
        top.push(view("\u{25a4}", "Files", SidebarView::Files));
    }
    top.push(view("\u{2387}", "Source control", SidebarView::Git));
    for panel in ui
        .plugin_panels
        .iter()
        .filter(|panel| panel.open && panel.side == PanelSide::Sidebar)
    {
        let icon = if panel.icon.is_empty() {
            panel
                .title
                .chars()
                .next()
                .map_or_else(|| "?".to_owned(), |ch| ch.to_uppercase().to_string())
        } else {
            panel.icon.clone()
        };
        top.push(view(
            &icon,
            &panel.title,
            SidebarView::Plugin {
                plugin: panel.plugin.clone(),
                id: panel.id.clone(),
            },
        ));
    }
    let action = |icon: &str, title: &str, command, lit| Button {
        icon: icon.to_owned(),
        title: title.to_owned(),
        target: Target::Command(command),
        lit,
    };
    let bottom = vec![
        action(
            "\u{2315}",
            "Search the project",
            "project_search.open",
            false,
        ),
        action("$", "Terminal", "terminal.toggle", ui.terminal_open),
        action("\u{2726}", "AI chat", "ai.chat", ui.chat.open),
        action("\u{2261}", "Settings", "settings.open", false),
    ];
    (top, bottom)
}

/// Returns where each button is drawn in `bar`.
fn button_rows(bar: Rect, top: usize, bottom: usize) -> (Vec<Rect>, Vec<Rect>) {
    let row = |y: u16| Rect::new(bar.x, y, bar.width, 1);
    let top_rows = (0..top)
        .map(|index| row(bar.y + 1 + BUTTON_PITCH * u16::try_from(index).unwrap_or(0)))
        .filter(|rect| rect.bottom() <= bar.bottom())
        .collect();
    let count = u16::try_from(bottom).unwrap_or(0);
    let bottom_rows = (0..count)
        .filter_map(|index| {
            let from_bottom = 1 + BUTTON_PITCH * (count - index);
            bar.bottom().checked_sub(from_bottom).map(row)
        })
        .collect();
    (top_rows, bottom_rows)
}

/// The bar of buttons at the edge of the screen.
#[derive(Debug, Default)]
pub struct ActivityBar {
    /// Where each button was drawn in the last frame, with what it does and what it is for.
    hits: Vec<(Rect, Target, String)>,
}

impl ActivityBar {
    /// Creates the layer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens the menu for moving or hiding the sidebar.
    fn open_menu(cx: &mut Context<'_>, at: Position) {
        let side = if cx.ui.sidebar.side == SidebarSide::Left {
            "Move the sidebar to the right"
        } else {
            "Move the sidebar to the left"
        };
        let items = vec![
            MenuItem::command(side, "sidebar.side", cx.ui),
            MenuItem::command("Hide the sidebar", "sidebar.toggle", cx.ui),
        ];
        menu::open_menu(cx.ui, at, items);
    }
}

impl Layer for ActivityBar {
    fn area(&self, layout: &Layout, _ui: &Ui) -> Rect {
        layout.activity_bar
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let theme = cx.theme;
        self.hits.clear();
        buf.set_style(area, theme.sidebar);
        // the rule faces the editor, or the view when one is open
        let on_right = cx.ui.sidebar.side == SidebarSide::Right;
        let rule_x = if on_right { area.x } else { area.right() - 1 };
        for y in area.top()..area.bottom() {
            buf.set_string(rule_x, y, "\u{2502}", theme.border);
        }
        let (top, bottom) = buttons(cx.ui);
        let (top_rows, bottom_rows) = button_rows(area, top.len(), bottom.len());
        let drawn = top
            .into_iter()
            .zip(top_rows)
            .chain(bottom.into_iter().zip(bottom_rows));
        for (button, row) in drawn {
            let style = if button.lit {
                theme.sidebar_active.patch(theme.sidebar_title)
            } else {
                theme.sidebar
            };
            // the rule column is left alone
            let cells = Rect {
                x: if on_right { row.x + 1 } else { row.x },
                width: row.width.saturating_sub(1),
                ..row
            };
            buf.set_style(cells, style);
            let width = u16::try_from(button.icon.width()).unwrap_or(1);
            let x = if width >= cells.width {
                cells.x
            } else {
                cells.x + (cells.width - width + u16::from(on_right)) / 2
            };
            buf.set_stringn(x, row.y, &button.icon, usize::from(cells.width), style);
            self.hits.push((row, button.target, button.title));
        }
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        let point = Position::new(event.column, event.row);
        let hit = self
            .hits
            .iter()
            .find(|(rect, _, _)| rect.contains(point))
            .cloned();
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let Some((_, target, title)) = hit else {
                    return EventResult::Consumed;
                };
                match target {
                    Target::View(view) => {
                        if view == SidebarView::Git && cx.ui.sidebar_active() != Some(&view) {
                            cx.ui.request(Command::Custom(REFRESH_COMMAND.into()));
                        }
                        cx.ui.toggle_view(view);
                    }
                    Target::Command(name) => cx.ui.request(Command::Custom(name.into())),
                }
                cx.editor.set_status(title);
            }
            MouseEventKind::Down(MouseButton::Right) => {
                Self::open_menu(cx, Position::new(event.column, event.row));
            }
            _ => {}
        }
        EventResult::Consumed
    }
}

/// Returns the column of the view that faces the editor and the whole view, if one is shown.
fn view_rect(layout: &Layout) -> Rect {
    if !layout.sidebar_view.is_empty() {
        return layout.sidebar_view;
    }
    if layout.explorer.is_empty() {
        return Rect::default();
    }
    Rect {
        height: layout.explorer.height + layout.explorer_footer.height,
        ..layout.explorer
    }
}

/// The edge of the sidebar view, which changes its width when dragged.
#[derive(Debug, Default)]
pub struct SidebarResize {
    /// The whole view in the last frame.
    view: Rect,
    /// Whether a drag is going on.
    dragging: bool,
}

impl SidebarResize {
    /// Creates the layer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the column of the edge in `view`.
    fn edge(view: Rect, ui: &Ui) -> u16 {
        if ui.sidebar.open && ui.sidebar.side == SidebarSide::Right {
            view.x
        } else {
            view.right().saturating_sub(1)
        }
    }
}

impl Layer for SidebarResize {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        let view = view_rect(layout);
        if view.is_empty() {
            return Rect::default();
        }
        // the cells around the edge count too, since one column is hard to hit
        Rect {
            x: Self::edge(view, ui).saturating_sub(1),
            width: 3,
            ..view
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        self.view = view_rect(&cx.ui.layout(buf.area));
        if self.dragging {
            let x = Self::edge(self.view, cx.ui);
            for y in area.top()..area.bottom() {
                buf.set_string(x, y, "\u{2503}", cx.theme.sidebar_title);
            }
        }
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => self.dragging = true,
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                let on_right = cx.ui.sidebar.open && cx.ui.sidebar.side == SidebarSide::Right;
                let width = if on_right {
                    self.view.right().saturating_sub(event.column)
                } else {
                    (event.column + 1).saturating_sub(self.view.x)
                };
                cx.ui.sidebar.width = Some(width.min(MAX_DRAGGED_WIDTH));
            }
            MouseEventKind::Up(_) => self.dragging = false,
            _ => {}
        }
        EventResult::Consumed
    }
}

/// What a click in the source control view lands on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum GitHit {
    /// A header button, with the command it runs.
    Button(&'static str),
    /// A changed file by row index, and whether the click was on its status letter.
    Row(usize, bool),
}

/// The changed files of the repository, in the sidebar.
#[derive(Debug, Default)]
pub struct SourceControlView {
    /// What was drawn in the last frame and what clicking it does.
    hits: Vec<(Rect, GitHit)>,
    /// The first line of the list shown.
    scroll: usize,
    /// How many lines of the list fit.
    rows: usize,
}

impl SourceControlView {
    /// Creates the layer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether the view is shown.
    fn shown(ui: &Ui) -> bool {
        ui.sidebar_active() == Some(&SidebarView::Git)
    }

    /// Moves the highlight by `delta` rows.
    fn move_selection(ui: &mut Ui, delta: isize) {
        let state = &mut ui.git_panel;
        let last = state.rows.len().saturating_sub(1);
        state.selected = state.selected.saturating_add_signed(delta).min(last);
    }

    /// Opens the menu for the highlighted row.
    fn open_menu(cx: &mut Context<'_>, at: Position) {
        let items = vec![
            MenuItem::command("Stage or unstage", TOGGLE_COMMAND, cx.ui),
            MenuItem::command("Open the file", OPEN_COMMAND, cx.ui),
            MenuItem::command("Show the diff", "git.panel", cx.ui),
        ];
        menu::open_menu(cx.ui, at, items);
    }
}

impl Layer for SourceControlView {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        if Self::shown(ui) {
            layout.sidebar_view
        } else {
            Rect::default()
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let (theme, ui) = (cx.theme, &*cx.ui);
        self.hits.clear();
        buf.set_style(area, theme.sidebar);
        let focused = ui.focus == Focus::Sidebar && ui.overlay.is_none();
        let on_right = ui.sidebar.side == SidebarSide::Right;
        let (rule_x, left) = if on_right {
            (area.x, area.x + 1)
        } else {
            (area.right() - 1, area.x)
        };
        let rule = if focused {
            theme.sidebar_title
        } else {
            theme.border
        };
        for y in area.top()..area.bottom() {
            buf.set_string(rule_x, y, "\u{2502}", rule);
        }
        let width = area.width.saturating_sub(1);
        let text = usize::from(width);
        buf.set_stringn(
            left,
            area.y,
            " \u{2387} SOURCE CONTROL",
            text,
            theme.sidebar_title,
        );
        let state = &ui.git_panel;
        let branch = match ui.branch.as_deref() {
            Some(branch) => format!(" {branch}"),
            None if state.loaded => " not a git repository".to_owned(),
            None => " asking git...".to_owned(),
        };
        buf.set_stringn(left, area.y + 1, branch, text, theme.popup_dim);
        let mut x = left + 1;
        for (label, command) in GIT_BUTTONS {
            let button_width = u16::try_from(label.width()).unwrap_or(0);
            if x + button_width > left + width {
                break;
            }
            buf.set_string(x, area.y + 2, label, theme.sidebar_active);
            self.hits.push((
                Rect::new(x, area.y + 2, button_width, 1),
                GitHit::Button(command),
            ));
            x += button_width + 1;
        }

        let list = Rect {
            y: area.y + GIT_HEADER,
            height: area.height.saturating_sub(GIT_HEADER),
            ..area
        };
        self.rows = usize::from(list.height);
        if state.rows.is_empty() {
            let message = if state.loaded {
                " nothing changed, mog approves"
            } else {
                " asking git..."
            };
            buf.set_stringn(left, list.y, message, text, theme.popup_dim);
            return;
        }
        // each section gets a heading line, so lines sit below their index
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
        let selected_line = lines
            .iter()
            .position(|(index, _)| *index == Some(state.selected))
            .unwrap_or(0);
        self.scroll = self.scroll.min(lines.len().saturating_sub(1));
        if selected_line < self.scroll {
            self.scroll = selected_line;
        } else if self.rows > 0 && selected_line >= self.scroll + self.rows {
            self.scroll = selected_line + 1 - self.rows;
        }
        for (offset, (index, label)) in lines.iter().skip(self.scroll).take(self.rows).enumerate() {
            let y = list.y + u16::try_from(offset).unwrap_or(0);
            let Some(index) = index else {
                buf.set_stringn(left, y, format!(" {label}"), text, theme.sidebar_title);
                continue;
            };
            let row = &state.rows[*index];
            let base = if *index == state.selected {
                if focused {
                    theme.sidebar.patch(theme.selection)
                } else {
                    theme.sidebar_active
                }
            } else {
                theme.sidebar
            };
            buf.set_style(Rect::new(left, y, width, 1), base);
            let (letter, style) = status_mark(row.status, theme);
            buf.set_string(left + 1, y, letter, base.patch(style));
            buf.set_stringn(left + 3, y, label, text.saturating_sub(3), base);
            self.hits
                .push((Rect::new(left, y, 3, 1), GitHit::Row(*index, true)));
            self.hits.push((
                Rect::new(left + 3, y, width.saturating_sub(3), 1),
                GitHit::Row(*index, false),
            ));
        }
    }

    fn handle_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        if !Self::shown(cx.ui) || cx.ui.focus != Focus::Sidebar || cx.ui.overlay.is_some() {
            return EventResult::Ignored;
        }
        if chord.mods.ctrl || chord.mods.alt {
            return EventResult::Ignored;
        }
        match chord.key {
            Key::Up | Key::Char('k') => Self::move_selection(cx.ui, -1),
            Key::Down | Key::Char('j') => Self::move_selection(cx.ui, 1),
            Key::Char(' ' | 's' | 'u') => cx.ui.request(Command::Custom(TOGGLE_COMMAND.into())),
            Key::Char('a') => cx.ui.request(Command::Custom(STAGE_ALL_COMMAND.into())),
            Key::Char('c') => cx.ui.request(Command::Custom("git.commit".into())),
            Key::Char('r') => cx.ui.request(Command::Custom(REFRESH_COMMAND.into())),
            Key::Char('d') => cx.ui.request(Command::Custom("git.panel".into())),
            Key::Enter => cx.ui.request(Command::Custom(OPEN_COMMAND.into())),
            Key::Esc => cx.ui.focus = Focus::Editor,
            _ => return EventResult::Ignored,
        }
        EventResult::Consumed
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        let point = Position::new(event.column, event.row);
        let hit = self
            .hits
            .iter()
            .find(|(rect, _)| rect.contains(point))
            .map(|(_, hit)| hit.clone());
        match (event.kind, hit) {
            (MouseEventKind::Down(MouseButton::Left), hit) => {
                cx.ui.focus = Focus::Sidebar;
                match hit {
                    Some(GitHit::Button(command)) => {
                        cx.ui.request(Command::Custom(command.into()));
                    }
                    Some(GitHit::Row(index, on_letter)) => {
                        cx.ui.git_panel.selected = index;
                        let command = if on_letter {
                            TOGGLE_COMMAND
                        } else {
                            OPEN_COMMAND
                        };
                        cx.ui.request(Command::Custom(command.into()));
                    }
                    None => {}
                }
            }
            (MouseEventKind::Down(MouseButton::Right), hit) => {
                cx.ui.focus = Focus::Sidebar;
                if let Some(GitHit::Row(index, _)) = hit {
                    cx.ui.git_panel.selected = index;
                    Self::open_menu(cx, point);
                }
            }
            (MouseEventKind::ScrollUp, _) => {
                self.scroll = self.scroll.saturating_sub(WHEEL_ROWS);
            }
            (MouseEventKind::ScrollDown, _) => {
                self.scroll = self.scroll.saturating_add(WHEEL_ROWS);
            }
            _ => {}
        }
        EventResult::Consumed
    }
}

#[cfg(test)]
/// Tests for the sidebar.
mod tests {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use mog_core::{Editor, MemoryClipboard};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    use super::{ActivityBar, SidebarResize, SourceControlView};
    use crate::{
        compositor::{Compositor, Context},
        theme::Theme,
        ui::{SidebarSide, SidebarView, Ui},
    };

    /// Shared state with a folder open and the sidebar on, but no extras.
    fn ui() -> Ui {
        let mut ui = Ui {
            has_explorer: true,
            explorer_open: true,
            ..Ui::default()
        };
        ui.sidebar.open = true;
        ui.config.ui.tabs = false;
        ui.config.ui.minimap = false;
        ui
    }

    /// Draws one frame of `compositor` on a screen of `size`.
    fn draw(compositor: &mut Compositor, ui: &mut Ui, editor: &mut Editor, size: (u16, u16)) {
        let theme = Theme::default();
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).expect("terminal");
        terminal
            .draw(|frame| {
                let mut cx = Context {
                    editor,
                    theme: &theme,
                    ui,
                };
                compositor.render(frame, &mut cx);
            })
            .expect("draw");
    }

    /// Sends a mouse event to `compositor`.
    fn mouse(
        compositor: &mut Compositor,
        ui: &mut Ui,
        editor: &mut Editor,
        kind: MouseEventKind,
        at: (u16, u16),
    ) {
        let theme = Theme::default();
        let event = MouseEvent {
            kind,
            column: at.0,
            row: at.1,
            modifiers: KeyModifiers::NONE,
        };
        let mut cx = Context {
            editor,
            theme: &theme,
            ui,
        };
        compositor.handle_mouse(event, Rect::new(0, 0, 100, 30), &mut cx);
    }

    /// The bar sits before the view on the left and after it on the right.
    #[test]
    fn bar_and_view_follow_the_side() {
        let mut ui = ui();
        let screen = Rect::new(0, 0, 100, 30);
        let layout = ui.layout(screen);
        assert_eq!(layout.activity_bar, Rect::new(0, 0, 4, 29));
        assert_eq!(layout.explorer, Rect::new(4, 0, 30, 29));
        assert_eq!(layout.editor.x, 34);
        ui.sidebar.side = SidebarSide::Right;
        let layout = ui.layout(screen);
        assert_eq!(layout.activity_bar, Rect::new(96, 0, 4, 29));
        assert_eq!(layout.explorer, Rect::new(66, 0, 30, 29));
        assert_eq!(layout.editor.x, 0);
        assert_eq!(layout.editor.right(), 66);
    }

    /// A view that is folded away leaves just the bar.
    #[test]
    fn folded_view_leaves_the_bar() {
        let mut ui = ui();
        ui.toggle_view(SidebarView::Files);
        let layout = ui.layout(Rect::new(0, 0, 100, 30));
        assert!(layout.explorer.is_empty());
        assert_eq!(layout.editor.x, 4);
        ui.toggle_view(SidebarView::Git);
        let layout = ui.layout(Rect::new(0, 0, 100, 30));
        assert_eq!(layout.sidebar_view, Rect::new(4, 0, 30, 29));
        assert!(layout.explorer.is_empty());
    }

    /// Clicking a button shows its view, clicking it again folds the view away.
    #[test]
    fn buttons_switch_views() {
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        let mut ui = ui();
        let mut compositor = Compositor::new();
        compositor.push(Box::new(SourceControlView::new()));
        compositor.push(Box::new(ActivityBar::new()));
        draw(&mut compositor, &mut ui, &mut editor, (100, 30));
        let click = (1, 3);
        let down = MouseEventKind::Down(MouseButton::Left);
        mouse(&mut compositor, &mut ui, &mut editor, down, click);
        assert_eq!(ui.sidebar.view, Some(SidebarView::Git));
        draw(&mut compositor, &mut ui, &mut editor, (100, 30));
        mouse(&mut compositor, &mut ui, &mut editor, down, click);
        assert_eq!(ui.sidebar.view, None);
    }

    /// Dragging the edge of the view changes its width on either side.
    #[test]
    fn dragging_the_edge_resizes() {
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        let mut ui = ui();
        let mut compositor = Compositor::new();
        compositor.push(Box::new(SidebarResize::new()));
        draw(&mut compositor, &mut ui, &mut editor, (100, 30));
        let down = MouseEventKind::Down(MouseButton::Left);
        let drag = MouseEventKind::Drag(MouseButton::Left);
        let up = MouseEventKind::Up(MouseButton::Left);
        mouse(&mut compositor, &mut ui, &mut editor, down, (32, 5));
        mouse(&mut compositor, &mut ui, &mut editor, drag, (40, 5));
        mouse(&mut compositor, &mut ui, &mut editor, up, (40, 5));
        assert_eq!(ui.sidebar.width, Some(37));
        assert_eq!(ui.layout(Rect::new(0, 0, 100, 30)).explorer.width, 37);

        ui.sidebar.width = None;
        ui.sidebar.side = SidebarSide::Right;
        draw(&mut compositor, &mut ui, &mut editor, (100, 30));
        mouse(&mut compositor, &mut ui, &mut editor, down, (67, 5));
        mouse(&mut compositor, &mut ui, &mut editor, drag, (60, 5));
        mouse(&mut compositor, &mut ui, &mut editor, up, (60, 5));
        assert_eq!(ui.sidebar.width, Some(36));
    }

    /// Without the sidebar the explorer is a plain column that can still be resized.
    #[test]
    fn explorer_column_resizes_without_the_sidebar() {
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        let mut ui = ui();
        ui.sidebar.open = false;
        let mut compositor = Compositor::new();
        compositor.push(Box::new(SidebarResize::new()));
        draw(&mut compositor, &mut ui, &mut editor, (100, 30));
        let down = MouseEventKind::Down(MouseButton::Left);
        let drag = MouseEventKind::Drag(MouseButton::Left);
        mouse(&mut compositor, &mut ui, &mut editor, down, (29, 5));
        mouse(&mut compositor, &mut ui, &mut editor, drag, (19, 5));
        assert_eq!(ui.layout(Rect::new(0, 0, 100, 30)).explorer.width, 20);
    }

    /// The terminal keeps the height it was given, within what the editor can spare.
    #[test]
    fn terminal_height_is_kept_and_clamped() {
        let mut ui = Ui {
            terminal_open: true,
            terminal_height: Some(10),
            ..Ui::default()
        };
        ui.config.ui.tabs = false;
        ui.config.ui.minimap = false;
        assert_eq!(ui.layout(Rect::new(0, 0, 80, 41)).terminal.height, 10);
        ui.terminal_height = Some(500);
        assert_eq!(ui.layout(Rect::new(0, 0, 80, 41)).terminal.height, 34);
        ui.terminal_height = Some(0);
        assert_eq!(ui.layout(Rect::new(0, 0, 80, 41)).terminal.height, 3);
    }
}
