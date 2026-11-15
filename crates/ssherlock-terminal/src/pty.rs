//! Pseudo-terminals: the byte stream between a terminal and the program running in it.
//!
//! [`PtyIo`] is the same pair of channels whatever is on the other end: a local shell started by
//! [`spawn_local`], or (from Phase 2) an SSH channel bridged with [`PtyIo::pair`]. So
//! [`TerminalHandle`](crate::TerminalHandle) never needs to know where its bytes come from.
//!
//! A local pty's API is blocking, so each one gets two dedicated threads: one reads output, one
//! writes input and applies resizes. They are plain threads rather than Tokio blocking tasks
//! because they live as long as the shell, and a blocked read must never hold up runtime
//! shutdown. They talk to async code only through bounded `tokio::sync::mpsc` channels.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::{fmt, thread};

use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ssherlock_core::app;
use tokio::sync::mpsc;

use crate::error::TerminalError;
use crate::size::TerminalSize;

/// Output chunks in flight from a pty to its terminal.
///
/// With chunks of up to [`READ_CHUNK`] bytes this buffers at most 512 KiB. When it's full the
/// reader blocks, so the program blocks on write: a flood (`cat` of a huge file) is slowed to
/// the speed the terminal can parse instead of filling memory.
pub const OUTPUT_CAPACITY: usize = 8;

/// Input messages queued for a pty. A message can be a whole paste, so a few dozen is plenty.
pub const INPUT_CAPACITY: usize = 64;

/// Largest single read from a local pty.
const READ_CHUNK: usize = 64 * 1024;

/// The terminal's end of a pty: output to read, and a sender for input and resizes.
#[derive(Debug)]
pub struct PtyIo {
    /// Output from the program, then one [`PtyOutput::Exited`] when it ends.
    pub output: mpsc::Receiver<PtyOutput>,
    /// Input and resizes for the program.
    pub input: mpsc::Sender<PtyInput>,
}

/// The program's end of a [`PtyIo`], for bridging another byte source (an SSH channel, a test)
/// to a terminal.
#[derive(Debug)]
pub struct PtyPeer {
    /// Where to send the program's output.
    pub output: mpsc::Sender<PtyOutput>,
    /// Input and resizes from the terminal.
    pub input: mpsc::Receiver<PtyInput>,
}

impl PtyIo {
    /// Both ends of a new pty-shaped channel pair.
    pub fn pair() -> (PtyIo, PtyPeer) {
        let (output_tx, output_rx) = mpsc::channel(OUTPUT_CAPACITY);
        let (input_tx, input_rx) = mpsc::channel(INPUT_CAPACITY);
        (
            PtyIo {
                output: output_rx,
                input: input_tx,
            },
            PtyPeer {
                output: output_tx,
                input: input_rx,
            },
        )
    }
}

/// Something from the program's side of a pty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PtyOutput {
    /// Bytes the program wrote.
    Data(Vec<u8>),
    /// The program exited; nothing follows.
    Exited(PtyExit),
}

/// Something for the program's side of a pty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PtyInput {
    /// Bytes to send, as if typed.
    Write(Vec<u8>),
    /// The terminal changed size; the program gets `SIGWINCH`.
    Resize(TerminalSize),
}

/// How the program in a pty ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PtyExit {
    /// It exited with this status code.
    Code(u32),
    /// It was killed by this signal (Unix), named as the OS describes it.
    Signal(String),
    /// It ended, but its status couldn't be read.
    Unknown,
}

impl PtyExit {
    /// Whether it exited with status 0.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Code(0))
    }
}

impl fmt::Display for PtyExit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Code(code) => write!(f, "exited with status {code}"),
            Self::Signal(signal) => write!(f, "killed by signal {signal}"),
            Self::Unknown => f.write_str("exited"),
        }
    }
}

impl From<portable_pty::ExitStatus> for PtyExit {
    fn from(status: portable_pty::ExitStatus) -> Self {
        match status.signal() {
            Some(signal) => Self::Signal(signal.to_owned()),
            None => Self::Code(status.exit_code()),
        }
    }
}

/// What to run in a local pty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ShellProgram {
    /// The user's shell (`$SHELL`, else the password database), started as a login shell.
    #[default]
    LoginShell,
    /// A specific program with arguments, looked up on `PATH`.
    Command {
        /// The program.
        program: OsString,
        /// Its arguments.
        args: Vec<OsString>,
    },
}

/// How to start a local shell.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellConfig {
    /// What to run.
    pub program: ShellProgram,
    /// Starting directory. `None`, or a directory that doesn't exist, means the home directory.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables, applied after the ones every terminal gets (`TERM` etc.).
    pub env: Vec<(OsString, OsString)>,
}

