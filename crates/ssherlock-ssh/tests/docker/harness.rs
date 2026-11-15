//! Starts throwaway OpenSSH containers for the integration tests.
//!
//! Each test calls [`TestServer::start`] and gets its own container, published on a random
//! localhost port and removed when the `TestServer` drops. If Docker isn't available the test is
//! skipped with a message, unless `SSHERLOCK_REQUIRE_DOCKER` is set (CI sets it on Linux), in
//! which case it fails.

// Each test crate includes this file and uses a different subset of it.
#![allow(dead_code)]

use std::io::Read;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The test user on the server.
pub const USER: &str = "tester";
/// The test user's password.
pub const PASSWORD: &str = "tester-password";
/// Passphrase of `fixtures/id_ed25519_encrypted`.
pub const KEY_PASSPHRASE: &str = "fixture-passphrase";

/// Image built from `compose.yml`.
const IMAGE: &str = "ssherlock-test-sshd";
/// Label on every container we start, so leftovers are easy to find and remove.
const LABEL: &str = "ssherlock-test=1";
/// Safety net: a container whose test process died without dropping its `TestServer` still
/// exits after this many seconds (`--rm` then deletes it).
const MAX_LIFETIME_SECS: u32 = 900;
/// How long to wait for sshd to send its banner after the container starts.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// A running OpenSSH container. Removed on drop.
pub struct TestServer {
    id: String,
    addr: SocketAddr,
}

impl TestServer {
    /// Starts a fresh server, or returns `None` (after saying why) when Docker isn't available
    /// and the test should be skipped.
    pub fn start(test: &str) -> Option<Self> {
        if let Err(reason) = docker_ready() {
            assert!(
                std::env::var_os("SSHERLOCK_REQUIRE_DOCKER").is_none(),
                "{test}: Docker is required (SSHERLOCK_REQUIRE_DOCKER is set): {reason}"
            );
            // Without this the skip would be silent; the message tells a developer why.
            #[allow(clippy::print_stderr)]
            {
                eprintln!("skipping {test}: {reason}");
            }
            return None;
        }

        let lifetime = MAX_LIFETIME_SECS.to_string();
        let out = docker(&[
            "run",
            "--detach",
            "--rm",
            "--label",
            LABEL,
            "--publish",
            "127.0.0.1::22",
            IMAGE,
            "timeout",
            &lifetime,
            "/usr/sbin/sshd.pam",
            "-D",
            "-e",
        ]);
        let id = stdout_of(&out, "docker run").trim().to_owned();
        // Constructed before anything else can fail, so the container is removed on any panic.
        let mut server = Self {
            id,
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
        };

        let port_out = docker(&["port", &server.id, "22/tcp"]);
        let port = stdout_of(&port_out, "docker port")
            .lines()
            .find_map(|line| line.rsplit(':').next()?.trim().parse::<u16>().ok())
            .unwrap_or_else(|| panic!("no published port for container {}", server.id));
        server.addr = SocketAddr::from(([127, 0, 0, 1], port));
        server.wait_for_banner();
        Some(server)
    }

    /// Where the server listens.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The server's host public key of `algorithm` (`ed25519`, `ecdsa` or `rsa`), as the
    /// `type base64 comment` line from its `.pub` file.
    pub fn host_key(&self, algorithm: &str) -> String {
        let path = format!("/etc/ssh/ssh_host_{algorithm}_key.pub");
        let out = docker(&["exec", &self.id, "cat", &path]);
        stdout_of(&out, "docker exec cat").trim().to_owned()
    }

    /// Polls until sshd has sent its `SSH-2.0-` banner.
    fn wait_for_banner(&self) {
        let deadline = Instant::now() + READY_TIMEOUT;
        while Instant::now() < deadline {
            if let Ok(mut stream) = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1)) {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                let mut buf = [0_u8; 8];
                if stream.read_exact(&mut buf).is_ok() && buf.starts_with(b"SSH-2.0-") {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "sshd in container {} sent no banner within {READY_TIMEOUT:?}",
            self.id
        );
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // Best effort: the container's `timeout` removes it anyway if this fails.
        let _ = Command::new("docker")
            .args(["rm", "--force", &self.id])
            .output();
    }
}

/// The path of a file in `tests/fixtures/`.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Checks Docker once per test binary and builds the image. `Err` says why tests can't run.
fn docker_ready() -> Result<(), String> {
    static READY: OnceLock<Result<(), String>> = OnceLock::new();
    READY
        .get_or_init(|| {
            let info = Command::new("docker")
                .args(["info", "--format", "{{.ServerVersion}}"])
                .output()
                .map_err(|e| format!("could not run docker: {e}"))?;
            // `docker info --format` exits 0 even when the daemon is down; then it prints nothing.
            if !info.status.success() || info.stdout.trim_ascii().is_empty() {
                return Err(format!(
                    "the Docker daemon is not reachable: {}",
                    String::from_utf8_lossy(&info.stderr).trim()
                ));
            }
            let compose = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/docker/compose.yml");
            let build = Command::new("docker")
                .arg("compose")
                .arg("--file")
                .arg(&compose)
                .args(["build", "--quiet"])
                .output()
                .map_err(|e| format!("could not run docker compose: {e}"))?;
            if !build.status.success() {
                return Err(format!(
                    "could not build the test server image: {}",
                    String::from_utf8_lossy(&build.stderr).trim()
                ));
            }
            Ok(())
        })
        .clone()
}

fn docker(args: &[&str]) -> Output {
    Command::new("docker")
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("could not run docker {args:?}: {e}"))
}

fn stdout_of<'a>(out: &'a Output, what: &str) -> &'a str {
    assert!(
        out.status.success(),
        "{what} failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    std::str::from_utf8(&out.stdout).unwrap_or_else(|e| panic!("{what} printed non-UTF-8: {e}"))
}
