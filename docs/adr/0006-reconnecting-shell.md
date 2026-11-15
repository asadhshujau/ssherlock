# 0006 — A supervisor task owns each remote shell and reconnects it

**Status:** accepted · 2026-11-15

## Context
Laptops sleep and Wi-Fi switches. CONN-10 asks for keep-alive, timeouts and auto-reconnect,
CONN-27 for a latency indicator. russh has keep-alives, but they don't report round-trip time,
and its session just ends when they fail. A terminal tab should survive a drop instead of dying.

## Decision
- `spawn_shell` returns one `PtyIo` for the tab's lifetime. A supervisor task behind it runs
  connect → log in → open shell → bridge. When the connection is lost it runs that sequence again
  and feeds the new shell into the same `PtyIo`.
- Liveness comes from our own heartbeat, not from russh's keep-alive: a `keepalive@openssh.com`
  global request with `want_reply`, once per interval. The reply (always a refusal) gives the
  latency. N unanswered pings in a row mean the connection is lost.
- Only transient errors are retried (TCP, timeout, protocol, channel closed), with exponential
  backoff up to a cap. A refused login, a rejected host key or a cancelled prompt ends the
  connection.
- The first connection is never retried. A typo'd host should show its error at once.
- Typing while disconnected is dropped, never replayed into the next shell.
- States (`Connecting`, `Authenticating`, `Connected { latency }`, `Reconnecting`,
  `Disconnected(reason)`) go to the UI as `SessionEvent::State`.

## Consequences
- The remote shell is new after a reconnect (no tmux-style resume); its output starts below the
  old output in the same tab.
- A login that needs a password asks again on every reconnect. Saved credentials (Phase 4)
  remove that.
- Dropping the `PtyIo` is the one way to stop the task, so closing a tab can't leave an orphan
  task behind.
