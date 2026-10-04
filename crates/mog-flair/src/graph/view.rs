//! The full screen project graph.

use std::{cmp::Reverse, time::Duration};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Key, KeyChord, fuzzy_match};
use mog_tui::{Context, EventResult, Focus, Layer, Layout, Overlay, Ui, icons, theme::mix};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    symbols::Marker,
    text::{Line, Span},
    widgets::{
        Clear, Widget,
        canvas::{Canvas, Circle, Line as Segment, Points},
    },
};

use crate::{
    graph::{
        scan::{Graph, scan},
        sim::Sim,
    },
    rng::Rng,
};

/// Simulation steps per animation frame.
const STEPS_PER_FRAME: usize = 3;

/// How much one scroll notch zooms.
const ZOOM_STEP: f32 = 1.2;

/// How far from a node, in cells, the mouse still counts as on it.
const HIT_RADIUS: f32 = 2.5;

/// The camera looking at the graph.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Camera {
    /// The world point in the middle of the screen.
    x: f32,
    /// The world point in the middle of the screen.
    y: f32,
    /// World units per cell column. Rows are twice as tall.
    scale: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            scale: 1.0,
        }
    }
}

impl Camera {
    /// Converts a screen cell in `area` to world coordinates.
    fn to_world(self, area: Rect, column: u16, row: u16) -> (f32, f32) {
        let dx = f32::from(column) - f32::from(area.x) + 0.5 - f32::from(area.width) / 2.0;
        let dy = f32::from(row) - f32::from(area.y) + 0.5 - f32::from(area.height) / 2.0;
        (self.x + dx * self.scale, self.y - dy * self.scale * 2.0)
    }
}

/// An Obsidian style graph of the files in the project and how they link.
#[derive(Debug, Default)]
pub struct GraphView {
    /// The popup generation the graph was built for.
    generation: u64,
    /// The files and links.
    graph: Graph,
    /// Where the nodes are.
    sim: Sim,
    /// What part of the graph is on screen.
    camera: Camera,
    /// The node under the mouse.
    hovered: Option<usize>,
    /// The node picked with the keyboard or search.
    selected: Option<usize>,
    /// The node being dragged and whether it moved.
    dragging: Option<(usize, bool)>,
    /// The last mouse cell while panning.
    panning: Option<(u16, u16)>,
    /// What was typed to search for a node.
    query: String,
    /// The graph area from the last render.
    area: Rect,
    /// Whether the graph is on screen, so the simulation can sleep when it is not.
    open: bool,
    /// Random numbers for the starting layout.
    rng: Rng,
}

impl GraphView {
    /// Creates the graph view.
    pub fn new() -> Self {
        Self::default()
    }

    /// Scans the project and lays it out.
    fn rebuild(&mut self, cx: &Context<'_>) {
        self.graph = scan(&cx.ui.root);
        self.sim = Sim::new(&self.graph, &mut self.rng);
        // settle a bit up front so the first frame is not a hairball
        for _ in 0..60 {
            self.sim.step(&self.graph, None);
        }
        self.query.clear();
        self.hovered = None;
        self.dragging = None;
        self.panning = None;
        let current = cx.editor.document().path();
        self.selected = self
            .graph
            .nodes
            .iter()
            .position(|node| Some(node.path.as_path()) == current);
        self.fit();
    }

    /// Zooms the camera so every node fits on screen.
    fn fit(&mut self) {
        if self.graph.nodes.is_empty() || self.area.width == 0 {
            self.camera = Camera::default();
            return;
        }
        let (min_x, min_y, max_x, max_y) = self.sim.bounds();
        let width = f32::from(self.area.width.max(10) - 4);
        let height = f32::from(self.area.height.max(6) - 4) * 2.0;
        let scale = ((max_x - min_x) / width)
            .max((max_y - min_y) / height)
            .max(0.2);
        self.camera = Camera {
            x: (min_x + max_x) / 2.0,
            y: (min_y + max_y) / 2.0,
            scale,
        };
    }

