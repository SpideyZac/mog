//! Git integration for mog.
//!
//! Everything here shells out to the `git` program so there is no C library to build. Calls block,
//! so the editor runs them off the event loop.

pub mod diff;
pub mod repo;

pub use diff::{Hunk, LineChange, apply_hunk, hunks, line_changes, map_line, revert_hunk};
pub use repo::{Blame, FileChange, FileStatus, Repo};
