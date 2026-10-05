//! The settings menu, and how each setting maps onto the config.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_config::{Config, SettingValue};
use mog_core::{Key, KeyChord, fuzzy_match};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Style,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    compositor::{Context, EventResult, Layer},
    popup,
    theme::Theme,
    ui::{Layout, Overlay, Ui},
};

/// The width of the settings popup.
const WIDTH: u16 = 92;

/// The height of the settings popup.
const HEIGHT: u16 = 30;

/// The volume change per step.
const VOLUME_STEP: i64 = 10;

/// The opacity change per step.
const OPACITY_STEP: i64 = 10;

/// A setting that can be changed in the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingKey {
    /// A toggle in the `[ui]` table, by field name.
    Ui(&'static str),
    /// The color theme.
    Theme,
    /// How solid backgrounds are.
    Opacity,
    /// A toggle in the `[editor]` table, by field name.
    Editor(&'static str),
    /// The tab width.
    TabWidth,
    /// Whether any flair is shown.
    FlairEnabled,
    /// Whether the flair with this id is shown.
    Flair(String),
    /// A toggle in the `[audio]` table, by field name.
    Audio(&'static str),
    /// Whether AI ghost text is on.
    GhostText,
    /// The volume.
    Volume,
    /// Whether Discord shows what you are editing.
    Discord,
    /// A toggle in the `[updates]` table, by field name.
    Updates(&'static str),
}

/// The built in toggles as `(category, key, label, help)`.
const TOGGLES: &[(&str, SettingKey, &str, &str)] = &[
    (
        "look",
        SettingKey::Ui("tabs"),
        "Tab bar",
        "Show open files as tabs along the top.",
    ),
    (
        "look",
        SettingKey::Ui("line_numbers"),
        "Line numbers",
        "Number every line in the gutter.",
    ),
    (
        "look",
        SettingKey::Ui("relative_line_numbers"),
        "Relative line numbers",
        "Count lines from the cursor, handy for jumping.",
    ),
    (
        "look",
        SettingKey::Ui("cursor_line"),
        "Highlight cursor line",
        "Tint the line the cursor is on.",
    ),
    (
        "look",
        SettingKey::Ui("indent_guides"),
        "Indent guides",
        "Draw thin lines at each indentation level.",
    ),
    (
        "look",
        SettingKey::Ui("rainbow_brackets"),
        "Rainbow brackets",
        "Color nested brackets so you can tell them apart.",
    ),
    (
        "look",
        SettingKey::Ui("syntax_highlighting"),
        "Syntax highlighting",
        "Color code with tree-sitter.",
    ),
    (
        "look",
        SettingKey::Ui("minimap"),
        "Minimap",
        "Show a zoomed out view of the file on the right.",
    ),
    (
        "look",
        SettingKey::Ui("icons"),
        "File icons",
        "Colored markers next to file names.",
    ),
    (
        "look",
        SettingKey::Ui("explorer"),
        "Explorer starts open",
        "Show the file explorer when a folder is opened.",
    ),
    (
        "code",
        SettingKey::Ui("error_lens"),
        "Error lens",
        "Write diagnostics at the end of their line and tint it.",
    ),
    (
        "code",
        SettingKey::Ui("semantic_highlighting"),
        "Semantic colors",
        "Let the language server tell parameters, macros and types apart.",
    ),
    (
        "code",
        SettingKey::Ui("inlay_hints"),
        "Inlay hints",
        "Show inferred types and parameter names in the code, greyed out.",
    ),
    (
        "code",
        SettingKey::Ui("git_gutter"),
        "Git gutter",
        "Mark added, changed and removed lines.",
    ),
    (
        "code",
        SettingKey::Ui("git_blame"),
        "Git blame",
        "Show who last touched the cursor line.",
    ),
    (
        "code",
        SettingKey::Editor("auto_close_brackets"),
        "Auto close brackets",
        "Typing ( also types ).",
    ),
    (
        "code",
        SettingKey::Editor("auto_complete"),
        "Auto complete",
        "Pop up completions while typing.",
    ),
    (
        "code",
        SettingKey::Editor("insert_spaces"),
        "Indent with spaces",
        "Tab inserts spaces instead of a tab char.",
    ),
    (
        "code",
        SettingKey::Editor("alt_gr"),
        "AltGr types symbols",
        "For layouts like QWERTZ or AZERTY. Turn off to use ctrl+alt symbol shortcuts.",
    ),
    (
        "code",
        SettingKey::GhostText,
        "AI ghost text",
        "Gray AI suggestions after the cursor, tab to accept.",
    ),
    (
        "sound",
        SettingKey::Audio("sound_effects"),
        "Sound effects",
        "Clicks, chimes and sad trombones.",
    ),
    (
        "sound",
        SettingKey::Audio("music"),
        "Background music",
        "Chill chiptune that gets tense when the build is broken.",
    ),
    (
        "social",
        SettingKey::Discord,
        "Discord status",
        "Show what you are editing on your Discord profile.",
    ),
    (
        "updates",
        SettingKey::Updates("check"),
        "Check for updates",
        "Look for a new mog release on GitHub when mog starts.",
    ),
    (
        "updates",
        SettingKey::Updates("install"),
        "Install updates",
        "Download new releases by itself so they are ready next time.",
    ),
];

/// One row of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    /// The category heading the row sits under.
    category: &'static str,
    /// What the row changes.
    key: SettingKey,
    /// The row text.
    label: String,
    /// A sentence about the setting.
    help: String,
}

/// Returns the current value of `key` in `config`.
pub fn get(config: &Config, key: &SettingKey) -> SettingValue {
    let ui = &config.ui;
    match key {
        SettingKey::Ui(name) => SettingValue::Bool(match *name {
            "tabs" => ui.tabs,
            "line_numbers" => ui.line_numbers,
            "relative_line_numbers" => ui.relative_line_numbers,
            "cursor_line" => ui.cursor_line,
            "indent_guides" => ui.indent_guides,
            "rainbow_brackets" => ui.rainbow_brackets,
            "syntax_highlighting" => ui.syntax_highlighting,
            "semantic_highlighting" => ui.semantic_highlighting,
            "inlay_hints" => ui.inlay_hints,
            "minimap" => ui.minimap,
            "error_lens" => ui.error_lens,
            "git_gutter" => ui.git_gutter,
            "git_blame" => ui.git_blame,
            "explorer" => ui.explorer,
            "icons" => ui.icons,
            "serious" => ui.serious,
            "reduced_motion" => ui.reduced_motion,
            _ => false,
        }),
        SettingKey::Theme => SettingValue::Text(ui.theme.clone()),
        SettingKey::Opacity => SettingValue::Int(i64::from(ui.opacity)),
        SettingKey::Editor(name) => SettingValue::Bool(match *name {
            "auto_close_brackets" => config.editor.auto_close_brackets,
            "auto_complete" => config.editor.auto_complete,
            "insert_spaces" => config.editor.insert_spaces,
            "alt_gr" => config.editor.alt_gr,
            _ => false,
        }),
        SettingKey::TabWidth => {
            SettingValue::Int(i64::try_from(config.editor.tab_width).unwrap_or(4))
        }
        SettingKey::FlairEnabled => SettingValue::Bool(config.flair.enabled),
        SettingKey::Flair(id) => SettingValue::Bool(!config.flair.disabled.contains(id)),
        SettingKey::Audio(name) => SettingValue::Bool(match *name {
            "sound_effects" => config.audio.sound_effects,
            "music" => config.audio.music,
            _ => false,
        }),
        SettingKey::Volume => SettingValue::Int(i64::from(config.audio.volume)),
        SettingKey::GhostText => SettingValue::Bool(config.ai.ghost_text),
        SettingKey::Discord => SettingValue::Bool(config.discord.enabled),
        SettingKey::Updates(name) => SettingValue::Bool(match *name {
            "check" => config.updates.check,
            "install" => config.updates.install,
            _ => false,
        }),
    }
}

/// Returns a mutable reference to the boolean behind `key`, if it is a plain toggle.
fn toggle_mut<'a>(config: &'a mut Config, key: &SettingKey) -> Option<&'a mut bool> {
    let ui = &mut config.ui;
    Some(match key {
        SettingKey::Ui(name) => match *name {
            "tabs" => &mut ui.tabs,
            "line_numbers" => &mut ui.line_numbers,
            "relative_line_numbers" => &mut ui.relative_line_numbers,
            "cursor_line" => &mut ui.cursor_line,
            "indent_guides" => &mut ui.indent_guides,
            "rainbow_brackets" => &mut ui.rainbow_brackets,
            "syntax_highlighting" => &mut ui.syntax_highlighting,
            "semantic_highlighting" => &mut ui.semantic_highlighting,
            "inlay_hints" => &mut ui.inlay_hints,
            "minimap" => &mut ui.minimap,
            "error_lens" => &mut ui.error_lens,
            "git_gutter" => &mut ui.git_gutter,
            "git_blame" => &mut ui.git_blame,
            "explorer" => &mut ui.explorer,
            "icons" => &mut ui.icons,
            "serious" => &mut ui.serious,
            "reduced_motion" => &mut ui.reduced_motion,
            _ => return None,
        },
        SettingKey::Editor(name) => match *name {
            "auto_close_brackets" => &mut config.editor.auto_close_brackets,
            "auto_complete" => &mut config.editor.auto_complete,
            "insert_spaces" => &mut config.editor.insert_spaces,
            "alt_gr" => &mut config.editor.alt_gr,
            _ => return None,
        },
        SettingKey::Audio(name) => match *name {
            "sound_effects" => &mut config.audio.sound_effects,
            "music" => &mut config.audio.music,
            _ => return None,
        },
        SettingKey::FlairEnabled => &mut config.flair.enabled,
        SettingKey::GhostText => &mut config.ai.ghost_text,
        SettingKey::Discord => &mut config.discord.enabled,
        SettingKey::Updates(name) => match *name {
            "check" => &mut config.updates.check,
            "install" => &mut config.updates.install,
            _ => return None,
        },
        _ => return None,
    })
}

