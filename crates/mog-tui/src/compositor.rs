//! Stacked layers that together make up the screen.

use crossterm::event::{MouseEvent, MouseEventKind};
use mog_core::Editor;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

use crate::theme::Theme;

/// Everything a layer may read or change while drawing or handling input.
pub struct Context<'a> {
    /// The editor state.
    pub editor: &'a mut Editor,
    /// The active theme.
    pub theme: &'a Theme,
}

/// Whether a layer used an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventResult {
    /// The event was used and should not reach lower layers.
    Consumed,
    /// The event should be offered to the next layer down.
    Ignored,
}

/// One piece of the screen, like the editor view, the status line or a flair widget.
pub trait Layer {
    /// Returns the part of `screen` this layer covers.
    fn area(&self, screen: Rect) -> Rect;

    /// Draws the layer into `buf` within `area`.
    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>);

    /// Handles a mouse event that landed in `area`, or that follows a press this layer took.
    fn handle_mouse(
        &mut self,
        _event: MouseEvent,
        _area: Rect,
        _cx: &mut Context<'_>,
    ) -> EventResult {
        EventResult::Ignored
    }

    /// Returns where the terminal cursor should be shown, if this layer owns it.
    fn cursor(&self, _area: Rect, _cx: &Context<'_>) -> Option<Position> {
        None
    }
}

/// The ordered stack of [`Layer`]s, bottom first.
#[derive(Default)]
pub struct Compositor {
    /// The layers, drawn first to last.
    layers: Vec<Box<dyn Layer>>,
    /// The layer that took the last mouse press, which gets drags and the release.
    mouse_owner: Option<usize>,
}

impl Compositor {
    /// Creates an empty compositor.
    pub fn new() -> Self {
        Self::default()
    }

    /// Puts `layer` on top of the stack.
    pub fn push(&mut self, layer: Box<dyn Layer>) {
        self.layers.push(layer);
    }

    /// Draws every layer and places the cursor.
    pub fn render(&mut self, frame: &mut Frame<'_>, cx: &mut Context<'_>) {
        let screen = frame.area();
        frame.buffer_mut().set_style(screen, cx.theme.background);
        for layer in &mut self.layers {
            let area = layer.area(screen).intersection(screen);
            layer.render(area, frame.buffer_mut(), cx);
        }
        let cursor = self
            .layers
            .iter()
            .rev()
            .find_map(|layer| layer.cursor(layer.area(screen), cx));
        if let Some(cursor) = cursor {
            frame.set_cursor_position(cursor);
        }
    }

    /// Routes a mouse event to the layer under it, or to the layer holding the mouse.
    pub fn handle_mouse(
        &mut self,
        event: MouseEvent,
        screen: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        let owner = match event.kind {
            MouseEventKind::Drag(_) | MouseEventKind::Up(_) => self.mouse_owner,
            _ => None,
        };
        if matches!(event.kind, MouseEventKind::Up(_)) {
            self.mouse_owner = None;
        }
        if let Some(index) = owner {
            let layer = &mut self.layers[index];
            let area = layer.area(screen);
            return layer.handle_mouse(event, area, cx);
        }
        let point = Position::new(event.column, event.row);
        for (index, layer) in self.layers.iter_mut().enumerate().rev() {
            let area = layer.area(screen);
            if !area.contains(point) {
                continue;
            }
            if layer.handle_mouse(event, area, cx) == EventResult::Consumed {
                if matches!(event.kind, MouseEventKind::Down(_)) {
                    self.mouse_owner = Some(index);
                }
                return EventResult::Consumed;
            }
        }
        EventResult::Ignored
    }
}
