# 0005 — Terminal task, snapshots over `watch`, ptys as channel pairs

**Status:** accepted · 2026-11-10

## Context
A terminal is fed by a program (local pty now, SSH channel in Phase 2) and painted by gpui on
the foreground thread. Zed shares `Arc<FairMutex<Term>>` between its I/O loop and the renderer.
CLAUDE.md asks for services that talk upward over channels, and no lock held across a render.
`portable-pty`'s API is blocking.

## Decision
- One Tokio task per terminal owns the `Terminal` (emulator) outright. The UI sends commands
  over a bounded `mpsc`. It reads the newest `Arc<Snapshot>` from a `watch` channel, plus
  coalesced `TerminalEvent`s from a bounded `mpsc`. Snapshots are capped at one per 8 ms.
- A pty, of any kind, is a `PtyIo`: an output `Receiver<PtyOutput>` and an input
  `Sender<PtyInput>`. Each local pty runs two named std threads (reader, writer) that bridge the
  blocking API to those channels. They are std threads rather than `spawn_blocking` so that
  a blocked `read` can't hold up runtime shutdown.

## Consequences
- No locks between I/O and rendering. A slow UI just skips stale frames. Bounded output
  channels throttle a flooding program at the parser's speed.
- Selection, search and scrolling become commands to the task, with answers via `oneshot`, and
  not direct grid access from the view.
- SSH (Phase 2) only has to provide a `PtyIo`, via `PtyIo::pair`. A `trait` isn't needed.
- Each snapshot copies the visible cells (~12k for 200×60). That's cheap at 120 Hz. Damage
  tracking can come later if profiling asks for it.
