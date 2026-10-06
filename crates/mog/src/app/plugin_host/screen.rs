//! Plugins on the screen and the keyboard: widgets they draw, keys they take before the
//! editor, the cursor shape and timers that wake them up.

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    mem,
    time::{Duration, Instant},
};

use mog_core::KeyChord;
use mog_plugin::parse_actions;
use mog_tui::{
    Anchor, CursorShape, CursorStyle, Edge, Focus, Motion, PluginWidget, WidgetLine, WidgetSpan,
};
use ratatui::layout::Rect;
use serde_json::{Value, json};

use crate::{app::App, plugins::PluginUpdate};

/// How long a plugin gets to answer a key before it is dropped.
const KEY_TIMEOUT: Duration = Duration::from_secs(1);

/// The most widgets one plugin can have on screen.
const MAX_WIDGETS: usize = 64;

/// The most frames one widget can cycle through.
const MAX_FRAMES: usize = 64;

/// The widest and tallest a widget can be, in cells.
const MAX_SIZE: usize = 400;

/// The most timers one plugin can have.
const MAX_TIMERS: usize = 16;

/// The shortest time between two timer ticks.
const MIN_TIMER: Duration = Duration::from_millis(30);

/// Which keys a plugin takes before the editor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capture {
    /// Whether it takes every key not in `except`.
    all: bool,
    /// The keys it takes when not taking them all.
    keys: HashSet<KeyChord>,
    /// The keys it leaves alone when taking them all.
    except: HashSet<KeyChord>,
}

impl Capture {
    /// Returns whether the plugin takes `chord`.
    fn takes(&self, chord: &KeyChord) -> bool {
        if self.all {
            !self.except.contains(chord)
        } else {
            self.keys.contains(chord)
        }
    }

    /// Returns whether it takes no key at all.
    fn is_empty(&self) -> bool {
        !self.all && self.keys.is_empty()
    }
}

/// A timer a plugin asked for.
#[derive(Debug, Clone)]
struct Timer {
    /// The plugin that gets the ticks.
    plugin: String,
    /// Its id inside the plugin.
    id: String,
    /// The time between ticks.
    every: Duration,
    /// When it ticks next.
    due: Instant,
}

/// What the app remembers about plugins on the screen and the keyboard.
#[derive(Debug, Default)]
pub struct ScreenState {
    /// The keys each plugin takes.
    captures: BTreeMap<String, Capture>,
    /// Whether a plugin is deciding what to do with a key.
    key_waiting: bool,
    /// Keys pressed while a plugin decides, handled in order once it answers.
    queued_keys: VecDeque<KeyChord>,
    /// The timers plugins asked for.
    timers: Vec<Timer>,
    /// The plugin that last set the cursor shape.
    cursor_owner: Option<String>,
}

/// Reads a list of key names.
///
/// # Errors
///
/// Returns which name is not a key.
fn parse_chords(value: &Value) -> Result<HashSet<KeyChord>, String> {
    let Some(names) = value.as_array() else {
        return Ok(HashSet::new());
    };
    names
        .iter()
        .map(|name| {
            let name = name.as_str().ok_or("keys must be strings")?;
            name.parse::<KeyChord>()
                .map_err(|_| format!("`{name}` is not a key"))
        })
        .collect()
}

/// Reads a `capture` message.
///
/// # Errors
///
/// Returns what is wrong with it, like a key that does not exist.
pub fn parse_capture(value: &Value) -> Result<Capture, String> {
    Ok(match &value["keys"] {
        Value::String(all) if all == "all" => Capture {
            all: true,
            keys: HashSet::new(),
            except: parse_chords(&value["except"])?,
        },
        Value::Null => Capture::default(),
        keys @ Value::Array(_) => Capture {
            all: false,
            keys: parse_chords(keys)?,
            except: HashSet::new(),
        },
        _ => return Err("`keys` must be \"all\" or a list of keys".into()),
    })
}

