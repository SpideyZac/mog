//! The Lua plugin host of mog.
//!
//! Plugins are directories holding an `init.lua`. They talk to the editor through the global
//! `mog` table and never touch editor state directly.

pub mod host;

pub use host::{PluginHost, PluginRequest};
