//! Widgets plugins put on the screen: boxes of styled text that can animate, wander and be
//! clicked.

use std::time::{Duration, Instant};

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
    ui::{Layout, Ui},
};

/// The fewest milliseconds a frame of a widget animation is shown.
const MIN_FRAME_MS: f32 = 30.0;

/// Where a widget is measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Anchor {
    /// The top left corner of the editor area, `x` and `y` going right and down.
    #[default]
    TopLeft,
    /// The top right corner of the editor area, `x` going left from the right edge.
    TopRight,
    /// The bottom left corner of the editor area, `y` going up from the bottom edge.
    BottomLeft,
    /// The bottom right corner of the editor area, `x` and `y` going inward.
    BottomRight,
    /// The middle of the editor area.
    Center,
    /// The text cursor, hidden while the cursor is off screen.
    Cursor,
    /// The top left corner of the whole screen, not clipped to the editor area.
    Screen,
}

impl Anchor {
    /// Reads an anchor name like `top_right`.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "top_left" => Self::TopLeft,
            "top_right" => Self::TopRight,
            "bottom_left" => Self::BottomLeft,
            "bottom_right" => Self::BottomRight,
            "center" => Self::Center,
            "cursor" => Self::Cursor,
            "screen" => Self::Screen,
            _ => return None,
        })
    }
}

/// What a wandering widget does at the edge of its area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Edge {
    /// Turns around.
    #[default]
    Bounce,
    /// Leaves on one side and comes back on the other.
    Wrap,
}

/// How a widget drifts on its own, in cells per second.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Motion {
    /// Cells per second to the right, negative for left.
    pub dx: f32,
    /// Cells per second down, negative for up.
    pub dy: f32,
    /// What happens at the edge.
    pub edge: Edge,
}

/// A piece of text in one style.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WidgetSpan {
    /// The text.
    pub text: String,
    /// The text color, a theme color name or hex code.
    pub fg: Option<String>,
    /// The background color, a theme color name or hex code.
    pub bg: Option<String>,
    /// Whether the text is bold.
    pub bold: bool,
    /// Whether the text is italic.
    pub italic: bool,
    /// Whether the text is underlined.
    pub underline: bool,
}

/// One row of a widget.
pub type WidgetLine = Vec<WidgetSpan>;

/// A box of text a plugin drew.
#[derive(Debug, Clone)]
pub struct PluginWidget {
    /// The plugin that drew it.
    pub plugin: String,
    /// Its id inside the plugin.
    pub id: String,
    /// Where it is measured from.
    pub anchor: Anchor,
    /// Cells from the anchor across.
    pub x: i32,
    /// Cells from the anchor down.
    pub y: i32,
    /// The pictures to cycle through, each a list of rows. Most widgets have one.
    pub frames: Vec<Vec<WidgetLine>>,
    /// Frames per second when there are several.
    pub fps: f32,
    /// Whether it is decoration, hidden with the rest of the flair.
    pub flair: bool,
    /// Whether spaces let what is underneath show through.
    pub transparent: bool,
    /// Whether clicking it is reported to the plugin instead of reaching what is underneath.
    pub clickable: bool,
    /// The default text color.
    pub fg: Option<String>,
    /// A background filling the whole box, if any.
    pub bg: Option<String>,
    /// Higher ones are drawn on top.
    pub z: i32,
    /// How it drifts on its own.
    pub motion: Option<Motion>,
    /// When it was drawn, which animations and motion count from.
    pub shown_at: Instant,
}

impl PluginWidget {
    /// Returns the width and height of its largest frame in cells.
    pub fn size(&self) -> (u16, u16) {
        let width = self
            .frames
            .iter()
            .flatten()
            .map(|line| line.iter().map(|span| span.text.width()).sum::<usize>())
            .max()
            .unwrap_or(0);
        let height = self.frames.iter().map(Vec::len).max().unwrap_or(0);
        (
            u16::try_from(width).unwrap_or(u16::MAX),
            u16::try_from(height).unwrap_or(u16::MAX),
        )
    }

    /// Returns which frame shows `elapsed` after it was drawn.
    pub fn frame_at(&self, elapsed: Duration) -> usize {
        if self.frames.len() < 2 || self.fps <= 0.0 {
            return 0;
        }
        let frame_ms = (1000.0 / self.fps).max(MIN_FRAME_MS);
        let step = (elapsed.as_secs_f32() * 1000.0 / frame_ms) as usize;
        step % self.frames.len()
    }

    /// Returns whether it changes over time.
    pub fn moves(&self) -> bool {
        (self.frames.len() > 1 && self.fps > 0.0) || self.motion.is_some()
    }

