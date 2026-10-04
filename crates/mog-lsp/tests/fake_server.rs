//! Tests the client against a scripted server over in memory pipes.

use std::{env, time::Duration};

use mog_lsp::{
    Client, LspEvent, Message,
    transport::{read_message, write_message},
};
use serde_json::{Value, json};
use tokio::{
    io::{self, BufReader},
    sync::mpsc,
    time,
};

/// The client does the handshake, gets diagnostics and gets request results.
#[tokio::test]
async fn handshake_diagnostics_and_requests() {
    let (client_reader, mut server_writer) = io::duplex(4096);
    let (server_reader, client_writer) = io::duplex(4096);
    let root = env::current_dir().expect("current dir");
    let (events, mut received) = mpsc::unbounded_channel();
    let (client, _task) = Client::connect(
        "fake",
        client_reader,
        client_writer,
        &root,
        &Value::Null,
        events,
    );

    let server = tokio::spawn(async move {
        let mut reader = BufReader::new(server_reader);
        let Some(Message::Request { id, method, .. }) =
            read_message(&mut reader).await.expect("read")
        else {
            panic!("expected initialize");
        };
        assert_eq!(method, "initialize");
        let response = Message::Response {
            id,
            result: Ok(json!({ "capabilities": {} })),
        };
        write_message(&mut server_writer, &response)
            .await
            .expect("write");
        let initialized = read_message(&mut reader).await.expect("read");
        assert!(
            matches!(initialized, Some(Message::Notification { method, .. }) if method == "initialized")
        );
        let diagnostics = Message::Notification {
            method: "textDocument/publishDiagnostics".into(),
            params: json!({ "uri": "file:///a.rs", "diagnostics": [] }),
        };
        write_message(&mut server_writer, &diagnostics)
            .await
            .expect("write");
        let Some(Message::Request { id, method, .. }) =
            read_message(&mut reader).await.expect("read")
        else {
            panic!("expected a request");
        };
        assert_eq!(method, "mog/ping");
        let pong = Message::Response {
            id,
            result: Ok(json!("pong")),
        };
        write_message(&mut server_writer, &pong)
            .await
            .expect("write");
    });

    let mut next = async || {
        time::timeout(Duration::from_secs(5), received.recv())
            .await
            .expect("no timeout")
    };
    assert!(matches!(next().await, Some(LspEvent::Ready { .. })));
    assert!(matches!(next().await, Some(LspEvent::Diagnostics { .. })));
    let pong = client.request("mog/ping", json!(null)).await.expect("pong");
    assert_eq!(pong, json!("pong"));
    server.await.expect("server finished");
}
