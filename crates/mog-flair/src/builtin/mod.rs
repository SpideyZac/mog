//! The flairs that ship with mog.

pub mod ai_meter;
pub mod aura;
pub mod badge;
pub mod combo;
pub mod critters;
pub mod fbi;
pub mod leaked_ip;
pub mod pet;
pub mod quips;
pub mod ram;
pub mod screensaver;
pub mod session;
pub mod sparks;
pub mod spectrum;
pub mod splash;
pub mod stonks;

pub use ai_meter::AiMeter;
pub use aura::Aura;
pub use badge::Badge;
pub use combo::Combo;
pub use critters::Critters;
pub use fbi::Fbi;
pub use leaked_ip::LeakedIp;
pub use pet::Pet;
pub use quips::Quips;
pub use ram::Ram;
pub use screensaver::Screensaver;
pub use session::Session;
pub use sparks::Sparks;
pub use spectrum::Spectrum;
pub use splash::Splash;
pub use stonks::Stonks;

use crate::flair::Flair;

/// Returns one of every built in flair.
pub fn all() -> Vec<Box<dyn Flair>> {
    vec![
        Box::new(Splash::new()),
        Box::new(Critters::new()),
        Box::new(Pet::new()),
        Box::new(Quips::new()),
        Box::new(LeakedIp::new()),
        Box::new(Ram::new()),
        Box::new(AiMeter::new()),
        Box::new(Aura::new()),
        Box::new(Fbi::new()),
        Box::new(Session::new()),
        Box::new(Badge::new()),
        Box::new(Sparks::new()),
        Box::new(Combo::new()),
        Box::new(Spectrum::new()),
        Box::new(Stonks::new()),
        Box::new(Screensaver::new()),
    ]
}
