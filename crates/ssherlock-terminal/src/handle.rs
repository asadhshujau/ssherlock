//! A running terminal: a [`Terminal`] fed by a pty, on its own Tokio task.
//!
//! The task owns the emulator outright, so nothing is locked. The UI talks to it only through
//! channels: commands go in, the latest [`Snapshot`] comes out through a `watch` channel (which
//! keeps only the newest, so a slow reader never queues stale frames), and [`TerminalEvent`]s
//! come out through a bounded `mpsc`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, sleep_until};
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::error::TerminalError;
use crate::palette::Palette;
use crate::pty::{PtyInput, PtyIo, PtyOutput};
use crate::selection::{CellSide, GridPoint, SelectionKind};
use crate::size::TerminalSize;
use crate::snapshot::Snapshot;
use crate::terminal::{Terminal, TerminalConfig, TerminalEvent};

/// Commands queued for a terminal's task. Keystrokes are tiny and the task takes them as fast as
/// they arrive; this absorbs a burst of key repeat or paste chunks while the task is busy
/// parsing a flood of output.
const COMMAND_CAPACITY: usize = 1024;

/// Events queued for the UI. Each batch of output yields at most one bell and one title change,
/// so this only fills if the UI stops reading altogether.
const EVENT_CAPACITY: usize = 64;

/// Shortest gap between two snapshots (about 120 per second). A flood of output costs at most
/// one snapshot per frame however many chunks arrive.
const FRAME: Duration = Duration::from_millis(8);

/// Most output parsed in one go before the task looks at commands again, so typing stays
/// responsive while a program prints without pause.
const BATCH_BYTES: usize = 1024 * 1024;

/// Controls a terminal running on the Tokio runtime.
///
/// Dropping the handle stops the task and hangs up the program.
#[derive(Debug)]
pub struct TerminalHandle {
    commands: mpsc::Sender<Command>,
    snapshots: watch::Receiver<Arc<Snapshot>>,
    _stop: DropGuard,
}

#[derive(Debug)]
enum Command {
    Write(Vec<u8>),
    Resize(TerminalSize),
    SetPalette(Palette),
    StartSelection(SelectionKind, GridPoint, CellSide),
    ExtendSelection(GridPoint, CellSide),
    ClearSelection,
    SelectionText(oneshot::Sender<Option<String>>),
}

impl TerminalHandle {
    /// Starts a terminal of `size` reading from and writing to `pty`, on `runtime`.
    ///
    /// Returns the handle and the terminal's events.
    pub fn spawn(
        pty: PtyIo,
        config: &TerminalConfig,
        size: TerminalSize,
        runtime: &Handle,
    ) -> (Self, mpsc::Receiver<TerminalEvent>) {
        let terminal = Terminal::new(config, size);
        let (snapshot_tx, snapshot_rx) = watch::channel(Arc::new(terminal.snapshot()));
        let (command_tx, command_rx) = mpsc::channel(COMMAND_CAPACITY);
        let (event_tx, event_rx) = mpsc::channel(EVENT_CAPACITY);
        let stop = CancellationToken::new();

        let task = Task {
            terminal,
            pty_output: Some(pty.output),
            pty_input: Some(pty.input),
            pending_input: VecDeque::new(),
            commands: command_rx,
            snapshots: snapshot_tx,
            events: event_tx,
            is_dirty: false,
            last_publish: Instant::now(),
        };
        runtime.spawn(task.run(stop.clone()));

        let handle = Self {
            commands: command_tx,
            snapshots: snapshot_rx,
            _stop: stop.drop_guard(),
        };
        (handle, event_rx)
    }

    /// Sends bytes to the program, as if typed.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Busy`] if the input queue is full, [`TerminalError::Stopped`] if the
    /// task has ended.
    pub fn write(&self, bytes: impl Into<Vec<u8>>) -> Result<(), TerminalError> {
        self.send(Command::Write(bytes.into()))
    }

    /// Changes the terminal's size and tells the program.
    ///
    /// # Errors
    ///
    /// As for [`write`](Self::write).
    pub fn resize(&self, size: TerminalSize) -> Result<(), TerminalError> {
        self.send(Command::Resize(size))
    }

