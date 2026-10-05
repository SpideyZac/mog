//! Syntax colors, bracket depths and git changes worked out on a background thread.
//!
//! Highlighting a big file takes far longer than a frame, so the editor view hands the text to a
//! [`Worker`] and keeps drawing with the colors it had, moved along with the edits made since.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::Duration,
};

use mog_core::{Transaction, movement};
use mog_git::{LineChange, line_changes};
use mog_syntax::{Edit, Highlighter, Incremental, Kind, Span};

/// Which document a job is for, as its index and path.
pub type DocumentKey = (usize, Option<PathBuf>);

/// How many documents the worker keeps parse trees for.
const KEPT_DOCUMENTS: usize = 16;

/// The work for one version of a document.
#[derive(Debug)]
pub struct Job {
    /// The document the text is from.
    pub document: DocumentKey,
    /// The document version the text is from.
    pub version: u64,
    /// The edits since an earlier version the worker saw, as that version and the changes of
    /// each transaction, so it can parse again from where it was.
    pub edits: Option<(u64, Vec<Vec<Edit>>)>,
    /// The language to highlight in, `None` for no colors.
    pub language: Option<&'static str>,
    /// The whole text.
    pub text: String,
    /// The text at the last commit, to mark changed lines against.
    pub base: Option<String>,
}

/// What a [`Job`] worked out.
#[derive(Debug, Default)]
pub struct Done {
    /// The document version the results are for.
    pub version: u64,
    /// The highlighted spans, sorted.
    pub spans: Vec<Span>,
    /// The bracket nesting depth at the start of each line.
    pub depths: Vec<usize>,
    /// The changed lines compared to the last commit.
    pub changes: Vec<(usize, LineChange)>,
}

/// Returns the bracket depth at the start of every line of `text`, skipping strings and
/// comments.
pub fn bracket_depths(text: &str, spans: &[Span]) -> Vec<usize> {
    let mut depths = vec![0];
    let mut depth = 0usize;
    let mut span = 0;
    for (pos, ch) in text.chars().enumerate() {
        if ch == '\n' {
            depths.push(depth);
            continue;
        }
        while span < spans.len() && spans[span].to <= pos {
            span += 1;
        }
        let quoted = spans
            .get(span)
            .is_some_and(|s| s.from <= pos && matches!(s.kind, Kind::String | Kind::Comment));
        if quoted {
            continue;
        }
        if movement::BRACKETS.iter().any(|(open, _)| *open == ch) {
            depth += 1;
        } else if movement::BRACKETS.iter().any(|(_, close)| *close == ch) {
            depth = depth.saturating_sub(1);
        }
    }
    depths
}

/// Converts editor transactions to the edits the highlighter replays.
pub fn to_edits(changes: &[&Transaction]) -> Vec<Vec<Edit>> {
    changes
        .iter()
        .map(|tx| {
            tx.changes()
                .iter()
                .map(|change| Edit {
                    start: change.start,
                    end: change.end,
                    text: change.text.clone(),
                })
                .collect()
        })
        .collect()
}

/// Moves `spans` through `changes`, dropping any that end up empty.
pub fn map_spans(spans: &mut Vec<Span>, changes: &[&Transaction]) {
    if changes.is_empty() {
        return;
    }
    for span in spans.iter_mut() {
        for tx in changes {
            span.from = tx.map_pos(span.from);
            span.to = tx.map_pos(span.to);
        }
    }
    spans.retain(|span| span.from < span.to);
}

/// The highlighting the worker keeps for each document, with the version it is for.
type Documents = HashMap<DocumentKey, (u64, Incremental)>;

/// Does `job` with `highlighter`, starting from what was kept for its document in `documents`.
fn run(job: Job, highlighter: &mut Highlighter, documents: &mut Documents) -> Done {
    let spans = match job.language {
        Some(language) => {
            if documents.len() >= KEPT_DOCUMENTS && !documents.contains_key(&job.document) {
                documents.clear();
            }
            let (version, state) = documents
                .entry(job.document.clone())
                .or_insert_with(|| (0, Incremental::new(language)));
            if state.language() != language {
                *state = Incremental::new(language);
            }
            // edits only help when they start from the version the kept tree is for
            let edits = job
                .edits
                .as_ref()
                .filter(|(from, _)| *from == *version)
                .map(|(_, edits)| edits.as_slice());
            state.update(highlighter, &job.text, edits);
            *version = job.version;
            state.spans().to_vec()
        }
        None => {
            documents.remove(&job.document);
            Vec::new()
        }
    };
    let depths = bracket_depths(&job.text, &spans);
    let changes = job
        .base
        .map(|base| line_changes(&base, &job.text))
        .unwrap_or_default();
    Done {
        version: job.version,
        spans,
        depths,
        changes,
    }
}