    /// Returns where its top left corner goes in `area` with the cursor at `cursor`,
    /// `elapsed` after it was drawn, or `None` when it has nowhere to be.
    pub fn place(
        &self,
        area: Rect,
        screen: Rect,
        cursor: Option<Position>,
        elapsed: Duration,
    ) -> Option<(i32, i32)> {
        let (width, height) = self.size();
        let (w, h) = (i32::from(width), i32::from(height));
        let (left, top) = (i32::from(area.x), i32::from(area.y));
        let (right, bottom) = (i32::from(area.right()), i32::from(area.bottom()));
        let (x, y) = match self.anchor {
            Anchor::TopLeft => (left + self.x, top + self.y),
            Anchor::TopRight => (right - w - self.x, top + self.y),
            Anchor::BottomLeft => (left + self.x, bottom - h - self.y),
            Anchor::BottomRight => (right - w - self.x, bottom - h - self.y),
            Anchor::Center => (
                left + (i32::from(area.width) - w) / 2 + self.x,
                top + (i32::from(area.height) - h) / 2 + self.y,
            ),
            Anchor::Cursor => {
                let cursor = cursor?;
                (i32::from(cursor.x) + self.x, i32::from(cursor.y) + self.y)
            }
            Anchor::Screen => (i32::from(screen.x) + self.x, i32::from(screen.y) + self.y),
        };
        let Some(motion) = self.motion else {
            return Some((x, y));
        };
        let bounds = if self.anchor == Anchor::Screen {
            screen
        } else {
            area
        };
        let secs = elapsed.as_secs_f32();
        let drift = |start: i32, speed: f32, low: u16, len: u16, size: i32| -> i32 {
            let low = i32::from(low);
            let len = i32::from(len);
            let moved = start + (speed * secs).floor() as i32;
            match motion.edge {
                Edge::Bounce => {
                    let room = (len - size).max(0);
                    if room == 0 {
                        return low;
                    }
                    let period = room * 2;
                    let at = (moved - low).rem_euclid(period);
                    low + if at > room { period - at } else { at }
                }
                Edge::Wrap => {
                    let span = len + size;
                    low - size + (moved - low + size).rem_euclid(span.max(1))
                }
            }
        };
        Some((
            drift(x, motion.dx, bounds.x, bounds.width, w),
            drift(y, motion.dy, bounds.y, bounds.height, h),
        ))
    }
}

/// A click on a widget, for the app to pass to the plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetClick {
    /// The plugin that drew the widget.
    pub plugin: String,
    /// The widget id.
    pub id: String,
    /// The column inside the widget, from 0.
    pub x: u16,
    /// The row inside the widget, from 0.
    pub y: u16,
    /// `left`, `right` or `middle`.
    pub button: &'static str,
}

/// The shape of the text cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    /// Whatever the terminal is set up to show.
    #[default]
    Default,
    /// A full block, like vim's normal mode.
    Block,
    /// A thin bar between chars.
    Bar,
    /// A line under the char.
    Underline,
}

/// The look of the text cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CursorStyle {
    /// Its shape.
    pub shape: CursorShape,
    /// Whether it blinks.
    pub blink: bool,
}

/// Returns whether flair of `plugin` is shown with the settings in `ui`, hiding flair that
/// `moves` when motion is reduced.
pub(crate) fn plugin_flair_shown(ui: &Ui, plugin: &str, moves: bool) -> bool {
    let config = &ui.config;
    let id = format!("plugin.{plugin}");
    config.flair.enabled
        && !config.ui.serious
        && (!config.ui.reduced_motion || !moves)
        && !config
            .flair
            .disabled
            .iter()
            .any(|off| *off == id || off == "plugins")
}

/// Returns whether `widget` is shown with the flair settings in `ui`.
fn is_shown(widget: &PluginWidget, ui: &Ui) -> bool {
    !widget.flair || plugin_flair_shown(ui, &widget.plugin, widget.motion.is_some())
}

