//! Copying one forwarded connection both ways, counting the bytes as they pass (FWD-07).

use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use super::Counters;

/// Big enough to keep a fast link busy, small enough for many idle connections.
const BUFFER: usize = 32 * 1024;

/// Copies `local` (a socket on this machine) to `tunnel` (the SSH channel) and back until both
/// directions have ended, or either fails. Bytes from `local` count as uploaded, bytes from
/// `tunnel` as downloaded. When one side stops sending, the other is told (a half-close), and
/// the copy carries on the other way, as TCP allows.
pub(crate) async fn pump<L, T>(local: L, tunnel: T, counters: &Counters)
where
    L: AsyncRead + AsyncWrite,
    T: AsyncRead + AsyncWrite,
{
    let (local_read, local_write) = tokio::io::split(local);
    let (tunnel_read, tunnel_write) = tokio::io::split(tunnel);
    let up = copy(local_read, tunnel_write, &counters.uploaded);
    let down = copy(tunnel_read, local_write, &counters.downloaded);
    tokio::pin!(up, down);
    let (mut up_done, mut down_done) = (false, false);
    while !(up_done && down_done) {
        let result = tokio::select! {
            result = &mut up, if !up_done => {
                up_done = true;
                result
            }
            result = &mut down, if !down_done => {
                down_done = true;
                result
            }
        };
        if let Err(error) = result {
            // A reset or a dead channel: the other direction can't usefully go on either.
            tracing::debug!(%error, "forwarded connection broke off");
            return;
        }
    }
}

/// Copies until `from` ends, then shuts `to` down so its reader sees the end too.
async fn copy<R, W>(mut from: R, mut to: W, counter: &AtomicU64) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0_u8; BUFFER];
    loop {
        let read = from.read(&mut buffer).await?;
        let Some(chunk) = buffer.get(..read).filter(|chunk| !chunk.is_empty()) else {
            break;
        };
        to.write_all(chunk).await?;
        counter.fetch_add(read as u64, Ordering::Relaxed);
    }
    to.shutdown().await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;
    use tokio::io::duplex;

    use super::*;

    #[tokio::test]
    async fn copies_both_ways_with_half_close_and_counts() {
        let counters = Counters::default();
        // `app` stands for the program on this machine, `server` for the far end of the channel.
        let (mut app, local) = duplex(64);
        let (tunnel, mut server) = duplex(64);
        let echo = async {
            // Reads everything, then answers, as a request/response protocol does.
            let mut request = Vec::new();
            server.read_to_end(&mut request).await.unwrap();
            server.write_all(b"pong:").await.unwrap();
            server.write_all(&request).await.unwrap();
            server.shutdown().await.unwrap();
        };
        let client = async {
            app.write_all(&vec![b'x'; 100_000]).await.unwrap();
            app.shutdown().await.unwrap();
            let mut answer = Vec::new();
            app.read_to_end(&mut answer).await.unwrap();
            answer
        };
        let ((), answer, ()) = tokio::join!(pump(local, tunnel, &counters), client, echo);
        assert_eq!(answer.len(), 100_005);
        assert!(answer.starts_with(b"pong:x"));
        assert_eq!(counters.uploaded.load(Ordering::Relaxed), 100_000);
        assert_eq!(counters.downloaded.load(Ordering::Relaxed), 100_005);
    }

    #[tokio::test]
    async fn ends_when_both_sides_hang_up() {
        let counters = Counters::default();
        let (app, local) = duplex(64);
        let (tunnel, server) = duplex(64);
        drop((app, server));
        pump(local, tunnel, &counters).await;
        assert_eq!(counters.uploaded.load(Ordering::Relaxed), 0);
    }
}