    /// Replaces the colours cells are drawn with.
    ///
    /// # Errors
    ///
    /// As for [`write`](Self::write).
    pub fn set_palette(&self, palette: Palette) -> Result<(), TerminalError> {
        self.send(Command::SetPalette(palette))
    }

    /// Starts a selection at a visible cell, replacing any existing one.
    ///
    /// # Errors
    ///
    /// As for [`write`](Self::write).
    pub fn start_selection(
        &self,
        kind: SelectionKind,
        at: GridPoint,
        side: CellSide,
    ) -> Result<(), TerminalError> {
        self.send(Command::StartSelection(kind, at, side))
    }

    /// Moves the free end of the selection to a visible cell.
    ///
    /// # Errors
    ///
    /// As for [`write`](Self::write).
    pub fn extend_selection(&self, to: GridPoint, side: CellSide) -> Result<(), TerminalError> {
        self.send(Command::ExtendSelection(to, side))
    }

    /// Removes the selection.
    ///
    /// # Errors
    ///
    /// As for [`write`](Self::write).
    pub fn clear_selection(&self) -> Result<(), TerminalError> {
        self.send(Command::ClearSelection)
    }

    /// The selected text, read by the terminal's task. Resolves to `None` when nothing is
    /// selected or the task has stopped.
    ///
    /// # Errors
    ///
    /// As for [`write`](Self::write), if the request can't be queued.
    pub fn selection_text(
        &self,
    ) -> Result<impl Future<Output = Option<String>> + Send + 'static, TerminalError> {
        let (reply, answer) = oneshot::channel();
        self.send(Command::SelectionText(reply))?;
        Ok(async move { answer.await.ok().flatten() })
    }

    /// The most recent snapshot.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshots.borrow())
    }

    /// A receiver that wakes whenever a new snapshot is published.
    pub fn subscribe(&self) -> watch::Receiver<Arc<Snapshot>> {
        self.snapshots.clone()
    }

    fn send(&self, command: Command) -> Result<(), TerminalError> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => TerminalError::Busy,
                mpsc::error::TrySendError::Closed(_) => TerminalError::Stopped,
            })
    }
}

/// What woke a terminal's task.
enum Wake {
    Command(Command),
    Output(Option<PtyOutput>),
    InputSent { is_open: bool },
    Deadline,
}

/// The state owned by a terminal's task.
struct Task {
    terminal: Terminal,
    /// `None` once the program has exited and its output channel closed.
    pty_output: Option<mpsc::Receiver<PtyOutput>>,
    /// `None` once the program stopped taking input.
    pty_input: Option<mpsc::Sender<PtyInput>>,
    /// Input waiting for room in the pty's queue. The task never waits on the pty while
    /// holding output, which would deadlock with a program that is itself blocked writing.
    /// Bounded in practice by what the user types or pastes.
    pending_input: VecDeque<PtyInput>,
    commands: mpsc::Receiver<Command>,
    snapshots: watch::Sender<Arc<Snapshot>>,
    events: mpsc::Sender<TerminalEvent>,
    /// The screen changed since the last snapshot.
    is_dirty: bool,
    last_publish: Instant,
}

