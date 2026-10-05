//! The compositor layer that draws every registered flair.

use std::{collections::HashSet, time::Duration};

use mog_tui::{Context, Layer, Layout, Segment, Ui};
use ratatui::{buffer::Buffer, layout::Rect};
use unicode_width::UnicodeWidthStr;

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
    /// Whether flair that flashes or moves is hidden and the rest holds still.
    reduced_motion: bool,
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

    /// Returns the rows the turned on sidebar flairs want under the file explorer.
    pub fn sidebar_height(&self) -> u16 {
        self.flairs
            .iter()
            .filter(|flair| self.is_on(flair.as_ref()))
            .map(|flair| match flair.placement() {
                Placement::Sidebar { height } => height,
                _ => 0,
            })
            .sum()
    }

    /// Turns reduced motion on or off for every flair.
    pub fn set_reduced_motion(&mut self, on: bool) {
        if self.reduced_motion != on {
            self.reduced_motion = on;
            for flair in &mut self.flairs {
                flair.set_reduced_motion(on);
            }
        }
    }

    /// Returns whether `flair` should run.
    fn is_on(&self, flair: &dyn Flair) -> bool {
        is_shown(flair, self.hidden, self.reduced_motion, &self.disabled)
    }
}

/// Returns whether `flair` should run with all flair `hidden`, `reduced_motion` and the flairs
/// in `disabled` turned off.
fn is_shown(
    flair: &dyn Flair,
    hidden: bool,
    reduced_motion: bool,
    disabled: &HashSet<String>,
) -> bool {
    !(hidden || (reduced_motion && flair.moves()) || disabled.contains(flair.id()))
}

/// The gap between tab bar pieces.
const TAB_GAP: u16 = 2;

/// Draws `segments` right aligned in `area`, dropping the last ones that do not fit.
fn draw_tab_segments(area: Rect, buf: &mut Buffer, segments: &[Segment]) {
    if area.is_empty() || area.height == 0 {
        return;
    }
    let width = |segment: &Segment| -> u16 {
        segment
            .parts
            .iter()
            .map(|(text, _)| u16::try_from(text.width()).unwrap_or(u16::MAX))
            .sum()
    };
    let mut shown = segments.len();
    let total =
        |count: usize| -> u16 { segments[..count].iter().map(|s| width(s) + TAB_GAP).sum() };
    while shown > 0 && total(shown) > area.width {
        shown -= 1;
    }
    let mut x = area.right() - total(shown);
    for segment in &segments[..shown] {
        for (text, style) in &segment.parts {
            x = buf
                .set_stringn(x, area.y, text, usize::from(area.right() - x), *style)
                .0;
        }
        x += TAB_GAP;
    }
}

impl Layer for FlairLayer {
    fn area(&self, layout: &Layout, _ui: &Ui) -> Rect {
        layout.screen
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        // the settings menu changes the config so follow it every frame
        let config = &cx.ui.config;
        self.hidden = !config.flair.enabled || config.ui.serious;
        self.disabled = config.flair.disabled.iter().cloned().collect();
        self.set_reduced_motion(config.ui.reduced_motion);
        let layout = cx.ui.layout(area);
        let flair_cx = FlairContext {
            editor: cx.editor,
            theme: cx.theme,
            ui: cx.ui,
            layout,
        };
        let mut segments = Vec::new();
        let mut tab_segments = Vec::new();
        let footer = layout.explorer_footer;
        let mut footer_wanted = 0;
        let mut footer_y = footer.y;
        let (hidden, reduced_motion, disabled) = (self.hidden, self.reduced_motion, &self.disabled);
        for flair in &mut self.flairs {
            if !is_shown(flair.as_ref(), hidden, reduced_motion, disabled) {
                continue;
            }
            for event in &flair_cx.ui.events {
                flair.observe(event);
            }
            match flair.placement() {
                Placement::Status(_) => segments.extend(flair.segment(&flair_cx)),
                Placement::TabBar => tab_segments.extend(flair.segment(&flair_cx)),
                Placement::Sidebar { height } => {
                    footer_wanted += height;
                    if !footer.is_empty() && footer_y + height <= footer.bottom() {
                        let rect = Rect::new(footer.x, footer_y, footer.width, height);
                        flair.render(rect, buf, &flair_cx);
                        footer_y += height;
                    }
                }
                placement => {
                    let flair_area = placement.area(&layout);
                    if !flair_area.is_empty() {
                        flair.render(flair_area, buf, &flair_cx);
                    }
                }
            }
        }
        let free = Rect {
            x: flair_cx.ui.tabs_end + 1,
            width: layout.tabs.right().saturating_sub(flair_cx.ui.tabs_end + 1),
            ..layout.tabs
        };
        draw_tab_segments(free, buf, &tab_segments);
        cx.ui.segments.extend(segments);
        cx.ui.explorer_footer = footer_wanted;
    }

    fn tick(&mut self, dt: Duration) {
        let (hidden, reduced_motion, disabled) = (self.hidden, self.reduced_motion, &self.disabled);
        for flair in &mut self.flairs {
            if is_shown(flair.as_ref(), hidden, reduced_motion, disabled) {
                flair.tick(dt);
            }
        }
    }

    fn is_animating(&self) -> bool {
        self.flairs
            .iter()
            .any(|flair| self.is_on(flair.as_ref()) && flair.is_animating())
    }
}

#[cfg(test)]
/// Tests for the flair layer.
mod tests {
    use mog_tui::{Layer, Segment, Side};
    use ratatui::{buffer::Buffer, layout::Rect, style::Style};

    use super::{FlairLayer, draw_tab_segments};
    use crate::builtin::{Badge, Sparks};

    /// Tab bar pieces hug the right and the ones that do not fit are dropped.
    #[test]
    fn tab_segments_fit_right() {
        let area = Rect::new(0, 0, 12, 1);
        let mut buf = Buffer::empty(area);
        let segments = [
            Segment::new("abc", Style::new(), Side::Right),
            Segment::new("too wide to fit", Style::new(), Side::Right),
        ];
        draw_tab_segments(area, &mut buf, &segments);
        let row: String = (0..12).map(|x| buf[(x, 0)].symbol().to_owned()).collect();
        assert_eq!(row, "       abc  ");
    }

    /// Reduced motion hides flair that moves and stills the rest.
    #[test]
    fn reduced_motion_hides_moving_flair() {
        let mut layer = FlairLayer::new();
        layer.register(Box::new(Sparks::new()));
        layer.register(Box::new(Badge::new()));
        assert!(layer.is_animating());
        layer.set_reduced_motion(true);
        assert!(!layer.is_animating());
        assert!(!layer.is_on(layer.flairs[0].as_ref()));
        assert!(layer.is_on(layer.flairs[1].as_ref()));
    }
}
