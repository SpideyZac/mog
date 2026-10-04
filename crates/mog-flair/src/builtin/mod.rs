//! The flairs that ship with mog.

pub mod badge;

pub use badge::Badge;

use crate::flair::Flair;

/// Returns one of every built in flair.
pub fn all() -> Vec<Box<dyn Flair>> {
    vec![Box::new(Badge::new())]
}