/// A background thread that does [`Job`]s one at a time.
#[derive(Debug)]
pub struct Worker {
    /// Where jobs go.
    jobs: Sender<Job>,
    /// Where results come back.
    done: Receiver<Done>,
    /// Whether a job was sent and its result has not been taken yet.
    busy: bool,
}

impl Default for Worker {
    fn default() -> Self {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (outbox, done) = mpsc::channel();
        thread::spawn(move || {
            let mut highlighter = Highlighter::new();
            let mut documents = Documents::new();
            while let Ok(mut job) = inbox.recv() {
                // only the newest text matters when edits came in faster than the work
                while let Ok(newer) = inbox.try_recv() {
                    job = newer;
                }
                if outbox
                    .send(run(job, &mut highlighter, &mut documents))
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            jobs,
            done,
            busy: false,
        }
    }
}

impl Worker {
    /// Starts `job`. Results of older jobs that are still running are dropped.
    pub fn send(&mut self, job: Job) {
        self.busy = self.jobs.send(job).is_ok();
    }

    /// Returns whether a job is running.
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// Takes the newest result, waiting up to `wait` for one.
    pub fn take(&mut self, wait: Duration) -> Option<Done> {
        if !self.busy {
            return None;
        }
        let mut newest = match self.done.recv_timeout(wait) {
            Ok(done) => done,
            Err(RecvTimeoutError::Timeout) => return None,
            Err(RecvTimeoutError::Disconnected) => {
                self.busy = false;
                return None;
            }
        };
        while let Ok(newer) = self.done.try_recv() {
            newest = newer;
        }
        self.busy = false;
        Some(newest)
    }
}

#[cfg(test)]
/// Tests for background highlighting.
mod tests {
    use std::time::Duration;

    use mog_core::Transaction;
    use mog_syntax::{Edit, Highlighter, Kind, Span};

    use super::{Job, Worker, bracket_depths, map_spans};

    /// Depths count brackets but not ones inside strings.
    #[test]
    fn counts_bracket_depths() {
        let spans = [Span {
            from: 5,
            to: 8,
            kind: Kind::String,
        }];
        assert_eq!(bracket_depths("f(\n  \"(\"\n)\n", &spans), [0, 1, 1, 0]);
    }

    /// Spans move with text inserted before them and grow with text typed inside.
    #[test]
    fn maps_spans_through_edits() {
        let mut spans = vec![Span {
            from: 4,
            to: 8,
            kind: Kind::Keyword,
        }];
        let before = Transaction::insert(0, "ab");
        let inside = Transaction::insert(8, "x");
        map_spans(&mut spans, &[&before, &inside]);
        assert_eq!((spans[0].from, spans[0].to), (6, 11));
    }

    /// A job with the edits since the last one gets the same colors as starting over.
    #[test]
    fn worker_follows_edits() {
        let mut worker = Worker::default();
        let first = "fn main() {}\n";
        worker.send(Job {
            document: (0, None),
            edits: None,
            version: 1,
            language: Some("rust"),
            text: first.into(),
            base: None,
        });
        worker.take(Duration::from_secs(30)).expect("first");
        let second = "fn main() { let s = \"hi\"; }\n";
        let edits = vec![vec![Edit {
            start: 11,
            end: 11,
            text: " let s = \"hi\";".into(),
        }]];
        worker.send(Job {
            document: (0, None),
            edits: Some((1, edits)),
            version: 2,
            language: Some("rust"),
            text: second.into(),
            base: None,
        });
        let done = worker.take(Duration::from_secs(30)).expect("second");
        assert_eq!(done.version, 2);
        assert_eq!(done.spans, Highlighter::new().highlight("rust", second));
    }

    /// The worker highlights in the background and hands back the result.
    #[test]
    fn worker_highlights() {
        let mut worker = Worker::default();
        worker.send(Job {
            document: (0, None),
            edits: None,
            version: 3,
            language: Some("rust"),
            text: "fn main() {}\n".into(),
            base: None,
        });
        let done = worker.take(Duration::from_secs(30)).expect("result");
        assert_eq!(done.version, 3);
        assert!(done.spans.iter().any(|span| span.kind == Kind::Keyword));
        assert!(!worker.is_busy());
    }
}