    /// Returns the node nearest to a screen cell, if one is close enough.
    fn node_at(&self, column: u16, row: u16) -> Option<usize> {
        let (wx, wy) = self.camera.to_world(self.area, column, row);
        let reach = HIT_RADIUS * self.camera.scale;
        self.sim
            .pos
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| (i, ((x - wx).powi(2) + ((y - wy) / 2.0).powi(2)).sqrt()))
            .filter(|(_, dist)| *dist <= reach)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// Returns the radius of node `i` in world units.
    fn radius(&self, i: usize) -> f32 {
        (0.6 + (self.graph.nodes[i].degree as f32).sqrt() * 0.45) * self.camera.scale.max(0.35)
    }

    /// Opens the file of node `i` and closes the graph.
    fn open_node(&mut self, i: usize, cx: &mut Context<'_>) {
        let path = self.graph.nodes[i].path.clone();
        cx.ui.close();
        cx.ui.focus = Focus::Editor;
        self.open = false;
        if let Err(err) = cx.editor.open(path.clone()) {
            cx.editor
                .set_status(format!("could not open {}: {err}", path.display()));
        }
    }

    /// Picks the best search match.
    fn search(&mut self) {
        self.selected = self
            .graph
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, node)| {
                fuzzy_match(&self.query, &node.name).map(|found| (i, found.score))
            })
            .max_by_key(|(_, score)| *score)
            .map(|(i, _)| i);
        if let Some(i) = self.selected {
            let (x, y) = self.sim.pos[i];
            self.camera.x = x;
            self.camera.y = y;
        }
    }

    /// Selects the next node by number of links, or the previous one if `forward` is not set.
    fn cycle(&mut self, forward: bool) {
        let mut order: Vec<usize> = (0..self.graph.nodes.len()).collect();
        order.sort_by_key(|&i| Reverse(self.graph.nodes[i].degree));
        if order.is_empty() {
            return;
        }
        let at = self
            .selected
            .and_then(|s| order.iter().position(|&i| i == s))
            .map_or(0, |p| {
                if forward {
                    (p + 1) % order.len()
                } else {
                    (p + order.len() - 1) % order.len()
                }
            });
        self.selected = Some(order[at]);
        let (x, y) = self.sim.pos[order[at]];
        self.camera.x = x;
        self.camera.y = y;
    }

    /// Returns the node whose neighbours are highlighted.
    fn focus_node(&self) -> Option<usize> {
        self.hovered.or(self.selected)
    }
}

