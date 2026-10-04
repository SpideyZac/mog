//! Showing what you are editing on your Discord profile.

use std::{
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use discord_rich_presence::{
    DiscordIpc, DiscordIpcClient,
    activity::{Activity, Assets, Timestamps},
};

/// How long to wait before trying again when Discord is not running.
const RETRY: Duration = Duration::from_secs(15);

/// The shortest time between two updates, since Discord drops ones that come faster.
const MIN_GAP: Duration = Duration::from_secs(4);

/// What the status says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The top line, like `editing main.rs`.
    pub details: String,
    /// The second line, like `in mog`.
    pub state: String,
}

/// A connection to Discord run on its own thread. Dropping it clears the status.
#[derive(Debug)]
pub struct Presence {
    /// Where new statuses go.
    sender: Sender<Status>,
    /// The client id it was started with, to notice config changes.
    client_id: String,
}

impl Presence {
    /// Starts talking to Discord as the application `client_id`, showing `large_image`.
    pub fn start(client_id: &str, large_image: &str) -> Self {
        let (sender, receiver) = mpsc::channel();
        let id = client_id.to_owned();
        let image = large_image.to_owned();
        thread::spawn(move || run(&id, &image, &receiver));
        Self {
            sender,
            client_id: client_id.to_owned(),
        }
    }

    /// Returns the application id the presence shows as.
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Shows `status`, as soon as Discord allows it.
    pub fn update(&self, status: Status) {
        let _ = self.sender.send(status);
    }
}

/// Keeps Discord showing the newest status from `receiver` until the sender is dropped.
fn run(client_id: &str, large_image: &str, receiver: &Receiver<Status>) {
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .unwrap_or(0);
    let mut client = DiscordIpcClient::new(client_id);
    let mut connected = false;
    let mut pending: Option<Status> = None;
    let mut sent_at: Option<Instant> = None;
    loop {
        let wait = if pending.is_some() { MIN_GAP } else { RETRY };
        match receiver.recv_timeout(wait) {
            Ok(status) => pending = Some(status),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // only the newest status matters
        while let Ok(status) = receiver.try_recv() {
            pending = Some(status);
        }
        if sent_at.is_some_and(|at| at.elapsed() < MIN_GAP) {
            continue;
        }
        if !connected {
            connected = client.connect().is_ok();
        }
        let Some(status) = &pending else {
            continue;
        };
        if !connected {
            continue;
        }
        let mut assets = Assets::new().large_text("mog, the editor that mogs");
        if !large_image.is_empty() {
            assets = assets.large_image(large_image);
        }
        let activity = Activity::new()
            .details(status.details.as_str())
            .state(status.state.as_str())
            .assets(assets)
            .timestamps(Timestamps::new().start(started));
        if client.set_activity(activity).is_ok() {
            pending = None;
            sent_at = Some(Instant::now());
        } else {
            // discord closed, so reconnect and send it again later
            connected = false;
            let _ = client.close();
        }
    }
    if connected {
        let _ = client.clear_activity();
        let _ = client.close();
    }
}
