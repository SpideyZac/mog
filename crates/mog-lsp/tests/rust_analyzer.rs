//! A smoke test against a real `rust-analyzer`, ignored by default since it needs one installed.

use std::{env, time::Duration};

use mog_lsp::{Client, LspEvent};
use tokio::{sync::mpsc, time};

/// The client finishes the handshake with `rust-analyzer`.
#[tokio::test]
#[ignore = "needs rust-analyzer on the path"]
async fn handshake_with_rust_analyzer() {
    let root = env::current_dir().expect("current dir");
    let (events, mut received) = mpsc::unbounded_channel();
    let client = Client::start("rust", "rust-analyzer", &[], &root, events).expect("spawn");
    let event = time::timeout(Duration::from_secs(30), received.recv())
        .await
        .expect("no timeout");
    assert!(matches!(event, Some(LspEvent::Ready { .. })), "{event:?}");
    drop(client);
}
