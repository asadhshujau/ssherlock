//! The one error type for this crate.

/// An error from starting or talking to a terminal.
#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    /// The operating system would not create a pseudo-terminal.
    #[error("could not open a pseudo-terminal")]
    OpenPty(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The shell or program could not be started in the pty.
    #[error("could not start {program} in a pseudo-terminal")]
    Spawn {
        /// The program, or `"the login shell"` when none was given.
        program: String,
        /// Why it failed.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The pty was created but its reading or writing end could not be opened.
    #[error("could not open the {end} end of the pseudo-terminal")]
    PtyEnd {
        /// `"reading"` or `"writing"`.
        end: &'static str,
        /// Why it failed.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// A background thread for pty I/O could not be started.
    #[error("could not start the {name} thread")]
    Thread {
        /// The thread's name.
        name: &'static str,
        /// Why it failed.
        #[source]
        source: std::io::Error,
    },

    /// The terminal's task has stopped, so it can't take more input.
    #[error("the terminal has stopped")]
    Stopped,

    /// The terminal is so far behind on input that it can't queue more right now.
    #[error("the terminal is not keeping up with input")]
    Busy,
}
