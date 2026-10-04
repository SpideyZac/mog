//! The flairs that ship with mog.

pub mod ai_meter;
pub mod badge;
pub mod combo;
pub mod leaked_ip;
pub mod pet;
pub mod quips;
pub mod session;
pub mod sparks;

pub use ai_meter::AiMeter;
pub use badge::Badge;
pub use combo::Combo;
pub use leaked_ip::LeakedIp;
pub use pet::Pet;
pub use quips::Quips;
pub use session::Session;
pub use sparks::Sparks;

use crate::flair::Flair;

/// Returns one of every built in flair.
pub fn all() -> Vec<Box<dyn Flair>> {
    vec![
        Box::new(Pet::new()),
        Box::new(Quips::new()),
        Box::new(LeakedIp::new()),
        Box::new(AiMeter::new()),
        Box::new(Session::new()),
        Box::new(Badge::new()),
        Box::new(Sparks::new()),
        Box::new(Combo::new()),
    ]
}
