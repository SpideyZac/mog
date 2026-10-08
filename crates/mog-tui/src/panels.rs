//! Panels plugins fill with styled text, docked on the right of the editor, under it or in the
//! sidebar.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    compositor::{Context, EventResult, Layer},
    theme::Theme,
    ui::{Layout, SidebarSide, SidebarView, Ui},
    widgets::{WidgetLine, WidgetSpan},
};

/// Where a panel is docked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelSide {
    /// On the right of the editor.
    #[default]
    Right,
    /// Under the editor.
    Bottom,
    /// In the sidebar, with a button in its bar.
    Sidebar,
}

/// A panel a plugin fills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPanel {
    /// The plugin that fills it.
    pub plugin: String,
    /// Its id inside the plugin.
    pub id: String,
    /// What its tab says.
    pub title: String,
    /// The few chars on its sidebar button.
    pub icon: String,
    /// Where it is docked.
    pub side: PanelSide,
    /// Columns wide on the right, rows tall at the bottom, borders included.
    pub size: u16,
    /// Its rows.
    pub lines: Vec<WidgetLine>,
    /// Whether it is shown.
    pub open: bool,
    /// When it was last opened, newer panels win their side.
    pub opened: u64,
    /// The first row shown.
    pub scroll: usize,
}

/// Something that happened to a plugin panel, for the app to pass on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelEvent {
    /// A row was clicked.
    Click {
        /// The plugin that fills the panel.
        plugin: String,
        /// The panel id.
        id: String,
        /// The row, from 0 at the top of the panel's lines.
        line: usize,
        /// The column inside the row, from 0.
        x: u16,
        /// `left`, `right` or `middle`.
        button: &'static str,
    },
    /// The user closed the panel.
    Closed {
        /// The plugin that fills the panel.
        plugin: String,
        /// The panel id.
        id: String,
    },
}

impl Ui {
    /// Returns the index of the panel shown on `side`, the most recently opened.
    pub fn shown_panel(&self, side: PanelSide) -> Option<usize> {
        self.plugin_panels
            .iter()
            .enumerate()
            .filter(|(_, panel)| panel.open && panel.side == side)
            .max_by_key(|(_, panel)| panel.opened)
            .map(|(index, _)| index)
    }

    /// Returns how much room the panel on `side` wants, 0 when none is shown.
    pub fn panel_size(&self, side: PanelSide) -> u16 {
        self.shown_panel(side)
            .map_or(0, |index| self.plugin_panels[index].size)
    }

    /// Shows the panel at `index` on top of the others on its side.
    pub fn raise_panel(&mut self, index: usize) {
        let newest = self
            .plugin_panels
            .iter()
            .map(|panel| panel.opened)
            .max()
            .unwrap_or(0);
        if let Some(panel) = self.plugin_panels.get_mut(index) {
            panel.open = true;
            panel.opened = newest + 1;
        }
    }
}