impl Layer for GraphView {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        if ui.overlay == Some(Overlay::Graph) {
            layout.screen
        } else {
            Rect::default()
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        self.open = true;
        self.area = Rect {
            y: area.y + 1,
            height: area.height.saturating_sub(2),
            ..area
        };
        if self.generation != cx.ui.overlay_generation {
            self.generation = cx.ui.overlay_generation;
            self.rebuild(cx);
        }
        let theme = cx.theme;
        let p = theme.palette;
        Clear.render(area, buf);
        buf.set_style(area, theme.background);

        let focus = self.focus_node();
        let neighbours: Vec<usize> = focus.map_or_else(Vec::new, |f| {
            self.graph
                .edges
                .iter()
                .filter_map(|&(a, b)| match (a == f, b == f) {
                    (true, _) => Some(b),
                    (_, true) => Some(a),
                    _ => None,
                })
                .collect()
        });
        let current = cx.editor.document().path();
        let hub = self
            .graph
            .nodes
            .iter()
            .map(|node| node.degree)
            .max()
            .unwrap_or(0)
            .max(4)
            / 2;
        let area = self.area;
        let camera = self.camera;
        let half_w = f32::from(area.width) * camera.scale / 2.0;
        let half_h = f32::from(area.height) * camera.scale;
        let dim_edge = mix(p.bg, p.fg, 0.18);
        let canvas = Canvas::default()
            .marker(Marker::Braille)
            .background_color(p.bg)
            .x_bounds([f64::from(camera.x - half_w), f64::from(camera.x + half_w)])
            .y_bounds([f64::from(camera.y - half_h), f64::from(camera.y + half_h)])
            .paint(|ctx| {
                for &(a, b) in &self.graph.edges {
                    let lit = focus.is_some_and(|f| f == a || f == b);
                    let (ax, ay) = self.sim.pos[a];
                    let (bx, by) = self.sim.pos[b];
                    ctx.draw(&Segment {
                        x1: f64::from(ax),
                        y1: f64::from(ay),
                        x2: f64::from(bx),
                        y2: f64::from(by),
                        color: if lit { p.accent } else { dim_edge },
                    });
                }
                ctx.layer();
                for (i, node) in self.graph.nodes.iter().enumerate() {
                    let (x, y) = self.sim.pos[i];
                    let is_current = Some(node.path.as_path()) == current;
                    let base = icons::file_color(&node.name).unwrap_or(p.fg);
                    let color = if Some(i) == focus {
                        p.accent2
                    } else if is_current {
                        p.accent
                    } else if focus.is_some() && !neighbours.contains(&i) {
                        mix(base, p.bg, 0.6)
                    } else {
                        base
                    };
                    ctx.draw(&Circle {
                        x: f64::from(x),
                        y: f64::from(y),
                        radius: f64::from(self.radius(i)),
                        color,
                    });
                    ctx.draw(&Points {
                        coords: &[(f64::from(x), f64::from(y))],
                        color,
                    });
                }
                ctx.layer();
                for (i, node) in self.graph.nodes.iter().enumerate() {
                    let is_current = Some(node.path.as_path()) == current;
                    let labelled = Some(i) == focus
                        || neighbours.contains(&i)
                        || is_current
                        || node.degree >= hub
                        || camera.scale < 0.45;
                    if !labelled {
                        continue;
                    }
                    let (x, y) = self.sim.pos[i];
                    let style = if Some(i) == focus {
                        Style::new().fg(p.accent2).add_modifier(Modifier::BOLD)
                    } else if is_current {
                        Style::new().fg(p.accent).add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().fg(mix(p.fg, p.bg, 0.25))
                    };
                    let r = self.radius(i);
                    ctx.print(
                        f64::from(x + r + camera.scale),
                        f64::from(y),
                        Line::from(Span::styled(node.name.clone(), style)),
                    );
                }
            });
        canvas.render(area, buf);

        let header = Rect {
            height: 1,
            y: area.y - 1,
            ..area
        };
        buf.set_style(header, theme.status);
        let title = format!(
            " \u{25c9} graph  {} files  {} links ",
            self.graph.nodes.len(),
            self.graph.edges.len()
        );
        buf.set_string(header.x, header.y, &title, theme.status_badge);
        let hint = if self.query.is_empty() {
            "  type to search \u{b7} drag to pan \u{b7} scroll to zoom \u{b7} click to open \u{b7} tab to hop \u{b7} esc to close".to_owned()
        } else {
            format!("  search: {}", self.query)
        };
        let after_title = header.x + u16::try_from(title.chars().count()).unwrap_or(0);
        buf.set_stringn(
            after_title,
            header.y,
            &hint,
            usize::from(header.right().saturating_sub(after_title)),
            theme.status,
        );
        let footer = Rect {
            y: area.bottom(),
            height: 1,
            ..area
        };
        buf.set_style(footer, theme.status);
        if let Some(i) = focus {
            let node = &self.graph.nodes[i];
            let path = node.path.strip_prefix(&cx.ui.root).unwrap_or(&node.path);
            let text = format!(
                " {}  \u{b7}  {} link{}",
                path.display(),
                node.degree,
                if node.degree == 1 { "" } else { "s" }
            );
            buf.set_stringn(
                footer.x,
                footer.y,
                &text,
                usize::from(footer.width),
                theme.status,
            );
        } else if self.graph.nodes.is_empty() {
            buf.set_string(
                footer.x,
                footer.y,
                " no files to graph here, open a folder with mog <folder>",
                theme.status,
            );
        }
        if self.sim.is_settled() {
            return;
        }
        let pulse = Style::new()
            .fg(p.accent)
            .bg(theme.status.bg.unwrap_or(Color::Reset));
        buf.set_string(
            footer.right().saturating_sub(12),
            footer.y,
            "settling...",
            pulse,
        );
    }

