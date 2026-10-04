//! A popup for making a theme by dragging colors around.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_config::theme::COLOR_NAMES;
use mog_core::{Command, Key, KeyChord};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
};

use crate::{
    compositor::{Context, EventResult, Layer},
    popup,
    theme::{Palette, Theme, from_hsl, parse_hex, to_hex, to_hsl},
    ui::{Layout, Overlay, Ui},
};

/// The width of the popup.
const WIDTH: u16 = 78;

/// The height of the popup.
const HEIGHT: u16 = 21;

/// The width of the color list on the left.
const LIST_WIDTH: u16 = 23;

/// The command that cancels editing and puts the old theme back.
pub const CANCEL_COMMAND: &str = "theme.cancel";

/// The command that asks for a name and saves the theme.
pub const SAVE_COMMAND: &str = "theme.save";

/// The names of the sliders, in order.
const SLIDERS: [&str; 3] = ["hue", "sat", "light"];

/// A theme being edited, shared with the app so it can preview it live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeDraft {
    /// The name of the theme editing started from.
    pub name: String,
    /// The colors as edited so far.
    pub palette: Palette,
    /// The colors before editing, for resetting one color.
    pub original: Palette,
}

impl ThemeDraft {
    /// Starts editing a copy of `theme`.
    pub fn new(theme: &Theme) -> Self {
        Self {
            name: theme.name.clone(),
            palette: theme.palette,
            original: theme.palette,
        }
    }
}

/// What the mouse is dragging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drag {
    /// A swatch from the list, to drop on another color and swap them.
    Swatch(usize),
    /// The hue and lightness field.
    Field,
    /// One of the sliders.
    Slider(usize),
}

/// The theme editor popup.
#[derive(Debug, Default)]
pub struct ThemeEditor {
    /// The popup generation the editor was reset for.
    generation: u64,
    /// The color being edited, an index into [`COLOR_NAMES`].
    selected: usize,
    /// The slider the arrow keys move.
    slider: usize,
    /// The hue, saturation and lightness of the selected color.
    ///
    /// Kept separately so hue survives dragging saturation to zero.
    hsl: (f32, f32, f32),
    /// What is being dragged and where the pointer is.
    drag: Option<(Drag, Position)>,
    /// A hex color being typed, after pressing `#`.
    hex: Option<String>,
    /// The popup box from the last render.
    area: Rect,
    /// The list row of every color from the last render.
    rows: Vec<(u16, usize)>,
    /// The hue and lightness field from the last render.
    field: Rect,
    /// The bars of the sliders from the last render.
    bars: [Rect; 3],
}

/// Returns black or white, whichever reads better on `color`.
fn contrast(color: Color) -> Color {
    if to_hsl(color).2 > 0.55 {
        Color::Black
    } else {
        Color::White
    }
}

/// Returns how far `at` is along `len` cells, from 0 to 1, aiming at cell centers.
fn fraction(at: u16, start: u16, len: u16) -> f32 {
    let offset = f32::from(at.saturating_sub(start).min(len.saturating_sub(1)));
    (offset + 0.5) / f32::from(len.max(1))
}

impl ThemeEditor {
    /// Creates the theme editor.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads the selected color from `draft` into the sliders.
    fn load(&mut self, draft: &ThemeDraft) {
        self.hsl = to_hsl(draft.palette.get(self.selected));
    }

    /// Writes the sliders into the selected color of `draft`.
    fn store(&self, draft: &mut ThemeDraft) {
        let (h, s, l) = self.hsl;
        draft.palette.set(self.selected, from_hsl(h, s, l));
    }

    /// Selects color `index`.
    fn select(&mut self, index: usize, draft: &ThemeDraft) {
        self.selected = index % COLOR_NAMES.len();
        self.load(draft);
    }

    /// Moves slider `index` by `steps`.
    fn nudge(&mut self, index: usize, steps: f32) {
        let (h, s, l) = &mut self.hsl;
        match index {
            0 => *h = (*h + steps * 2.0).rem_euclid(360.0),
            1 => *s = (*s + steps / 100.0).clamp(0.0, 1.0),
            _ => *l = (*l + steps / 100.0).clamp(0.0, 1.0),
        }
    }

    /// Sets slider `index` to `value` from 0 to 1.
    fn set_slider(&mut self, index: usize, value: f32) {
        let (h, s, l) = &mut self.hsl;
        match index {
            0 => *h = value * 360.0,
            1 => *s = value,
            _ => *l = value,
        }
    }