/// Changes `key` by one step: toggles flip, choices and numbers move by `delta`.
pub fn change(config: &mut Config, key: &SettingKey, delta: i64) {
    if let Some(flag) = toggle_mut(config, key) {
        *flag = !*flag;
        return;
    }
    match key {
        SettingKey::Theme => {
            let names = Theme::all_names(config);
            let current = names
                .iter()
                .position(|name| name.eq_ignore_ascii_case(&config.ui.theme))
                .unwrap_or(0);
            let count = i64::try_from(names.len()).unwrap_or(1);
            let next = (i64::try_from(current).unwrap_or(0) + delta).rem_euclid(count);
            config.ui.theme = names[usize::try_from(next).unwrap_or(0)].to_owned();
        }
        SettingKey::TabWidth => {
            let width = i64::try_from(config.editor.tab_width).unwrap_or(4) + delta;
            config.editor.tab_width = usize::try_from(width.clamp(1, 16)).unwrap_or(4);
        }
        SettingKey::Volume => {
            let volume = i64::from(config.audio.volume) + delta * VOLUME_STEP;
            config.audio.volume = u8::try_from(volume.clamp(0, 100)).unwrap_or(50);
        }
        SettingKey::Opacity => {
            let opacity = i64::from(config.ui.opacity) + delta * OPACITY_STEP;
            config.ui.opacity = u8::try_from(opacity.clamp(0, 100)).unwrap_or(100);
        }
        SettingKey::Flair(id) => {
            let disabled = &mut config.flair.disabled;
            if let Some(at) = disabled.iter().position(|other| other == id) {
                disabled.remove(at);
            } else {
                disabled.push(id.clone());
            }
        }
        _ => {}
    }
}

