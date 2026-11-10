# SSHerlock — Implementation Plan

Phase-by-phase plan to build everything in `FEATURES.md`. Each phase ends with something
you can run. Feature IDs (`CONN-01` …) point into the checklist. Sizes are relative: S < M < L < XL.

Decisions already made:

- **Pricing:** none. Every feature, including everything Termius sells as Pro or Team, is free. No tiers, no licence checks, no account requirement, no feature flags that gate functionality by plan.
- **Language:** Rust, edition 2024, stable toolchain, zero `unsafe` in our crates.
- **UI:** [gpui](https://crates.io/crates/gpui) `0.2.x` for everything on screen. No web view, no other toolkit.
- **SSH:** [russh](https://crates.io/crates/russh) `0.65.x` (keys via `russh::keys`), [russh-sftp](https://crates.io/crates/russh-sftp) `3.x`.
- **Terminal emulation:** [alacritty_terminal](https://crates.io/crates/alacritty_terminal) `0.26.x` for the VT state machine and grid (same choice Zed made), [portable-pty](https://crates.io/crates/portable-pty) `0.9.x` for local shells.
- **Storage:** SQLite via [rusqlite](https://crates.io/crates/rusqlite) `0.41.x` (bundled), secrets encrypted before they hit the database.
- **Async:** one Tokio runtime for all network I/O; gpui's executor for UI. The two talk through `tokio::sync` channels, which work on any executor.

---

## Architecture

```
┌──────────────────────────── gpui foreground thread ────────────────────────────┐
│  ssherlock (binary)   wires it all, owns the Tokio runtime handle               │
│  ssherlock-ui         windows, views, components, theme, keymap, actions        │
│       │  calls async fns, receives events over channels, updates Entities       │
└───────┼─────────────────────────────────────────────────────────────────────────┘
        ▼
┌──────────────────────────── pure Rust, no gpui ────────────────────────────────┐
│  ssherlock-core      domain types: Host, Group, Identity, Snippet, Rule, ids    │
│  ssherlock-store     SQLite persistence, migrations, vault encryption           │
│  ssherlock-terminal  VT grid (alacritty_terminal), pty, terminal events         │
│  ssherlock-ssh       russh sessions, auth, channels, forwarding, sftp           │
│  ssherlock-import    parsers for ssh_config, PuTTY, Termius, cloud providers    │
│  ssherlock-sync      E2E sync protocol + client            (Phase 11)           │
│  ssherlock-ai        provider abstraction, prompt building  (Phase 13)           │
└─────────────────────────────────────────────────────────────────────────────────┘
```

Rules that keep this shape (details in `CLAUDE.md`):

1. Only `ssherlock` and `ssherlock-ui` depend on `gpui`. Every other crate compiles headless and is testable with `cargo test` alone.
2. `ssherlock-core` depends on no other workspace crate. Everything depends on it.
3. Network and disk I/O run on Tokio or gpui's background executor — never on the foreground thread.
4. Every feature is built as the same **feature slice**: `model → store → service → view`, so once you've read one feature you can read them all.

Workspace layout after Phase 0:

```
SSHerlock/
├── Cargo.toml                # [workspace], shared deps, shared lints
├── CLAUDE.md                 # rules for humans and Claude
├── docs/FEATURES.md
├── docs/PLAN.md
├── docs/adr/                 # one short file per architecture decision
└── crates/
    ├── ssherlock/            # bin
    ├── ssherlock-ui/
    ├── ssherlock-core/
    ├── ssherlock-store/
    ├── ssherlock-terminal/
    └── ssherlock-ssh/
```

Milestones:

- **M1 — Daily driver (Phases 0–5):** connect, hosts, keys, forwarding. Covers Termius' free tier.
- **M2 — Full parity (Phases 6–10):** snippets, SFTP, chaining/proxies, terminal polish, import — everything Termius charges Pro for, free.
- **M3 — Sync & Team (Phases 11–12):** everything Termius charges Team for, free.
- **M4 — Beyond (Phases 13–14):** AI, release engineering, the `(+)` items.

---

## Phase 0 — Foundations (S)

**Goal:** an empty gpui window opens on macOS and Linux (X11 and Wayland), the workspace compiles with strict lints, CI is green.

Work:

- Convert to a Cargo workspace with the six crates above; `ssherlock` is the only binary.
- `[workspace.lints]` with `unsafe_code = "forbid"`, clippy pedantic subset, `unwrap_used = "deny"` (see `CLAUDE.md`).
- `[workspace.dependencies]` pinning `gpui = "=0.2.2"` (its API moves; pin exactly) and the other crates.
- `ssherlock-core`: `Id<T>` newtype (UUID v7), `Error` conventions, `SecretString` re-export.
- Logging with `tracing` + `tracing-subscriber`, log file in the platform data dir (`directories`).
- Tokio runtime created in `main`, handle stored as a gpui `Global`.
- Hello window: title bar, one `div` saying "SSHerlock", theme tokens plumbed (no hard-coded colours even here).
- `justfile` or `cargo xtask`: `fmt`, `lint`, `test`, `run`.
- GitHub Actions: fmt + clippy + test on macOS and Linux (Linux gpui needs `libxkbcommon`, Wayland/X11 headers, Vulkan loader — listed in the README's *Development setup*; CI installs the same packages).
- `docs/adr/0001-gpui.md`, `0002-russh.md`, `0003-sqlite.md` — three paragraphs each.

Exit: `cargo run` opens the window on macOS and on Linux; `just lint` passes with zero warnings; CI green.

Features: APP-01 (started), APP-05 (tokens only).

---

## Phase 1 — Terminal core (L)

**Goal:** a local shell in a gpui tab that feels right. Everything SSH later renders through this.

Work:

- `ssherlock-terminal`: wrap `alacritty_terminal::Term` behind a `Terminal` struct; input bytes in, `TerminalEvent`s out (title, bell, clipboard request, resize). Pty via `portable-pty`.
- Reader task per terminal on Tokio; damage-tracked grid snapshot handed to UI.
- `ssherlock-ui::terminal::TerminalView`: custom `Element` that paints cells with gpui text runs, cursor, selection, scrollbar. Key → bytes mapping (platform-aware, Option-as-Meta).
- Tabs (`TabBar`, `Pane`) with open/close/reorder/rename.
- Selection (char/word/line/block), copy, paste with multi-line confirm, URL detection and click.
- Scrollback with search.
- Theme: 16 ANSI + fg/bg/cursor/selection as tokens; two built-in themes.
- Bench: `cat` a 100 MB file, paste 1 MB; profile with `tracing` spans.

Exit: open 10 local tabs, run `htop`, `vim`, `tmux`; all render correctly; selection/copy/paste/search work.

Features: CONN-22, TERM-01, 02, 03, 05, 08, 09, 10, 11, 12, 13, 19, 25, 27, 28.

---

## Phase 2 — SSH core (L)

**Goal:** connect to a real server from a typed address, with host key checks and all common auth methods.

Work:

- `ssherlock-ssh::Session`: connect, `Handler` for host key verification (callback to UI for TOFU / changed-key dialog), open PTY channel, window resize, exit status.
- Auth strategies as an enum tried in order: agent → key(s) → password → keyboard-interactive. Each is its own small type with the same `authenticate(&mut self, …)` shape.
- `KnownHosts` store: read/write OpenSSH format, hashed hosts supported.
- Keep-alive, timeout, reconnect with backoff; `ConnectionState` enum emitted to UI.
- Quick connect bar parsing `user@host:port` and `ssh://`.
- Terminal tab bound to an SSH channel instead of a pty (same `Terminal` type — the byte source is a trait).
- Integration tests against a dockerised OpenSSH server (`tests/docker/`), run in CI.

Exit: connect by password, key and agent to a test server; wrong fingerprint is refused; dropping the network reconnects.

Features: CONN-01…04, 07, 08, 09, 10, 17, 18, 23, 27.

---

## Phase 3 — Hosts, groups & storage (M)

**Goal:** saved hosts you can search, group and connect to with one key press.

Work:

- `ssherlock-store`: open DB in the platform data dir, migrations via numbered SQL files, `Repository<T>` trait with `get / list / upsert / delete` implemented per model — same shape for every table.
- Models in core: `Host`, `Group`, `Tag`; inheritance resolved by a pure function `resolve(host, ancestors) -> EffectiveHost` (unit-tested).
- Host list view: sidebar with groups tree, search box, list/grid, favourites, recents, multi-select, keyboard navigation.
- Host editor form with the shared `Form` components (every later editor reuses them).
- Trash table and restore.
- Connect flow: select host → `EffectiveHost` → `ssherlock-ssh`.

Exit: create 50 hosts in nested groups, search returns in <5 ms, connect from keyboard only.

Features: HOST-01…09, 12, 13, 14, APP-16 (basic).

---

## Phase 4 — Keychain (M)

**Goal:** keys, passwords and identities stored safely and used by hosts.

Work:

- Vault: master key in the OS keychain (`keyring`), per-secret ChaCha20-Poly1305 with random nonce; `SecretString` everywhere; `Zeroize` on drop.
- Models: `Key`, `Password`, `Identity`, `Certificate`. Hosts reference these by `Id`.
- Key generation via `russh::keys` (Ed25519, ECDSA, RSA), passphrase support, fingerprint + randomart.
- Import from file/clipboard/`~/.ssh` scan; export OpenSSH / PEM / public.
- "Export public key to host" (append to `authorized_keys` over the open session).
- Passphrase cache with timeout.
- Keychain view: list, detail, "used by N hosts".

Exit: generate a key, push to a host, connect with it; restart app, secrets still work; grep logs for key material finds nothing.

Features: KEY-01…07, 11, 13, 14, SEC-01, SEC-11.

---

## Phase 5 — Port forwarding (M)

**Goal:** `-L`, `-R`, `-D` with saved rules and live status.

Work:

- `ssherlock-ssh::forward`: `LocalForward`, `RemoteForward`, `DynamicForward` (SOCKS5 server) — one trait `Forward` with `start / stop / stats`.
- Model `ForwardRule` saved per host, auto-start flag.
- Forwarding-only sessions (no terminal).
- UI: rules editor in host form, "Port Authority" panel listing active forwards with byte counters and restart on reconnect.

Exit: forward a local port to a remote DB and connect a client through it; SOCKS proxy works in a browser.

Features: FWD-01…08, HOST-11.

**M1 reached.** Tag `v0.1.0`.

---

## Phase 6 — Snippets (M)

Work:

- Models `Snippet`, `SnippetPackage`, variable syntax `{{name}}` / `{{name:default}}` / `{{secret:name}}` with a small parser (unit-tested).
- Run in current terminal; run on many hosts in parallel via `ssherlock-ssh::exec` (CONN-26) with per-host output panel and exit codes.
- Startup snippet per host (executes once the shell prompt is detected or after a delay).
- Palette and hotkeys; save selection as snippet.

Exit: run a snippet with two variables on five hosts and see five outputs.

Features: SNIP-01…08, CONN-16, CONN-26.

---

## Phase 7 — SFTP (L)

Work:

- `ssherlock-ssh::sftp` over `russh-sftp`: list, stat, read/write streams, rename, remove, mkdir, chmod, readlink.
- `TransferQueue` service: jobs with progress, pause/resume/retry/cancel, resume partial via offset.
- Dual-pane browser view: local pane uses `tokio::fs`; remote pane uses SFTP; same `FilePane` component with a `FileSystem` trait behind it — so remote↔remote is free.
- Drag and drop (gpui external drag), context menu, inline rename, permissions editor.
- Edit remote file: download to temp, watch (`notify`), upload on save.
- Preview text/images.

Exit: transfer a 2 GB file with pause/resume; edit a remote config and see it uploaded.

Features: SFTP-01…09.

---

## Phase 8 — Advanced connections (M)

Work:

- Jump hosts: open a direct-tcpip channel through hop N to reach hop N+1; arbitrary chain.
- HTTP CONNECT and SOCKS4/5 proxies as `Transport` implementations; chaining and proxies share the same `Transport` trait used by Phase 2.
- Agent forwarding, environment variables, certificate auth, FIDO2 keys (`ssh-key` sk types; hardware interaction via `ctap-hid-fido2` or similar — spike first).
- Mosh (spawn `mosh-client`, bootstrap over our SSH session), Telnet, Serial (`serialport` crate, pure Rust).
- `ssh://` OS URL handler.

Exit: connect through two jump hosts and a SOCKS proxy; serial console on a USB device; mosh survives Wi-Fi toggle.

Features: CONN-05, 06, 11…15, 19, 20, 21, 24, KEY-08, 09.

---

## Phase 9 — Terminal polish (L)

Work:

- Split panes, multiple windows, detach tab to window.
- Shell integration (OSC 133) install snippet for bash/zsh/fish; prompt marks, cwd tracking.
- Autocomplete: history-backed first; path completion via `ls` probe or SFTP listing; UI popup component shared with palette.
- Command history per host, searchable.
- Session logging; broadcast input; theme editor and importers (iTerm2, Windows Terminal, base16); font fallback and ligatures; encodings; status bar; bell notifications; OSC 52 opt-in.
- Per-host terminal overrides (HOST-10).
- Image protocols (Sixel, Kitty) — spike; ship if `alacritty_terminal` path is reasonable, else defer.

Exit: four-pane split with broadcast; autocomplete suggests from history; theme import of an `.itermcolors` file.

Features: TERM-04, 06, 07, 14…18, 20…24, 26, HOST-10, HOST-15, APP-07.

---

## Phase 10 — Import / export (M)

Work:

- `ssherlock-import` crate: each source is a `Importer` trait impl returning `Vec<ImportedHost>` + `Vec<ImportedKey>`; UI shows a diff/merge preview before writing.
- `ssh_config` parser (with `Include`, `Match` basics, `ProxyJump`); `known_hosts`; PuTTY registry/sessions; Termius export; SecureCRT, MobaXterm, iTerm2 profiles.
- Cloud providers over their HTTPS APIs with `reqwest`: AWS, DigitalOcean, GCP, Azure, Linode, Vultr, Hetzner; credentials stored in the vault; re-sync tags by provider/region.
- Export to `ssh_config` and JSON.
- Built-in SSH agent (KEY-10) lands here since it's needed by "export to other tools" workflows.

Exit: import a 200-host `ssh_config` with includes; import 30 EC2 instances tagged by region.

Features: IMP-01…11, KEY-10, APP-14.

**M2 reached.** Tag `v0.5.0`.

---

## Phase 11 — Sync & security (XL)

Work:

- Master password (Argon2id) and auto-lock; biometric unlock through platform APIs behind a `Biometrics` trait (macOS LocalAuthentication via `objc2` bindings crate — bindings are a dependency, our code stays safe; Windows Hello; Linux polkit fallback = password).
- `ssherlock-sync`: client-side encrypted records (per-record keys wrapped by vault key), append-only change log, last-writer-wins with history, device enrolment via QR/code.
- Sync server: small Rust service (`axum`) storing opaque blobs; self-hostable; hosted instance optional.
- Multiple vaults; encrypted backup export/import; account 2FA (TOTP).
- Clipboard clear after copying secrets.

Exit: two machines converge after offline edits on both; server operator can read nothing useful.

Features: SEC-02…10, 12, APP-06.

---

## Phase 12 — Team (L)

Work:

- Shared vaults with member keys (vault key wrapped per member's public key); roles and permissions enforced server-side and mirrored client-side.
- Share host without revealing credential: credential lives in a shared vault the member can *use* but not *export* (client-enforced + server audit).
- Audit log events emitted by client, stored by server, viewable in app.
- SSO (OIDC first, SAML via server), SCIM, policies.

Exit: invite a member, they connect to a shared host, admin sees it in the audit log, member can't export the key.

Features: TEAM-01…08, SNIP-09.

**M3 reached.** Tag `v0.8.0`.

---

## Phase 13 — Watson, the AI assistant (M)

Work:

- `ssherlock-ai`: `Provider` trait; first impl Claude API (default model `claude-sonnet-5-5`, user-selectable), BYOK key stored in the vault; HTTP via `reqwest`.
- Redaction pass (hostnames, IPs, tokens, anything from the vault) before any request; visible "what will be sent" panel.
- Commands: "ask Watson" → NL to command with explanation, inserted into the prompt but never executed; "explain this error" on selected output.
- Context: host OS, shell, cwd from shell integration.
- Entire feature hidden when no provider configured.

Exit: ask for a command, see it explained, run it yourself; redaction tests pass.

Features: AI-01…06.

---

## Phase 14 — Release engineering & the rest (M)

Work:

- Signed, notarised `.dmg` + Homebrew cask; `.deb`/`.rpm`/AppImage/Flatpak; `.msi` + winget; GitHub Releases pipeline with `cargo-dist`.
- Auto-update (signed manifests), opt-in crash reporting, telemetry off by default.
- CLI entry points, settings hot reload, localisation (`fluent`), accessibility pass, onboarding, changelog view.
- X11 forwarding, SCP fallback, folder sync, key rotation helper, remote path bookmarks, forwarded-port-in-browser — the remaining `(+)` items as time allows.

Exit: a stranger installs from the website on each OS and connects within two minutes.

Features: CONN-25, SFTP-10…12, KEY-15, FWD-09, HOST-16, SNIP-10, APP-02, 03, 04, 08…13, 15.

**M4 reached.** Tag `v1.0.0`.

---

## How to work a phase

1. Read the phase, open `FEATURES.md`, mark its items `[~]`.
2. For each feature, build the slice in order: **model** (core) → **store** (if persisted) → **service** (ssh/terminal/etc.) → **view** (ui), tests at every step. Don't start the view before the service has tests.
3. Keep PRs to one feature or one slice layer. Title: `feat(hosts): HOST-04 instant search`.
4. When the exit criteria pass, tick the boxes, update the progress table, write the ADR if a real decision was made, tag if the phase ends a milestone.
5. If a phase reveals a wrong decision, change the plan here first, then the code.