impl Task {
    async fn run(mut self, stop: CancellationToken) {
        loop {
            let deadline = self.deadline();
            // Branches only say what woke the task; it is handled below, once the branch
            // futures (which borrow fields of `self`) are gone.
            let wake = tokio::select! {
                biased;
                () = stop.cancelled() => break,
                Some(command) = self.commands.recv() => Wake::Command(command),
                output = recv_or_pending(self.pty_output.as_mut()) => Wake::Output(output),
                is_open = send_pending(self.pty_input.as_ref(), &mut self.pending_input) => {
                    Wake::InputSent { is_open }
                }
                () = sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                    Wake::Deadline
                }
            };
            match wake {
                Wake::Command(command) => self.command(command),
                Wake::Output(Some(output)) => self.output(output).await,
                Wake::Output(None) => self.pty_output = None,
                Wake::InputSent { is_open: false } => self.input_closed(),
                Wake::InputSent { is_open: true } | Wake::Deadline => {}
            }
            self.end_sync_if_due();
            self.publish_if_due();
        }
        tracing::debug!("terminal task stopped");
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Write(bytes) => self.queue_input(PtyInput::Write(bytes)),
            Command::Resize(size) => {
                if size != self.terminal.size() {
                    self.terminal.resize(size);
                    self.queue_input(PtyInput::Resize(size));
                    self.is_dirty = true;
                }
            }
            Command::SetPalette(palette) => {
                self.terminal.set_palette(palette);
                self.is_dirty = true;
            }
            Command::StartSelection(kind, at, side) => {
                self.terminal.start_selection(kind, at, side);
                self.is_dirty = true;
            }
            Command::ExtendSelection(to, side) => {
                self.terminal.extend_selection(to, side);
                self.is_dirty = true;
            }
            Command::ClearSelection => {
                self.terminal.clear_selection();
                self.is_dirty = true;
            }
            Command::SelectionText(reply) => {
                if reply.send(self.terminal.selection_text()).is_err() {
                    tracing::debug!("selection text requested and then abandoned");
                }
            }
        }
    }

    /// Parses one item of output plus whatever else is already waiting, up to [`BATCH_BYTES`].
    async fn output(&mut self, first: PtyOutput) {
        let mut parsed = 0;
        let mut exit = None;
        let mut next = Some(first);
        while let Some(output) = next.take() {
            match output {
                PtyOutput::Data(bytes) => {
                    self.terminal.feed(&bytes);
                    parsed += bytes.len();
                }
                PtyOutput::Exited(status) => exit = Some(status),
            }
            if parsed < BATCH_BYTES && exit.is_none() {
                next = self.pty_output.as_mut().and_then(|rx| rx.try_recv().ok());
            }
        }
        self.is_dirty = true;

        let reply = self.terminal.take_reply();
        if !reply.is_empty() {
            self.queue_input(PtyInput::Write(reply));
        }
        for event in coalesce(self.terminal.drain_events()) {
            emit(&self.events, event).await;
        }
        if let Some(status) = exit {
            self.pty_output = None;
            // Show the program's last output before saying it ended.
            self.publish();
            emit(&self.events, TerminalEvent::Exited(status)).await;
        }
    }

    fn queue_input(&mut self, input: PtyInput) {
        if self.pty_input.is_some() {
            self.pending_input.push_back(input);
        } else {
            tracing::debug!("dropping input for a program that has stopped reading");
        }
    }

    fn input_closed(&mut self) {
        tracing::debug!("the program stopped taking input");
        self.pty_input = None;
        self.pending_input.clear();
    }

    /// The next moment the loop must wake without input: a frame boundary for a pending
    /// snapshot, or the end of a synchronized update.
    fn deadline(&self) -> Option<Instant> {
        let frame = self.is_dirty.then(|| self.last_publish + FRAME);
        let sync = self.terminal.sync_deadline().map(Instant::from_std);
        match (frame, sync) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn end_sync_if_due(&mut self) {
        if let Some(deadline) = self.terminal.sync_deadline()
            && Instant::from_std(deadline) <= Instant::now()
        {
            self.terminal.end_sync();
            self.is_dirty = true;
        }
    }

    fn publish_if_due(&mut self) {
        if self.is_dirty && Instant::now() >= self.last_publish + FRAME {
            self.publish();
        }
    }

    fn publish(&mut self) {
        self.snapshots
            .send_replace(Arc::new(self.terminal.snapshot()));
        self.is_dirty = false;
        self.last_publish = Instant::now();
    }
}

/// Sends an event to the UI. Takes only the sender so `Task` (which isn't `Sync`) isn't borrowed
/// across the await.
async fn emit(events: &mpsc::Sender<TerminalEvent>, event: TerminalEvent) {
    if events.send(event).await.is_err() {
        tracing::debug!("nobody is listening for terminal events");
    }
}

/// Keeps the last title change and one bell from a batch: a program that rings the bell a
/// million times shouldn't wake the UI a million times.
fn coalesce(events: Vec<TerminalEvent>) -> Vec<TerminalEvent> {
    let title = events
        .iter()
        .rev()
        .find(|event| matches!(event, TerminalEvent::TitleChanged(_)))
        .cloned();
    let bell = events
        .iter()
        .any(|event| matches!(event, TerminalEvent::Bell))
        .then_some(TerminalEvent::Bell);
    let exits = events
        .into_iter()
        .filter(|event| matches!(event, TerminalEvent::Exited(_)));
    title.into_iter().chain(bell).chain(exits).collect()
}

