//! The project graph: files as nodes, links between them as edges.

pub mod scan;
pub mod sim;
pub mod view;

pub use scan::{Graph, Node, scan};
pub use sim::Sim;
pub use view::GraphView;