/// Reads an optional color name.
fn color(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

/// Reads one span of a widget row, a string or an object with `text` and its style.
fn parse_span(value: &Value) -> Result<WidgetSpan, String> {
    match value {
        Value::String(text) => Ok(WidgetSpan {
            text: text.clone(),
            ..WidgetSpan::default()
        }),
        Value::Object(_) => Ok(WidgetSpan {
            text: value["text"]
                .as_str()
                .ok_or("a span needs a string `text`")?
                .to_owned(),
            fg: color(&value["fg"]),
            bg: color(&value["bg"]),
            bold: value["bold"].as_bool().unwrap_or(false),
            italic: value["italic"].as_bool().unwrap_or(false),
            underline: value["underline"].as_bool().unwrap_or(false),
        }),
        _ => Err("a span must be a string or an object".into()),
    }
}

/// Reads the rows of one frame, each a string or a list of spans.
fn parse_frame(value: &Value) -> Result<Vec<WidgetLine>, String> {
    let rows = value.as_array().ok_or("`lines` must be a list")?;
    if rows.len() > MAX_SIZE {
        return Err(format!("a widget can be at most {MAX_SIZE} rows"));
    }
    rows.iter()
        .map(|row| match row {
            Value::Array(spans) => spans.iter().map(parse_span).collect(),
            other => Ok(vec![parse_span(other)?]),
        })
        .collect()
}

/// Reads a `draw` message from `plugin`, or `None` when it has nothing to show.
///
/// # Errors
///
/// Returns what is wrong with it, like an unknown anchor.
pub fn parse_widget(plugin: &str, value: &Value) -> Result<Option<PluginWidget>, String> {
    let id = value["id"].as_str().unwrap_or("widget").to_owned();
    let frames: Vec<Vec<WidgetLine>> = match (&value["frames"], &value["lines"]) {
        (Value::Array(frames), _) => frames.iter().map(parse_frame).collect::<Result<_, _>>()?,
        (_, Value::Null) => Vec::new(),
        (_, lines) => vec![parse_frame(lines)?],
    };
    if frames.len() > MAX_FRAMES {
        return Err(format!("a widget can have at most {MAX_FRAMES} frames"));
    }
    if frames.iter().all(Vec::is_empty) {
        return Ok(None);
    }
    let anchor = match value["anchor"].as_str() {
        None => Anchor::TopLeft,
        Some(name) => Anchor::from_name(name).ok_or_else(|| {
            format!(
                "`{name}` is not an anchor, use top_left, top_right, bottom_left, bottom_right, \
                 center, cursor or screen"
            )
        })?,
    };
    let int = |key: &str| -> i32 {
        value[key]
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .unwrap_or(0)
    };
    let motion = match &value["motion"] {
        Value::Null => None,
        motion => Some(Motion {
            dx: number(&motion["dx"]),
            dy: number(&motion["dy"]),
            edge: match motion["edge"].as_str() {
                Some("wrap") => Edge::Wrap,
                _ => Edge::Bounce,
            },
        }),
    };
    let widget = PluginWidget {
        plugin: plugin.to_owned(),
        id,
        anchor,
        x: int("x"),
        y: int("y"),
        frames,
        fps: number(&value["fps"]),
        flair: value["flair"].as_bool().unwrap_or(true),
        transparent: value["transparent"].as_bool().unwrap_or(false),
        clickable: value["clickable"].as_bool().unwrap_or(false),
        fg: color(&value["fg"]),
        bg: color(&value["bg"]),
        z: int("z"),
        motion,
        shown_at: Instant::now(),
    };
    if usize::from(widget.size().0) > MAX_SIZE {
        return Err(format!("a widget can be at most {MAX_SIZE} cells wide"));
    }
    Ok(Some(widget))
}

/// Reads a number that fits an `f32`, 0 when missing.
fn number(value: &Value) -> f32 {
    value.as_f64().map_or(0.0, |n| n as f32)
}

/// Reads a `cursor` message.
fn parse_cursor(value: &Value) -> Result<CursorStyle, String> {
    let shape = match value["shape"].as_str() {
        None | Some("default") => CursorShape::Default,
        Some("block") => CursorShape::Block,
        Some("bar" | "line") => CursorShape::Bar,
        Some("underline") => CursorShape::Underline,
        Some(other) => {
            return Err(format!(
                "`{other}` is not a cursor shape, use default, block, bar or underline"
            ));
        }
    };
    Ok(CursorStyle {
        shape,
        blink: value["blink"].as_bool().unwrap_or(false),
    })
}

impl App {
    /// Handles the screen and keyboard notifications of a plugin, returning `None` for ones
    /// it does not know.
    pub(super) fn screen_notification(
        &mut self,
        plugin: &str,
        method: &str,
        params: &Value,
    ) -> Option<Result<(), String>> {
        Some(match method {
            "draw" => self.draw_widget(plugin, params),
            "clear" => {
                let id = params["id"].as_str();
                self.ui.plugin_widgets.retain(|widget| {
                    widget.plugin != plugin || id.is_some_and(|id| widget.id != id)
                });
                Ok(())
            }
            "cursor" => parse_cursor(params).map(|style| {
                self.ui.cursor_style = style;
                self.plugin_state.screen.cursor_owner = Some(plugin.to_owned());
            }),
            "capture" => parse_capture(params).map(|capture| self.set_capture(plugin, capture)),
            "timer" => self.set_timer(plugin, params),
            _ => return None,
        })
    }

    /// Puts a widget `plugin` drew on screen, replacing one with the same id.
    fn draw_widget(&mut self, plugin: &str, params: &Value) -> Result<(), String> {
        let widget = parse_widget(plugin, params)?;
        let id = params["id"].as_str().unwrap_or("widget");
        let widgets = &mut self.ui.plugin_widgets;
        let at = widgets
            .iter()
            .position(|widget| widget.plugin == plugin && widget.id == id);
        let Some(mut widget) = widget else {
            if let Some(at) = at {
                widgets.remove(at);
            }
            return Ok(());
        };
        if widget.flair {
            let flair = format!("plugin.{plugin}");
            if !self.ui.flairs.iter().any(|(id, _)| *id == flair) {
                self.ui
                    .flairs
                    .push((flair, format!("Flair drawn by the {plugin} plugin")));
            }
        }
        match at {
            Some(at) => {
                // animations keep going when a widget is moved or changed
                if !params["restart"].as_bool().unwrap_or(false) {
                    widget.shown_at = widgets[at].shown_at;
                }
                widgets[at] = widget;
            }
            None if widgets.iter().filter(|w| w.plugin == plugin).count() >= MAX_WIDGETS => {
                return Err(format!("a plugin can have at most {MAX_WIDGETS} widgets"));
            }
            None => widgets.push(widget),
        }
        Ok(())
    }

    /// Sets the keys `plugin` takes before the editor.
    fn set_capture(&mut self, plugin: &str, capture: Capture) {
        let captures = &mut self.plugin_state.screen.captures;
        if capture.is_empty() {
            captures.remove(plugin);
        } else {
            captures.insert(plugin.to_owned(), capture);
        }
    }

    /// Starts, changes or stops a timer of `plugin`.
    fn set_timer(&mut self, plugin: &str, params: &Value) -> Result<(), String> {
        let id = params["id"].as_str().unwrap_or("timer").to_owned();
        let timers = &mut self.plugin_state.screen.timers;
        timers.retain(|timer| timer.plugin != plugin || timer.id != id);
        let Some(every) = params["every"].as_u64().filter(|ms| *ms > 0) else {
            return Ok(());
        };
        if timers.iter().filter(|timer| timer.plugin == plugin).count() >= MAX_TIMERS {
            return Err(format!("a plugin can have at most {MAX_TIMERS} timers"));
        }
        let every = Duration::from_millis(every).max(MIN_TIMER);
        timers.push(Timer {
            plugin: plugin.to_owned(),
            id,
            every,
            due: Instant::now() + every,
        });
        Ok(())
    }

    /// Returns when the next plugin timer ticks, if any are running.
    pub(in crate::app) fn next_plugin_timer(&self) -> Option<Instant> {
        self.plugin_state
            .screen
            .timers
            .iter()
            .map(|timer| timer.due)
            .min()
    }

    /// Tells plugins about the timers that are due and passes on clicks on their widgets.
    pub(super) fn sync_plugin_screen(&mut self) {
        let now = Instant::now();
        for timer in &mut self.plugin_state.screen.timers {
            if timer.due > now {
                continue;
            }
            // a slow plugin skips ticks rather than getting a burst of them
            timer.due = now + timer.every;
            if let Some(plugin) = self.plugins.get(&timer.plugin) {
                plugin.event("timer", json!({ "id": timer.id }));
            }
        }
        for click in mem::take(&mut self.ui.segment_clicks) {
            let (plugin, id) = match click.segment.split_once('/') {
                Some((plugin, id)) => (plugin.to_owned(), Some(id.to_owned())),
                None => (click.segment, None),
            };
            if let Some(plugin) = self.plugins.get(&plugin) {
                plugin.event("segment_click", json!({ "id": id, "button": click.button }));
            }
        }
        for click in mem::take(&mut self.ui.widget_clicks) {
            if let Some(plugin) = self.plugins.get(&click.plugin) {
                plugin.event(
                    "click",
                    json!({
                        "id": click.id,
                        "x": click.x,
                        "y": click.y,
                        "button": click.button,
                    }),
                );
            }
        }
    }

    /// Answers `ui/layout`: where things are on screen.
    pub(super) fn screen_layout(&self) -> Value {
        let layout = self.ui.layout(self.screen);
        let rect = |rect: Rect| json!({ "x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height });
        let config = &self.ui.config;
        json!({
            "screen": rect(layout.screen),
            "editor": rect(layout.editor),
            "split": (!layout.split.is_empty()).then(|| rect(layout.split)),
            "status": rect(layout.status),
            "cursor": self.ui.cursor_screen.map(|at| json!({ "x": at.x, "y": at.y })),
            "flair": config.flair.enabled && !config.ui.serious,
            "reduced_motion": config.ui.reduced_motion,
            "theme": config.ui.theme,
        })
    }

    /// Forgets what `plugin` put on the screen and the keys it took.
    pub(super) fn clear_plugin_screen(&mut self, plugin: &str) {
        self.ui
            .plugin_widgets
            .retain(|widget| widget.plugin != plugin);
        let screen = &mut self.plugin_state.screen;
        screen.captures.remove(plugin);
        screen.timers.retain(|timer| timer.plugin != plugin);
        if screen.cursor_owner.as_deref() == Some(plugin) {
            screen.cursor_owner = None;
            self.ui.cursor_style = CursorStyle::default();
        }
    }

    /// Returns whether keys are waiting for a plugin to decide about them.
    #[cfg(test)]
    pub(in crate::app) fn plugin_keys_pending(&self) -> bool {
        let screen = &self.plugin_state.screen;
        screen.key_waiting || !screen.queued_keys.is_empty()
    }

    /// Returns the plugin that takes `chord` right now, if any.
    fn key_taker(&self, chord: &KeyChord) -> Option<String> {
        let ui = &self.ui;
        let typing_in_editor = ui.focus == Focus::Editor
            && ui.overlay.is_none()
            && ui.prompt.is_none()
            && !ui.annotate.active;
        if !typing_in_editor {
            return None;
        }
        self.plugin_state
            .screen
            .captures
            .iter()
            .find(|(_, capture)| capture.takes(chord))
            .map(|(plugin, _)| plugin.clone())
    }

    /// Hands `chord` to the plugin that takes it, or queues it behind a key a plugin is still
    /// deciding about.
    ///
    /// Returns `false` when no plugin wants it and the editor should handle it.
    pub(in crate::app) fn plugin_takes_key(&mut self, chord: KeyChord) -> bool {
        if self.plugin_state.screen.key_waiting {
            self.plugin_state.screen.queued_keys.push_back(chord);
            return true;
        }
        let Some(name) = self.key_taker(&chord) else {
            return false;
        };
        let Some(plugin) = self.plugins.get(&name).cloned() else {
            self.plugin_state.screen.captures.remove(&name);
            return false;
        };
        let mut params = self.cursor_context();
        params["key"] = json!(chord.to_string());
        params["char"] = json!(chord.typed_char().map(String::from));
        self.plugin_state.screen.key_waiting = true;
        self.plugins.spawn(async move {
            let answer = plugin.request("key", params, KEY_TIMEOUT).await;
            PluginUpdate::Key {
                plugin: name,
                chord,
                answer,
            }
        });
        true
    }

    /// Does what a plugin answered about a key, then handles the keys pressed meanwhile.
    pub(super) fn finish_key(
        &mut self,
        plugin: &str,
        chord: KeyChord,
        answer: Result<Value, String>,
    ) {
        self.plugin_state.screen.key_waiting = false;
        match answer {
            Ok(answer) => {
                if !answer["capture"].is_null() {
                    match parse_capture(&answer["capture"]) {
                        Ok(capture) => self.set_capture(plugin, capture),
                        Err(err) => self.plugins.log(plugin, format!("capture: {err}")),
                    }
                }
                if answer["handled"].as_bool() == Some(false) {
                    self.editor_key(chord);
                } else {
                    // a key the plugin used is not one the completion menu should also see
                    self.ui.completion = None;
                    let applied =
                        parse_actions(&answer).and_then(|actions| self.apply_actions(actions));
                    if let Err(err) = applied {
                        self.plugins.log(plugin, err.clone());
                        self.editor.set_status(format!("{plugin}: {err}"));
                    }
                }
            }
            Err(err) => {
                self.plugins.log(plugin, err.clone());
                self.editor.set_status(format!("{plugin}: {err}"));
            }
        }
        while let Some(next) = self.plugin_state.screen.queued_keys.pop_front() {
            if self.plugin_takes_key(next) {
                break;
            }
            self.editor_key(next);
        }
    }
}

#[cfg(test)]
/// Tests for reading what plugins draw and which keys they take.
mod tests {
    use mog_core::KeyChord;
    use mog_tui::{Anchor, Edge};
    use serde_json::json;

    use super::{parse_capture, parse_cursor, parse_widget};

    /// Returns the chord called `name`.
    fn key(name: &str) -> KeyChord {
        name.parse().expect("valid chord")
    }

    /// Plugins take every key but some, a few keys, or none.
    #[test]
    fn reads_captures() {
        let all = parse_capture(&json!({ "keys": "all", "except": ["ctrl+s"] })).expect("all");
        assert!(all.takes(&key("j")) && all.takes(&key("shift+g")));
        assert!(!all.takes(&key("ctrl+s")));
        let some = parse_capture(&json!({ "keys": ["esc"] })).expect("some");
        assert!(some.takes(&key("esc")) && !some.takes(&key("j")));
        assert!(parse_capture(&json!({})).expect("none").is_empty());
        assert!(parse_capture(&json!({ "keys": ["nope+x"] })).is_err());
        assert!(parse_capture(&json!({ "keys": 3 })).is_err());
    }

    /// Widgets read rows, spans, frames and motion, and an empty one means remove.
    #[test]
    fn reads_widgets() {
        let widget = parse_widget(
            "fish",
            &json!({
                "id": "nemo",
                "anchor": "bottom_right",
                "x": 2,
                "frames": [["><>"], [[{ "text": "<", "fg": "orange", "bold": true }, "><"]]],
                "fps": 3,
                "transparent": true,
                "motion": { "dx": -2, "edge": "wrap" },
            }),
        )
        .expect("valid")
        .expect("shown");
        assert_eq!(widget.id, "nemo");
        assert_eq!(widget.anchor, Anchor::BottomRight);
        assert_eq!(widget.frames.len(), 2);
        assert_eq!(widget.frames[1][0].len(), 2);
        assert!(widget.frames[1][0][0].bold);
        assert_eq!(widget.size(), (3, 1));
        assert!(widget.flair && widget.transparent);
        assert_eq!(widget.motion.expect("motion").edge, Edge::Wrap);
        assert!(
            parse_widget("p", &json!({ "id": "x" }))
                .expect("ok")
                .is_none()
        );
        assert!(
            parse_widget("p", &json!({ "lines": [] }))
                .expect("ok")
                .is_none()
        );
        assert!(parse_widget("p", &json!({ "lines": ["a"], "anchor": "moon" })).is_err());
        assert!(parse_widget("p", &json!({ "lines": [4] })).is_err());
    }

    /// Cursor shapes are read by name.
    #[test]
    fn reads_cursors() {
        let block = parse_cursor(&json!({ "shape": "block", "blink": true })).expect("block");
        assert!(block.blink);
        assert!(parse_cursor(&json!({ "shape": "star" })).is_err());
    }
}