/// Returns where `key` lives in the config file and the value to write there.
pub fn persisted(config: &Config, key: &SettingKey) -> (Vec<&'static str>, SettingValue) {
    match key {
        SettingKey::Ui(name) => (vec!["ui", name], get(config, key)),
        SettingKey::Theme => (vec!["ui", "theme"], get(config, key)),
        SettingKey::Opacity => (vec!["ui", "opacity"], get(config, key)),
        SettingKey::Editor(name) => (vec!["editor", name], get(config, key)),
        SettingKey::TabWidth => (vec!["editor", "tab_width"], get(config, key)),
        SettingKey::FlairEnabled => (vec!["flair", "enabled"], get(config, key)),
        SettingKey::Flair(_) => (
            vec!["flair", "disabled"],
            SettingValue::List(config.flair.disabled.clone()),
        ),
        SettingKey::Audio(name) => (vec!["audio", name], get(config, key)),
        SettingKey::Volume => (vec!["audio", "volume"], get(config, key)),
        SettingKey::GhostText => (vec!["ai", "ghost_text"], get(config, key)),
        SettingKey::Discord => (vec!["discord", "enabled"], get(config, key)),
        SettingKey::Updates(name) => (vec!["updates", name], get(config, key)),
    }
}

/// Builds every row, flairs included.
fn rows(ui: &Ui) -> Vec<Row> {
    let row = |category, key, label: &str, help: &str| Row {
        category,
        key,
        label: label.to_owned(),
        help: help.to_owned(),
    };
    let mut rows = vec![row(
        "look",
        SettingKey::Theme,
        "Theme",
        "Pick a color palette. Changes right away.",
    )];
    rows.push(row(
        "look",
        SettingKey::Opacity,
        "Opacity",
        "Below 100 a see through terminal shows through mog.",
    ));
    // the toggles are listed look first, then code, then sound, then social
    for (category, key, label, help) in TOGGLES
        .iter()
        .filter(|t| !matches!(t.0, "sound" | "social"))
    {
        rows.push(row(category, key.clone(), label, help));
    }
    rows.push(row(
        "code",
        SettingKey::TabWidth,
        "Tab width",
        "How many columns one indentation level takes.",
    ));
    rows.push(row(
        "flair",
        SettingKey::Ui("serious"),
        "Serious mode",
        "Turns off every flair, sound and music at once. For demos and deadlines.",
    ));
    rows.push(row(
        "flair",
        SettingKey::Ui("reduced_motion"),
        "Reduced motion",
        "Hides sparks, combos, critters and matrix rain, and stops shimmering.",
    ));
    rows.push(row(
        "flair",
        SettingKey::FlairEnabled,
        "All flair",
        "The master switch for every silly extra.",
    ));
    for (id, description) in &ui.flairs {
        rows.push(row("flair", SettingKey::Flair(id.clone()), id, description));
    }
    for (category, key, label, help) in TOGGLES.iter().filter(|t| t.0 == "sound") {
        rows.push(row(category, key.clone(), label, help));
    }
    rows.push(row(
        "sound",
        SettingKey::Volume,
        "Volume",
        "How loud the effects and music are.",
    ));
    for (category, key, label, help) in TOGGLES.iter().filter(|t| t.0 == "social") {
        rows.push(row(category, key.clone(), label, help));
    }
    rows
}

