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

# Start the OpenSSH test server on 127.0.0.1:2222 (user `tester`, password `tester-password`)
ssh-test-server:
    docker compose --file crates/ssherlock-ssh/tests/docker/compose.yml up --build --detach
    @echo "ssh -p 2222 tester@127.0.0.1   (password: tester-password)"

# Stop the test server and remove any containers the integration tests left behind
ssh-test-clean:
    docker compose --file crates/ssherlock-ssh/tests/docker/compose.yml down
    docker ps --quiet --filter label=ssherlock-test=1 | xargs -r docker rm --force
