//! Plugins for mog: programs that talk JSON-RPC over stdio and add commands to the editor.
//!
//! The protocol is described in `docs/plugins.md`. Plugins run as their own processes, so they
//! can be written in any language and a broken one cannot take mog down with it.

pub mod host;
pub mod manifest;
pub mod protocol;
pub mod stats;

pub use host::{DEFAULT_TIMEOUT, Events, Plugin, PluginEvent, Spec};
pub use manifest::{Activation, Contributions, MANIFEST_FILE, Manifest, discover};
pub use protocol::{
    Action, Edit, FileEdit, Hello, Languages, Level, PROTOCOL_VERSION, PluginCommand, PluginTool,
    Segment, parse_actions, parse_segment,
};
pub use stats::{Outcome, Stats};

#[cfg(test)]
/// Tests for talking to plugins.
mod tests {
    use std::{future::Future, time::Duration};

    use mog_lsp::transport::{self, Message};
    use serde_json::{Value, json};
    use tokio::{
        io::{self, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf},
        sync::mpsc::{self, Receiver},
        time,
    };

    use super::{Action, Plugin, PluginEvent};

    /// The plugin side of a connection: what it reads from and writes to.
    type Side = (BufReader<ReadHalf<DuplexStream>>, WriteHalf<DuplexStream>);

