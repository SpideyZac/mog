//! A fake mog for testing plugins without starting the editor.
//!
//! [`FakeHost`] runs a plugin, as a process or over in-memory pipes, and answers it the way mog
//! would from documents kept in memory. [`Script`] describes a test as steps in TOML, which is
//! what `mog plugin test` runs.
//!
//! ```no_run
//! # async fn test(spec: mog_plugin::Spec) -> Result<(), String> {
//! use mog_plugin_test::FakeHost;
//! use serde_json::Value;
//!
//! let mut host = FakeHost::start(&spec, std::path::Path::new(".")).await?;
//! host.open("notes.md", "hello world");
//! host.command("count", Value::Null).await?;
//! assert_eq!(host.status(), Some("2 words"));
//! # Ok(())
//! # }
//! ```

pub mod host;
pub mod matching;
pub mod script;

pub use host::{FakeDocument, FakeHost, Seen};
pub use matching::{matches, mismatch};
pub use script::{Script, StepReport, run_on, run_script};

#[cfg(test)]
/// Tests for the fake host, against a plugin running in the test over in-memory pipes.
mod tests {
    use std::{path::Path, time::Duration};

    use mog_lsp::transport::{self, Message};
    use serde_json::{Value, json};
    use tokio::io::{self, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

    use crate::{FakeHost, Script, run_on};

    /// Sends `message` to the host.
    async fn send(writer: &mut WriteHalf<DuplexStream>, message: Message) {
        transport::write_message(writer, &message)
            .await
            .expect("sent");
    }

    /// Asks the host `method` and returns its answer.
    async fn ask(
        reader: &mut BufReader<ReadHalf<DuplexStream>>,
        writer: &mut WriteHalf<DuplexStream>,
        id: u64,
        method: &str,
        params: Value,
    ) -> Value {
        let request = Message::Request {
            id: json!(id),
            method: method.into(),
            params,
        };
        send(writer, request).await;
        loop {
            match transport::read_message(reader).await {
                Ok(Some(Message::Response { id: got, result })) if got == json!(id) => {
                    return result.unwrap_or(Value::Null);
                }
                Ok(Some(_)) => {}
                _ => return Value::Null,
            }
        }
    }

    /// A plugin that counts words, inserts what the user picks and says when a file is saved.
    async fn words(
        mut reader: BufReader<ReadHalf<DuplexStream>>,
        mut writer: WriteHalf<DuplexStream>,
    ) {
        while let Ok(Some(message)) = transport::read_message(&mut reader).await {
            match message {
                Message::Request { id, method, params } => {
                    let result = match (method.as_str(), params["command"].as_str()) {
                        ("initialize", _) => json!({
                            "protocolVersion": 2,
                            "commands": [
                                { "name": "count", "title": "Count" },
                                { "name": "choose", "title": "Choose" },
                            ],
                            "events": ["saved"],
                        }),
                        ("command", Some("count")) => {
                            let text =
                                ask(&mut reader, &mut writer, 100, "editor/text", json!({})).await;
                            let words = text["text"].as_str().unwrap_or("").split_whitespace();
                            let status = format!("{} words", words.count());
                            json!({ "actions": [{ "type": "status", "text": status }] })
                        }
                        ("command", Some("choose")) => {
                            let items = json!({ "items": ["a", "b"] });
                            let picked = ask(&mut reader, &mut writer, 101, "ui/pick", items).await;
                            let text = picked["item"].as_str().unwrap_or("none").to_owned();
                            json!({ "actions": [{ "type": "insert", "text": text }] })
                        }
                        _ => json!({}),
                    };
                    let reply = Message::Response {
                        id,
                        result: Ok(result),
                    };
                    send(&mut writer, reply).await;
                }
                Message::Notification { method, params } => {
                    if method == "shutdown" {
                        break;
                    }
                    if params["kind"] == "saved" {
                        let segment = Message::Notification {
                            method: "segment".into(),
                            params: json!({ "text": "saved" }),
                        };
                        send(&mut writer, segment).await;
                    }
                }
                Message::Response { .. } => {}
            }
        }
        let _ = writer.shutdown().await;
    }

    /// Starts the word plugin and a host talking to it.
    async fn host() -> FakeHost {
        let (host_side, plugin_side) = io::duplex(1 << 16);
        let (host_read, host_write) = io::split(host_side);
        let (plugin_read, plugin_write) = io::split(plugin_side);
        tokio::spawn(words(BufReader::new(plugin_read), plugin_write));
        FakeHost::connect(
            "words",
            host_read,
            host_write,
            Path::new("/project"),
            &json!({}),
        )
        .await
        .expect("started")
    }

    /// Commands get answers to what they ask, and their actions change the fake editor.
    #[tokio::test]
    async fn runs_commands() {
        let mut host = host().await;
        assert_eq!(host.hello().commands.len(), 2);
        host.open("notes.md", "hello brave world");
        host.command("count", Value::Null).await.expect("counted");
        assert_eq!(host.status(), Some("3 words"));
        assert_eq!(host.requests()[0].method, "editor/text");
        host.queue_pick(Some(1));
        host.command("choose", Value::Null).await.expect("chose");
        assert_eq!(host.text(), "bhello brave world");
        host.send_event("saved", json!({ "path": "notes.md" }));
        let segment = |host: &FakeHost| {
            host.notifications()
                .iter()
                .any(|seen| seen.method == "segment")
        };
        assert!(host.wait_for(Duration::from_secs(2), segment).await);
    }

    /// A script runs step by step and says what did not happen.
    #[tokio::test]
    async fn runs_scripts() {
        let script = Script::parse(
            r#"
            [[files]]
            path = "notes.md"
            text = "one two"

            [[steps]]
            command = "count"
            expect = { status = "2 words", requests = [{ method = "editor/text" }] }

            [[steps]]
            name = "picks the first"
            pick = 0
            command = "choose"
            expect.text = "aone two"
            expect.result = { actions = [{ type = "insert", text = "a" }] }

            [[steps]]
            event = "saved"
            params = { path = "notes.md" }
            expect.notifications = [{ method = "segment", params = { text = "saved" } }]

            [[steps]]
            name = "wrong on purpose"
            command = "count"
            wait = 100
            expect.status = "9 words"

            [[steps]]
            event = "idle"
            "#,
        )
        .expect("valid script");
        let reports = run_on(host().await, &script).await.expect("ran");
        let failed: Vec<&str> = reports
            .iter()
            .filter(|report| !report.passed())
            .map(|report| report.name.as_str())
            .collect();
        assert_eq!(failed, ["wrong on purpose", "event idle"], "{reports:#?}");
        assert_eq!(reports[1].name, "picks the first");
        assert!(
            reports[3].problems[0].contains("\"2 words\""),
            "{reports:#?}"
        );
    }
}