/// Starts a program in a new local pty of `size`.
///
/// The program sees `TERM=xterm-256color` and `COLORTERM=truecolor`. Dropping the returned
/// [`PtyIo::input`] hangs up the program (`SIGHUP` on Unix), the same as closing a terminal
/// window.
///
/// # Errors
///
/// Fails if the OS can't create a pty, the program can't be started, or the I/O threads can't
/// be spawned.
pub fn spawn_local(config: &ShellConfig, size: TerminalSize) -> Result<PtyIo, TerminalError> {
    let pair = native_pty_system()
        .openpty(pty_size(size))
        .map_err(|error| TerminalError::OpenPty(error.into()))?;

    let program_name = match &config.program {
        ShellProgram::LoginShell => "the login shell".to_owned(),
        ShellProgram::Command { program, .. } => program.to_string_lossy().into_owned(),
    };
    let child =
        pair.slave
            .spawn_command(command(config))
            .map_err(|error| TerminalError::Spawn {
                program: program_name.clone(),
                source: error.into(),
            })?;
    // Only the child may hold the program's end open, or reads would never see it close.
    drop(pair.slave);

    let master = pair.master;
    let reader = master
        .try_clone_reader()
        .map_err(|error| TerminalError::PtyEnd {
            end: "reading",
            source: error.into(),
        })?;
    let writer = master
        .take_writer()
        .map_err(|error| TerminalError::PtyEnd {
            end: "writing",
            source: error.into(),
        })?;
    let killer = child.clone_killer();

    let (io, peer) = PtyIo::pair();
    let PtyPeer { output, input } = peer;
    spawn_thread("pty-reader", move || read_loop(reader, child, &output))?;
    // If this fails, dropping the closure drops the killer and master; the reader thread then
    // ends when the program does.
    spawn_thread("pty-writer", move || {
        write_loop(writer, &*master, killer, input);
    })?;

    tracing::info!(program = %program_name, columns = size.columns(), rows = size.rows(), "started local pty");
    Ok(io)
}

fn command(config: &ShellConfig) -> CommandBuilder {
    let mut command = match &config.program {
        ShellProgram::LoginShell => CommandBuilder::new_default_prog(),
        ShellProgram::Command { program, args } => {
            let mut command = CommandBuilder::new(program);
            command.args(args);
            command
        }
    };
    if let Some(cwd) = &config.cwd {
        command.cwd(cwd);
    }
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("TERM_PROGRAM", app::NAME);
    command.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
    for (key, value) in &config.env {
        command.env(key, value);
    }
    command
}

fn pty_size(size: TerminalSize) -> PtySize {
    PtySize {
        rows: size.rows(),
        cols: size.columns(),
        // The kernel's winsize holds the whole text area in pixels, not one cell.
        pixel_width: size.columns().saturating_mul(size.cell_width()),
        pixel_height: size.rows().saturating_mul(size.cell_height()),
    }
}

fn spawn_thread(
    name: &'static str,
    body: impl FnOnce() + Send + 'static,
) -> Result<(), TerminalError> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(body)
        .map(drop)
        .map_err(|source| TerminalError::Thread { name, source })
}

/// Forwards output until the program closes the pty, then reports how it exited.
fn read_loop(
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn Child + Send + Sync>,
    output: &mpsc::Sender<PtyOutput>,
) {
    let mut buffer = vec![0; READ_CHUNK];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                let chunk = buffer.get(..read).unwrap_or_default().to_vec();
                if output.blocking_send(PtyOutput::Data(chunk)).is_err() {
                    // The terminal is gone; the writer thread hangs up the program.
                    break;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                // Linux reports EIO once the program and everything it started have closed the
                // pty, which is the normal way for a session to end.
                tracing::debug!(%error, "pty read ended");
                break;
            }
        }
    }

    let exit = match child.wait() {
        Ok(status) => PtyExit::from(status),
        Err(error) => {
            tracing::warn!(%error, "could not read the exit status of the program in the pty");
            PtyExit::Unknown
        }
    };
    tracing::info!(%exit, "program in local pty ended");
    if output.blocking_send(PtyOutput::Exited(exit)).is_err() {
        tracing::debug!("terminal closed before the program's exit could be reported");
    }
}

/// Applies input and resizes until the terminal drops its sender, then hangs up the program.
fn write_loop(
    mut writer: Box<dyn Write + Send>,
    master: &dyn MasterPty,
    mut killer: Box<dyn ChildKiller + Send + Sync>,
    mut input: mpsc::Receiver<PtyInput>,
) {
    while let Some(message) = input.blocking_recv() {
        match message {
            PtyInput::Write(bytes) => {
                if let Err(error) = writer.write_all(&bytes).and_then(|()| writer.flush()) {
                    tracing::warn!(%error, "could not write to the pty, the program has probably exited");
                    break;
                }
            }
            PtyInput::Resize(size) => {
                if let Err(error) = master.resize(pty_size(size)) {
                    tracing::warn!(%error, "could not resize the pty");
                }
            }
        }
    }
    if let Err(error) = killer.kill() {
        // Fails only when the program has already gone, which is fine.
        tracing::debug!(%error, "could not hang up the program in the pty");
    }
}