/// Returns the style of `span` over the defaults of its widget.
fn span_style(span: &WidgetSpan, widget: &PluginWidget, theme: &Theme) -> Style {
    let mut style = Style::new();
    let fg = span.fg.as_deref().or(widget.fg.as_deref());
    if let Some(color) = fg.and_then(|name| theme.color(name)) {
        style = style.fg(color);
    } else if !widget.transparent {
        style = style.fg(theme.palette.fg);
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

/// Draws `widget` with its top left corner at `(x, y)`, only inside `clip`.
///
/// Returns the part of the screen it covers.
fn draw_widget(
    widget: &PluginWidget,
    frame: usize,
    (x, y): (i32, i32),
    clip: Rect,
    buf: &mut Buffer,
    theme: &Theme,
) -> Rect {
    let (width, height) = widget.size();
    let inside = |cx: i32, cy: i32| -> Option<(u16, u16)> {
        let (cx, cy) = (u16::try_from(cx).ok()?, u16::try_from(cy).ok()?);
        clip.contains(Position::new(cx, cy)).then_some((cx, cy))
    };
    if let Some(bg) = widget.bg.as_deref().and_then(|name| theme.color(name)) {
        for row in 0..i32::from(height) {
            for col in 0..i32::from(width) {
                if let Some(cell) = inside(x + col, y + row) {
                    buf[cell].set_char(' ').set_bg(bg);
                }
            }
        }
    }
    let Some(lines) = widget.frames.get(frame) else {
        return Rect::default();
    };
    for (row, line) in (0..).zip(lines) {
        let mut col = 0;
        for span in line {
            let style = span_style(span, widget, theme);
            for ch in span.text.chars() {
                let wide = i32::try_from(ch.width().unwrap_or(0)).unwrap_or(0);
                if wide == 0 {
                    continue;
                }
                let skip = widget.transparent && ch == ' ' && span.bg.is_none();
                // a wide char only fits when both of its cells do
                let fits = inside(x + col, y + row).zip(inside(x + col + wide - 1, y + row));
                if let (false, Some(((cx, cy), _))) = (skip, fits) {
                    let mut text = [0; 4];
                    buf.set_stringn(cx, cy, ch.encode_utf8(&mut text), 2, style);
                }
                col += wide;
            }
        }
    }
    let covered = |start: i32, len: u16| -> (u16, u16) {
        let from = u16::try_from(start.max(0)).unwrap_or(u16::MAX);
        let to = u16::try_from((start + i32::from(len)).max(0)).unwrap_or(u16::MAX);
        (from, to.saturating_sub(from))
    };
    let (rx, rw) = covered(x, width);
    let (ry, rh) = covered(y, height);
    Rect::new(rx, ry, rw, rh).intersection(clip)
}

/// Draws the widgets plugins put on the screen and reports clicks on them.
#[derive(Debug, Default)]
pub struct PluginWidgets {
    /// Where each clickable widget was drawn last frame, topmost last, as
    /// `(area, plugin, id)`.
    hits: Vec<(Rect, String, String)>,
    /// Whether a shown widget animates or drifts.
    animating: bool,
}

impl PluginWidgets {
    /// Creates the layer.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Layer for PluginWidgets {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        if ui.plugin_widgets.is_empty() {
            Rect::default()
        } else {
            layout.screen
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let layout = cx.ui.layout(area);
        let editor = if layout.split.is_empty() {
            layout.editor
        } else {
            layout.editor.union(layout.split)
        };
        let now = Instant::now();
        let still = cx.ui.config.ui.reduced_motion;
        let mut order: Vec<&PluginWidget> = cx
            .ui
            .plugin_widgets
            .iter()
            .filter(|widget| is_shown(widget, cx.ui))
            .collect();
        order.sort_by_key(|widget| widget.z);
        self.hits.clear();
        self.animating = false;
        for widget in order {
            // reduced motion shows every widget as it was drawn
            let elapsed = if still {
                Duration::ZERO
            } else {
                now.saturating_duration_since(widget.shown_at)
            };
            let clip = if widget.anchor == Anchor::Screen {
                area
            } else {
                editor
            };
            let frame = widget.frame_at(elapsed);
            let Some(at) = widget.place(editor, area, cx.ui.cursor_screen, elapsed) else {
                continue;
            };
            self.animating |= widget.moves() && !still;
            let drawn = draw_widget(widget, frame, at, clip, buf, cx.theme);
            if widget.clickable && !drawn.is_empty() {
                self.hits
                    .push((drawn, widget.plugin.clone(), widget.id.clone()));
            }
        }
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        let MouseEventKind::Down(button) = event.kind else {
            return EventResult::Ignored;
        };
        if cx.ui.overlay.is_some() {
            return EventResult::Ignored;
        }
        let point = Position::new(event.column, event.row);
        let Some((rect, plugin, id)) = self.hits.iter().rev().find(|(r, ..)| r.contains(point))
        else {
            return EventResult::Ignored;
        };
        cx.ui.widget_clicks.push(WidgetClick {
            plugin: plugin.clone(),
            id: id.clone(),
            x: point.x - rect.x,
            y: point.y - rect.y,
            button: match button {
                MouseButton::Left => "left",
                MouseButton::Right => "right",
                MouseButton::Middle => "middle",
            },
        });
        EventResult::Consumed
    }

    fn is_animating(&self) -> bool {
        self.animating
    }
}

#[cfg(test)]
/// Tests for plugin widgets.
mod tests {
    use std::time::{Duration, Instant};

    use ratatui::{
        buffer::Buffer,
        layout::{Position, Rect},
        style::Style,
    };

    use super::{Anchor, Edge, Motion, PluginWidget, WidgetSpan, draw_widget};
    use crate::theme::Theme;

    /// Returns a widget showing `rows` as plain text.
    fn widget(rows: &[&str]) -> PluginWidget {
        PluginWidget {
            plugin: "p".into(),
            id: "w".into(),
            anchor: Anchor::TopLeft,
            x: 0,
            y: 0,
            frames: vec![
                rows.iter()
                    .map(|row| {
                        vec![WidgetSpan {
                            text: (*row).to_owned(),
                            ..WidgetSpan::default()
                        }]
                    })
                    .collect(),
            ],
            fps: 0.0,
            flair: true,
            transparent: false,
            clickable: false,
            fg: None,
            bg: None,
            z: 0,
            motion: None,
            shown_at: Instant::now(),
        }
    }

    /// Returns row `y` of `buf` as text.
    fn row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_owned())
            .collect()
    }

    /// Corners measure inward and the cursor anchor follows the cursor.
    #[test]
    fn places_by_anchor() {
        let area = Rect::new(10, 1, 40, 20);
        let screen = Rect::new(0, 0, 60, 25);
        let mut boxed = widget(&["abcd", "ef"]);
        let at = |w: &PluginWidget, cursor| w.place(area, screen, cursor, Duration::ZERO);
        assert_eq!(at(&boxed, None), Some((10, 1)));
        boxed.anchor = Anchor::BottomRight;
        boxed.x = 1;
        assert_eq!(at(&boxed, None), Some((45, 19)));
        boxed.anchor = Anchor::Center;
        boxed.x = 0;
        assert_eq!(at(&boxed, None), Some((28, 10)));
        boxed.anchor = Anchor::Cursor;
        boxed.y = 1;
        assert_eq!(at(&boxed, None), None);
        assert_eq!(at(&boxed, Some(Position::new(5, 5))), Some((5, 6)));
        boxed.anchor = Anchor::Screen;
        assert_eq!(at(&boxed, None), Some((0, 1)));
    }

    /// Frames cycle at their rate and a single frame never changes.
    #[test]
    fn cycles_frames() {
        let mut fish = widget(&["><>"]);
        assert_eq!(fish.frame_at(Duration::from_secs(5)), 0);
        fish.frames.push(fish.frames[0].clone());
        fish.fps = 2.0;
        assert_eq!(fish.frame_at(Duration::from_millis(400)), 0);
        assert_eq!(fish.frame_at(Duration::from_millis(600)), 1);
        assert_eq!(fish.frame_at(Duration::from_millis(1100)), 0);
        assert!(fish.moves());
    }

    /// Drifting widgets bounce off the edges or wrap around.
    #[test]
    fn drifts() {
        let area = Rect::new(0, 0, 10, 5);
        let mut fish = widget(&["><>"]);
        fish.motion = Some(Motion {
            dx: 1.0,
            dy: 0.0,
            edge: Edge::Bounce,
        });
        let at = |w: &PluginWidget, secs| {
            w.place(area, area, None, Duration::from_secs(secs))
                .expect("placed")
                .0
        };
        assert_eq!(at(&fish, 3), 3);
        assert_eq!(at(&fish, 7), 7);
        assert_eq!(at(&fish, 9), 5);
        fish.motion = Some(Motion {
            dx: 1.0,
            dy: 0.0,
            edge: Edge::Wrap,
        });
        assert_eq!(at(&fish, 9), 9);
        assert_eq!(at(&fish, 11), -2);
    }

    /// Transparent widgets leave what is under their spaces and drawing stops at the clip.
    #[test]
    fn draws_clipped_and_transparent() {
        let theme = Theme::default();
        let area = Rect::new(0, 0, 8, 2);
        let mut buf = Buffer::empty(area);
        buf.set_string(0, 0, "xxxxxxxx", Style::new());
        let mut cat = widget(&["a b", "cd"]);
        cat.transparent = true;
        let drawn = draw_widget(&cat, 0, (6, 0), area, &mut buf, &theme);
        assert_eq!(row(&buf, 0), "xxxxxxax");
        assert_eq!(row(&buf, 1), "      cd");
        assert_eq!(drawn, Rect::new(6, 0, 2, 2));
    }
}
