//! Test helpers for the forwarding views: an in-process SSH server that forwards, an echo
//! server, and a way to wait for Tokio's network work from a gpui test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::{Duration, Instant};

/// An SSH server in this process that lets anyone in and connects `direct-tcpip`
/// channels, as OpenSSH does for `-L`.
struct Forwarder;

impl russh::server::Handler for Forwarder {
    type Error = russh::Error;

    fn auth_none(
        &mut self,
        _: &str,
    ) -> impl Future<Output = Result<russh::server::Auth, Self::Error>> + Send {
        std::future::ready(Ok(russh::server::Auth::Accept))
    }

    fn channel_open_direct_tcpip(
        &mut self,
        channel: russh::Channel<russh::server::Msg>,
        host: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: russh::server::ChannelOpenHandle,
        _: &mut russh::server::Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        let target = (host.to_owned(), u16::try_from(port).unwrap());
        tokio::spawn(async move {
            if let Ok(mut socket) = tokio::net::TcpStream::connect(target).await {
                reply.accept().await;
                let mut stream = channel.into_stream();
                let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
            }
        });
        std::future::ready(Ok(()))
    }
}

/// Starts the SSH server and an echo server; returns their ports.
pub async fn servers() -> (u16, u16) {
    let ssh = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ports = (
        ssh.local_addr().unwrap().port(),
        echo.local_addr().unwrap().port(),
    );
    // A generated test key (see that folder's README), never used against a real server.
    let key = russh::keys::load_secret_key(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../ssherlock-ssh/tests/fixtures/id_ed25519"
        ),
        None,
    )
    .unwrap();
    let config = std::sync::Arc::new(russh::server::Config {
        keys: vec![key],
        ..russh::server::Config::default()
    });
    tokio::spawn(async move {
        while let Ok((stream, _)) = ssh.accept().await {
            if let Ok(running) = russh::server::run_stream(config.clone(), stream, Forwarder).await
            {
                tokio::spawn(running);
            }
        }
    });
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = echo.accept().await {
            tokio::spawn(async move {
                let (mut read, mut write) = socket.split();
                let _ = tokio::io::copy(&mut read, &mut write).await;
            });
        }
    });
    ports
}

/// Runs the app until `done` holds, letting Tokio's network work happen meanwhile.
pub fn wait_until(
    cx: &mut gpui::VisualTestContext,
    what: &str,
    mut done: impl FnMut(&mut gpui::VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
