//! A shell running in a pseudo terminal.

use std::{
    env,
    io::{self, Read, Write},
    path::Path,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use vt100::Parser;

/// How many lines of output are kept above the screen.
const SCROLLBACK: usize = 2000;

/// What a program sends to ask where the cursor is.
const CURSOR_QUERY: &[u8] = b"\x1b[6n";

/// The shell output writer, shared with the thread that answers cursor queries.
type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Locks `mutex`, ignoring poisoning since the data stays usable.
fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Returns the shell to run when the config does not name one.
fn default_shell() -> (String, Vec<String>) {
    if cfg!(windows) {
        ("powershell.exe".into(), vec!["-NoLogo".into()])
    } else {
        (
            env::var("SHELL").unwrap_or_else(|_| "sh".into()),
            Vec::new(),
        )
    }
}

/// A running shell and the screen it draws on.
pub struct Shell {
    /// The parsed screen, fed by the reader thread.
    parser: Arc<Mutex<Parser>>,
    /// Where keys for the shell go.
    writer: SharedWriter,
    /// The controlling side of the pty, kept for resizing.
    master: Box<dyn MasterPty + Send>,
    /// The shell process.
    child: Box<dyn Child + Send + Sync>,
    /// Set by the reader thread once the shell closed its output.
    exited: Arc<AtomicBool>,
    /// The size of the screen as `(rows, cols)`.
    size: (u16, u16),
    /// The name of the program, for the panel title.
    name: String,
}

impl Shell {
    /// Starts `program` with `args` in `cwd` on a screen of `rows` by `cols`.
    ///
    /// An empty `program` picks the default shell for the platform.
    ///
    /// # Errors
    ///
    /// Returns an error if the pty cannot be opened or the shell cannot be started.
    pub fn spawn(
        program: &str,
        args: &[String],
        cwd: &Path,
        rows: u16,
        cols: u16,
    ) -> io::Result<Self> {
        let (program, args) = if program.is_empty() {
            default_shell()
        } else {
            (program.to_owned(), args.to_vec())
        };
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(io::Error::other)?;
        let mut command = CommandBuilder::new(&program);
        command.args(&args);
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(io::Error::other)?;
        // the shell holds its own handle and ours would keep the pty open after it exits
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(io::Error::other)?;
        let writer: SharedWriter = Arc::new(Mutex::new(
            pair.master.take_writer().map_err(io::Error::other)?,
        ));
        let parser = Arc::new(Mutex::new(Parser::new(rows, cols, SCROLLBACK)));
        let exited = Arc::new(AtomicBool::new(false));
        {
            let (parser, writer, exited) = (parser.clone(), writer.clone(), exited.clone());
            thread::spawn(move || read_output(reader, &parser, &writer, &exited));
        }
        let name = Path::new(&program)
            .file_stem()
            .map_or_else(|| program.clone(), |stem| stem.to_string_lossy().into());
        Ok(Self {
            parser,
            writer,
            master: pair.master,
            child,
            exited,
            size: (rows, cols),
            name,
        })
    }

    /// Returns the name of the shell program, like `powershell`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Sends `bytes` to the shell as if they were typed.
    pub fn write(&self, bytes: &[u8]) {
        let mut writer = lock(&self.writer);
        // a shell that went away just stops listening
        let _ = writer.write_all(bytes).and_then(|()| writer.flush());
    }

    /// Changes the screen size to `rows` by `cols`.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        if self.size == (rows, cols) || rows == 0 || cols == 0 {
            return;
        }
        self.size = (rows, cols);
        lock(&self.parser).screen_mut().set_size(rows, cols);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    /// Returns the parsed screen, locked for reading.
    pub fn parser(&self) -> MutexGuard<'_, Parser> {
        lock(&self.parser)
    }

    /// Returns `true` once the shell has exited.
    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::Relaxed)
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Feeds shell output into `parser` until the shell closes it, then sets `exited`.
fn read_output(
    mut reader: Box<dyn Read + Send>,
    parser: &Mutex<Parser>,
    writer: &SharedWriter,
    exited: &AtomicBool,
) {
    let mut buf = [0; 8192];
    loop {
        let read = match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let chunk = &buf[..read];
        let mut parser = lock(parser);
        parser.process(chunk);
        // conpty asks where the cursor is on startup and waits for the answer
        if chunk.windows(CURSOR_QUERY.len()).any(|w| w == CURSOR_QUERY) {
            let (row, col) = parser.screen().cursor_position();
            let answer = format!("\x1b[{};{}R", row + 1, col + 1);
            let mut writer = lock(writer);
            let _ = writer
                .write_all(answer.as_bytes())
                .and_then(|()| writer.flush());
        }
    }
    exited.store(true, Ordering::Relaxed);
}

#[cfg(test)]
/// Tests for [`Shell`].
mod tests {
    use std::{
        env, thread,
        time::{Duration, Instant},
    };

    use super::Shell;

    /// The default shell runs a command and its output lands on the screen.
    #[test]
    fn runs_a_command() {
        let cwd = env::temp_dir();
        let shell = Shell::spawn("", &[], &cwd, 24, 80).expect("shell starts");
        shell.write(b"echo mog$((1+1))mog\r");
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(20) {
            let contents = shell.parser().screen().contents();
            // powershell does not do the math so accept either
            if contents.matches("mog").count() >= 4 {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("no output: {}", shell.parser().screen().contents());
    }
}
