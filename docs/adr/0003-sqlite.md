# 0003 — SQLite for local storage

**Status:** accepted · 2026-11-10

## Context
Hosts, groups, identities, keys, snippets and rules are relational (groups nest, hosts reference
identities), need search, and must survive crashes. Secrets must never be stored in plain text.
Sync (Phase 11) needs a reliable local change log.

## Decision
Store everything in one SQLite database via [rusqlite](https://crates.io/crates/rusqlite)
`0.41.x` with the `bundled` feature, in WAL mode with foreign keys on. Schema changes are numbered
SQL migrations. Secret columns hold ciphertext produced by `ssherlock-store::vault`, whose key
lives in the OS keychain.

## Consequences
- Transactions, constraints and indexes for free; one file to back up; tests run on in-memory
  databases with every migration applied.
- `bundled` compiles SQLite from C source, which adds build time but avoids depending on the
  system library's version.
- rusqlite is synchronous, so all store calls run off the UI thread (Tokio or gpui background
  executor).
- Shipped migrations are never edited, only superseded, so schema mistakes are permanent history.
