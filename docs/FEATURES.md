# SSHerlock — Feature Checklist

Everything Termius does — including everything behind its Pro and Team paywalls — plus a few things it doesn't.
**SSHerlock has no tiers. Every item below ships free, to everyone, with no account required.**
Tick items as they ship. Each item has an ID so `PLAN.md` can refer to it.

**Legend**

- `[ ]` not started · `[~]` in progress · `[x]` done
- **(Free)** / **(Pro)** / **(Team)** — what *Termius* charges for this feature (approximate). Kept only as a reference for parity; it has no effect on SSHerlock, where all of it is free.
- **(+)** — not in Termius; something we add because it's cheap or obviously useful

**Definition of done for any item:** implemented following `CLAUDE.md`, has tests, keyboard-reachable,
documented in a doc comment, no secrets logged, box ticked here.

---

## 1. Connections (CONN)

- [ ] **CONN-01** SSH over TCP, IPv4 and IPv6 (Free)
- [ ] **CONN-02** Password authentication (Free)
- [ ] **CONN-03** Public-key authentication: Ed25519, ECDSA, RSA (Free)
- [ ] **CONN-04** Keyboard-interactive auth (2FA prompts, PAM) (Free)
- [ ] **CONN-05** OpenSSH certificate authentication (Pro)
- [ ] **CONN-06** FIDO2 / hardware security keys (`sk-ssh-ed25519`, `sk-ecdsa`) (Pro)
- [ ] **CONN-07** Authenticate via system `ssh-agent` (Free)
- [ ] **CONN-08** Host key verification: trust-on-first-use, OpenSSH-compatible `known_hosts`, fingerprint display (SHA-256 + randomart) (Free)
- [ ] **CONN-09** Host key changed → hard warning with diff and explicit accept ("Moriarty alert") (Free)
- [ ] **CONN-10** Keep-alive interval, connection timeout, auto-reconnect with backoff (Free)
- [ ] **CONN-11** Host chaining / jump hosts (ProxyJump), multiple hops (Pro)
- [ ] **CONN-12** HTTP CONNECT proxy (Pro)
- [ ] **CONN-13** SOCKS4 / SOCKS5 proxy (Pro)
- [ ] **CONN-14** Agent forwarding (Pro)
- [ ] **CONN-15** Environment variables per host (Pro)
- [ ] **CONN-16** Startup snippet / command run on connect (Pro)
- [ ] **CONN-17** Per-host cipher, KEX, MAC and host-key algorithm preferences (Free)
- [ ] **CONN-18** Compression toggle (Free)
- [ ] **CONN-19** Mosh (Free)
- [ ] **CONN-20** Telnet (Free)
- [ ] **CONN-21** Serial / COM port connections (Free, desktop)
- [ ] **CONN-22** Local terminal (user's default shell) (Free)
- [ ] **CONN-23** Quick connect bar: `user@host:port`, `ssh://` URLs, fuzzy match saved hosts (Free)
- [ ] **CONN-24** `ssh://` URL scheme handler at OS level (Free)
- [ ] **CONN-25** X11 forwarding (+)
- [ ] **CONN-26** Run a single remote command without opening a shell (+)
- [ ] **CONN-27** Connection status indicator: connected / reconnecting / latency (Free)

## 2. Hosts & organisation (HOST)

- [ ] **HOST-01** Host record: label, address, port, username, credential or identity, group, tags, notes, colour (Free)
- [ ] **HOST-02** Groups, nested; children inherit settings (credentials, env, terminal settings) with per-host override (Free)
- [ ] **HOST-03** Tags, filter by tag (Free)
- [ ] **HOST-04** Instant search over label, address, tags, notes (Free)
- [ ] **HOST-05** Sort by name / last used / created; list and grid views (Free)
- [ ] **HOST-06** Favourites / pinned hosts (Free)
- [ ] **HOST-07** Recent connections (Free)
- [ ] **HOST-08** Duplicate host, bulk edit, bulk move to group (Free)
- [ ] **HOST-09** Trash with restore and permanent delete (Pro)
- [ ] **HOST-10** Per-host terminal overrides: theme, font, charset, cursor (Pro)
- [ ] **HOST-11** Per-host saved port-forwarding rules (see FWD) (Pro)
- [ ] **HOST-12** Host "Case File" detail panel: notes, last connected, key fingerprint, open sessions (Free)
- [ ] **HOST-13** Multi-select → connect to all (Free)
- [ ] **HOST-14** Keyboard-only navigation and editing of the whole host list (Free)
- [ ] **HOST-15** Detect OS / distro on connect and show icon (Free)
- [ ] **HOST-16** Host "address book" with multiple addresses per host (LAN/WAN) (+)

## 3. Terminal (TERM)

- [ ] **TERM-01** Full VT/xterm emulation: `xterm-256color`, truecolor, alt screen, scrolling regions, wide chars, combining chars (Free)
- [ ] **TERM-02** Mouse reporting (X10, SGR, URXVT), bracketed paste, focus events (Free)
- [ ] **TERM-03** OSC 8 hyperlinks, auto-detected URLs, click to open (Free)
- [ ] **TERM-04** OSC 52 clipboard (opt-in, with prompt) (Free)
- [ ] **TERM-05** Tabs: open, close, reorder, rename, pin, colour by host (Free)
- [ ] **TERM-06** Split panes: horizontal/vertical, resize, focus navigation, zoom pane (Free)
- [ ] **TERM-07** Multiple windows; detach tab to a window (Free)
- [ ] **TERM-08** Configurable scrollback, scrollback search with highlights (Free)
- [ ] **TERM-09** Selection: character, word, line, block (alt-drag); copy on select; smart word boundaries (Free)
- [ ] **TERM-10** Paste options: right-click paste, middle-click, confirm multi-line paste, strip trailing newline (Free)
- [ ] **TERM-11** Fonts: family, size, ligatures, line height, fallback fonts, zoom (Free)
- [ ] **TERM-12** Cursor: block/bar/underline, blink, colour (Free)
- [ ] **TERM-13** Bell: audible, visual, none; notify on bell in background tab (Free)
- [ ] **TERM-14** Themes: built-in set, custom editor, import iTerm2 / Windows Terminal / base16 (Pro)
- [ ] **TERM-15** Autocomplete: commands, paths and flags, learned from history and remote filesystem (Pro)
- [ ] **TERM-16** Command history per host, searchable, saveable as snippet (Pro)
- [ ] **TERM-17** Session logging to file, with timestamp option (Free)
- [ ] **TERM-18** Broadcast input to several terminals at once (Free)
- [ ] **TERM-19** Text reflow on resize (Free)
- [ ] **TERM-20** Charset / encoding selection (Free)
- [ ] **TERM-21** Keyboard: fully configurable shortcuts, Option-as-Meta on macOS, Ctrl/Cmd handling per platform (Free)
- [ ] **TERM-22** Terminal status bar: host, latency, cwd (via shell integration), encoding (Free)
- [ ] **TERM-23** Shell integration (OSC 133): prompt marks, jump between prompts, cwd tracking (+)
- [ ] **TERM-24** Image protocols: Sixel, Kitty graphics (+)
- [ ] **TERM-25** Clear scrollback, reset terminal (Free)
- [ ] **TERM-26** Find-and-copy of common patterns: IPs, paths, hashes (+)
- [ ] **TERM-27** Scroll to bottom on input, scroll indicator (Free)
- [ ] **TERM-28** Performance: 60 fps under `yes`/`cat` of large files; no UI stall on paste of 1 MB (Free)

## 4. Keychain (KEY)

- [ ] **KEY-01** Store SSH private keys (encrypted at rest) (Free)
- [ ] **KEY-02** Store passwords (Free)
- [ ] **KEY-03** Identities: username + key/password reusable across hosts, change once → applies everywhere (Free)
- [ ] **KEY-04** Generate keys: Ed25519, ECDSA P-256/384/521, RSA 2048–4096, with passphrase (Free)
- [ ] **KEY-05** Import keys from file, clipboard, `~/.ssh` scan (Free)
- [ ] **KEY-06** Export keys: OpenSSH, PEM/PKCS#8, public key only (Free)
- [ ] **KEY-07** Export public key to host (`ssh-copy-id` equivalent) (Free)
- [ ] **KEY-08** OpenSSH certificates stored alongside keys (Pro)
- [ ] **KEY-09** FIDO2 key enrolment and use (Pro)
- [ ] **KEY-10** Built-in SSH agent (serve keys to other apps) (Pro)
- [ ] **KEY-11** Passphrase caching with timeout (Free)
- [ ] **KEY-12** Biometric unlock (Touch ID / Windows Hello) and OS keychain integration (Pro)
- [ ] **KEY-13** Show fingerprint (SHA-256, MD5) and randomart (Free)
- [ ] **KEY-14** Which hosts use this key / identity (Free)
- [ ] **KEY-15** Key rotation helper: generate new, push to all hosts using old, retire old (+)

## 5. Port forwarding (FWD)

- [ ] **FWD-01** Local forwarding (`-L`) (Free)
- [ ] **FWD-02** Remote forwarding (`-R`) (Free)
- [ ] **FWD-03** Dynamic SOCKS5 proxy (`-D`) (Free)
- [ ] **FWD-04** Saved rules per host, named, enable/disable (Pro)
- [ ] **FWD-05** Auto-start rules on connect (Pro)
- [ ] **FWD-06** Forwarding-only sessions (no terminal) (Free)
- [ ] **FWD-07** Status, byte counters, active connections, restart on reconnect (Free)
- [ ] **FWD-08** Bind address options (localhost / all interfaces) (Free)
- [ ] **FWD-09** Open forwarded port in browser button (+)

## 6. Snippets (SNIP)

- [ ] **SNIP-01** Snippets: name, script, description, tags (Free)
- [ ] **SNIP-02** Packages / folders of snippets (Pro)
- [ ] **SNIP-03** Variables `{{name}}` prompted at run time, defaults, secret variables masked (Pro)
- [ ] **SNIP-04** Run in current terminal (Free)
- [ ] **SNIP-05** Run on selected hosts / group, parallel, per-host output and exit status (Pro)
- [ ] **SNIP-06** Startup snippet per host (Pro)
- [ ] **SNIP-07** Snippet hotkeys and command-palette access (Free)
- [ ] **SNIP-08** Save selection or last command as snippet (Pro)
- [ ] **SNIP-09** Shared team snippets (Team)
- [ ] **SNIP-10** Snippet run history (+)

## 7. SFTP & file transfer (SFTP)

- [ ] **SFTP-01** Remote file browser: list, sort, hidden files, permissions, sizes, dates (Pro)
- [ ] **SFTP-02** Dual pane: local ↔ remote, and remote ↔ remote (Pro)
- [ ] **SFTP-03** Upload / download with queue, progress, speed, pause/resume/retry/cancel (Pro)
- [ ] **SFTP-04** Drag and drop in both directions, including from OS file manager (Pro)
- [ ] **SFTP-05** Rename, delete, new folder, chmod, follow symlinks (Pro)
- [ ] **SFTP-06** Edit remote file in built-in or external editor, auto-upload on save (Pro)
- [ ] **SFTP-07** Preview text and images (Pro)
- [ ] **SFTP-08** Resume partial transfers (Pro)
- [ ] **SFTP-09** Open SFTP from any terminal tab, follows terminal cwd (via TERM-23) (Pro)
- [ ] **SFTP-10** SCP fallback for servers without SFTP (+)
- [ ] **SFTP-11** Folder sync (one-way mirror) (+)
- [ ] **SFTP-12** Bookmarks for remote paths (+)

## 8. Sync, vaults & security (SEC)

- [ ] **SEC-01** Local encrypted storage at rest: master key in OS keychain, data encrypted with ChaCha20-Poly1305 (Free)
- [ ] **SEC-02** Optional master password with Argon2id; auto-lock after idle (Pro)
- [ ] **SEC-03** Biometric unlock (Pro)
- [ ] **SEC-04** End-to-end encrypted sync across devices, zero-knowledge server (Pro)
- [ ] **SEC-05** Conflict resolution for sync (last-writer-wins with history) (Pro)
- [ ] **SEC-06** Self-hostable sync server (+)
- [ ] **SEC-07** Multiple vaults (personal, work) (Team)
- [ ] **SEC-08** Encrypted backup export / import (Pro)
- [ ] **SEC-09** Local-only mode: works fully without any account (Free)
- [ ] **SEC-10** Account 2FA (Pro)
- [ ] **SEC-11** Secrets never in logs, crash reports, or `Debug` output (Free)
- [ ] **SEC-12** Clipboard clear after copying a secret (+)

## 9. Import / export (IMP)

- [ ] **IMP-01** Import `~/.ssh/config` incl. `Include`, `ProxyJump`, `IdentityFile`, `Host` patterns (Free)
- [ ] **IMP-02** Import `~/.ssh/known_hosts` (Free)
- [ ] **IMP-03** Import from PuTTY (Windows registry / sessions) (Pro)
- [ ] **IMP-04** Import from Termius export (Pro)
- [ ] **IMP-05** Import from SecureCRT, MobaXterm, iTerm2 profiles (Pro)
- [ ] **IMP-06** Cloud import: AWS EC2 (Pro)
- [ ] **IMP-07** Cloud import: DigitalOcean (Pro)
- [ ] **IMP-08** Cloud import: Google Cloud, Azure (Pro)
- [ ] **IMP-09** Cloud import: Linode, Vultr, Hetzner (Pro)
- [ ] **IMP-10** Export hosts to `ssh_config` and JSON (Free)
- [ ] **IMP-11** Re-sync cloud import on demand, tag by provider / region (+)

## 10. Team (TEAM)

- [ ] **TEAM-01** Shared vaults with members (Team)
- [ ] **TEAM-02** Roles: owner, admin, member; per-vault permissions (Team)
- [ ] **TEAM-03** Share a host without revealing the credential (Team)
- [ ] **TEAM-04** Audit log: who connected where and when (Team)
- [ ] **TEAM-05** SSO: SAML / OIDC (Team)
- [ ] **TEAM-06** SCIM user provisioning (Team)
- [ ] **TEAM-07** Policies: enforce 2FA, forbid password auth, session recording (Team)
- [ ] **TEAM-08** Invite flow, remove member → rotate shared secrets (Team)

## 11. AI assistant — "Watson" (AI)

- [ ] **AI-01** Natural language → shell command, shown with explanation, never auto-run (Pro)
- [ ] **AI-02** Explain the last error output (Pro)
- [ ] **AI-03** Host-aware context: OS, shell, cwd (Pro)
- [ ] **AI-04** Bring-your-own-key provider config (Claude API first), off by default (Pro)
- [ ] **AI-05** Redact secrets and hostnames before sending context (Free)
- [ ] **AI-06** Local-only mode: feature hidden when no provider configured (Free)

## 12. App & platform (APP)

- [~] **APP-01** macOS, Linux and Windows builds from one gpui codebase (Free)
- [ ] **APP-02** Settings UI plus `settings.json` with hot reload (Free)
- [ ] **APP-03** Command palette for every action (Free)
- [ ] **APP-04** Customisable keymap, keymap file (Free)
- [~] **APP-05** Light / dark / system appearance, app themes (Free)
- [ ] **APP-06** Restore windows, tabs and sessions on launch (Pro)
- [ ] **APP-07** Desktop notifications: disconnect, transfer finished, bell in background (Free)
- [ ] **APP-08** Auto-update with signed releases (Free)
- [ ] **APP-09** Opt-in crash reporting and telemetry; off by default (Free)
- [ ] **APP-10** Localisation framework, English first (Free)
- [ ] **APP-11** Accessibility: high contrast, scalable UI, screen-reader labels where gpui allows (Free)
- [ ] **APP-12** CLI: `ssherlock connect <host>`, `ssherlock import`, `ssherlock --new-window` (+)
- [ ] **APP-13** Packaging: `.dmg` + Homebrew cask; `.deb`, `.rpm`, AppImage, Flatpak; `.msi` + winget (Free)
- [ ] **APP-14** Onboarding / first-run: import existing config, create first host (Free)
- [ ] **APP-15** In-app changelog and "what's new" (Free)
- [ ] **APP-16** Dashboard home: recent hosts, active forwards, transfers (Free)

---

## Progress

| Area | Done | Total |
|------|------|-------|
| CONN | 0 | 27 |
| HOST | 0 | 16 |
| TERM | 0 | 28 |
| KEY  | 0 | 15 |
| FWD  | 0 | 9  |
| SNIP | 0 | 10 |
| SFTP | 0 | 12 |
| SEC  | 0 | 12 |
| IMP  | 0 | 11 |
| TEAM | 0 | 8  |
| AI   | 0 | 6  |
| APP  | 0 | 16 |
| **Total** | **0** | **170** |

Update this table when ticking boxes.
