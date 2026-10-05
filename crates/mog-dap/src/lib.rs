//! The debug adapter client of mog.
//!
//! Debuggers like `lldb-dap` and `debugpy` speak the debug adapter protocol, which frames JSON
//! like language servers do but has its own message shapes.

pub mod client;
pub mod transport;

pub use client::{DapEvent, DebugClient, Frame, Scope, Variable};