/// Returns the style of `span` in a panel.
fn span_style(span: &WidgetSpan, theme: &Theme) -> Style {
    let mut style = theme.sidebar;
    if let Some(color) = span.fg.as_deref().and_then(|name| theme.color(name)) {
        style = style.fg(color);
    }
    if let Some(color) = span.bg.as_deref().and_then(|name| theme.color(name)) {
        style = style.bg(color);
    }
    if span.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if span.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if span.underline {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

/// Returns the index of the plugin panel the sidebar shows, if it shows one.
pub fn sidebar_panel(ui: &Ui) -> Option<usize> {
    let Some(SidebarView::Plugin { plugin, id }) = ui.sidebar_active() else {
        return None;
    };
    ui.plugin_panels
        .iter()
        .position(|panel| panel.plugin == *plugin && panel.id == *id)
}

/// What clicking a part of a panel does.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Hit {
    /// Shows the panel at this index.
    Tab(usize),
    /// Closes the panel at this index.
    Close(usize),
    /// The text of the panel at this index, starting at this row of its lines.
    Text(usize, usize),
}

/// Draws the shown plugin panels and reports clicks on them.
#[derive(Debug, Default)]
pub struct PluginPanels {
    /// What each part drawn last frame does, as `(area, what)`.
    hits: Vec<(Rect, Hit)>,
}

impl PluginPanels {
    /// Creates the layer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Draws the panel at `index` and the tabs of the others on its side into `area`.
    fn draw(&mut self, index: usize, area: Rect, buf: &mut Buffer, ui: &Ui, theme: &Theme) {
        let panel = &ui.plugin_panels[index];
        buf.set_style(area, theme.sidebar);
        let (title_row, body) = match panel.side {
            PanelSide::Right => {
                for y in area.y..area.bottom() {
                    buf.set_string(area.x, y, "\u{2502}", theme.border);
                }
                (
                    Rect::new(area.x + 1, area.y, area.width.saturating_sub(1), 1),
                    Rect::new(
                        area.x + 2,
                        area.y + 1,
                        area.width.saturating_sub(3),
                        area.height.saturating_sub(1),
                    ),
                )
            }
            PanelSide::Bottom => {
                let rule = "\u{2500}".repeat(usize::from(area.width));
                buf.set_string(area.x, area.y, rule, theme.border);
                (
                    Rect::new(area.x, area.y, area.width, 1),
                    Rect::new(
                        area.x + 1,
                        area.y + 1,
                        area.width.saturating_sub(2),
                        area.height.saturating_sub(1),
                    ),
                )
            }
            PanelSide::Sidebar => {
                // the rule sits on the edge facing the editor, like the explorer's
                let on_right = ui.sidebar.side == SidebarSide::Right;
                let rule_x = if on_right { area.x } else { area.right() - 1 };
                for y in area.y..area.bottom() {
                    buf.set_string(rule_x, y, "\u{2502}", theme.border);
                }
                let left = if on_right { area.x + 1 } else { area.x };
                let width = area.width.saturating_sub(1);
                (
                    Rect::new(left, area.y, width, 1),
                    Rect::new(
                        left + 1,
                        area.y + 1,
                        width.saturating_sub(2),
                        area.height.saturating_sub(1),
                    ),
                )
            }
        };
        let mut x = title_row.x + 1;
        let tabs: Vec<usize> = if panel.side == PanelSide::Sidebar {
            vec![index]
        } else {
            ui.plugin_panels
                .iter()
                .enumerate()
                .filter(|(_, other)| other.open && other.side == panel.side)
                .map(|(other, _)| other)
                .collect()
        };
        for other in tabs {
            let label = format!(" {} ", ui.plugin_panels[other].title);
            let width = u16::try_from(label.width()).unwrap_or(0);
            if x + width + 3 > title_row.right() {
                break;
            }
            let style = if other == index {
                theme.sidebar_active
            } else {
                theme.sidebar_title
            };
            buf.set_string(x, title_row.y, &label, style);
            self.hits
                .push((Rect::new(x, title_row.y, width, 1), Hit::Tab(other)));
            x += width + 1;
        }
        let close = Rect::new(title_row.right().saturating_sub(2), title_row.y, 1, 1);
        buf.set_string(close.x, close.y, "\u{00d7}", theme.sidebar_title);
        self.hits.push((close, Hit::Close(index)));
        let scroll = panel
            .scroll
            .min(panel.lines.len().saturating_sub(usize::from(body.height)));
        for (y, line) in (body.y..body.bottom()).zip(panel.lines.iter().skip(scroll)) {
            let mut x = body.x;
            for span in line {
                let style = span_style(span, theme);
                for ch in span.text.chars() {
                    let wide = u16::try_from(ch.width().unwrap_or(0)).unwrap_or(0);
                    if wide == 0 {
                        continue;
                    }
                    if x + wide > body.right() {
                        break;
                    }
                    let mut text = [0; 4];
                    buf.set_stringn(x, y, ch.encode_utf8(&mut text), 2, style);
                    x += wide;
                }
            }
        }
        self.hits.push((body, Hit::Text(index, scroll)));
    }
}

impl Layer for PluginPanels {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        let sidebar = layout.sidebar_view.is_empty() || sidebar_panel(ui).is_none();
        if layout.plugin_right.is_empty() && layout.plugin_bottom.is_empty() && sidebar {
            Rect::default()
        } else {
            layout.screen
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let layout = cx.ui.layout(area);
        self.hits.clear();
        for (side, rect) in [
            (PanelSide::Right, layout.plugin_right),
            (PanelSide::Bottom, layout.plugin_bottom),
        ] {
            let rect = rect.intersection(area);
            if let (Some(index), false) = (cx.ui.shown_panel(side), rect.is_empty()) {
                self.draw(index, rect, buf, cx.ui, cx.theme);
            }
        }
        let rect = layout.sidebar_view.intersection(area);
        if let (Some(index), false) = (sidebar_panel(cx.ui), rect.is_empty()) {
            self.draw(index, rect, buf, cx.ui, cx.theme);
        }
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        let point = Position::new(event.column, event.row);
        let Some((rect, hit)) = self
            .hits
            .iter()
            .find(|(rect, _)| rect.contains(point))
            .cloned()
        else {
            return EventResult::Ignored;
        };
        let button = match event.kind {
            MouseEventKind::Down(MouseButton::Left) => "left",
            MouseEventKind::Down(MouseButton::Right) => "right",
            MouseEventKind::Down(MouseButton::Middle) => "middle",
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if let Hit::Text(index, _) = hit
                    && let Some(panel) = cx.ui.plugin_panels.get_mut(index)
                {
                    panel.scroll = if event.kind == MouseEventKind::ScrollUp {
                        panel.scroll.saturating_sub(3)
                    } else {
                        (panel.scroll + 3).min(panel.lines.len().saturating_sub(1))
                    };
                }
                return EventResult::Consumed;
            }
            _ => return EventResult::Consumed,
        };
        match hit {
            Hit::Tab(index) => cx.ui.raise_panel(index),
            Hit::Close(index) => {
                if let Some(panel) = cx.ui.plugin_panels.get_mut(index) {
                    panel.open = false;
                    let event = PanelEvent::Closed {
                        plugin: panel.plugin.clone(),
                        id: panel.id.clone(),
                    };
                    cx.ui.panel_events.push(event);
                }
            }
            Hit::Text(index, scroll) => {
                if let Some(panel) = cx.ui.plugin_panels.get(index) {
                    let line = scroll + usize::from(event.row - rect.y);
                    if line < panel.lines.len() {
                        let event = PanelEvent::Click {
                            plugin: panel.plugin.clone(),
                            id: panel.id.clone(),
                            line,
                            x: event.column - rect.x,
                            button,
                        };
                        cx.ui.panel_events.push(event);
                    }
                }
            }
        }
        EventResult::Consumed
    }
}
