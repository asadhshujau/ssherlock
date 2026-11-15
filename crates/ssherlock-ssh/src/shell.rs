//! An interactive shell on the server, bridged to a terminal as a [`PtyIo`].
//!
//! **Service** layer. [`Session::open_shell`] opens a session channel, asks for a pty and a
//! shell, then hands the channel to a bridge task that copies bytes both ways (the reconnecting
//! version is [`spawn_shell`](crate::connection::spawn_shell)). The terminal sees
//! exactly what it sees for a local shell (ADR 0005); it never learns the bytes come over SSH.

use russh::client::Msg;
use russh::{Channel, ChannelMsg, Sig};
use ssherlock_terminal::{PtyExit, PtyInput, PtyIo, PtyOutput, PtyPeer, TerminalSize};

use crate::SshError;
use crate::session::Session;

/// The terminal type announced to the server; matches what local shells get.
const TERM: &str = "xterm-256color";

/// A shell channel that has been granted its pty and shell, plus any output that arrived while
/// waiting for the server's confirmations.
pub(crate) struct OpenShell {
    channel: Channel<Msg>,
    early: Vec<Vec<u8>>,
}

/// How a bridged shell ended.
#[derive(Debug)]
pub(crate) enum ShellEnd {
    /// The channel closed. `exit` is set if the server reported how the shell ended; if not,
    /// the connection probably dropped.
    ChannelClosed { exit: Option<PtyExit> },
    /// The terminal went away (tab closed); the channel has been closed.
    TerminalClosed,
}

impl Session {
    /// Opens a login shell with a pty of `size`, as `ssh host` does.
    ///
    /// The returned [`PtyIo`] is fed by a background task that ends when the shell exits (after
    /// sending [`PtyOutput::Exited`]) or when the `PtyIo` is dropped (closing the channel). It
    /// doesn't reconnect; [`spawn_shell`](crate::connection::spawn_shell) does.
    ///
    /// # Errors
    ///
    /// [`SshError::ChannelRequestRefused`] if the server refuses the pty or the shell,
    /// [`SshError::ChannelClosed`] if the channel closes before the shell starts,
    /// [`SshError::Protocol`] if the connection fails.
    pub async fn open_shell(&self, size: TerminalSize) -> Result<PtyIo, SshError> {
        let shell = self.open_shell_channel(size).await?;
        let (io, mut peer) = PtyIo::pair();
        let span = tracing::info_span!("shell", host = %self.config().host());
        tokio::spawn(tracing::Instrument::instrument(
            async move {
                let mut size = size;
                if let ShellEnd::ChannelClosed { exit } = bridge(shell, &mut peer, &mut size).await
                {
                    // The terminal may already be gone; then there's nobody to tell.
                    let _ = peer
                        .output
                        .send(PtyOutput::Exited(exit.unwrap_or(PtyExit::Unknown)))
                        .await;
                }
            },
            span,
        ));
        Ok(io)
    }

    /// Opens a session channel and starts a login shell on a pty of `size`.
    pub(crate) async fn open_shell_channel(
        &self,
        size: TerminalSize,
    ) -> Result<OpenShell, SshError> {
        let mut channel = self.handle().channel_open_session().await?;
        let (pixel_width, pixel_height) = pixels(size);
        channel
            .request_pty(
                true,
                TERM,
                size.columns().into(),
                size.rows().into(),
                pixel_width,
                pixel_height,
                &[],
            )
            .await?;
        let mut early = expect_success(&mut channel, "pty").await?;
        channel.request_shell(true).await?;
        early.extend(expect_success(&mut channel, "shell").await?);
        Ok(OpenShell { channel, early })
    }
}

/// Waits for the reply to a `want_reply` request, keeping any output that arrives first.
async fn expect_success(
    channel: &mut Channel<Msg>,
    request: &'static str,
) -> Result<Vec<Vec<u8>>, SshError> {
    let mut early = Vec::new();
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Success) => return Ok(early),
            Some(ChannelMsg::Failure) => return Err(SshError::ChannelRequestRefused { request }),
            Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                early.push(data.to_vec());
            }
            Some(ChannelMsg::Close | ChannelMsg::Eof) | None => {
                return Err(SshError::ChannelClosed);
            }
            Some(_) => {}
        }
    }
}

/// Copies shell output to the terminal and terminal input to the shell until either side ends.
/// `size` follows the terminal's resizes, so a reconnect can open the next shell at the right
/// size. The peer is borrowed, not consumed, so one terminal can outlive many channels.
pub(crate) async fn bridge(
    shell: OpenShell,
    peer: &mut PtyPeer,
    size: &mut TerminalSize,
) -> ShellEnd {
    let OpenShell { channel, early } = shell;
    let (mut read, write) = channel.split();
    let mut exit = None;
    let mut early = early.into_iter();
    let end = loop {
        // Output that arrived before the shell was confirmed goes first.
        if let Some(data) = early.next() {
            if peer.output.send(PtyOutput::Data(data)).await.is_err() {
                break ShellEnd::TerminalClosed;
            }
            continue;
        }
        tokio::select! {
            message = read.wait() => match message {
                // Both stdout and stderr go to the terminal, as with a local pty.
                Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                    if peer.output.send(PtyOutput::Data(data.to_vec())).await.is_err() {
                        break ShellEnd::TerminalClosed;
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => exit = Some(PtyExit::Code(exit_status)),
                Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                    exit = Some(PtyExit::Signal(signal(&signal_name)));
                }
                Some(ChannelMsg::Close) | None => {
                    tracing::info!(?exit, "shell channel closed");
                    return ShellEnd::ChannelClosed { exit };
                }
                Some(_) => {}
            },
            command = peer.input.recv() => match command {
                Some(PtyInput::Write(bytes)) => {
                    if let Err(e) = write.data_bytes(bytes).await {
                        // The connection is failing; the caller learns why from the session.
                        tracing::warn!(error = %e, "could not send input to the shell");
                        return ShellEnd::ChannelClosed { exit };
                    }
                }
                Some(PtyInput::Resize(new_size)) => {
                    *size = new_size;
                    let (pixel_width, pixel_height) = pixels(new_size);
                    if let Err(e) = write
                        .window_change(new_size.columns().into(), new_size.rows().into(), pixel_width, pixel_height)
                        .await
                    {
                        // A missed resize only leaves the remote side with the old size.
                        tracing::warn!(error = %e, "could not send the new terminal size");
                    }
                }
                None => break ShellEnd::TerminalClosed,
            },
        }
    };
    tracing::debug!("terminal closed; closing the shell channel");
    if let Err(e) = write.close().await {
        // The connection is already gone, which closes the channel anyway.
        tracing::debug!(error = %e, "shell channel was already closed");
    }
    end
}

/// The terminal's size in pixels, or zeros when the cell size isn't known.
fn pixels(size: TerminalSize) -> (u32, u32) {
    (
        u32::from(size.columns()) * u32::from(size.cell_width()),
        u32::from(size.rows()) * u32::from(size.cell_height()),
    )
}

fn signal(sig: &Sig) -> String {
    match sig {
        Sig::Custom(name) => name.clone(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn pixel_size() {
        assert_eq!(pixels(TerminalSize::new(80, 24)), (0, 0));
        assert_eq!(
            pixels(TerminalSize::new(80, 24).with_cell_size(9, 18)),
            (720, 432)
        );
    }

    #[test]
    fn signal_names() {
        assert_eq!(signal(&Sig::INT), "INT");
        assert_eq!(signal(&Sig::Custom("XCPU".to_owned())), "XCPU");
    }
}