/// Describes a value for display.
fn show(value: &SettingValue) -> (String, bool) {
    match value {
        SettingValue::Bool(true) => ("\u{25cf} on ".into(), true),
        SettingValue::Bool(false) => ("\u{25cb} off".into(), false),
        SettingValue::Int(n) => (format!("\u{25c2} {n} \u{25b8}"), true),
        SettingValue::Text(text) => (format!("\u{25c2} {text} \u{25b8}"), true),
        SettingValue::List(items) => (items.join(", "), true),
    }
}

/// The settings menu.
#[derive(Debug, Default)]
pub struct SettingsPanel {
    /// The popup generation the rows were built for.
    generation: u64,
    /// Every row.
    rows: Vec<Row>,
    /// The rows that match the filter, as indexes into `rows`.
    visible: Vec<usize>,
    /// What was typed to filter rows.
    filter: String,
    /// The highlighted entry of `visible`.
    selected: usize,
    /// The first visible entry of `visible`.
    scroll: usize,
    /// The list area and the visible index on each of its lines, from the last render.
    lines: Vec<(u16, usize)>,
    /// The popup box from the last render.
    area: Rect,
}

impl SettingsPanel {
    /// Creates the settings menu.
    pub fn new() -> Self {
        Self::default()
    }

    /// Recomputes which rows match the filter.
    fn refilter(&mut self) {
        self.visible = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                fuzzy_match(&self.filter, &format!("{} {}", row.label, row.category)).is_some()
            })
            .map(|(index, _)| index)
            .collect();
        self.selected = self.selected.min(self.visible.len().saturating_sub(1));
    }

    /// Changes the highlighted setting by `delta` and tells the app.
    fn change_selected(&mut self, cx: &mut Context<'_>, delta: i64) {
        let Some(row) = self.visible.get(self.selected).map(|&i| &self.rows[i]) else {
            return;
        };
        change(&mut cx.ui.config, &row.key, delta);
        cx.ui.setting_changes.push(row.key.clone());
    }
}

