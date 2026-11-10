# SSHerlock — Engineering Rules

SSHerlock is a desktop SSH client (a Termius replacement) written in Rust with gpui.
This file is the contract for everyone who writes code here, human or Claude.
Read `docs/PLAN.md` for architecture and phases, `docs/FEATURES.md` for what to build.

## 1. Non-negotiables

1. **No `unsafe`.** Every crate has `#![forbid(unsafe_code)]` and the workspace lint forbids it. If something seems to need `unsafe`, find a safe crate that wraps it or change the design. Dependencies may contain `unsafe`; our code never does.
2. **The Rust way wins.** Idiomatic Rust (ownership, enums, traits, iterators, `Result`) takes precedence over patterns from other languages. Only deviate when it is genuinely impossible, and then write an ADR in `docs/adr/` explaining why.
3. **gpui for all UI.** No web views, no other toolkits, no platform UI except through gpui or a thin `platform` module (biometrics, URL handlers).
4. **Never block the foreground thread.** Network, disk, crypto and pty I/O run on Tokio or `cx.background_spawn`. If a function can take more than ~1 ms, it is `async` or runs in the background.
5. **Secrets never leak.** Keys, passwords, passphrases, tokens are `SecretString` / `SecretBox<[u8]>`, never `String`. No `Debug` derive on types holding secrets without a manual redacting impl. Never logged, never in panics, never in crash reports.
6. **No panics in reachable code.** `unwrap()`, `expect()`, `panic!`, indexing that may fail — not in library or UI code. Use `?`, `Option` combinators, `get()`. Tests may `unwrap()`.
7. **Same shape everywhere.** Every feature follows the slice layout in §3 and the naming in §4 so that learning one feature teaches all of them.
8. **Everything is free.** No tiers, no licence checks, no `is_pro` / `plan` / `entitlement` fields, no feature gated behind an account. Sync and team features need a server by nature, but the client talks to any instance (including self-hosted) and never asks for payment.

## 2. Workspace

```
crates/
  ssherlock/            bin   – main.rs only wires things together
  ssherlock-ui/         lib   – gpui: windows, views, components, theme, keymap
  ssherlock-core/       lib   – domain types, ids, errors, traits. Depends on nothing internal.
  ssherlock-store/      lib   – SQLite persistence, migrations, vault encryption
  ssherlock-terminal/   lib   – VT grid, pty, terminal events
  ssherlock-ssh/        lib   – russh sessions, auth, forwarding, sftp
  ssherlock-import/     lib   – importers (Phase 10)
  ssherlock-sync/       lib   – sync client (Phase 11)
  ssherlock-ai/         lib   – AI providers (Phase 13)
```

Dependency rules:

- Only `ssherlock` and `ssherlock-ui` may depend on `gpui`.
- `ssherlock-core` depends on no workspace crate; everyone depends on it.
- Library crates never depend on `ssherlock-ui` or `ssherlock`.
- All external versions live in `[workspace.dependencies]`; crates use `dep = { workspace = true }`. `gpui` is pinned exactly (`"=0.2.2"`) because its API changes between minor versions.
- Adding a dependency: prefer pure Rust, actively maintained, permissive licence, few transitive deps. Check `cargo deny check` passes. Add a one-line reason in the workspace `Cargo.toml` comment.

Edition 2024 module style: `foo.rs` + `foo/bar.rs`. **No `mod.rs` files.**

## 3. The feature slice

A feature (hosts, keychain, snippets, forwarding, …) is built as these layers, in this order, each with tests before the next begins:

| Layer | Crate | File | Contains |
|-------|-------|------|----------|
| model | `ssherlock-core` | `src/<feature>.rs` | plain data types, `Id<T>`, validation, pure functions |
| store | `ssherlock-store` | `src/<feature>.rs` | `impl Repository<Model> for Store`, SQL, row mapping |
| service | `ssherlock-ssh` / `-terminal` / etc. | `src/<feature>.rs` | async logic, I/O, emits events |
| view | `ssherlock-ui` | `src/<feature>.rs` + `src/<feature>/` | gpui `Entity`s, `Render`, actions, keymap entries |

View folder layout, identical for every feature:

```
ssherlock-ui/src/hosts.rs            pub mod + re-exports + `init(cx)` that registers actions
ssherlock-ui/src/hosts/actions.rs    actions!(hosts, [NewHost, ConnectSelected, ...])
ssherlock-ui/src/hosts/list.rs       HostList entity (state + render)
ssherlock-ui/src/hosts/editor.rs     HostEditor entity
ssherlock-ui/src/hosts/item.rs       stateless row component
```