    /// Connects a plugin called `name` to a fake whose side runs `fake`, returning the handle
    /// and mog's events.
    fn fake<F, Fut>(
        name: &str,
        timeout: Duration,
        fake: F,
    ) -> (Plugin, Receiver<(u64, PluginEvent)>)
    where
        F: FnOnce(Side) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (mog_side, plugin_side) = io::duplex(1 << 16);
        let (mog_read, mog_write) = io::split(mog_side);
        let (plugin_read, plugin_write) = io::split(plugin_side);
        let (events, events_rx) = mpsc::channel(64);
        let (plugin, _task) =
            Plugin::connect(name, 7, mog_read, mog_write, json!({}), timeout, events);
        tokio::spawn(fake((BufReader::new(plugin_read), plugin_write)));
        (plugin, events_rx)
    }

    /// Reads requests on `side` and answers each with `answer(method, params)`, or not at all
    /// when it gives `None`. Returns the notifications it saw once mog says `shutdown`.
    async fn serve(
        (mut reader, mut writer): Side,
        answer: impl Fn(&str, &Value) -> Option<Value>,
    ) -> Vec<(String, Value)> {
        let mut seen = Vec::new();
        while let Ok(Some(message)) = transport::read_message(&mut reader).await {
            match message {
                Message::Request { id, method, params } => {
                    if let Some(result) = answer(&method, &params) {
                        let reply = Message::Response {
                            id,
                            result: Ok(result),
                        };
                        if transport::write_message(&mut writer, &reply).await.is_err() {
                            break;
                        }
                    }
                }
                Message::Notification { method, params } => {
                    let done = method == "shutdown";
                    seen.push((method, params));
                    if done {
                        break;
                    }
                }
                Message::Response { .. } => {}
            }
        }
        seen
    }

    /// Waits for the next event.
    async fn next(events: &mut Receiver<(u64, PluginEvent)>) -> PluginEvent {
        let (instance, event) = time::timeout(Duration::from_secs(60), events.recv())
            .await
            .expect("in time")
            .expect("event");
        assert_eq!(instance, 7);
        event
    }

    /// The handshake reports the commands and a command run comes back with its actions.
    #[tokio::test]
    async fn runs_a_command() {
        let (plugin, mut events) = fake("words", Duration::from_secs(5), |side| async move {
            serve(side, |method, params| {
                Some(if method == "initialize" {
                    json!({ "commands": [{ "name": "count", "title": "Words: Count", "keys": ["alt+w"] }] })
                } else {
                    let words = params["context"]["text"]
                        .as_str()
                        .unwrap_or_default()
                        .split_whitespace()
                        .count();
                    json!({ "actions": [{ "type": "status", "text": format!("{words} words") }] })
                })
            })
            .await;
        });
        let PluginEvent::Ready { hello, .. } = next(&mut events).await else {
            panic!("expected ready");
        };
        assert_eq!(hello.commands[0].title, "Words: Count");
        assert_eq!(hello.commands[0].keys, ["alt+w"]);
        let actions = plugin
            .run("count", json!({ "text": "one two three" }), Value::Null)
            .await
            .expect("ran");
        assert_eq!(actions, [Action::Status("3 words".into())]);
    }

    /// A request from the plugin reaches mog and the answer gets back to the plugin.
    #[tokio::test]
    async fn answers_plugin_requests() {
        let (answer_tx, mut answer_rx) = mpsc::channel(1);
        let (plugin, mut events) = fake(
            "ask",
            Duration::from_secs(5),
            |(mut reader, mut writer)| async move {
                let Ok(Some(Message::Request { id, .. })) =
                    transport::read_message(&mut reader).await
                else {
                    panic!("expected initialize");
                };
                let hello = Message::Response {
                    id,
                    result: Ok(json!({})),
                };
                transport::write_message(&mut writer, &hello)
                    .await
                    .expect("write");
                let ask = Message::Request {
                    id: json!("q"),
                    method: "editor/context".into(),
                    params: json!({}),
                };
                transport::write_message(&mut writer, &ask)
                    .await
                    .expect("write");
                while let Ok(Some(message)) = transport::read_message(&mut reader).await {
                    if let Message::Response { id, result } = message {
                        assert_eq!(id, json!("q"));
                        let _ = answer_tx.send(result.expect("answered")).await;
                        return;
                    }
                }
            },
        );
        loop {
            if let PluginEvent::Request { id, method, .. } = next(&mut events).await {
                assert_eq!(method, "editor/context");
                plugin.respond(id, Ok(json!({ "line": 4 })));
                break;
            }
        }
        let result = time::timeout(Duration::from_secs(5), answer_rx.recv())
            .await
            .expect("in time")
            .expect("plugin side");
        assert_eq!(result["line"], 4);
    }

    /// A command that never answers times out and the plugin is told to cancel it.
    #[tokio::test]
    async fn times_out_and_cancels() {
        let (seen_tx, mut seen_rx) = mpsc::channel(1);
        let (plugin, mut events) = fake("slow", Duration::from_millis(100), |side| async move {
            let seen = serve(side, |method, _| {
                (method == "initialize").then(|| json!({}))
            })
            .await;
            let _ = seen_tx.send(seen).await;
        });
        assert!(matches!(next(&mut events).await, PluginEvent::Ready { .. }));
        let err = plugin
            .run("nap", json!({}), Value::Null)
            .await
            .expect_err("timed out");
        assert!(err.contains("did not answer"), "{err}");
        // a second request goes through the queue after the cancel, so the cancel was sent
        let _ = plugin
            .request("ping", json!({}), Duration::from_millis(50))
            .await;
        let stats = plugin.stats();
        assert_eq!((stats.requests, stats.timeouts), (2, 2));
        let slow: Vec<String> = plugin
            .take_slow()
            .into_iter()
            .map(|(method, _)| method)
            .collect();
        assert_eq!(slow, ["ping"], "commands do not count as slow");
        drop(plugin);
        let seen = time::timeout(Duration::from_secs(5), seen_rx.recv())
            .await
            .expect("in time")
            .expect("seen");
        assert!(
            seen.iter()
                .any(|(method, params)| method == "$/cancelRequest" && params["id"].is_u64()),
            "{seen:?}"
        );
    }

    /// Stopping a plugin that never answers ends it with a reason.
    #[tokio::test]
    async fn stops_on_request() {
        let (plugin, mut events) = fake("stuck", Duration::from_secs(60), |side| async move {
            serve(side, |method, _| {
                (method == "initialize").then(|| json!({}))
            })
            .await;
        });
        assert!(matches!(next(&mut events).await, PluginEvent::Ready { .. }));
        let waiting = plugin.clone();
        let asked = tokio::spawn(async move {
            waiting
                .request("nap", json!({}), Duration::from_secs(60))
                .await
        });
        plugin.stop();
        match next(&mut events).await {
            PluginEvent::Exited { reason, .. } => {
                assert_eq!(reason.as_deref(), Some("stopped by mog"));
            }
            other => panic!("{other:?}"),
        }
        let answer = asked.await.expect("joined");
        assert!(answer.is_err_and(|err| err.contains("stopped")));
    }

    /// Broken actions come back as an error instead of a partial edit.
    #[tokio::test]
    async fn reports_broken_actions() {
        let (plugin, mut events) = fake("bad", Duration::from_secs(5), |side| async move {
            serve(side, |method, _| {
                Some(if method == "initialize" {
                    json!({})
                } else {
                    json!({ "actions": [
                        { "type": "edit", "changes": [{ "start": 0, "end": 1, "text": "x" }] },
                        { "type": "edit", "changes": [{ "start": "nope" }] },
                    ] })
                })
            })
            .await;
        });
        assert!(matches!(next(&mut events).await, PluginEvent::Ready { .. }));
        let err = plugin
            .run("go", json!({}), Value::Null)
            .await
            .expect_err("broken");
        assert!(err.contains("action 1"), "{err}");
    }

    /// A plugin that never says hello is stopped with a reason.
    #[tokio::test(start_paused = true)]
    async fn stops_a_silent_plugin() {
        let (_plugin, mut events) = fake("mute", Duration::from_secs(5), |side| async move {
            serve(side, |_, _| None).await;
        });
        let PluginEvent::Exited { reason, .. } = next(&mut events).await else {
            panic!("expected exit");
        };
        assert!(reason.expect("reason").contains("initialize"));
    }

    /// A plugin speaking a protocol from the future is stopped and says why.
    #[tokio::test]
    async fn refuses_future_protocols() {
        let (_plugin, mut events) = fake("future", Duration::from_secs(5), |side| async move {
            serve(side, |_, _| Some(json!({ "protocolVersion": 99 }))).await;
        });
        let PluginEvent::Exited { reason, .. } = next(&mut events).await else {
            panic!("expected exit");
        };
        assert!(reason.expect("reason").contains("protocol 99"));
    }

    /// A message over the size limit stops the plugin instead of filling memory.
    #[tokio::test]
    async fn stops_on_huge_messages() {
        let (_plugin, mut events) = fake(
            "huge",
            Duration::from_secs(5),
            |(_reader, mut writer)| async move {
                let _ = writer
                    .write_all(b"Content-Length: 999999999999\r\n\r\n{}")
                    .await;
                let _ = writer.flush().await;
                time::sleep(Duration::from_secs(10)).await;
            },
        );
        let PluginEvent::Exited { reason, .. } = next(&mut events).await else {
            panic!("expected exit");
        };
        assert!(reason.expect("reason").contains("broken message"));
    }

    /// Requests to a plugin that crashed fail right away instead of waiting out the timeout.
    #[tokio::test]
    async fn fails_fast_after_a_crash() {
        let (plugin, mut events) = fake("crash", Duration::from_secs(60), |side| async move {
            let (mut reader, writer) = side;
            let _ = transport::read_message(&mut reader).await;
            drop((reader, writer));
        });
        assert!(matches!(
            next(&mut events).await,
            PluginEvent::Exited { .. }
        ));
        let started = time::Instant::now();
        assert!(plugin.run("x", json!({}), Value::Null).await.is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!plugin.is_running());
    }
}
