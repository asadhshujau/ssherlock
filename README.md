# SSHerlock

A Rust SSH client. Hosts, keys, port forwarding, snippets, SFTP, sync — everything a modern
SSH client should do, with no paid tier. Built with [gpui](https://crates.io/crates/gpui) and
[russh](https://crates.io/crates/russh).

> **Status: early development (Phase 0 done, Phase 1 next).** Nothing usable yet: the app opens an
> empty window. Follow [docs/PLAN.md](docs/PLAN.md) for the build order and [docs/FEATURES.md](docs/FEATURES.md)
> for the full checklist.

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

### 4. Docker (needed from Phase 2)

The SSH integration tests run against an OpenSSH server in Docker (`crates/ssherlock-ssh/tests/docker/`).
Install Docker Engine or Docker Desktop (macOS) or Podman with Docker compatibility. If Docker is
missing, those tests are skipped with a message.

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

Run it sandboxed, so neither the app nor the shell it starts reads or writes your real
configuration. The app writes logs under `XDG_DATA_HOME`, and its terminal runs `$SHELL` as a
login shell:

```sh
cargo build -p ssherlock
sandbox=$(mktemp -d)
Xvfb :99 -screen 0 1280x800x24 & xvfb=$!
env -u WAYLAND_DISPLAY DISPLAY=:99 HOME="$sandbox" XDG_DATA_HOME="$sandbox/share" SHELL=/bin/sh \
  VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json ./target/debug/ssherlock & app=$!
sleep 3
DISPLAY=:99 xdotool type 'echo hello' && DISPLAY=:99 xdotool key Return
DISPLAY=:99 import -window root target/screenshot.png
kill "$app" "$xvfb"
```

On macOS the app runs on the real display. `screencapture -x out.png` is built in; the app that
runs it (your terminal or editor) needs the Screen Recording permission.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Contributions are accepted under the same terms.