impl Layer for SettingsPanel {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        if ui.overlay == Some(Overlay::Settings) {
            layout.screen
        } else {
            Rect::default()
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        if self.generation != cx.ui.overlay_generation {
            self.generation = cx.ui.overlay_generation;
            self.rows = rows(cx.ui);
            self.filter.clear();
            self.selected = 0;
            self.scroll = 0;
            self.refilter();
        }
        let theme = cx.theme;
        self.area = popup::centered(area, WIDTH, HEIGHT);
        popup::dim_around(area, self.area, buf, theme);
        let inner = popup::frame(self.area, buf, theme, "\u{2699} settings");
        if inner.height < 6 || inner.width < 30 {
            return;
        }
        let x = inner.x + 1;
        let width = inner.width - 2;
        let filter = if self.filter.is_empty() {
            (
                "type to filter, arrows to move, enter to toggle, left and right to cycle"
                    .to_owned(),
                theme.popup_dim,
            )
        } else {
            (self.filter.clone(), theme.popup)
        };
        buf.set_string(x, inner.y, "\u{276f} ", theme.popup_title);
        buf.set_stringn(x + 2, inner.y, &filter.0, usize::from(width - 2), filter.1);

        let help_y = inner.bottom() - 1;
        let list_top = inner.y + 2;
        let list_height = usize::from(help_y.saturating_sub(list_top + 1));
        // lay out rows with a heading line whenever the category changes
        let mut lines: Vec<(Option<&str>, Option<usize>)> = Vec::new();
        let mut last_category = None;
        for (index, &row) in self.visible.iter().enumerate() {
            let category = self.rows[row].category;
            if last_category != Some(category) {
                lines.push((Some(category), None));
                last_category = Some(category);
            }
            lines.push((None, Some(index)));
        }
        let selected_line = lines
            .iter()
            .position(|(_, index)| *index == Some(self.selected))
            .unwrap_or(0);
        if selected_line < self.scroll {
            self.scroll = selected_line.saturating_sub(1);
        } else if selected_line >= self.scroll + list_height {
            self.scroll = selected_line + 1 - list_height;
        }
        self.lines.clear();
        let value_width = 18;
        for (offset, (heading, index)) in
            lines.iter().enumerate().skip(self.scroll).take(list_height)
        {
            let y = list_top + u16::try_from(offset - self.scroll).unwrap_or(0);
            if let Some(heading) = heading {
                let icon = match *heading {
                    "look" => "\u{2726}",
                    "code" => "\u{2692}",
                    "flair" => "\u{273f}",
                    "social" => "\u{263a}",
                    _ => "\u{266b}",
                };
                buf.set_string(
                    x,
                    y,
                    format!("{icon} {}", heading.to_uppercase()),
                    theme.popup_title,
                );
                continue;
            }
            let Some(index) = *index else { continue };
            let row = &self.rows[self.visible[index]];
            let selected = index == self.selected;
            let base = if selected {
                theme.popup_selected
            } else {
                theme.popup
            };
            buf.set_style(Rect::new(x, y, width, 1), base);
            let pointer = if selected { "\u{25b8} " } else { "  " };
            buf.set_string(x, y, pointer, base.patch(theme.popup_title));
            buf.set_stringn(
                x + 2,
                y,
                &row.label,
                usize::from(width.saturating_sub(value_width + 3)),
                base,
            );
            let (text, on) = show(&get(&cx.ui.config, &row.key));
            let style = if on {
                base.patch(Style::new().fg(theme.palette.accent2))
            } else {
                base.patch(theme.popup_dim)
            };
            let text_width = u16::try_from(text.width()).unwrap_or(0);
            buf.set_string(x + width - text_width.min(width), y, &text, style);
            self.lines.push((y, index));
        }
        if let Some(row) = self.visible.get(self.selected).map(|&i| &self.rows[i]) {
            let rule = "\u{2500}".repeat(usize::from(width));
            buf.set_string(x, help_y - 1, rule, theme.popup_border);
            buf.set_stringn(x, help_y, &row.help, usize::from(width), theme.popup_dim);
        }
        if self.visible.is_empty() {
            buf.set_string(x, list_top, "no settings match", theme.popup_dim);
        }
    }

