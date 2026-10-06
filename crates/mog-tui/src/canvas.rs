//! Flair plugins paint cell by cell over the editor, like rain or a starfield, drawn with the
//! built in flair and under plugin widgets.

use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Style},
};
use unicode_width::UnicodeWidthChar;

use crate::{
    compositor::{Context, Layer},
    ui::{Layout, Ui},
    widgets::plugin_flair_shown,
};

/// One painted cell, at a column and row from the top left of the editor area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasCell {
    /// The column, from 0.
    pub x: u16,
    /// The row, from 0.
    pub y: u16,
    /// The char shown.
    pub ch: char,
    /// The text color, a theme color name or hex code.
    pub fg: Option<String>,
    /// The background color, a theme color name or hex code.
    pub bg: Option<String>,
}

/// The cells one plugin painted for one canvas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCanvas {
    /// The plugin that paints it.
    pub plugin: String,
    /// Its id inside the plugin.
    pub id: String,
    /// The painted cells, later ones over earlier ones.
    pub cells: Vec<CanvasCell>,
    /// Whether it stays still, so reduced motion keeps it.
    pub still: bool,
}

/// Draws plugin canvases as flair.
#[derive(Debug, Default)]
pub struct PluginCanvases;

impl PluginCanvases {
    /// Creates the layer.
    pub fn new() -> Self {
        Self
    }
}

impl Layer for PluginCanvases {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        if ui.plugin_canvases.is_empty() {
            Rect::default()
        } else if layout.split.is_empty() {
            layout.editor
        } else {
            layout.editor.union(layout.split)
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let theme = cx.theme;
        let mut colors: HashMap<String, Option<Color>> = HashMap::new();
        let mut color = |name: &Option<String>| -> Option<Color> {
            let name = name.as_deref()?;
            if let Some(known) = colors.get(name) {
                return *known;
            }
            let found = theme.color(name);
            colors.insert(name.to_owned(), found);
            found
        };
        for canvas in &cx.ui.plugin_canvases {
            if !plugin_flair_shown(cx.ui, &canvas.plugin, !canvas.still) {
                continue;
            }
            for cell in &canvas.cells {
                let (Some(x), Some(y)) = (area.x.checked_add(cell.x), area.y.checked_add(cell.y))
                else {
                    continue;
                };
                let wide = u16::try_from(cell.ch.width().unwrap_or(0)).unwrap_or(0);
                let fits = area.contains(Position::new(x, y))
                    && area.contains(Position::new(x + wide.saturating_sub(1), y));
                if wide == 0 || !fits {
                    continue;
                }
                let mut style = Style::new();
                if let Some(fg) = color(&cell.fg) {
                    style = style.fg(fg);
                }
                if let Some(bg) = color(&cell.bg) {
                    style = style.bg(bg);
                }
                let mut text = [0; 4];
                buf.set_stringn(x, y, cell.ch.encode_utf8(&mut text), 2, style);
            }
        }
    }
}
