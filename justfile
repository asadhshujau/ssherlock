# SSHerlock tasks. `just` with no arguments lists them.

default:
    @just --list

# Format all code (nightly rustfmt: rustfmt.toml uses nightly-only import options)
fmt:
    cargo +nightly fmt --all

# Fail if any file is not formatted
fmt-check:
    cargo +nightly fmt --all -- --check

# Clippy on every crate and target; warnings are errors
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Run all tests
test:
    cargo test --workspace

# Check dependency licences, advisories, bans and sources
deny:
    cargo deny check

# Everything CI runs; do this before every commit
check: fmt-check lint test deny

# Build and launch the app
run *ARGS:
    cargo run -p ssherlock -- {{ARGS}}
