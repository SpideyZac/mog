//! A force directed layout that makes the graph look like a graph.

use crate::{graph::scan::Graph, rng::Rng};

/// The distance linked nodes like to keep.
const LINK_LENGTH: f32 = 9.0;

/// How hard nodes push each other away.
const REPULSION: f32 = 120.0;

/// How hard links pull.
const SPRING: f32 = 0.08;

/// How hard everything is pulled to the middle.
const GRAVITY: f32 = 0.015;

/// How much speed survives each step.
const DAMPING: f32 = 0.82;

/// The fastest a node moves per step.
const MAX_SPEED: f32 = 4.0;

/// How fast the simulation cools down per step.
const COOLING: f32 = 0.992;

/// The temperature below which the layout counts as settled.
const SETTLED: f32 = 0.02;

/// Positions and speeds of every node.
#[derive(Debug, Clone, Default)]
pub struct Sim {
    /// Node positions in world units.
    pub pos: Vec<(f32, f32)>,
    /// Node speeds in world units per step.
    vel: Vec<(f32, f32)>,
    /// How lively the simulation is, from 1 down to 0.
    temperature: f32,
}

impl Sim {
    /// Scatters the nodes of `graph` around the middle.
    pub fn new(graph: &Graph, rng: &mut Rng) -> Self {
        let count = graph.nodes.len();
        let spread = (count as f32).sqrt() * LINK_LENGTH * 0.6 + 1.0;
        let pos = (0..count)
            .map(|_| (rng.range(-spread, spread), rng.range(-spread, spread)))
            .collect();
        Self {
            pos,
            vel: vec![(0.0, 0.0); count],
            temperature: 1.0,
        }
    }

    /// Returns whether the layout has stopped moving.
    pub fn is_settled(&self) -> bool {
        self.temperature < SETTLED
    }

    /// Wakes the simulation up, for example after a node was dragged.
    pub fn reheat(&mut self) {
        self.temperature = self.temperature.max(0.4);
    }

    /// Moves every node one step. Node `pinned` stays where it is.
    pub fn step(&mut self, graph: &Graph, pinned: Option<usize>) {
        let count = self.pos.len();
        let mut force = vec![(0.0f32, 0.0f32); count];
        for i in 0..count {
            for j in i + 1..count {
                let dx = self.pos[i].0 - self.pos[j].0;
                let dy = self.pos[i].1 - self.pos[j].1;
                let dist2 = (dx * dx + dy * dy).max(0.25);
                let push = REPULSION / dist2;
                let dist = dist2.sqrt();
                let (fx, fy) = (dx / dist * push, dy / dist * push);
                force[i].0 += fx;
                force[i].1 += fy;
                force[j].0 -= fx;
                force[j].1 -= fy;
            }
        }
        for &(a, b) in &graph.edges {
            let dx = self.pos[b].0 - self.pos[a].0;
            let dy = self.pos[b].1 - self.pos[a].1;
            let dist = (dx * dx + dy * dy).sqrt().max(0.01);
            let pull = (dist - LINK_LENGTH) * SPRING;
            let (fx, fy) = (dx / dist * pull, dy / dist * pull);
            force[a].0 += fx;
            force[a].1 += fy;
            force[b].0 -= fx;
            force[b].1 -= fy;
        }
        let nodes = self.pos.iter_mut().zip(&mut self.vel).zip(force);
        for (i, ((pos, vel), (fx, fy))) in nodes.enumerate() {
            if Some(i) == pinned {
                *vel = (0.0, 0.0);
                continue;
            }
            let (x, y) = *pos;
            let fx = fx - x * GRAVITY;
            let fy = fy - y * GRAVITY;
            let mut vx = (vel.0 + fx * self.temperature) * DAMPING;
            let mut vy = (vel.1 + fy * self.temperature) * DAMPING;
            let speed = (vx * vx + vy * vy).sqrt();
            if speed > MAX_SPEED {
                vx *= MAX_SPEED / speed;
                vy *= MAX_SPEED / speed;
            }
            *vel = (vx, vy);
            *pos = (x + vx, y + vy);
        }
        self.temperature *= COOLING;
    }

    /// Returns the smallest box holding every node as `(min_x, min_y, max_x, max_y)`.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        self.pos.iter().fold(
            (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
            |(a, b, c, d), &(x, y)| (a.min(x), b.min(y), c.max(x), d.max(y)),
        )
    }
}

#[cfg(test)]
/// Tests for the layout.
mod tests {
    use std::path::PathBuf;

    use super::{LINK_LENGTH, Sim};
    use crate::{
        graph::scan::{Graph, Node},
        rng::Rng,
    };

    /// Builds a graph of `count` nodes with `edges`.
    fn graph(count: usize, edges: Vec<(usize, usize)>) -> Graph {
        Graph {
            nodes: (0..count)
                .map(|i| Node {
                    path: PathBuf::from(i.to_string()),
                    name: i.to_string(),
                    degree: 0,
                })
                .collect(),
            edges,
        }
    }

    /// Linked nodes end up near each other and unlinked ones further apart.
    #[test]
    fn links_pull_together() {
        let graph = graph(3, vec![(0, 1)]);
        let mut sim = Sim::new(&graph, &mut Rng::seeded(7));
        for _ in 0..800 {
            sim.step(&graph, None);
        }
        let dist = |a: usize, b: usize| {
            let (dx, dy) = (sim.pos[a].0 - sim.pos[b].0, sim.pos[a].1 - sim.pos[b].1);
            (dx * dx + dy * dy).sqrt()
        };
        assert!(dist(0, 1) < LINK_LENGTH * 2.0, "{}", dist(0, 1));
        assert!(dist(0, 2) > dist(0, 1));
        assert!(sim.is_settled());
    }
}
