# SSHerlock

A Rust SSH client. Hosts, keys, port forwarding, snippets, SFTP, sync — everything a modern
SSH client should do, with no paid tier. Built with [gpui](https://crates.io/crates/gpui) and
[russh](https://crates.io/crates/russh).

> **Status: early development (Phase 5 done, milestone M1; Phase 6 next).** SSHerlock runs
> local terminals in tabs, keeps saved hosts in nested groups, connects to them with saved keys
> and passwords, and forwards ports over them. Snippets come in Phase 6. Follow
> [docs/PLAN.md](docs/PLAN.md) for the build order and [docs/FEATURES.md](docs/FEATURES.md) for
> the full checklist.

## What works today

- **Local terminals** in tabs: rename, reorder, search the scrollback, copy and paste, open
  links, zoom.
- **Saved hosts** on the **Hosts** tab, where SSHerlock opens, in front of the terminal tabs.
  Group them (groups nest,
  and a host inherits the port, user, colour, algorithms and compression of its groups unless it
  sets its own), tag them, star favourites, and search as you type. Sort by name, last use or
  creation, in a list or a grid. Select several to connect to all of them, edit them together or
  move them to a group (typing a new name in a group picker creates the group). Deleted hosts and groups go to the trash, where they can be restored.
  The whole page works from the keyboard: arrows, Shift to select, Enter to connect, N new
  host, E edit, F favourite, Delete to trash, / to search, Tab between the parts.
- **SSH via quick connect.** Type `user@host`, `user@host:port` or `ssh://user@host:port`, or part
  of a saved host's name: matching saved hosts are listed, and Enter connects to the first one.
  If you leave out the user, your local user name is used.
- **Tabs** opened from a saved host carry its colour. Pin a tab to keep it at the front; a
  pinned tab can't be closed until it's unpinned.
- **Keychain** (in the Hosts page's sidebar): generate Ed25519, ECDSA or RSA keys, import them
  from a file, the clipboard or `~/.ssh`, and export them as OpenSSH, PKCS#8 or the public key.
  Each key shows its SHA-256 and MD5 fingerprints, randomart, and which hosts use it.
  **Export to host** adds a key to a server's `authorized_keys`, like `ssh-copy-id`.
  **Identities** bundle a user name with a password and/or key. Hosts and groups pick an
  identity, a key and a saved password, and inherit them like any other setting.
- **Login:** the host's saved key first, then your SSH agent, then `~/.ssh/id_ed25519`,
  `id_ecdsa` and `id_rsa` (asking for a passphrase if a key needs one and remembering it for 15
  minutes), then the saved password, then asking for one, then keyboard-interactive (2FA, PAM).
- **Host keys:** a new host shows its SHA-256 fingerprint and randomart before you trust it. A
  changed key gets a red warning that Enter can't accept. Hosts already in
  `~/.ssh/known_hosts` are trusted without asking. SSHerlock reads that file but never writes
  to it; keys you trust are saved in SSHerlock's own `known_hosts`, in the data folder below.
- **Staying connected:** keep-alive every 15 seconds with the latency in the status bar, and
  automatic reconnection with backoff when the network drops or the machine sleeps.
- **Port forwarding:** each host keeps its own rules, edited under **Port forwarding** in the
  host's editor. There are three kinds:
  - **Local** (`ssh -L`): a port on this machine reaches a host and port the server can reach,
    such as a database behind it.
  - **Remote** (`ssh -R`): a port on the server reaches something on this machine.
  - **SOCKS proxy** (`ssh -D`): a SOCKS5 proxy on this machine whose connections leave from the
    server.

  Each rule listens on localhost only or on all interfaces. It can be disabled without losing
  its settings, and set to start whenever a tab to the host opens.

  The **Port forwarding → Rules** page in the Hosts sidebar lists every rule, grouped by host.
  It shows what each rule is doing, its traffic and open connections, and why it failed if it
  did. Rules started there run over a connection of their own, with no terminal, which closes
  once its last rule stops. Forwards wait out a dropped connection and start again when it's
  back. On that page, Enter or Space starts or stops the rule under the cursor, E opens its
  host's editor, and Shift+S stops everything.

| Shortcut | macOS | Linux |
|----------|-------|-------|
| Quick connect | Cmd+O | Ctrl+Shift+O |
| Hosts page | Cmd+Shift+H | Ctrl+Shift+H |
| Pin / unpin tab | Cmd+Alt+P | Ctrl+Shift+Alt+P |
| Add a key to this server's `authorized_keys` | Cmd+Alt+K | Ctrl+Shift+Alt+K |
| New tab / close tab | Cmd+T / Cmd+W | Ctrl+Shift+T / Ctrl+Shift+W |
| Next / previous tab | Ctrl+Tab / Ctrl+Shift+Tab | Ctrl+Tab / Ctrl+Shift+Tab |
| Rename tab | Cmd+R | Ctrl+Shift+R |
| Copy / paste | Cmd+C / Cmd+V | Ctrl+Shift+C / Ctrl+Shift+V |
| Find in scrollback | Cmd+F | Ctrl+Shift+F |
| Zoom in / out / reset | Cmd+= / Cmd+- / Cmd+0 | Ctrl+= / Ctrl+- / Ctrl+0 |

On Linux, middle-click pastes the primary selection into terminals and text fields, and
selecting text (or Ctrl+A in a field) puts it there, when the desktop has middle-click paste
turned on. GNOME ships it off; turn it on in Settings → Mouse & Touchpad (or with
`gsettings set org.gnome.desktop.interface gtk-enable-primary-paste true`). SSHerlock reads the
setting when it starts.

On Linux the shortcuts add Shift so plain Ctrl keys still reach the shell. All of them are in
[keymap/default.json](crates/ssherlock-ui/src/keymap/default.json). Right-click in a terminal
for copy, paste, find, clear scrollback and reset.

SSHerlock keeps its database of saved hosts, keys, identities and forwarding rules
(`ssherlock.db`), its `known_hosts` and its logs in its data folder:
`~/.local/share/ssherlock/` on Linux, `~/Library/Application Support/io.github.asadhshujau.SSHerlock/`
on macOS.

Saved keys and passwords are encrypted in the database with a key kept in the OS keychain
(Keychain on macOS, GNOME Keyring or KWallet on Linux), so copying the database alone reveals
nothing. Without a keychain, as in some minimal Linux sessions, that key is kept in `vault.key`
in the data folder, readable only by you, and SSHerlock says so the first time. See
[ADR 0008](docs/adr/0008-vault.md).

## Principles

- Every feature is free. No tiers, no licence checks, no account required.
- No `unsafe` code in this repository.
- Desktop-first: macOS, Linux and Windows from one codebase.

## Development setup

SSHerlock is developed on macOS and Linux. Do this once before building.

### 1. Rust toolchain

Install stable Rust with [rustup](https://rustup.rs). The minimum is **1.89**, which is what
`rusqlite` needs. You also need nightly **rustfmt** for formatting only: `rustfmt.toml` uses
import-grouping options that stable rustfmt ignores. Building and testing use stable.

```sh
rustup toolchain install stable --component rustfmt,clippy
rustup toolchain install nightly --profile minimal --component rustfmt
```

### 2. Cargo tools

`just` runs the project tasks and `cargo-deny` checks dependency licences and advisories.

```sh
cargo install cargo-binstall   # skip if you already have it
cargo binstall just cargo-deny
```

### 3. System libraries

gpui renders with Metal on macOS and Vulkan on Linux, so it needs native libraries at build time.

**macOS:** install the full **Xcode** from the App Store, not just the Command Line Tools,
because gpui compiles Metal shaders during the build. Then:

```sh
sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
xcodebuild -downloadComponent MetalToolchain   # Xcode 26 and later ship it separately
```

**Linux, Debian or Ubuntu:**

```sh
sudo apt install build-essential clang pkg-config \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev libxcb1-dev \
  libvulkan-dev mesa-vulkan-drivers libfontconfig-dev libfreetype-dev
```

**Linux, Fedora:**

```sh
sudo dnf install gcc gcc-c++ clang pkgconf-pkg-config \
  libxkbcommon-devel libxkbcommon-x11-devel wayland-devel libxcb-devel \
  vulkan-loader-devel mesa-vulkan-drivers fontconfig-devel freetype-devel
```

**Linux, Arch:**

```sh
sudo pacman -S base-devel clang pkgconf libxkbcommon libxkbcommon-x11 wayland libxcb \
  vulkan-icd-loader fontconfig freetype2
```

On Linux you also need a working Vulkan driver for your GPU. Mesa covers Intel and AMD; on NVIDIA,
use the proprietary driver. Run `vulkaninfo --summary` from `vulkan-tools` to check. Both X11 and
Wayland sessions are supported.

### 4. Docker

The SSH integration tests run against an OpenSSH server in Docker (`crates/ssherlock-ssh/tests/docker/`).
Install Docker Engine or Docker Desktop (macOS) or Podman with Docker compatibility. If Docker is
missing, those tests are skipped with a message. On Linux, add yourself to the `docker` group
(`sudo usermod -aG docker $USER`, then log in again) so the tests can reach the daemon without
`sudo`.

Each test starts its own container on a random localhost port and removes it afterwards. To try
the server by hand, `just ssh-test-server` starts one on `127.0.0.1:2222` (user `tester`,
password `tester-password`). `just ssh-test-clean` stops it and removes any containers a test
left behind. The keys in `crates/ssherlock-ssh/tests/fixtures/` were generated for these tests
only.

### 5. Build and check

```sh
just run     # build and launch the app
just check   # fmt, clippy, tests and cargo-deny; run this before every commit
```

Read [CLAUDE.md](CLAUDE.md) before writing code. It holds the engineering rules every change
follows.

### 6. Checking the UI headlessly (optional; for AI assistants and CI)

Tests can't tell whether the terminal *looks* right. If an AI coding assistant works on this
repo, give it a way to launch the app, drive it and look at a screenshot without your desktop.
On Linux that is a virtual X display. gpui renders into it with Mesa's software Vulkan driver
(lavapipe, from `mesa-vulkan-drivers` above), so no GPU is needed.

| Tool | Used for |
|------|----------|
| `xvfb` | a virtual X display to run the app in |
| `xdotool` | typing keys and clicking |
| `imagemagick` | `import` takes the screenshot |
| `xclip` | putting text on the clipboard, to test paste |

```sh
sudo apt install xvfb xdotool imagemagick xclip                     # Debian, Ubuntu
sudo dnf install xorg-x11-server-Xvfb xdotool ImageMagick xclip     # Fedora
sudo pacman -S xorg-server-xvfb xdotool imagemagick xclip           # Arch
```

Run it sandboxed, so nothing reaches your real session:

- `HOME`, `XDG_DATA_HOME` and `SHELL` keep the app's logs and its login shell away from your
  configuration.
- Unsetting `SSH_AUTH_SOCK` and `SSH_AGENT_PID` keeps SSH logins away from your real agent.
- `DBUS_SESSION_BUS_ADDRESS=disabled:` cuts the app off from your desktop's session bus.
  Otherwise "open this link" goes through the desktop portal, which opens it in your real
  browser.
- Stub `xdg-open`/`gio`/… scripts first on `PATH` catch the fallback openers. They log what
  would have been opened, so you can check it.

```sh
cargo build -p ssherlock
sandbox=$(mktemp -d)
mkdir "$sandbox/stubs"
for opener in xdg-open gio gnome-open kde-open wslview; do
  printf '#!/bin/sh\necho "$*" >> "%s/opened.log"\n' "$sandbox" > "$sandbox/stubs/$opener"
  chmod +x "$sandbox/stubs/$opener"
done
Xvfb :99 -screen 0 1280x800x24 & xvfb=$!
sleep 1   # let the display come up
env -u WAYLAND_DISPLAY -u SSH_AUTH_SOCK -u SSH_AGENT_PID \
  DISPLAY=:99 HOME="$sandbox" XDG_DATA_HOME="$sandbox/share" SHELL=/bin/sh \
  DBUS_SESSION_BUS_ADDRESS=disabled: PATH="$sandbox/stubs:$PATH" \
  VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json ./target/debug/ssherlock & app=$!
DISPLAY=:99 xdotool search --sync --name SSHerlock   # wait for the window
sleep 2   # first frame: software Vulkan compiles its shaders
DISPLAY=:99 xdotool type 'echo hello' && DISPLAY=:99 xdotool key Return
sleep 1
DISPLAY=:99 import -window root target/screenshot.png
kill "$app" "$xvfb"
```

To test middle-click paste in the sandbox, turn the setting on in the sandbox's own dconf:
`env -u DBUS_SESSION_BUS_ADDRESS HOME="$sandbox" dbus-run-session -- gsettings set
org.gnome.desktop.interface gtk-enable-primary-paste true`.

Under Xvfb the window can stay black until the first input event: with no window manager,
nothing asks gpui to draw it. Move the pointer (`xdotool mousemove 600 400`) before the first
screenshot. Three more things are expected under Xvfb and aren't bugs. There is no title bar, because no window
manager is running, so gpui falls back to server-side decorations. The log has a "Found no
xinput mouse pointers" error, because Xvfb has no input devices. Opening a link logs "Failed to
open with dbus", because the bus is disabled on purpose.

Xvfb only exercises gpui's X11 backend. To test as a Wayland client, under weston or a headless
GNOME Shell (for clipboard, focus and keyboard behaviour), see
[docs/headless-wayland.md](docs/headless-wayland.md).

On macOS the app runs on the real display. `screencapture -x out.png` is built in; the app that
runs it (your terminal or editor) needs the Screen Recording permission.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Contributions are accepted under the same terms.