#[cfg(all(test, unix))]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::time::Duration;

    use pretty_assertions::assert_eq;
    use tokio::time::timeout;

    use super::*;

    const LIMIT: Duration = Duration::from_secs(10);

    fn sh(script: &str) -> ShellConfig {
        ShellConfig {
            program: ShellProgram::Command {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), script.into()],
            },
            ..ShellConfig::default()
        }
    }

    /// Reads until the program exits, returning everything it printed and how it ended.
    async fn run_to_exit(io: &mut PtyIo) -> (String, PtyExit) {
        let mut text = Vec::new();
        loop {
            let output = timeout(LIMIT, io.output.recv())
                .await
                .expect("pty timed out")
                .expect("pty closed without reporting an exit");
            match output {
                PtyOutput::Data(bytes) => text.extend(bytes),
                PtyOutput::Exited(exit) => {
                    return (String::from_utf8_lossy(&text).into_owned(), exit);
                }
            }
        }
    }

    #[tokio::test]
    async fn runs_a_program_and_reports_its_exit_status() {
        let mut io = spawn_local(&sh("printf hello; exit 3"), TerminalSize::default()).unwrap();
        let (text, exit) = run_to_exit(&mut io).await;
        assert_eq!(text, "hello");
        assert_eq!(exit, PtyExit::Code(3));
        assert!(!exit.is_success());
    }

    #[tokio::test]
    async fn sets_the_terminal_environment_and_directory() {
        let mut config = sh(r#"printf '%s|%s|%s|%s' "$TERM" "$COLORTERM" "$EXTRA" "$(pwd)""#);
        config.cwd = Some("/".into());
        config.env.push(("EXTRA".into(), "yes".into()));
        let mut io = spawn_local(&config, TerminalSize::default()).unwrap();

        let (text, exit) = run_to_exit(&mut io).await;
        assert_eq!(text, "xterm-256color|truecolor|yes|/");
        assert!(exit.is_success());
    }

    #[tokio::test]
    async fn input_and_resizes_reach_the_program() {
        let script = r#"read line; stty size; printf 'got:%s' "$line""#;
        let mut io = spawn_local(&sh(script), TerminalSize::new(80, 24)).unwrap();
        io.input
            .send(PtyInput::Resize(TerminalSize::new(100, 40)))
            .await
            .unwrap();
        io.input
            .send(PtyInput::Write(b"abc\n".to_vec()))
            .await
            .unwrap();

        let (text, exit) = run_to_exit(&mut io).await;
        // The pty echoes input with CRLF line endings.
        assert_eq!(text, "abc\r\n40 100\r\ngot:abc");
        assert!(exit.is_success());
    }

    #[tokio::test]
    async fn dropping_the_input_hangs_up_the_program() {
        let PtyIo { mut output, input } =
            spawn_local(&sh("sleep 30"), TerminalSize::default()).unwrap();
        drop(input);

        let exit = loop {
            let next = timeout(LIMIT, output.recv())
                .await
                .expect("no hang-up")
                .expect("pty closed without reporting an exit");
            if let PtyOutput::Exited(exit) = next {
                break exit;
            }
        };
        assert!(matches!(exit, PtyExit::Signal(_)), "{exit:?}");
    }

    #[tokio::test]
    async fn a_missing_program_is_an_error() {
        let config = ShellConfig {
            program: ShellProgram::Command {
                program: "/nonexistent/ssherlock-test".into(),
                args: Vec::new(),
            },
            ..ShellConfig::default()
        };
        let error = spawn_local(&config, TerminalSize::default()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "could not start /nonexistent/ssherlock-test in a pseudo-terminal"
        );
    }

    #[test]
    fn pty_size_reports_the_whole_text_area_in_pixels() {
        let size = pty_size(TerminalSize::new(80, 24).with_cell_size(9, 18));
        assert_eq!(
            (size.cols, size.rows, size.pixel_width, size.pixel_height),
            (80, 24, 720, 432)
        );
    }

    #[test]
    fn exit_descriptions() {
        assert_eq!(PtyExit::Code(0).to_string(), "exited with status 0");
        assert_eq!(
            PtyExit::Signal("Hangup".into()).to_string(),
            "killed by signal Hangup"
        );
        assert!(PtyExit::Code(0).is_success());
        assert!(!PtyExit::Unknown.is_success());
    }
}
