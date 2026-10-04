//! Shows your LAN ip in the status line, for drama.

use std::{
    net::{IpAddr, Ipv4Addr, UdpSocket},
    time::Duration,
};

use mog_tui::{Segment, Side};
use ratatui::style::{Color, Modifier, Style};

use crate::flair::{Flair, FlairContext, Placement};

/// How long the warning dot takes to blink on and off.
const BLINK: Duration = Duration::from_millis(1600);

/// Returns the address this machine uses on the local network.
///
/// Connecting a UDP socket only picks a route, it never sends a packet.
fn local_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(10, 254, 254, 254), 1)).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then_some(ip)
}

/// Shows `leaked ip` and the LAN address with a blinking red dot.
#[derive(Debug)]
pub struct LeakedIp {
    /// The address, found once at startup.
    ip: Option<IpAddr>,
    /// Time into the current blink.
    elapsed: Duration,
}

impl Default for LeakedIp {
    fn default() -> Self {
        Self {
            ip: local_ip(),
            elapsed: Duration::ZERO,
        }
    }
}

impl LeakedIp {
    /// Creates the flair, looking up the address.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Flair for LeakedIp {
    fn id(&self) -> &str {
        "leaked_ip"
    }

    fn description(&self) -> &str {
        "Shows your LAN ip like it got leaked. Nobody can do anything with it."
    }

    fn placement(&self) -> Placement {
        Placement::Status(Side::Left)
    }

    fn tick(&mut self, dt: Duration) {
        self.elapsed = (self.elapsed + dt)
            .checked_sub(BLINK)
            .unwrap_or(self.elapsed + dt);
    }

    fn is_animating(&self) -> bool {
        self.ip.is_some()
    }

    fn segment(&mut self, cx: &FlairContext<'_>) -> Option<Segment> {
        let ip = self.ip?;
        let bg = cx.theme.status.bg.unwrap_or(Color::Reset);
        let red = Style::new().fg(cx.theme.palette.red).bg(bg);
        let dot = if self.elapsed < BLINK / 2 {
            "\u{25cf}"
        } else {
            "\u{25cb}"
        };
        Some(Segment {
            parts: vec![
                (format!("{dot} "), red.add_modifier(Modifier::BOLD)),
                ("leaked ip ".into(), red),
                (ip.to_string(), cx.theme.status.add_modifier(Modifier::BOLD)),
            ],
            side: Side::Left,
        })
    }
}