    /// Returns the color slider `index` would make at `value` from 0 to 1.
    fn slider_color(&self, index: usize, value: f32) -> Color {
        let (h, s, l) = self.hsl;
        match index {
            0 => from_hsl(value * 360.0, s.max(0.5), l.clamp(0.25, 0.75)),
            1 => from_hsl(h, value, l),
            _ => from_hsl(h, s, value),
        }
    }

    /// Returns the value of slider `index` from 0 to 1.
    fn slider_value(&self, index: usize) -> f32 {
        let (h, s, l) = self.hsl;
        match index {
            0 => h / 360.0,
            1 => s,
            _ => l,
        }
    }

    /// Returns the color list index of the row at `y`.
    fn row_at(&self, y: u16) -> Option<usize> {
        self.rows
            .iter()
            .find(|(row, _)| *row == y)
            .map(|(_, index)| *index)
    }

    /// Changes the colors under the pointer at `at` while dragging `drag`.
    fn drag_to(&mut self, drag: Drag, at: Position, draft: &mut ThemeDraft) {
        match drag {
            Drag::Field => {
                let field = self.field;
                self.hsl.0 = fraction(at.x, field.x, field.width) * 360.0;
                self.hsl.2 = 1.0 - fraction(at.y, field.y, field.height);
                self.store(draft);
            }
            Drag::Slider(index) => {
                let bar = self.bars[index];
                self.set_slider(index, fraction(at.x, bar.x, bar.width));
                self.store(draft);
            }
            Drag::Swatch(_) => {}
        }
    }

    /// Draws the color list into `area`.
    fn render_list(&mut self, area: Rect, buf: &mut Buffer, cx: &Context<'_>, draft: &ThemeDraft) {
        let theme = cx.theme;
        self.rows.clear();
        let target = match self.drag {
            Some((Drag::Swatch(from), at)) => self.row_at_y(area, at.y).filter(|&to| to != from),
            _ => None,
        };
        for (index, name) in COLOR_NAMES.iter().enumerate() {
            let y = area.y + u16::try_from(index).unwrap_or(u16::MAX);
            if y >= area.bottom() {
                break;
            }
            self.rows.push((y, index));
            let color = draft.palette.get(index);
            let selected = index == self.selected;
            let style = if Some(index) == target {
                theme.popup_selected.add_modifier(Modifier::BOLD)
            } else if selected {
                theme.popup_selected
            } else {
                theme.popup
            };
            buf.set_style(Rect::new(area.x, y, area.width, 1), style);
            let pointer = if Some(index) == target {
                "\u{21c4} "
            } else if selected {
                "\u{276f} "
            } else {
                "  "
            };
            buf.set_string(area.x, y, pointer, style.patch(theme.popup_title));
            buf.set_string(area.x + 2, y, "  ", Style::new().bg(color));
            let changed = color != draft.original.get(index);
            let label = format!(" {name:<8}{}", if changed { "*" } else { " " });
            buf.set_string(area.x + 4, y, label, style);
            buf.set_string(area.x + 14, y, to_hex(color), style.patch(theme.popup_dim));
        }
    }

    /// Returns the color index listed on row `y` of a list starting at `area`.
    fn row_at_y(&self, area: Rect, y: u16) -> Option<usize> {
        let index = usize::from(y.checked_sub(area.y)?);
        (index < COLOR_NAMES.len()).then_some(index)
    }

    /// Draws the hue and lightness field into `area`, two samples per cell.
    fn render_field(&mut self, area: Rect, buf: &mut Buffer) {
        self.field = area;
        let (h, s, l) = self.hsl;
        let samples = f32::from(area.height) * 2.0;
        for row in 0..area.height {
            let top = 1.0 - (f32::from(row) * 2.0 + 0.5) / samples;
            let bottom = 1.0 - (f32::from(row) * 2.0 + 1.5) / samples;
            for col in 0..area.width {
                let hue = fraction(area.x + col, area.x, area.width) * 360.0;
                if let Some(cell) = buf.cell_mut((area.x + col, area.y + row)) {
                    cell.set_char('\u{2580}')
                        .set_fg(from_hsl(hue, s, top))
                        .set_bg(from_hsl(hue, s, bottom));
                }
            }
        }
        // the clamps keep the casts inside the field
        let col = ((h / 360.0) * f32::from(area.width)).clamp(0.0, f32::from(area.width) - 1.0);
        let row = ((1.0 - l) * f32::from(area.height)).clamp(0.0, f32::from(area.height) - 1.0);
        let at = (area.x + col as u16, area.y + row as u16);
        if let Some(cell) = buf.cell_mut(at) {
            let under = from_hsl(h, s, l);
            cell.set_char('\u{25c6}')
                .set_fg(contrast(under))
                .set_bg(under);
        }
    }