    fn handle_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        if cx.ui.overlay != Some(Overlay::Graph) {
            return EventResult::Ignored;
        }
        let pan = self.camera.scale * 4.0;
        match chord.key {
            Key::Esc if !self.query.is_empty() => {
                self.query.clear();
                self.selected = None;
            }
            Key::Esc => {
                cx.ui.close();
                self.open = false;
            }
            Key::Enter => {
                if let Some(i) = self.focus_node() {
                    self.open_node(i, cx);
                }
            }
            Key::Tab => self.cycle(!chord.mods.shift),
            Key::Left => self.camera.x -= pan,
            Key::Right => self.camera.x += pan,
            Key::Up => self.camera.y += pan * 2.0,
            Key::Down => self.camera.y -= pan * 2.0,
            Key::Backspace => {
                self.query.pop();
                self.search();
            }
            Key::Char('+' | '=') => self.camera.scale /= ZOOM_STEP,
            Key::Char('-') => self.camera.scale *= ZOOM_STEP,
            Key::Char('0') if self.query.is_empty() => self.fit(),
            _ => match chord.typed_char() {
                Some(ch) => {
                    self.query.push(ch);
                    self.search();
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
        match event.kind {
            MouseEventKind::Moved => self.hovered = self.node_at(event.column, event.row),
            MouseEventKind::Down(MouseButton::Left) => {
                match self.node_at(event.column, event.row) {
                    Some(i) => self.dragging = Some((i, false)),
                    None => self.panning = Some((event.column, event.row)),
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some((i, _)) = self.dragging {
                    self.sim.pos[i] = self.camera.to_world(self.area, event.column, event.row);
                    self.dragging = Some((i, true));
                    self.sim.reheat();
                } else if let Some((column, row)) = self.panning {
                    let dx = f32::from(event.column) - f32::from(column);
                    let dy = f32::from(event.row) - f32::from(row);
                    self.camera.x -= dx * self.camera.scale;
                    self.camera.y += dy * self.camera.scale * 2.0;
                    self.panning = Some((event.column, event.row));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some((i, false)) = self.dragging {
                    self.open_node(i, cx);
                }
                self.dragging = None;
                self.panning = None;
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                // zoom around the mouse so the point under it stays put
                let before = self.camera.to_world(self.area, event.column, event.row);
                if event.kind == MouseEventKind::ScrollUp {
                    self.camera.scale /= ZOOM_STEP;
                } else {
                    self.camera.scale *= ZOOM_STEP;
                }
                let after = self.camera.to_world(self.area, event.column, event.row);
                self.camera.x += before.0 - after.0;
                self.camera.y += before.1 - after.1;
            }
            _ => {}
        }
        EventResult::Consumed
    }

    fn tick(&mut self, _dt: Duration) {
        if !self.open {
            return;
        }
        let pinned = self.dragging.map(|(i, _)| i);
        for _ in 0..STEPS_PER_FRAME {
            if self.sim.is_settled() && pinned.is_none() {
                break;
            }
            self.sim.step(&self.graph, pinned);
        }
    }

    fn is_animating(&self) -> bool {
        self.open && (!self.sim.is_settled() || self.dragging.is_some())
    }
}

#[cfg(test)]
/// Tests for the graph camera.
mod tests {
    use ratatui::layout::Rect;

    use super::Camera;

    /// The middle of the screen is the camera point and rows count double.
    #[test]
    fn maps_cells_to_world() {
        let camera = Camera {
            x: 10.0,
            y: 5.0,
            scale: 1.0,
        };
        let area = Rect::new(0, 0, 10, 10);
        assert_eq!(camera.to_world(area, 4, 4), (9.5, 6.0));
        assert_eq!(camera.to_world(area, 5, 5), (10.5, 4.0));
    }
}