    fn handle_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        if cx.ui.overlay != Some(Overlay::Settings) {
            return EventResult::Ignored;
        }
        let last = self.visible.len().saturating_sub(1);
        match chord.key {
            Key::Esc => cx.ui.close(),
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down | Key::Tab => self.selected = (self.selected + 1).min(last),
            Key::PageUp => self.selected = self.selected.saturating_sub(10),
            Key::PageDown => self.selected = (self.selected + 10).min(last),
            Key::Enter | Key::Char(' ') if !chord.mods.ctrl => self.change_selected(cx, 1),
            Key::Right => self.change_selected(cx, 1),
            Key::Left => self.change_selected(cx, -1),
            Key::Backspace => {
                self.filter.pop();
                self.refilter();
            }
            _ => match chord.typed_char() {
                Some(ch) => {
                    self.filter.push(ch);
                    self.selected = 0;
                    self.refilter();
                }
                None => return EventResult::Ignored,
            },
        }
        EventResult::Consumed
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        let inside = self.area.contains(Position::new(event.column, event.row));
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if !inside => cx.ui.close(),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(&(_, index)) = self.lines.iter().find(|(y, _)| *y == event.row) {
                    self.selected = index;
                    self.change_selected(cx, 1);
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if let Some(&(_, index)) = self.lines.iter().find(|(y, _)| *y == event.row) {
                    self.selected = index;
                    self.change_selected(cx, -1);
                }
            }
            MouseEventKind::ScrollUp => self.selected = self.selected.saturating_sub(3),
            MouseEventKind::ScrollDown => {
                self.selected = (self.selected + 3).min(self.visible.len().saturating_sub(1));
            }
            _ => {}
        }
        EventResult::Consumed
    }
}

#[cfg(test)]
/// Tests for settings.
mod tests {
    use mog_config::{Config, SettingValue};

    use super::{SettingKey, TOGGLES, change, get, persisted};

    /// Every built in toggle reads and flips.
    #[test]
    fn toggles_flip() {
        let mut config = Config::default();
        for (_, key, label, _) in TOGGLES {
            let before = get(&config, key);
            change(&mut config, key, 1);
            assert_ne!(get(&config, key), before, "{label}");
        }
    }

    /// Themes cycle both ways and numbers clamp.
    #[test]
    fn cycles_and_clamps() {
        let mut config = Config::default();
        change(&mut config, &SettingKey::Theme, 1);
        assert_eq!(config.ui.theme, "synthwave");
        change(&mut config, &SettingKey::Theme, -2);
        assert_eq!(config.ui.theme, "paper");
        for _ in 0..20 {
            change(&mut config, &SettingKey::Volume, 1);
        }
        assert_eq!(config.audio.volume, 100);
    }

    /// Turning a flair off adds it to the disabled list that gets saved.
    #[test]
    fn flair_toggles_save_the_list() {
        let mut config = Config::default();
        let key = SettingKey::Flair("badge".into());
        change(&mut config, &key, 1);
        assert_eq!(get(&config, &key), SettingValue::Bool(false));
        assert_eq!(
            persisted(&config, &key),
            (
                vec!["flair", "disabled"],
                SettingValue::List(vec!["badge".into()])
            )
        );
    }
}
