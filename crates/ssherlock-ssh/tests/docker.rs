//! Integration tests against a real OpenSSH server running in Docker (`tests/docker/`).
//!
//! Skipped with a message when Docker isn't available; CI on Linux sets
//! `SSHERLOCK_REQUIRE_DOCKER=1` so they always run there.
#![forbid(unsafe_code)]
#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code: failing loudly is the point"
)]

// Crate roots resolve `mod` from their own directory; the harness lives next to compose.yml.
#[path = "docker/harness.rs"]
mod harness;

use std::io::Read;
use std::net::TcpStream;
use std::time::Duration;

use harness::TestServer;

#[test]
fn test_server_speaks_ssh() {
    let Some(server) = TestServer::start("test_server_speaks_ssh") else {
        return;
    };
    let mut stream = TcpStream::connect(server.addr()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut banner = [0_u8; 15];
    stream.read_exact(&mut banner).unwrap();
    assert_eq!(&banner, b"SSH-2.0-OpenSSH");
}

#[test]
fn test_server_has_host_keys() {
    let Some(server) = TestServer::start("test_server_has_host_keys") else {
        return;
    };
    for (algorithm, prefix) in [
        ("ed25519", "ssh-ed25519 "),
        ("ecdsa", "ecdsa-sha2-nistp256 "),
        ("rsa", "ssh-rsa "),
    ] {
        let key = server.host_key(algorithm);
        assert!(key.starts_with(prefix), "{algorithm} host key: {key}");
    }
}
