//! The flairs that ship with mog.

pub mod ai_meter;
pub mod badge;
pub mod leaked_ip;

pub use ai_meter::AiMeter;
pub use badge::Badge;
pub use leaked_ip::LeakedIp;

use crate::flair::Flair;

/// Returns one of every built in flair.
pub fn all() -> Vec<Box<dyn Flair>> {
    vec![
        Box::new(LeakedIp::new()),
        Box::new(AiMeter::new()),
        Box::new(Badge::new()),
    ]
}
