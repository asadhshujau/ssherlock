# 0002 — russh for SSH

**Status:** accepted · 2026-11-10

## Context
SSHerlock needs SSH client sessions, every common auth method, port forwarding, agent
forwarding, jump hosts and SFTP, driven from async code without blocking the UI thread.
Options: shell out to the system `ssh`, bind `libssh2`/`libssh` through FFI, or a pure Rust
implementation.

## Decision
Use [russh](https://crates.io/crates/russh) `0.65.x` (keys via `russh::keys`) and
[russh-sftp](https://crates.io/crates/russh-sftp) `3.x`, running on the app's Tokio runtime.

## Consequences
- Pure Rust and async-native: no C library to build or ship, no FFI `unsafe` in our tree, and
  sessions fit the "services emit events over channels" design.
- We control host-key verification and auth prompts directly, so the TOFU and changed-key dialogs
  are ours rather than parsed from a subprocess.
- We implement what OpenSSH gives for free: `known_hosts` handling, `ssh_config` semantics,
  ProxyJump chaining. Behaviour must be tested against a real OpenSSH server (`tests/docker/`).
- Algorithm support follows russh; anything it lacks (e.g. some FIDO2 flows) waits on upstream.
