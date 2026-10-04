//! The compositor layer that draws every registered flair.

use std::{collections::HashSet, time::Duration};

use mog_tui::{Context, Layer, Layout, Ui};
use ratatui::{buffer::Buffer, layout::Rect};

use crate::flair::{Flair, FlairContext, Placement};

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

    /// Returns every registered flair as `(id, description)`.
    pub fn describe(&self) -> Vec<(String, String)> {
        self.flairs
            .iter()
            .map(|flair| (flair.id().to_owned(), flair.description().to_owned()))
            .collect()
    }

    /// Returns whether the flair called `id` should run.
    fn is_on(&self, id: &str) -> bool {
        !self.hidden && !self.disabled.contains(id)
    }
}

impl Layer for FlairLayer {
    fn area(&self, layout: &Layout, _ui: &Ui) -> Rect {
        layout.screen
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        // the settings menu changes the config so follow it every frame
        self.hidden = !cx.ui.config.flair.enabled;
        self.disabled = cx.ui.config.flair.disabled.iter().cloned().collect();
        let layout = cx.ui.layout(area);
        let flair_cx = FlairContext {
            editor: cx.editor,
            theme: cx.theme,
            ui: cx.ui,
            layout,
        };
        let mut segments = Vec::new();
        let (hidden, disabled) = (self.hidden, &self.disabled);
        for flair in &mut self.flairs {
            if hidden || disabled.contains(flair.id()) {
                continue;
            }
            for event in &flair_cx.ui.events {
                flair.observe(event);
            }
            match flair.placement() {
                Placement::Status(_) => segments.extend(flair.segment(&flair_cx)),
                placement => {
                    let flair_area = placement.area(&layout);
                    if !flair_area.is_empty() {
                        flair.render(flair_area, buf, &flair_cx);
                    }
                }
            }
        }
        cx.ui.segments.extend(segments);
    }

    fn tick(&mut self, dt: Duration) {
        let (hidden, disabled) = (self.hidden, &self.disabled);
        for flair in &mut self.flairs {
            if !hidden && !disabled.contains(flair.id()) {
                flair.tick(dt);
            }
        }
    }

    fn is_animating(&self) -> bool {
        self.flairs
            .iter()
            .any(|flair| self.is_on(flair.id()) && flair.is_animating())
    }
}
