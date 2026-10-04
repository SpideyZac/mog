//! The compositor layer that draws every registered flair.

use std::{collections::HashSet, time::Duration};

use mog_tui::{Context, Layer, Layout, Ui};
use ratatui::{buffer::Buffer, layout::Rect};

use crate::flair::{Flair, FlairContext};

/// Holds the registered flairs and draws them above the editor.
#[derive(Default)]
pub struct FlairLayer {
    /// The registered flairs in draw order.
    flairs: Vec<Box<dyn Flair>>,
    /// The ids of flairs that are turned off.
    disabled: HashSet<String>,
    /// Whether all flair is turned off.
    hidden: bool,
}

impl FlairLayer {
    /// Creates an empty flair layer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `flair` on top of the ones already registered.
    pub fn register(&mut self, flair: Box<dyn Flair>) {
        self.flairs.push(flair);
    }

    /// Turns the flair with `id` off.
    pub fn disable(&mut self, id: impl Into<String>) {
        self.disabled.insert(id.into());
    }

    /// Shows or hides all flair at once.
    pub fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
    }

    /// Returns the ids of every registered flair.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.flairs.iter().map(|flair| flair.id())
    }

    /// Returns the flairs that should currently run.
    fn active(&self) -> impl Iterator<Item = &dyn Flair> {
        self.flairs
            .iter()
            .filter(|flair| !self.hidden && !self.disabled.contains(flair.id()))
            .map(AsRef::as_ref)
    }
}

impl Layer for FlairLayer {
    fn area(&self, layout: &Layout, _ui: &Ui) -> Rect {
        layout.editor
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let flair_cx = FlairContext {
            editor: cx.editor,
            theme: cx.theme,
        };
        for flair in self.active() {
            let flair_area = flair.placement().area(area);
            flair.render(flair_area, buf, &flair_cx);
        }
    }

    fn tick(&mut self, dt: Duration) {
        if self.hidden {
            return;
        }
        for flair in &mut self.flairs {
            if !self.disabled.contains(flair.id()) {
                flair.tick(dt);
            }
        }
    }

    fn is_animating(&self) -> bool {
        self.active().any(Flair::is_animating)
    }
}
