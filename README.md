# SSHerlock

A Rust SSH client. Hosts, keys, port forwarding, snippets, SFTP, sync — everything a modern
SSH client should do, with no paid tier. Built with [gpui](https://crates.io/crates/gpui) and
[russh](https://crates.io/crates/russh).

> **Status: name reserved, nothing usable yet.** This release exists only to claim the crate name.
> Follow [docs/PLAN.md](docs/PLAN.md) for the build order and [docs/FEATURES.md](docs/FEATURES.md)
> for the full checklist.

## Principles

- Every feature is free. No tiers, no licence checks, no account required.
- No `unsafe` code in this repository.
- Desktop-first: macOS, Linux and Windows from one codebase.

## Development setup

SSHerlock is developed on macOS and Linux. Do this once before building.

### 1. Rust toolchain

Install stable Rust with [rustup](https://rustup.rs). The minimum is **1.89**, which is what
`rusqlite` needs. Add the components the checks use:

```sh
rustup toolchain install stable
rustup component add rustfmt clippy
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

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Contributions are accepted under the same terms.