Shape of each layer (illustrative; adapt types, keep the shape):

```rust
// ── model (ssherlock-core/src/hosts.rs) ─────────────────────────────────────
/// A saved SSH host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub id: Id<Host>,
    pub label: String,
    pub address: Address,          // enum, not a stringly-typed field
    pub port: u16,
    pub username: Option<String>,
    pub identity: Option<Id<Identity>>,
    pub group: Option<Id<Group>>,
    pub tags: Vec<Tag>,
    pub notes: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Host {
    /// Pure, total, unit-tested. Business rules live here, not in the view.
    pub fn validate(&self) -> Result<(), HostError> { /* … */ }
}

// ── store (ssherlock-store/src/hosts.rs) ────────────────────────────────────
impl Repository<Host> for Store {
    fn get(&self, id: Id<Host>) -> Result<Option<Host>, StoreError> { /* … */ }
    fn list(&self) -> Result<Vec<Host>, StoreError> { /* … */ }
    fn upsert(&self, host: &Host) -> Result<(), StoreError> { /* … */ }
    fn delete(&self, id: Id<Host>) -> Result<(), StoreError> { /* … */ }
}

// ── service (ssherlock-ssh/src/session.rs) ──────────────────────────────────
pub struct Session { /* … */ }

#[derive(Debug, Clone)]
pub enum SessionEvent { Connected, Data(Bytes), Disconnected(Reason) }

impl Session {
    pub async fn connect(cfg: ConnectConfig, events: mpsc::Sender<SessionEvent>) -> Result<Self, SshError> { /* … */ }
}

// ── view (ssherlock-ui/src/hosts/list.rs) ───────────────────────────────────
pub struct HostList {
    hosts: Vec<Host>,
    selected: Option<Id<Host>>,
    store: Arc<Store>,
}

pub enum HostListEvent { Connect(Id<Host>) }
impl EventEmitter<HostListEvent> for HostList {}

impl HostList {
    pub fn new(store: Arc<Store>, cx: &mut Context<Self>) -> Self { /* … */ }
    fn reload(&mut self, cx: &mut Context<Self>) { /* background load, then cx.notify() */ }
    fn connect_selected(&mut self, _: &ConnectSelected, _: &mut Window, cx: &mut Context<Self>) { /* cx.emit(...) */ }
}

impl Render for HostList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().key_context("HostList").on_action(cx.listener(Self::connect_selected)) /* … */
    }
}
```

Rules that follow from the shape:

- Views hold **state and render only**. Business rules live in the model, I/O in services and stores.
- Services communicate upward with **events over channels**, never by holding UI handles.
- Every persisted type has exactly one `Repository` impl and one migration file that created its table.
- Every user-visible operation is an **action** (`actions!`), bound in the keymap, listed in the command palette. No click-only features.

## 4. Naming