/// Receives from `rx`, or waits forever when there is no receiver.
async fn recv_or_pending<T>(rx: Option<&mut mpsc::Receiver<T>>) -> Option<T> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Sends the oldest pending input once `tx` has room, or waits forever when there is nothing to
/// send. Returns whether the program is still taking input.
async fn send_pending<T>(tx: Option<&mpsc::Sender<T>>, pending: &mut VecDeque<T>) -> bool {
    let Some(tx) = tx.filter(|_| !pending.is_empty()) else {
        return std::future::pending().await;
    };
    let Ok(permit) = tx.reserve().await else {
        return false;
    };
    if let Some(input) = pending.pop_front() {
        permit.send(input);
    }
    true
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::future::Future;

    use pretty_assertions::assert_eq;
    use tokio::time::timeout;

    use super::*;
    use crate::pty::{PtyExit, PtyPeer};

    const LIMIT: Duration = Duration::from_secs(5);

    async fn within<T>(future: impl Future<Output = T>) -> T {
        timeout(LIMIT, future).await.expect("timed out")
    }

    fn start(size: TerminalSize) -> (TerminalHandle, mpsc::Receiver<TerminalEvent>, PtyPeer) {
        let (io, peer) = PtyIo::pair();
        let (handle, events) =
            TerminalHandle::spawn(io, &TerminalConfig::default(), size, &Handle::current());
        (handle, events, peer)
    }

    /// Waits for a snapshot whose first row starts with `text`.
    async fn wait_for_row(handle: &TerminalHandle, text: &str) -> Arc<Snapshot> {
        let mut rx = handle.subscribe();
        within(async {
            loop {
                let snapshot = Arc::clone(&rx.borrow_and_update());
                let row: String = snapshot
                    .lines()
                    .next()
                    .unwrap()
                    .iter()
                    .map(|c| c.ch)
                    .collect();
                if row.starts_with(text) {
                    return snapshot;
                }
                rx.changed().await.unwrap();
            }
        })
        .await
    }

    #[tokio::test]
    async fn output_is_published_as_snapshots() {
        let (handle, _events, peer) = start(TerminalSize::new(20, 4));
        peer.output
            .send(PtyOutput::Data(b"hello".to_vec()))
            .await
            .unwrap();

        let snapshot = wait_for_row(&handle, "hello").await;
        assert_eq!(snapshot.cursor.unwrap().column, 5);
        assert_eq!(handle.snapshot(), snapshot);
    }

    #[tokio::test]
    async fn replies_to_queries_go_back_to_the_program() {
        let (_handle, _events, mut peer) = start(TerminalSize::new(20, 4));
        peer.output
            .send(PtyOutput::Data(b"ab\x1b[6n".to_vec()))
            .await
            .unwrap();

        let reply = within(peer.input.recv()).await.unwrap();
        assert_eq!(reply, PtyInput::Write(b"\x1b[1;3R".to_vec()));
    }

    #[tokio::test]
    async fn writes_and_resizes_reach_the_program_in_order() {
        let (handle, _events, mut peer) = start(TerminalSize::new(20, 4));
        handle.write(b"ls\r".to_vec()).unwrap();
        handle.resize(TerminalSize::new(30, 5)).unwrap();
        // Unchanged size: nothing is sent.
        handle.resize(TerminalSize::new(30, 5)).unwrap();
        handle.write("é").unwrap();

        assert_eq!(
            within(peer.input.recv()).await.unwrap(),
            PtyInput::Write(b"ls\r".to_vec())
        );
        assert_eq!(
            within(peer.input.recv()).await.unwrap(),
            PtyInput::Resize(TerminalSize::new(30, 5))
        );
        assert_eq!(
            within(peer.input.recv()).await.unwrap(),
            PtyInput::Write("é".into())
        );

        let mut rx = handle.subscribe();
        within(rx.wait_for(|s| s.columns() == 30 && s.rows() == 5))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn events_are_coalesced_per_batch() {
        let (_handle, mut events, peer) = start(TerminalSize::new(20, 4));
        peer.output
            .send(PtyOutput::Data(
                b"\x07\x1b]0;one\x07\x07\x07\x1b]0;two\x07".to_vec(),
            ))
            .await
            .unwrap();

        assert_eq!(
            within(events.recv()).await.unwrap(),
            TerminalEvent::TitleChanged(Some("two".into()))
        );
        assert_eq!(within(events.recv()).await.unwrap(), TerminalEvent::Bell);
    }

    #[tokio::test]
    async fn exit_is_reported_after_the_last_output() {
        let (handle, mut events, peer) = start(TerminalSize::new(20, 4));
        peer.output
            .send(PtyOutput::Data(b"bye".to_vec()))
            .await
            .unwrap();
        peer.output
            .send(PtyOutput::Exited(PtyExit::Code(0)))
            .await
            .unwrap();

        assert_eq!(
            within(events.recv()).await.unwrap(),
            TerminalEvent::Exited(PtyExit::Code(0))
        );
        let row: String = handle
            .snapshot()
            .lines()
            .next()
            .unwrap()
            .iter()
            .map(|c| c.ch)
            .collect();
        assert!(row.starts_with("bye"), "{row:?}");
    }

    #[tokio::test]
    async fn input_after_the_program_stops_reading_is_dropped() {
        let (handle, _events, peer) = start(TerminalSize::new(20, 4));
        drop(peer.input);
        handle.write(b"a".to_vec()).unwrap();
        handle.write(b"b".to_vec()).unwrap();
        // The task keeps running and still publishes output.
        peer.output
            .send(PtyOutput::Data(b"still here".to_vec()))
            .await
            .unwrap();
        wait_for_row(&handle, "still here").await;
    }

    #[tokio::test]
    async fn dropping_the_handle_stops_the_task_and_closes_the_pty() {
        let (handle, _events, mut peer) = start(TerminalSize::new(20, 4));
        drop(handle);
        assert_eq!(within(peer.input.recv()).await, None);
        within(peer.output.closed()).await;
    }

    #[tokio::test]
    async fn palette_changes_are_published() {
        let (handle, _events, _peer) = start(TerminalSize::new(20, 4));
        let palette = Palette {
            background: crate::palette::Rgb::new(1, 2, 3),
            ..Palette::default()
        };
        handle.set_palette(palette).unwrap();
        let mut rx = handle.subscribe();
        within(rx.wait_for(|s| s.background == crate::palette::Rgb::new(1, 2, 3)))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn selections_are_drawn_and_copied() {
        let (handle, _events, peer) = start(TerminalSize::new(20, 4));
        peer.output
            .send(PtyOutput::Data(b"hello world".to_vec()))
            .await
            .unwrap();
        wait_for_row(&handle, "hello world").await;

        handle
            .start_selection(SelectionKind::Word, GridPoint::new(0, 7), CellSide::Left)
            .unwrap();
        let mut rx = handle.subscribe();
        within(rx.wait_for(|s| s.selection.is_some()))
            .await
            .unwrap();
        let text = within(handle.selection_text().unwrap()).await;
        assert_eq!(text.as_deref(), Some("world"));

        handle
            .extend_selection(GridPoint::new(0, 0), CellSide::Left)
            .unwrap();
        let text = within(handle.selection_text().unwrap()).await;
        assert_eq!(text.as_deref(), Some("hello world"));

        handle.clear_selection().unwrap();
        within(rx.wait_for(|s| s.selection.is_none()))
            .await
            .unwrap();
        assert_eq!(within(handle.selection_text().unwrap()).await, None);
    }

    #[test]
    fn coalesce_keeps_the_last_title_and_one_bell() {
        let events = vec![
            TerminalEvent::Bell,
            TerminalEvent::TitleChanged(Some("a".into())),
            TerminalEvent::Bell,
            TerminalEvent::TitleChanged(None),
        ];
        assert_eq!(
            coalesce(events),
            [TerminalEvent::TitleChanged(None), TerminalEvent::Bell]
        );
        assert_eq!(coalesce(Vec::new()), []);
    }
}
