# 0009 — Port forwards follow the connection's current session

**Status:** accepted · 2026-11-21

## Context
Phase 5 runs local, remote and SOCKS forwards (FWD-01…03) that must survive reconnects and
start again on the new session (FWD-07), run over a connection with no shell (FWD-06), and
report status and byte counts. `PLAN.md` sketched "one trait `Forward` with
`start / stop / stats`". The kinds are a closed set, saved as data, and differ only in where
they listen and how each connection finds its destination.

## Decision
`Forward` is an enum in `ssherlock-core`, not a trait. `ConnectionHandle::start_forward`
spawns one Tokio task per forward and returns a `RunningForward`. Its status is a `watch`
receiver, its stats are atomic counters, and dropping it stops the forward. Stop and stats
are methods on that handle, not trait methods.

The reconnecting supervisor publishes its session as `watch<Option<Arc<Session>>>`: `Some`
while connected, `None` while connecting or reconnecting. Forwards read it rather than owning a
session:

- Local and SOCKS listeners stay bound across a drop and use whatever session is current.
- A remote forward asks each new session to listen again, retrying a refusal for a short while
  in case the dead connection still holds the server's port.

Connections arriving on remote ports are routed by port, through a table the session shares
with its russh handler. They are accepted or refused only after the local destination answers.
`spawn_forwarding` is the same supervisor without a shell, and it ends when its last handle is
dropped.

## Consequences
- Reconnect handling lives in one place, the supervisor. Forwards need no reconnect logic of
  their own.
- A local port never flaps during a drop. Connections made while the link is down are closed
  at once.
- Views follow `watch` receivers and poll counters. They never hold service internals.