- Crates: `ssherlock-<area>`. Modules: `snake_case`, singular for a type's module (`host.rs` holds `Host`), plural for a collection feature (`hosts.rs` holds the list view).
- Types `UpperCamelCase`, functions/vars `snake_case`, consts `SCREAMING_SNAKE_CASE`.
- Ids: `Id<Host>`, never raw `Uuid` or `String` in signatures.
- Events: `<Thing>Event` enums. Errors: `<Area>Error` enums. Configs: `<Thing>Config` structs with a builder when >4 fields.
- Async functions do not carry an `_async` suffix; blocking ones that must exist are suffixed `_blocking`.
- Booleans in struct fields read as predicates: `is_favourite`, `auto_start`. Prefer a two-variant enum over a `bool` parameter in a function signature.
- gpui actions are imperative verbs: `NewHost`, `ConnectSelected`, `CloseTab`.
- Terminology is fixed: **Host**, **Group**, **Identity** (username + credential), **Key**, **Snippet**, **Rule** (forwarding), **Vault**, **Session** (live connection), **Tab**/**Pane**. Don't introduce synonyms.

## 5. Errors

- Library crates: one `thiserror` enum per crate (`SshError`, `StoreError`, …), variants carry context, `#[from]` for wrapped errors.
- `ssherlock-core` defines nothing dependent on other crates' errors.
- The binary and UI crate use `anyhow::Result` at the top and convert to user-facing messages in one place (`ssherlock-ui::notify::error`).
- Error messages are full sentences without trailing periods, lower-case start, name the thing that failed: `"could not open known_hosts at {path}"`.
- Never swallow an error silently. Either propagate it, show it, or `tracing::warn!` it with the reason it is safe to ignore.

## 6. Async & concurrency

- One Tokio multi-thread runtime, created in `main`, exposed as a gpui `Global` (`TokioHandle`). Services return futures; the UI drives them with `cx.spawn` / `cx.background_spawn` or submits them to Tokio with `handle.spawn`.
- Cross-executor communication uses `tokio::sync::{mpsc, oneshot, watch}` — they are executor-agnostic. Never share `Rc`/`RefCell` across threads; in UI code let gpui `Entity`s own shared state.
- Cancellation: every long-lived task owns a `CancellationToken` or is dropped via its `JoinHandle` abort. No orphan tasks after a tab closes.
- Locks: `tokio::sync::Mutex` only around `.await`; `std::sync::Mutex` otherwise; hold neither across a UI render.
- Backpressure: bounded channels with explicit capacity; document the choice at the declaration.

## 7. gpui conventions

- One `Entity<T>` per stateful view; stateless pieces are plain functions returning `impl IntoElement` or `#[derive(IntoElement)]` components in `ssherlock-ui/src/components/`.
- Colours, spacing, radii and fonts come from `theme::Theme` tokens via `cx.theme()`. **No literal colours or pixel values in views**; add a token if one is missing.
- Register actions in each feature's `init(cx)`; bind keys only in `ssherlock-ui/src/keymap/default.json` with `key_context` matching the view.
- Subscriptions and observers are stored in `_subscriptions: Vec<Subscription>` on the entity so they live as long as it does.
- Call `cx.notify()` exactly when state that affects render changed; batch updates, don't notify in loops.
- Platform differences (Cmd vs Ctrl, Option-as-Meta) are resolved in `ssherlock-ui/src/platform.rs`, nowhere else.
- Components are built so a new feature can be assembled from `components::{Button, Input, List, Form, Modal, TabBar, Popover, Toast}` without new widgets. If you need a new widget, add it to `components/` first, generically.

## 8. Data & persistence

- SQLite via `rusqlite` with `bundled`; `PRAGMA journal_mode=WAL; foreign_keys=ON`.
- Migrations are numbered SQL files `crates/ssherlock-store/migrations/NNNN_description.sql`, applied in order, recorded in `schema_migrations`. Never edit a shipped migration; add a new one.
- Ids are UUID v7 stored as `TEXT`. Timestamps are RFC 3339 UTC `TEXT`.
- Secrets are stored only as ciphertext columns (`BLOB`) produced by `ssherlock-store::vault`. The vault key lives in the OS keychain via `keyring`.
- `serde` derives on all models; JSON is the import/export and settings format; `#[serde(default)]` on every new optional field so old files still load.

## 9. Security

- `secrecy` for secrets, `zeroize` on drop for any buffer that held key material.
- Crypto only from RustCrypto crates (`chacha20poly1305`, `argon2`, `sha2`, `hkdf`) or `russh::keys`. No home-made primitives, no `rand::thread_rng` for keys — use `OsRng`.
- Host key verification is never bypassed by code; only by the user through the dialog, and the decision is recorded.
- Any outbound HTTP (cloud import, AI, sync) goes through `ssherlock-core::net::Client` so proxies, TLS settings and redaction are applied once.
- Before adding telemetry or AI context, the payload passes `redact::scrub()` and has a test proving a seeded secret does not survive.

## 10. Testing

- Unit tests next to the code in `#[cfg(test)] mod tests`. Pure functions (validation, parsers, inheritance resolution, variable substitution) must have table-driven tests.
- Store tests run against an in-memory SQLite and apply all migrations — a test asserts migrations apply cleanly from empty and from each prior version's fixture.
- `ssherlock-ssh` integration tests live in `crates/ssherlock-ssh/tests/` and target a dockerised OpenSSH server (`tests/docker/compose.yml`); skipped with a clear message when Docker is absent, always run in CI.
- UI logic that matters (selection, inheritance, keymap resolution) is in plain functions tested without gpui. gpui views get smoke tests via `gpui::TestAppContext` where practical.
- A test that needs `unwrap()`/`expect()` may use it. Nothing else may.
- Bug fix = regression test in the same PR.

## 11. Documentation

- `//!` module docs explaining what the module is for and which slice layer it is.
- `///` on every `pub` item — one sentence of *what*, then *why* if non-obvious. `missing_docs` is a warning we keep at zero.
- Comments explain intent and invariants, not what the code already says.
- Decisions with trade-offs get a 10–20 line ADR in `docs/adr/NNNN-title.md`: context, decision, consequences.
- Keep `docs/FEATURES.md` boxes and `docs/PLAN.md` phase status current in the same PR that ships the work.

## 12. Lints & tooling

Workspace `Cargo.toml`:

```toml
[workspace.lints.rust]
unsafe_code = "forbid"
missing_docs = "warn"
unused_must_use = "deny"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
todo = "warn"
dbg_macro = "deny"
print_stdout = "deny"       # use tracing
print_stderr = "deny"
indexing_slicing = "warn"
# pedantic lints we turn off because they fight gpui builder chains or add noise
module_name_repetitions = "allow"
must_use_candidate = "allow"
too_many_lines = "allow"
```

Every crate root: `#![forbid(unsafe_code)]`. Tests may `#![allow(clippy::unwrap_used, clippy::expect_used)]` at the test module.

Before every commit (`just check` runs all of these):

```sh
cargo +nightly fmt --all -- --check   # nightly: rustfmt.toml uses nightly-only import options
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

Formatting is default `rustfmt` with `rustfmt.toml`: `edition = "2024"`, `imports_granularity = "Module"`, `group_imports = "StdExternalCrate"`.

Logging: `tracing` macros only, with structured fields (`info!(host = %host.label, "connected")`), never `println!`. Spans around every connection and transfer.

## 13. Git & PRs

- Conventional commits: `feat(hosts): HOST-04 instant search`, `fix(ssh): reconnect after sleep`, `refactor(ui): …`, `docs: …`, `test: …`, `chore: …`. Scope = feature area; include the feature ID when one applies.
- One feature or one slice layer per PR. Small PRs beat big ones.
- PR description: what, why, how tested, which `FEATURES.md` boxes it ticks.
- CI must be green; no `#[allow]` added to silence a lint without a comment saying why.
- Never commit secrets, real hostnames or test keys that were ever used against a real server. Test fixtures are generated keys in `tests/fixtures/` with a `README` saying so.

## 14. Approved crates

| Purpose | Crate |
|---------|-------|
| UI | `gpui` (pinned) |
| SSH, keys | `russh`, `russh-sftp`, `ssh-key` (via `russh::keys`) |
| Terminal | `alacritty_terminal`, `portable-pty` |
| Async | `tokio`, `futures`, `tokio-util` (CancellationToken) |
| Storage | `rusqlite` (bundled), `serde`, `serde_json` |
| Errors | `thiserror` (libs), `anyhow` (bin/ui) |
| Logging | `tracing`, `tracing-subscriber`, `tracing-appender` |
| Secrets, crypto | `secrecy`, `zeroize`, `chacha20poly1305`, `argon2`, `sha2`, `hkdf`, `rand` (OsRng), `keyring` |
| Ids, time | `uuid` (v7), `jiff` |
| Paths, config | `directories`, `notify` (file watching) |
| HTTP | `reqwest` (rustls) |
| Serial | `serialport` |
| Testing | `pretty_assertions`, `insta` (snapshots), `proptest` (parsers) |
| Version pins only (not used in code) | `libc` (see workspace `Cargo.toml` for why) |

Anything not in this table needs a reason in the PR and an entry added here.

## 15. When in doubt

- Prefer `std` over a crate, a crate over writing it yourself, and writing it yourself over `unsafe`.
- Prefer borrowing to cloning; prefer cloning to `Rc<RefCell<_>>`; prefer an `Entity` to either in UI code.
- Prefer an `enum` with data to a struct of `Option`s that are only valid in combinations.
- Prefer a newtype to a type alias when the value has meaning (`Port(u16)`, `Id<T>`).
- Prefer `impl Trait` in argument position for `Into<String>`/`AsRef<Path>`, concrete types in return position unless returning an iterator or element.
- Prefer small functions that return early over nested `if let`.
- Prefer making invalid states unrepresentable over validating them later.
- If two features would do the same thing slightly differently, make them identical and extract the shared part into `components/` or `core`.
- If you can't explain a design in the ADR format in under 20 lines, it's too clever.