    /// Draws the three sliders, one per row starting at `area`.
    fn render_sliders(&mut self, area: Rect, buf: &mut Buffer, cx: &Context<'_>) {
        let theme = cx.theme;
        for (index, name) in SLIDERS.iter().enumerate() {
            let y = area.y + u16::try_from(index).unwrap_or(0) * 2;
            if y >= area.bottom() {
                break;
            }
            let active = index == self.slider;
            let label_style = if active {
                theme.popup_title
            } else {
                theme.popup_dim
            };
            buf.set_string(area.x, y, format!("{name:<6}"), label_style);
            let bar = Rect::new(area.x + 6, y, area.width.saturating_sub(12), 1);
            self.bars[index] = bar;
            for col in 0..bar.width {
                let value = fraction(bar.x + col, bar.x, bar.width);
                if let Some(cell) = buf.cell_mut((bar.x + col, y)) {
                    cell.set_char(' ').set_bg(self.slider_color(index, value));
                }
            }
            let value = self.slider_value(index);
            let col = (value * f32::from(bar.width)).clamp(0.0, f32::from(bar.width) - 1.0);
            // the clamp keeps the cast inside the bar
            let x = bar.x + col as u16;
            if let Some(cell) = buf.cell_mut((x, y)) {
                let under = self.slider_color(index, value);
                cell.set_char('\u{2503}').set_fg(contrast(under));
            }
            let text = if index == 0 {
                format!(" {:>3}\u{b0}", self.hsl.0.round())
            } else {
                format!(" {:>3}%", (value * 100.0).round())
            };
            buf.set_string(bar.right(), y, text, theme.popup);
        }
    }
}

impl Layer for ThemeEditor {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        if ui.overlay == Some(Overlay::ThemeEditor) && ui.theme_draft.is_some() {
            layout.screen
        } else {
            Rect::default()
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let Some(draft) = cx.ui.theme_draft.clone() else {
            return;
        };
        if self.generation != cx.ui.overlay_generation {
            self.generation = cx.ui.overlay_generation;
            self.drag = None;
            self.hex = None;
            self.select(self.selected, &draft);
        }
        let theme = cx.theme;
        // no dimming around it, the editor behind is the live preview
        self.area = popup::centered(area, WIDTH, HEIGHT);
        let title = format!("\u{25d0} theme editor, from {}", draft.name);
        let inner = popup::frame(self.area, buf, theme, &title);
        if inner.height < 4 || inner.width < LIST_WIDTH + 10 {
            return;
        }
        let body = Rect {
            height: inner.height - 2,
            ..inner
        };
        let list = Rect {
            width: LIST_WIDTH,
            ..body
        };
        self.render_list(list, buf, cx, &draft);
        let right = Rect {
            x: list.right() + 2,
            width: body.width.saturating_sub(LIST_WIDTH + 2),
            ..body
        };
        let sliders_height = 5;
        let field = Rect {
            height: right.height.saturating_sub(sliders_height + 1),
            ..right
        };
        self.render_field(field, buf);
        let sliders = Rect {
            y: field.bottom() + 1,
            height: sliders_height,
            ..right
        };
        self.render_sliders(sliders, buf, cx);
        let help_y = inner.bottom() - 1;
        let help = match &self.hex {
            Some(text) => format!("hex color: #{text}\u{2581}   enter apply, esc cancel"),
            None => "drag swatches to swap  tab \u{2190}\u{2192} sliders  # hex  r reset  enter \
                     save  esc cancel"
                .to_owned(),
        };
        let style = if self.hex.is_some() {
            theme.popup_title
        } else {
            theme.popup_dim
        };
        buf.set_stringn(inner.x, help_y, help, usize::from(inner.width), style);
        if let Some((Drag::Swatch(from), at)) = self.drag
            && let Some(cell) = buf.cell_mut((at.x, at.y))
        {
            cell.set_char('\u{2588}').set_fg(draft.palette.get(from));
        }
    }

