//! Git integration for mog.
//!
//! Everything here shells out to the `git` program so there is no C library to build. Calls block,
//! so the editor runs them off the event loop.

pub mod diff;
pub mod repo;

pub use diff::{LineChange, line_changes};
pub use repo::{Blame, FileStatus, Repo};