    fn handle_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        if cx.ui.overlay != Some(Overlay::ThemeEditor) {
            return EventResult::Ignored;
        }
        let Some(mut draft) = cx.ui.theme_draft.clone() else {
            return EventResult::Ignored;
        };
        if let Some(text) = &mut self.hex {
            match chord.key {
                Key::Esc => self.hex = None,
                Key::Backspace => {
                    text.pop();
                }
                Key::Enter => {
                    if let Some(color) = parse_hex(text) {
                        draft.palette.set(self.selected, color);
                        self.load(&draft);
                    }
                    self.hex = None;
                }
                Key::Char(ch) if ch.is_ascii_hexdigit() && text.len() < 6 => text.push(ch),
                _ => {}
            }
            cx.ui.theme_draft = Some(draft);
            return EventResult::Consumed;
        }
        let step = if chord.mods.shift { 5.0 } else { 1.0 };
        let count = COLOR_NAMES.len();
        match chord.key {
            Key::Esc => {
                cx.ui.request(Command::Custom(CANCEL_COMMAND.into()));
                return EventResult::Consumed;
            }
            Key::Enter => {
                cx.ui.request(Command::Custom(SAVE_COMMAND.into()));
                return EventResult::Consumed;
            }
            Key::Up => self.select(self.selected + count - 1, &draft),
            Key::Down => self.select(self.selected + 1, &draft),
            Key::Tab if chord.mods.shift => self.slider = (self.slider + 2) % 3,
            Key::Tab => self.slider = (self.slider + 1) % 3,
            Key::Left => {
                self.nudge(self.slider, -step);
                self.store(&mut draft);
            }
            Key::Right => {
                self.nudge(self.slider, step);
                self.store(&mut draft);
            }
            Key::Char('#') => self.hex = Some(String::new()),
            Key::Char('r') => {
                draft
                    .palette
                    .set(self.selected, draft.original.get(self.selected));
                self.load(&draft);
            }
            _ if chord.mods.ctrl || chord.mods.alt => return EventResult::Ignored,
            _ => {}
        }
        cx.ui.theme_draft = Some(draft);
        EventResult::Consumed
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        let Some(mut draft) = cx.ui.theme_draft.clone() else {
            return EventResult::Ignored;
        };
        let at = Position::new(event.column, event.row);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = self.row_at(at.y).filter(|_| at.x < self.field.x) {
                    self.select(index, &draft);
                    self.drag = Some((Drag::Swatch(index), at));
                } else if self.field.contains(at) {
                    self.drag = Some((Drag::Field, at));
                    self.drag_to(Drag::Field, at, &mut draft);
                } else if let Some(index) = self.bars.iter().position(|bar| bar.contains(at)) {
                    self.slider = index;
                    self.drag = Some((Drag::Slider(index), at));
                    self.drag_to(Drag::Slider(index), at, &mut draft);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some((drag, _)) = self.drag {
                    self.drag = Some((drag, at));
                    self.drag_to(drag, at, &mut draft);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some((Drag::Swatch(from), _)) = self.drag.take()
                    && let Some(to) = self.row_at(at.y).filter(|&to| to != from)
                {
                    let (a, b) = (draft.palette.get(from), draft.palette.get(to));
                    draft.palette.set(from, b);
                    draft.palette.set(to, a);
                    self.select(to, &draft);
                }
                self.drag = None;
            }
            MouseEventKind::ScrollUp => {
                let count = COLOR_NAMES.len();
                self.select(self.selected + count - 1, &draft);
            }
            MouseEventKind::ScrollDown => self.select(self.selected + 1, &draft),
            _ => {}
        }
        cx.ui.theme_draft = Some(draft);
        EventResult::Consumed
    }
}

#[cfg(test)]
/// Tests for the theme editor.
mod tests {
    use ratatui::layout::{Position, Rect};

    use super::{Drag, ThemeDraft, ThemeEditor, fraction};
    use crate::theme::Theme;

    /// Fractions aim at the middle of each cell and stay in range.
    #[test]
    fn fractions_stay_in_range() {
        assert!((fraction(10, 10, 10) - 0.05).abs() < 1e-6);
        assert!((fraction(19, 10, 10) - 0.95).abs() < 1e-6);
        assert!((fraction(50, 10, 10) - 0.95).abs() < 1e-6);
        assert!((fraction(0, 10, 10) - 0.05).abs() < 1e-6);
    }

    /// Dragging a slider changes the selected color only.
    #[test]
    fn slider_drag_changes_selected_color() {
        let mut draft = ThemeDraft::new(&Theme::default());
        let mut editor = ThemeEditor::new();
        editor.select(6, &draft);
        editor.bars[2] = Rect::new(0, 0, 10, 1);
        editor.drag_to(Drag::Slider(2), Position::new(9, 0), &mut draft);
        assert_ne!(draft.palette.accent, draft.original.accent);
        assert_eq!(draft.palette.bg, draft.original.bg);
    }
}
